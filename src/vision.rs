//! `--image`, `--vision` and `--find`: the page's content images, best first, with Clef looking at
//! the pixels when asked.

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use futures::future::join_all;
use reqwest::Client;
use serde_json::{Map, Value, json};

use crate::{
    config::Config,
    decide::{self, choice, is_api_error, noul},
    extract::{Extracted, Image},
    fetch,
    judge::{Ctx, Item},
    output::{Rendered, not_found},
    timing::Timer,
};

/// --vision looks at this many images, starting while Jev is still thinking.
const VISION_MAX: usize = 12;
/// --find looks at up to this many; on bigger pages Jev shortlists them by alt/caption first.
const FIND_MAX: usize = 80;
/// Past this, an image keeps its text-only score. JURL_VISION_TIMEOUT_MS raises it for batch use, where a slow host
/// (full-size images, a far CDN) matters more than a second of waiting.
const VISION_DEADLINE_MS: u64 = 2500;

fn vision_deadline() -> Duration {
    let ms = std::env::var("JURL_VISION_TIMEOUT_MS").ok().and_then(|v| v.trim().parse::<u64>().ok());
    Duration::from_millis(ms.filter(|&m| m > 0).unwrap_or(VISION_DEADLINE_MS))
}
/// Clef's latency has a long tail: if a call is slower than this, race a duplicate.
const VISION_HEDGE: Duration = Duration::from_millis(700);
const VISION_PX: u32 = 384;
const CONTENT_KINDS: &[&str] = &["photo", "chart", "diagram", "screenshot", "illustration", "product"];
static VISUAL_KINDS: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
    json!({
        "photo": "A photograph of a scene, object, place or person",
        "chart": "A chart, graph or plot of data",
        "diagram": "A diagram, flowchart, architecture drawing or map",
        "screenshot": "A screenshot of software, a terminal or a web page",
        "illustration": "A drawing, painting or editorial illustration",
        "product": "A product shot",
        "logo": "A brand logo or wordmark",
        "icon": "A small icon, badge or emoji",
        "avatar": "A small profile picture of an author or user",
        "ad": "An advertisement or promotional banner",
        "decoration": "A background, divider, pattern or other decoration",
    })
});

/// --image / --vision: content images, best first.
pub(crate) async fn images(ctx: &Ctx<'_>, cfg: &Config, ex: &Extracted, t: &mut Timer) -> Result<Rendered> {
    if ex.images.is_empty() {
        return Err(not_found(format!("no images found in {}", ctx.url)));
    }
    let clef_keys = if ctx.args.vision {
        let account = cfg
            .get("CLOUDFLARE_ACCOUNT_ID")
            .context("--vision and --find need a Cloudflare Workers AI token: run `jurl init`")?;
        let token = cfg
            .get("CLOUDFLARE_AI_TOKEN")
            .context("--vision and --find need a Cloudflare Workers AI token: run `jurl init`")?;
        Some((account, token))
    } else {
        None
    };

    // Text-only judgement: alt, caption, file name and size are usually enough.
    let items = ex
        .images
        .iter()
        .map(|i| Item {
            state: json!({
                "i": i.i,
                "file": i.url.path_segments().and_then(|mut s| s.next_back()).unwrap_or(""),
                "alt": i.alt,
                "caption": i.caption,
                "width": i.width,
                "height": i.height,
            }),
            questions: vec![(
                format!("img{}", i.i),
                match &ctx.args.ask {
                    Some(q) => noul(format!(
                        "Judging by its file name, alt text and caption, the image in `images` with i={} shows: {q}",
                        i.i
                    )),
                    None => noul(format!(
                        "The image in `images` with i={} is meaningful content of this page (photo, diagram, chart, \
                         screenshot, illustration or product shot), not a logo, icon, avatar, ad, badge or decoration.",
                        i.i
                    )),
                },
            )],
        })
        .collect();
    let query = ctx.args.ask.as_deref().filter(|_| ctx.args.vision);

    // Clef starts at the same time as Jev, on the first images in page order (the noise
    // filter already dropped icons and trackers), so --vision costs max(jev, clef), not the sum.
    // One HTTP/1 connection per Clef call: multiplexing them all over a single HTTP/2
    // connection measured ~2x slower at the tail.
    let clef_client = Client::builder().http1_only().timeout(Duration::from_secs(10)).build()?;
    let cap = if query.is_some() { FIND_MAX } else { VISION_MAX };
    let (a, looks) = if query.is_some() && ex.images.len() > cap {
        // Too many to look at: Jev shortlists by text, then Clef looks at the shortlist.
        let a = ctx.judge("images", items, Map::new()).await?;
        t.lap(a.label());
        let mut ranked: Vec<&Image> = ex.images.iter().collect();
        let p = |i: &Image| a.noul(&format!("img{}", i.i)).unwrap_or(0.0);
        ranked.sort_by(|x, y| p(y).total_cmp(&p(x)));
        ranked.truncate(cap);
        let looks = look_all(ctx.client, &clef_client, clef_keys.as_ref(), &ex.title, ranked, query).await;
        t.lap(format!("clef({} img)", looks.len()));
        (a, looks)
    } else {
        let (a, looks) = tokio::join!(
            ctx.judge("images", items, Map::new()),
            look_all(
                ctx.client,
                &clef_client,
                clef_keys.as_ref(),
                &ex.title,
                ex.images.iter().take(cap).collect(),
                query
            )
        );
        let a = a?;
        t.lap(if ctx.args.vision { format!("{} ‖ clef({} img)", a.label(), looks.len()) } else { a.label() });
        (a, looks)
    };

    let mut scored: Vec<(&Image, f64)> = ex
        .images
        .iter()
        .map(|img| {
            let p_text = a.noul(&format!("img{}", img.i)).unwrap_or(0.0);
            let p = match looks.get(img.i) {
                // Searching: Clef saw the pixels *and* the alt/caption, so it decides.
                Some(Ok(p_pixels)) if query.is_some() => *p_pixels,
                // Pixels say what it is; Jev's page context says whether it belongs here.
                Some(Ok(p_pixels)) => (p_pixels + p_text) / 2.0,
                Some(Err(e)) => {
                    if ctx.args.timing {
                        eprintln!("jurl: clef skipped {}: {e:#}", img.url);
                    }
                    p_text
                }
                None => p_text,
            };
            (img, p)
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));

    let best = scored.first().map(|(i, p)| (i.url.clone(), *p));
    let kept: Vec<_> = scored
        .into_iter()
        .filter(|(_, p)| *p >= ctx.args.threshold())
        .take(ctx.args.limit(if query.is_some() { 1 } else { usize::MAX }))
        .collect();
    // Every image failed: an error, not "nothing looks like that". Clef's own errors (a bad token, no credits) say
    // Clef couldn't look; any other error is about getting the images to Clef at all.
    if kept.is_empty()
        && !looks.is_empty()
        && let Some(e) = looks.0.values().try_fold(None, |_, r| r.as_ref().err().map(Some)).flatten()
    {
        let what = if is_api_error(e) { "Clef couldn't look at any image" } else { "couldn't download any image" };
        bail!("{what} in {}: {e:#}", ctx.url);
    }
    if kept.is_empty()
        && let (Some(q), Some((url, p))) = (query, best)
    {
        return Err(not_found(format!("no image in {} looks like \"{q}\" (closest: {url}, p={p:.2})", ctx.url)));
    }
    if kept.is_empty() {
        return Err(not_found(format!("no content images in {} (try a lower --threshold)", ctx.url)));
    }
    let v: Vec<_> = kept
        .iter()
        .map(|(i, p)| json!({ "url": i.url.as_str(), "alt": i.alt, "caption": i.caption, "p": p }))
        .collect();
    let text = kept.iter().map(|(i, _)| format!("{}\n", i.url)).collect();
    Ok(Rendered { text, json: json!({ "url": ctx.url.as_str(), "title": ex.title, "images": v }) })
}

/// Clef's answers for the images it looked at, by image index (`Image::i`).
#[derive(Default)]
struct Looks(HashMap<usize, Result<f64>>);

impl Looks {
    fn get(&self, i: usize) -> Option<&Result<f64>> {
        self.0.get(&i)
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Clef on several images at once, each bounded by the vision deadline.
async fn look_all(
    client: &Client,
    clef_client: &Client,
    keys: Option<&(String, String)>,
    title: &str,
    imgs: Vec<&Image>,
    query: Option<&str>,
) -> Looks {
    let Some((account, token)) = keys else { return Looks::default() };
    let deadline = vision_deadline();
    let answers = join_all(imgs.into_iter().map(|img| async move {
        let look = tokio::time::timeout(deadline, look(client, clef_client, account, token, title, img, query)).await;
        (img.i, look.unwrap_or_else(|_| Err(anyhow!("over {}ms (JURL_VISION_TIMEOUT_MS)", deadline.as_millis()))))
    }))
    .await;
    Looks(answers.into_iter().collect())
}

/// Clef's view of one image: with a query, P(it shows that); without, P(it is content, not chrome).
#[allow(clippy::too_many_arguments)]
async fn look(
    client: &Client,
    clef_client: &Client,
    account: &str,
    token: &str,
    title: &str,
    img: &Image,
    query: Option<&str>,
) -> Result<f64> {
    let data = thumbnail(client, &img.preview).await?;
    let state = json!({ "page_title": title, "alt": img.alt, "caption": img.caption });
    // Clef answers "what is this?" far better than "does this matter?", so without a query
    // ask the factual question and add up the content classes here.
    let question = match query {
        Some(q) => noul(format!("The attached image shows: {q}")),
        None => choice("What does the attached image show?", VISUAL_KINDS.clone()),
    };
    let qs = Map::from_iter([("q".to_string(), question)]);
    let call = || decide::clef(clef_client, account, token, state.clone(), qs.clone(), vec![data.clone()]);
    let a = hedged(call, VISION_HEDGE).await?;
    if query.is_some() {
        return a.noul("q").context("clef returned no answer");
    }
    let probs = a.probabilities("q").context("clef returned no answer")?;
    Ok(CONTENT_KINDS.iter().filter_map(|k| probs.get(*k)).sum())
}

/// Start `call`; if it hasn't finished after `after`, start a second one and take whichever answers first.
/// Once both are running, a copy that fails doesn't end the race: the other one's answer still counts.
/// When both fail, the later error is returned.
async fn hedged<F, Fut, T>(call: F, after: Duration) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let first = call();
    tokio::pin!(first);
    tokio::select! {
        r = &mut first => return r,
        _ = tokio::time::sleep(after) => {}
    }
    let second = call();
    tokio::pin!(second);
    // The first copy to finish, unless it failed: then the other copy's result (if both failed, the later error).
    let (done, other) = tokio::select! {
        r = &mut first => (r, second),
        r = &mut second => (r, first),
    };
    if done.is_ok() { done } else { other.await }
}

/// Download and shrink to a small JPEG: fewer vision tokens, faster Clef.
async fn thumbnail(client: &Client, url: &url::Url) -> Result<String> {
    let bytes = fetch::fetch_bytes(client, url).await?;
    tokio::task::spawn_blocking(move || -> Result<String> {
        let img = image::load_from_memory(&bytes)?;
        let img =
            if img.width() > VISION_PX || img.height() > VISION_PX { img.thumbnail(VISION_PX, VISION_PX) } else { img };
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 75).encode_image(&img.to_rgb8())?;
        Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Two copies of a call, hedged after 100 ms. Each answers its value, or fails (`None`), after its own delay in ms.
    /// Returns the result and how many copies were started.
    async fn hedge(first: (u64, Option<u32>), second: (u64, Option<u32>)) -> (Result<u32>, usize) {
        let calls = AtomicUsize::new(0);
        let call = || {
            let (ms, answer) = if calls.fetch_add(1, Ordering::SeqCst) == 0 { first } else { second };
            async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                answer.ok_or_else(|| anyhow!("failed after {ms} ms"))
            }
        };
        let got = hedged(call, Duration::from_millis(100)).await;
        (got, calls.into_inner())
    }

    #[tokio::test]
    async fn an_answer_before_the_hedge_starts_no_second_copy() {
        let (got, calls) = hedge((0, Some(1)), (0, Some(2))).await;
        assert_eq!(got.unwrap(), 1);
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn a_failed_first_copy_hands_the_race_to_the_second() {
        let (got, calls) = hedge((300, None), (300, Some(2))).await;
        assert_eq!(got.unwrap(), 2);
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn a_failed_second_copy_hands_the_race_to_the_first() {
        let (got, _) = hedge((500, Some(1)), (200, None)).await;
        assert_eq!(got.unwrap(), 1);
    }

    #[tokio::test]
    async fn when_both_copies_fail_the_later_error_is_returned() {
        let (got, calls) = hedge((300, None), (500, None)).await;
        assert_eq!(got.unwrap_err().to_string(), "failed after 500 ms");
        assert_eq!(calls, 2);
    }
}

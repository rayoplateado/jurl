//! `--image`, `--vision` and `--find`: the page's content images, best first, with Clef looking at
//! the pixels when asked.

use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use futures::future::join_all;
use reqwest::Client;
use serde_json::{Map, Value, json};

use crate::{
    config::Config,
    decide::{self, Answers, choice, is_api_error, noul},
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

/// Clef's Cloudflare account and API token, from `jurl init`.
struct ClefKeys {
    account: String,
    token: String,
}

/// Both keys are needed for --vision and --find; the message is the same whichever is missing.
const MISSING_KEYS: &str = "--vision and --find need a Cloudflare Workers AI token: run `jurl init`";

impl ClefKeys {
    fn from_config(cfg: &Config) -> Result<Self> {
        Ok(Self {
            account: cfg.get("CLOUDFLARE_ACCOUNT_ID").context(MISSING_KEYS)?,
            token: cfg.get("CLOUDFLARE_AI_TOKEN").context(MISSING_KEYS)?,
        })
    }
}

/// --image / --vision: content images, best first.
pub(crate) async fn images(ctx: &Ctx<'_>, cfg: &Config, ex: &Extracted, t: &mut Timer) -> Result<Rendered> {
    if ex.images.is_empty() {
        return Err(not_found(format!("no images found in {}", ctx.url)));
    }
    let clef_keys = if ctx.args.vision { Some(ClefKeys::from_config(cfg)?) } else { None };

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
                image_id(i),
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
    let req = clef_keys.as_ref().map(|keys| LookRequest {
        http: ctx.client,
        clef: &clef_client,
        keys,
        title: &ex.title,
        query,
    });
    let cap = look_cap(query);
    let (a, looks) = if needs_shortlist(ex.images.len(), cap, query) {
        // Too many to look at: Jev shortlists by text, then Clef looks at the shortlist.
        let a = ctx.judge("images", items, Map::new()).await?;
        t.lap(a.label());
        let picks = shortlist(&ex.images, cap, |img| text_score(&a, img));
        let looks = look_all(req.as_ref(), picks).await;
        t.lap(format!("clef({} img)", looks.len()));
        (a, looks)
    } else {
        let (a, looks) = tokio::join!(
            ctx.judge("images", items, Map::new()),
            look_all(req.as_ref(), first_in_page_order(&ex.images, cap))
        );
        let a = a?;
        t.lap(if ctx.args.vision { format!("{} ‖ clef({} img)", a.label(), looks.len()) } else { a.label() });
        (a, looks)
    };

    let mut scored: Vec<(&Image, f64)> = ex
        .images
        .iter()
        .map(|img| {
            let p_text = text_score(&a, img);
            let pixels = match looks.get(img.i) {
                Some(Ok(p_pixels)) => Some(*p_pixels),
                Some(Err(e)) => {
                    if ctx.args.timing {
                        eprintln!("jurl: clef skipped {}: {e:#}", img.url);
                    }
                    None
                }
                None => None,
            };
            (img, blend(p_text, pixels, query.is_some()))
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
        && let Some(e) = looks.all_failed()
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

/// How many images Clef looks at: --find (a query) looks at more.
fn look_cap(query: Option<&str>) -> usize {
    if query.is_some() { FIND_MAX } else { VISION_MAX }
}

/// Whether Jev has to shortlist: a query, and more images than Clef looks at.
fn needs_shortlist(images: usize, cap: usize, query: Option<&str>) -> bool {
    query.is_some() && images > cap
}

/// The `cap` images that Jev's text score ranks best, best first (ties keep page order).
fn shortlist(images: &[Image], cap: usize, text: impl Fn(&Image) -> f64) -> Vec<&Image> {
    let mut ranked: Vec<&Image> = images.iter().collect();
    ranked.sort_by(|x, y| text(y).total_cmp(&text(x)));
    ranked.truncate(cap);
    ranked
}

/// The first `cap` images in page order: what Clef looks at when no shortlist is needed.
fn first_in_page_order(images: &[Image], cap: usize) -> Vec<&Image> {
    images.iter().take(cap).collect()
}

/// The question id of an image's answer from Jev.
fn image_id(img: &Image) -> String {
    format!("img{}", img.i)
}

/// Jev's text score for an image, 0 when Jev didn't answer it.
fn text_score(a: &Answers, img: &Image) -> f64 {
    a.noul(&image_id(img)).unwrap_or(0.0)
}

/// An image's score from Jev's text score and, when Clef answered, its pixel score. Without an answer the text score
/// stands.
fn blend(p_text: f64, pixels: Option<f64>, searching: bool) -> f64 {
    match pixels {
        // Searching: Clef saw the pixels *and* the alt/caption, so it decides.
        Some(p_pixels) if searching => p_pixels,
        // Pixels say what it is; Jev's page context says whether it belongs here.
        Some(p_pixels) => (p_pixels + p_text) / 2.0,
        None => p_text,
    }
}

/// Clef's answers for the images it looked at, by image index (`Image::i`), lowest index first.
#[derive(Default)]
struct Looks(BTreeMap<usize, Result<f64>>);

impl Looks {
    fn get(&self, i: usize) -> Option<&Result<f64>> {
        self.0.get(&i)
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    /// Every look failed, so no image has Clef's score: the error to report is the one for the lowest image index,
    /// so the same failures always report the same error. None when a look answered, or none ran.
    fn all_failed(&self) -> Option<&anyhow::Error> {
        if self.0.values().any(Result::is_ok) {
            return None;
        }
        self.0.values().find_map(|r| r.as_ref().err())
    }
}

/// What the looks of one `images` call share: the clients, Clef's keys, the page title and the --find question.
struct LookRequest<'a> {
    /// Downloads each thumbnail: the page's own client.
    http: &'a Client,
    /// Calls Clef: one HTTP/1 connection per call.
    clef: &'a Client,
    keys: &'a ClefKeys,
    title: &'a str,
    query: Option<&'a str>,
}

/// Clef on several images at once, each bounded by the vision deadline.
async fn look_all(req: Option<&LookRequest<'_>>, imgs: Vec<&Image>) -> Looks {
    let Some(req) = req else { return Looks::default() };
    let deadline = vision_deadline();
    let answers = join_all(imgs.into_iter().map(|img| async move {
        let look = tokio::time::timeout(deadline, look(req, img)).await;
        (img.i, look.unwrap_or_else(|_| Err(anyhow!("over {}ms (JURL_VISION_TIMEOUT_MS)", deadline.as_millis()))))
    }))
    .await;
    Looks(answers.into_iter().collect())
}

/// Clef's view of one image: with a query, P(it shows that); without, P(it is content, not chrome).
async fn look(req: &LookRequest<'_>, img: &Image) -> Result<f64> {
    let data = thumbnail(req.http, &img.preview).await?;
    let state = json!({ "page_title": req.title, "alt": img.alt, "caption": img.caption });
    // Clef answers "what is this?" far better than "does this matter?", so without a query
    // ask the factual question and add up the content classes here.
    let question = match req.query {
        Some(q) => noul(format!("The attached image shows: {q}")),
        None => choice("What does the attached image show?", VISUAL_KINDS.clone()),
    };
    let qs = Map::from_iter([("q".to_string(), question)]);
    let call =
        || decide::clef(req.clef, &req.keys.account, &req.keys.token, state.clone(), qs.clone(), vec![data.clone()]);
    let a = hedged(call, VISION_HEDGE).await?;
    if req.query.is_some() {
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

    /// An image known only by its index, for the choosing tests.
    fn image(i: usize) -> Image {
        let url = url::Url::parse(&format!("https://example.test/{i}.jpg")).unwrap();
        Image {
            i,
            url: url.clone(),
            preview: url,
            alt: String::new(),
            caption: String::new(),
            width: None,
            height: None,
        }
    }

    #[test]
    fn a_query_looks_at_more_images_and_shortlists_past_its_cap() {
        assert_eq!(look_cap(None), VISION_MAX);
        assert_eq!(look_cap(Some("a cat")), FIND_MAX);
        // Without a query there is never a shortlist: the first images in page order are looked at.
        assert!(!needs_shortlist(90, look_cap(None), None));
        // With a query, every image up to the cap is looked at, and one more than the cap needs a shortlist.
        assert!(!needs_shortlist(FIND_MAX, look_cap(Some("a cat")), Some("a cat")));
        assert!(needs_shortlist(FIND_MAX + 1, look_cap(Some("a cat")), Some("a cat")));
    }

    #[test]
    fn without_a_shortlist_the_first_images_in_page_order_are_looked_at() {
        let images: Vec<Image> = (0..15).map(image).collect();
        let looked: Vec<usize> = first_in_page_order(&images, look_cap(None)).iter().map(|i| i.i).collect();
        assert_eq!(looked, (0..VISION_MAX).collect::<Vec<_>>());
    }

    #[test]
    fn the_shortlist_is_the_best_text_scores_with_ties_in_page_order() {
        let images: Vec<Image> = (0..6).map(image).collect();
        let text = |i: &Image| [0.1, 0.9, 0.5, 0.9, 0.2, 0.5][i.i];
        let picked: Vec<usize> = shortlist(&images, 4, text).iter().map(|i| i.i).collect();
        assert_eq!(picked, vec![1, 3, 2, 5]);
    }

    #[test]
    fn searching_takes_the_pixel_score_alone() {
        assert_eq!(blend(0.75, Some(0.25), true), 0.25);
    }

    #[test]
    fn otherwise_the_pixel_and_text_scores_average() {
        assert_eq!(blend(0.75, Some(0.25), false), 0.5);
    }

    #[test]
    fn without_a_pixel_score_the_text_score_stands() {
        assert_eq!(blend(0.75, None, true), 0.75);
        assert_eq!(blend(0.75, None, false), 0.75);
    }

    #[test]
    fn when_every_look_fails_the_lowest_image_index_is_reported() {
        let looks = Looks(BTreeMap::from([(7, Err(anyhow!("seven"))), (3, Err(anyhow!("three")))]));
        assert_eq!(format!("{:#}", looks.all_failed().unwrap()), "three");
    }

    #[test]
    fn an_answer_or_no_look_at_all_is_not_a_failure() {
        let answered = Looks(BTreeMap::from([(0, Err(anyhow!("down"))), (1, Ok(0.4))]));
        assert!(answered.all_failed().is_none());
        assert!(Looks::default().all_failed().is_none());
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

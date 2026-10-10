//! `--image`, `--vision` and `--find`: the page's content images, best first, with Clef looking at
//! the pixels when asked.

use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, anyhow};
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
/// The largest image downloaded for a look: a bigger one is not read, and its look fails.
const IMAGE_MAX: usize = 15 << 20;
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

impl ClefKeys {
    fn from_config(cfg: &Config) -> Result<Self> {
        Self::from_lookup(|name| cfg.get(name))
    }

    /// Both keys are needed for --vision and --find. An empty one is unset, and the error names each unset variable.
    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let account = get("CLOUDFLARE_ACCOUNT_ID").filter(|v| !v.is_empty());
        let token = get("CLOUDFLARE_AI_TOKEN").filter(|v| !v.is_empty());
        match (account, token) {
            (Some(account), Some(token)) => Ok(Self { account, token }),
            (account, token) => {
                let missing: Vec<&str> = [("CLOUDFLARE_ACCOUNT_ID", account), ("CLOUDFLARE_AI_TOKEN", token)]
                    .into_iter()
                    .filter(|(_, v)| v.is_none())
                    .map(|(name, _)| name)
                    .collect();
                Err(anyhow!(missing_keys_message(&missing)))
            }
        }
    }
}

/// The error when Clef's keys are not set: `missing` names the variables that are.
fn missing_keys_message(missing: &[&str]) -> String {
    let verb = if missing.len() == 1 { "is" } else { "are" };
    format!("--vision and --find need {}, which {verb} missing: run `jurl init`", missing.join(" and "))
}

/// --image / --vision: content images, best first.
pub(crate) async fn images(ctx: &Ctx<'_>, cfg: &Config, ex: &Extracted, t: &mut Timer) -> Result<Rendered> {
    if ex.images.is_empty() {
        return Err(not_found(format!("no images found in {}", ctx.url)));
    }
    let clef_keys = if ctx.args.vision { Some(ClefKeys::from_config(cfg)?) } else { None };
    let items = judge_items(ex, ctx.args.ask.as_deref());
    let query = ctx.args.ask.as_deref().filter(|_| ctx.args.vision);
    // One HTTP/1 connection per Clef call: multiplexing them all over a single HTTP/2
    // connection measured ~2x slower at the tail.
    let clef_client = Client::builder().http1_only().timeout(Duration::from_secs(10)).build()?;
    let retry = fetch::Retry::for_run(
        ctx.args.no_browser_retry,
        ctx.args.timing,
        &ctx.args.reach,
        &ctx.args.cookies,
        ctx.args.stealth.as_ref(),
        &ctx.args.fallback,
    );
    let req = clef_keys.as_ref().map(|keys| LookRequest {
        retry,
        page: ctx.url,
        clef: &clef_client,
        keys,
        title: &ex.title,
        query,
    });

    let (a, looks) = judge_and_look(ctx, ex, items, req.as_ref(), query, t).await?;
    let scored = score(ex, &a, &looks, query.is_some(), ctx.args.timing);
    let best = scored.first().map(|(i, p)| (i.url.clone(), *p));
    let limit = ctx.args.limit(if query.is_some() { 1 } else { usize::MAX });
    let kept = keep(scored, ctx.args.threshold(), limit);
    if kept.is_empty() {
        return Err(nothing_kept(ctx.url, &looks, ex.images.len(), query, best));
    }
    if let Some(notice) = text_only_notice(&looks, ex.images.len()) {
        eprintln!("{notice}");
    }
    if let Some(notice) = refused_notice(&kept, &looks) {
        eprintln!("{notice}");
    }
    Ok(render(ctx, ex, &kept, &looks, req.is_some()))
}

/// Jev's items for the text-only judgement of each image.
fn judge_items(ex: &Extracted, ask: Option<&str>) -> Vec<Item> {
    // Text-only judgement: alt, caption, file name and size are usually enough.
    ex.images
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
                match ask {
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
        .collect()
}

/// Jev's answers and Clef's looks. With a query and more images than Clef looks at, Jev shortlists first and Clef
/// looks at what it ranks best; otherwise Clef starts with Jev.
async fn judge_and_look(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    items: Vec<Item>,
    req: Option<&LookRequest<'_>>,
    query: Option<&str>,
    t: &mut Timer,
) -> Result<(Answers, Looks)> {
    let cap = look_cap(query);
    if needs_shortlist(ex.images.len(), cap, query) {
        // Too many to look at: Jev shortlists by text, then Clef looks at the shortlist.
        let a = ctx.judge("images", items, Map::new()).await?;
        t.lap(a.label());
        let picks = shortlist(&ex.images, cap, |img| text_score(&a, img));
        let looks = look_all(req, picks).await;
        t.lap(format!("clef({} img)", looks.len()));
        Ok((a, looks))
    } else {
        // Clef starts at the same time as Jev, on the first images in page order (the noise
        // filter already dropped icons and trackers), so --vision costs max(jev, clef), not the sum.
        let (a, looks) =
            tokio::join!(ctx.judge("images", items, Map::new()), look_all(req, first_in_page_order(&ex.images, cap)));
        let a = a?;
        t.lap(if ctx.args.vision { format!("{} ‖ clef({} img)", a.label(), looks.len()) } else { a.label() });
        Ok((a, looks))
    }
}

/// Every image with its score, best first (ties keep page order). A look that failed leaves the text score; with
/// -t, the failure is reported on stderr.
fn score<'a>(ex: &'a Extracted, a: &Answers, looks: &Looks, searching: bool, timing: bool) -> Vec<(&'a Image, f64)> {
    let mut scored: Vec<(&Image, f64)> = ex
        .images
        .iter()
        .map(|img| {
            let p_text = text_score(a, img);
            let pixels = match looks.get(img.i) {
                Some(Ok(p_pixels)) => Some(*p_pixels),
                Some(Err(e)) => {
                    if timing {
                        eprintln!("jurl: clef skipped {}: {e:#}", img.url);
                    }
                    None
                }
                None => None,
            };
            (img, blend(p_text, pixels, searching))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
}

/// The images at or above the threshold, in order, up to `limit`.
fn keep(scored: Vec<(&Image, f64)>, threshold: f64, limit: usize) -> Vec<(&Image, f64)> {
    scored.into_iter().filter(|(_, p)| *p >= threshold).take(limit).collect()
}

/// The error for a run that kept nothing: every look failed, or a query found nothing close enough, or no content
/// image passed the threshold.
fn nothing_kept(
    url: &url::Url,
    looks: &Looks,
    total: usize,
    query: Option<&str>,
    best: Option<(url::Url, f64)>,
) -> anyhow::Error {
    // Every image failed: an error, not "nothing looks like that".
    if let Some(e) = looks.all_failed() {
        return anyhow!(why_no_look(url, e, looks.len(), total));
    }
    if let (Some(q), Some((closest, p))) = (query, best) {
        return not_found(format!("no image in {url} looks like \"{q}\" (closest: {closest}, p={p:.2})"));
    }
    not_found(format!("no content images in {url} (try a lower --threshold)"))
}

/// What a run reports when every look failed: a timeout, a Clef error (a bad token, no credits) or a download error
/// (anything else is about getting the images to Clef at all). `looked` images were tried out of `total` on the page.
fn why_no_look(url: &url::Url, e: &anyhow::Error, looked: usize, total: usize) -> String {
    if let Some(timeout) = e.chain().find_map(|c| c.downcast_ref::<Timeout>()) {
        let who = if total > looked { format!("none of the {looked} images it looked at") } else { "no image".into() };
        return format!("{who} answered within {} ms (JURL_VISION_TIMEOUT_MS) in {url}", timeout.0.as_millis());
    }
    let what = if is_api_error(e) { "Clef couldn't look at" } else { "couldn't download" };
    format!("{what} {} in {url}: {e:#}", tried(looked, total))
}

/// The images a look covered, as a message names them: "any image", or "any of the 12 images it looked at" when the
/// page has more images than that.
fn tried(looked: usize, total: usize) -> String {
    if total > looked { format!("any of the {looked} images it looked at") } else { "any image".into() }
}

/// The stderr line for a result that is from the text score alone because Clef looked at no image. None when a look
/// answered, or when none ran (--image).
fn text_only_notice(looks: &Looks, total: usize) -> Option<String> {
    looks.all_failed()?;
    Some(format!(
        "jurl: Clef couldn't look at {}, so this result is from text only (-t shows why)",
        tried(looks.len(), total)
    ))
}

/// The stderr line for kept images that their hosts refused: their p is from text only. None when no kept image was refused.
fn refused_notice(kept: &[(&Image, f64)], looks: &Looks) -> Option<String> {
    let refused = kept.iter().filter(|(i, _)| pixels_state(looks.get(i.i)) == "refused").count();
    (refused > 0).then(|| {
        format!(
            "jurl: {refused} of the {} kept images were refused by their hosts, so their p is from text only (-t shows why)",
            kept.len()
        )
    })
}

/// The result: each kept image's URL on its own line, and the same images as JSON.
fn render(ctx: &Ctx<'_>, ex: &Extracted, kept: &[(&Image, f64)], looks: &Looks, looked: bool) -> Rendered {
    let v: Vec<_> = kept.iter().map(|(i, p)| image_json(i, *p, looks, looked)).collect();
    let text = kept.iter().map(|(i, _)| format!("{}\n", i.url)).collect();
    Rendered { text, json: json!({ "url": ctx.url.as_str(), "title": ex.title, "images": v }) }
}

/// One kept image in the result: its URL, alt, caption and p. `pixels` says what became of the image's look (see
/// [`pixels_state`]). It is left out when no look ran (--image).
fn image_json(img: &Image, p: f64, looks: &Looks, looked: bool) -> Value {
    let mut out = json!({ "url": img.url.as_str(), "alt": img.alt, "caption": img.caption, "p": p });
    if looked {
        out["pixels"] = json!(pixels_state(looks.get(img.i)));
    }
    out
}

/// The `pixels` of an image: "looked" when Clef looked at its pixels; "refused" when its host refused the image; "skipped"
/// when it was not looked at (past the look cap, after its host refused an earlier image, or a look that failed otherwise).
fn pixels_state(look: Option<&Result<f64>>) -> &'static str {
    match look {
        Some(Ok(_)) => "looked",
        Some(Err(e)) if e.downcast_ref::<fetch::ImageRefused>().is_some() => "refused",
        _ => "skipped",
    }
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

/// What the looks of one `images` call share: the run's downloads, Clef's keys, the page title and the --find question.
struct LookRequest<'a> {
    /// Downloads each thumbnail, under the run's reach and cookies (see `fetch::image_bytes`).
    retry: fetch::Retry<'a>,
    /// The page the images are on: their Referer, and the Sec-Fetch-Site of each download.
    page: &'a url::Url,
    /// Calls Clef: one HTTP/1 connection per call.
    clef: &'a Client,
    keys: &'a ClefKeys,
    title: &'a str,
    query: Option<&'a str>,
}

/// Clef on several images at once, each bounded by the vision deadline. The images are grouped by the origin they are
/// downloaded from, and each group's first image is asked before the rest of its group. A host that refuses that image gets
/// no more requests in this run: a refusal costs one or two requests, not one per image, and the rest are skipped.
async fn look_all(req: Option<&LookRequest<'_>>, imgs: Vec<&Image>) -> Looks {
    let Some(req) = req else { return Looks::default() };
    let deadline = vision_deadline();
    let mut groups: Vec<(String, Vec<&Image>)> = Vec::new();
    for img in imgs {
        let origin = img.preview.origin().ascii_serialization();
        match groups.iter_mut().find(|(o, _)| *o == origin) {
            Some((_, group)) => group.push(img),
            None => groups.push((origin, vec![img])),
        }
    }
    let answers = join_all(groups.into_iter().map(|(_, group)| async move {
        let mut group = group.into_iter();
        let mut answers = Vec::new();
        let Some(first) = group.next() else { return answers };
        let first_answer = (first.i, within(deadline, look(req, first)).await);
        let refused = first_answer.1.as_ref().err().is_some_and(|e| e.downcast_ref::<fetch::ImageRefused>().is_some());
        answers.push(first_answer);
        if !refused {
            let rest = group.map(|img| async move { (img.i, within(deadline, look(req, img)).await) });
            answers.extend(join_all(rest).await);
        }
        answers
    }))
    .await;
    Looks(answers.into_iter().flatten().collect())
}

/// `fut`'s answer, or a [`Timeout`] once `deadline` has passed.
async fn within<T>(deadline: Duration, fut: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(deadline, fut).await.unwrap_or_else(|_| Err(Timeout(deadline).into()))
}

/// A look that ran past the vision deadline. It is its own error, so a report can say it timed out rather than that
/// the download or Clef failed.
#[derive(Debug)]
struct Timeout(Duration);

impl std::fmt::Display for Timeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "timed out after {} ms (JURL_VISION_TIMEOUT_MS)", self.0.as_millis())
    }
}

impl std::error::Error for Timeout {}

/// What Clef is told about one image: its state (the page title, alt and caption) and the question.
fn clef_ask(title: &str, img: &Image, query: Option<&str>) -> (Value, Value) {
    let state = json!({ "page_title": title, "alt": img.alt, "caption": img.caption });
    // Clef answers "what is this?" far better than "does this matter?", so without a query
    // ask the factual question and add up the content classes here.
    let question = match query {
        Some(q) => noul(format!("The attached image shows: {q}")),
        None => choice("What does the attached image show?", VISUAL_KINDS.clone()),
    };
    (state, question)
}

/// Clef's view of one image: with a query, P(it shows that); without, P(it is content, not chrome).
async fn look(req: &LookRequest<'_>, img: &Image) -> Result<f64> {
    let data = thumbnail(req.retry, req.page, &img.preview).await?;
    let (state, question) = clef_ask(req.title, img, req.query);
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

/// Download and shrink to a small JPEG: fewer vision tokens, faster Clef. An image past `IMAGE_MAX` is not read.
async fn thumbnail(retry: crate::fetch::Retry<'_>, page: &url::Url, url: &url::Url) -> Result<String> {
    let bytes = fetch::image_bytes(page, url, retry, IMAGE_MAX).await?;
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
    fn the_threshold_and_the_limit_keep_the_best_first() {
        let images: Vec<Image> = (0..4).map(image).collect();
        let scored = vec![(&images[0], 0.9), (&images[3], 0.7), (&images[1], 0.5), (&images[2], 0.49)];
        let kept: Vec<usize> = keep(scored.clone(), 0.5, usize::MAX).iter().map(|(i, _)| i.i).collect();
        assert_eq!(kept, vec![0, 3, 1]);
        let one: Vec<usize> = keep(scored, 0.5, 1).iter().map(|(i, _)| i.i).collect();
        assert_eq!(one, vec![0]);
    }

    #[test]
    fn each_image_is_asked_under_the_id_its_score_is_read_from() {
        let url = url::Url::parse("https://example.test/page").unwrap();
        let ex = crate::extract::html(r#"<p>Text</p><img src="/a.jpg" width="200" height="150">"#, &url);
        assert_eq!(ex.images.len(), 1);
        let items = judge_items(&ex, None);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].questions[0].0, image_id(&ex.images[0]));
        assert_eq!(items[0].questions[0].0, "img0");
    }

    #[test]
    fn clef_is_asked_the_query_or_what_the_image_shows() {
        let img = image(4);
        let (state, question) = clef_ask("Trip", &img, Some("a cathedral"));
        assert_eq!(state, json!({ "page_title": "Trip", "alt": "", "caption": "" }));
        assert_eq!(question, json!({ "type": "noul", "instructions": "The attached image shows: a cathedral" }));
        let (_, question) = clef_ask("Trip", &img, None);
        assert_eq!(question["type"], "choice");
        assert_eq!(question["criteria"]["photo"], "A photograph of a scene, object, place or person");
    }

    /// The variables that are set, as a lookup: any other name is unset.
    fn env_of(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_error_names_each_missing_cloudflare_variable() {
        let both = env_of(&[("CLOUDFLARE_ACCOUNT_ID", "acct"), ("CLOUDFLARE_AI_TOKEN", "tok")]);
        assert!(ClefKeys::from_lookup(both).is_ok());
        let token_only = env_of(&[("CLOUDFLARE_AI_TOKEN", "tok")]);
        let err = ClefKeys::from_lookup(token_only).err().expect("no account id");
        assert_eq!(
            format!("{err:#}"),
            "--vision and --find need CLOUDFLARE_ACCOUNT_ID, which is missing: run `jurl init`"
        );
        let account_only = env_of(&[("CLOUDFLARE_ACCOUNT_ID", "acct")]);
        let err = ClefKeys::from_lookup(account_only).err().expect("no token");
        assert_eq!(
            format!("{err:#}"),
            "--vision and --find need CLOUDFLARE_AI_TOKEN, which is missing: run `jurl init`"
        );
        let neither = env_of(&[]);
        let err = ClefKeys::from_lookup(neither).err().expect("no keys");
        assert_eq!(
            format!("{err:#}"),
            "--vision and --find need CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_AI_TOKEN, which are missing: run `jurl init`"
        );
    }

    #[test]
    fn an_empty_cloudflare_variable_is_missing_too() {
        let empty_token = env_of(&[("CLOUDFLARE_ACCOUNT_ID", "acct"), ("CLOUDFLARE_AI_TOKEN", "")]);
        let err = ClefKeys::from_lookup(empty_token).err().expect("empty token");
        assert!(format!("{err:#}").contains("need CLOUDFLARE_AI_TOKEN, which is missing"), "{err:#}");
    }

    #[tokio::test]
    async fn a_look_past_the_deadline_is_a_timeout() {
        let slow = async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok::<f64, anyhow::Error>(0.5)
        };
        let err = within(Duration::from_millis(10), slow).await.unwrap_err();
        assert!(err.chain().any(|c| c.is::<Timeout>()), "{err:#}");
        assert_eq!(format!("{err:#}"), "timed out after 10 ms (JURL_VISION_TIMEOUT_MS)");
    }

    #[test]
    fn a_timeout_is_reported_as_one_not_as_a_download_error() {
        let url = url::Url::parse("https://example.test/page").unwrap();
        let timed_out = anyhow::Error::from(Timeout(Duration::from_millis(2500)));
        assert_eq!(
            why_no_look(&url, &timed_out, 3, 3),
            "no image answered within 2500 ms (JURL_VISION_TIMEOUT_MS) in https://example.test/page"
        );
        let download = anyhow!("connection reset");
        assert_eq!(
            why_no_look(&url, &download, 3, 3),
            "couldn't download any image in https://example.test/page: connection reset"
        );
    }

    #[test]
    fn with_more_images_than_it_looked_at_the_report_says_how_many_it_tried() {
        let url = url::Url::parse("https://example.test/page").unwrap();
        let download = anyhow!("connection reset");
        assert_eq!(
            why_no_look(&url, &download, 12, 73),
            "couldn't download any of the 12 images it looked at in https://example.test/page: connection reset"
        );
        let timed_out = anyhow::Error::from(Timeout(Duration::from_millis(2500)));
        assert_eq!(
            why_no_look(&url, &timed_out, 12, 73),
            "none of the 12 images it looked at answered within 2500 ms (JURL_VISION_TIMEOUT_MS) in https://example.test/page"
        );
    }

    /// An image of the site at `base`, the `i`th on its page.
    fn sample_image(i: usize, base: &str) -> Image {
        let url = url::Url::parse(&format!("{base}/{i}.png")).expect("a URL");
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
    fn an_image_says_whether_clef_looked_at_its_pixels() {
        let img = sample_image(0, "https://site.test");
        let looked = Looks(BTreeMap::from([(0, Ok(0.8))]));
        assert_eq!(image_json(&img, 0.7, &looked, true)["pixels"], json!("looked"));
        let refused = Looks(BTreeMap::from([(0, Err(anyhow::Error::new(fetch::ImageRefused("HTTP 403".into()))))]));
        assert_eq!(image_json(&img, 0.4, &refused, true)["pixels"], json!("refused"));
        let failed = Looks(BTreeMap::from([(0, Err(anyhow!("Clef: HTTP 500")))]));
        assert_eq!(image_json(&img, 0.4, &failed, true)["pixels"], json!("skipped"));
        assert_eq!(image_json(&img, 0.4, &Looks::default(), true)["pixels"], json!("skipped"), "past the look cap");
        assert!(image_json(&img, 0.4, &looked, false).get("pixels").is_none(), "--image looks at nothing");
    }

    #[test]
    fn the_notice_counts_the_kept_images_their_hosts_refused() {
        let imgs: Vec<Image> = (0..3).map(|i| sample_image(i, "https://site.test")).collect();
        let kept: Vec<(&Image, f64)> = imgs.iter().map(|i| (i, 0.5)).collect();
        let looks = Looks(BTreeMap::from([
            (0, Err(anyhow::Error::new(fetch::ImageRefused("HTTP 403".into())))),
            (1, Ok(0.9)),
            (2, Err(anyhow::Error::new(fetch::ImageRefused("HTTP 429".into())))),
        ]));
        let notice = refused_notice(&kept, &looks).expect("two were refused");
        assert!(notice.starts_with("jurl: 2 of the 3 kept images were refused"), "{notice}");
        assert!(refused_notice(&kept[1..2], &looks).is_none(), "an image that was looked at is not refused");
    }

    #[tokio::test]
    async fn a_host_that_refuses_its_first_image_gets_no_more_requests_from_the_run() {
        let (base, served) =
            fetch::test_server::serve_routed(|_| Some(fetch::test_server::reply("403 Forbidden", "", b"")));
        let images: Vec<Image> = (0..3).map(|i| sample_image(i, &base)).collect();
        let keys = ClefKeys { account: "account".into(), token: "token".into() };
        let clef = Client::new();
        let memo = fetch::Memo::default();
        let page = url::Url::parse(&base).expect("a URL");
        let req = LookRequest {
            retry: fetch::test_server::retry_on(&memo),
            page: &page,
            clef: &clef,
            keys: &keys,
            title: "A page",
            query: None,
        };
        let looks = look_all(Some(&req), images.iter().collect()).await;
        assert!(matches!(looks.get(0), Some(Err(e)) if e.downcast_ref::<fetch::ImageRefused>().is_some()));
        assert!(looks.get(1).is_none() && looks.get(2).is_none(), "the rest of the host's images are skipped");
        // The first image: one plain request and the browser client's retry. Nothing else reaches the host.
        assert_eq!(served.count(), 2);
    }

    #[tokio::test]
    async fn an_image_past_its_cap_is_not_read() {
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let url = url::Url::parse(&fetch::test_server::serve(head, vec![0u8; IMAGE_MAX + 1], true)).unwrap();
        let err = thumbnail(fetch::test_server::retry_at(&crate::reach::Reach::Private), &url, &url).await.unwrap_err();
        assert!(format!("{err:#}").ends_with("image larger than 15 MB"), "{err:#}");
    }

    #[tokio::test]
    async fn an_image_under_a_public_run_is_not_read_from_a_private_address() {
        // The page is public; its image sits on another loopback address, which the run never asks for.
        let reach = crate::reach::Reach::Public {
            allowed: std::collections::HashSet::from([std::net::IpAddr::from([127, 0, 0, 1])]),
        };
        let url = url::Url::parse("http://127.0.0.2:1/photo.png").unwrap();
        let err = thumbnail(fetch::test_server::retry_at(&reach), &url, &url).await.unwrap_err();
        assert_eq!(format!("{err:#}"), "not a public address");
    }

    #[tokio::test]
    async fn a_small_image_is_downloaded_and_shrunk_for_clef() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4).write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let png = cursor.into_inner();
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", png.len());
        let url = url::Url::parse(&fetch::test_server::serve(head, png, false)).unwrap();
        let data = thumbnail(fetch::test_server::retry_at(&crate::reach::Reach::Private), &url, &url).await.unwrap();
        assert!(data.starts_with("data:image/jpeg;base64,"), "{}", data.chars().take(40).collect::<String>());
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
    fn a_result_from_text_alone_says_so_when_clef_looked_at_no_image() {
        let failed = Looks(BTreeMap::from([(0, Err(anyhow!("down"))), (1, Err(anyhow!("down")))]));
        assert_eq!(
            text_only_notice(&failed, 2).as_deref(),
            Some("jurl: Clef couldn't look at any image, so this result is from text only (-t shows why)")
        );
        assert_eq!(
            text_only_notice(&failed, 80).as_deref(),
            Some(
                "jurl: Clef couldn't look at any of the 2 images it looked at, so this result is from text only (-t shows why)"
            )
        );
        let one_answer = Looks(BTreeMap::from([(0, Err(anyhow!("down"))), (1, Ok(0.4))]));
        assert_eq!(text_only_notice(&one_answer, 2), None);
        // --image: no look ran, so nothing was asked of Clef and there is nothing to say.
        assert_eq!(text_only_notice(&Looks::default(), 2), None);
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

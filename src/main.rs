mod answer;
mod blocks;
mod cli;
mod config;
mod decide;
mod extract;
mod fetch;
mod follow;
mod judge;
mod lightpanda;
mod links;
mod mcp;
mod output;
mod precise;
mod setup;
mod timing;
mod update;

use std::{collections::HashMap, io::stdout, process::ExitCode, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use clap::Parser;
use futures::future::join_all;
use reqwest::Client;
use serde_json::{Map, Value, json};

use crate::{
    cli::Args,
    config::Config,
    decide::{choice, is_api_error, noul},
    extract::{Extracted, Image, Kind},
    judge::{Ctx, Item},
    output::{Rendered, exit_code, not_found, stdout_for, write_out},
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

#[tokio::main]
async fn main() -> ExitCode {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            let _ = e.print();
            // A flag this jurl doesn't know may be one a newer jurl does.
            if e.kind() == clap::error::ErrorKind::UnknownArgument
                && let Some(hint) = update::hint().await
            {
                eprintln!("\n{hint}");
            }
            return ExitCode::from(e.exit_code() as u8);
        }
    };
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jurl: {e:#}");
            ExitCode::from(exit_code(&e))
        }
    }
}

async fn run(mut args: Args) -> Result<()> {
    let mut cfg = Config::load();
    let client = Client::builder()
        .user_agent(concat!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0) AppleWebKit/605.1.15 (KHTML, like Gecko) jurl/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()?;
    if args.url == "update" {
        return update::run().await;
    }
    if args.url == "init" {
        return setup::init(&mut cfg, &client).await;
    }
    if args.url == "mcp" {
        return mcp::serve(client).await;
    }
    prepare(&mut args)?;
    let key = setup::typesafe_key(&mut cfg, &client).await?;

    let mut t = Timer::new();
    let done = page(&args, &cfg, &client, &key, &mut t).await;
    // A miss costs tokens too: -t reports them either way.
    if args.timing {
        t.report();
    }
    if let Some(text) = stdout_for(&args, &done, &decide::USAGE)? {
        write_out(&mut stdout().lock(), &text)?;
    }
    done.map(|_| ())
}

/// The URL and flags made whole and checked: `--find` is a question for `--vision`, a bare domain gets https://.
fn prepare(args: &mut Args) -> Result<()> {
    if !args.url.contains("://") {
        args.url = format!("https://{}", args.url);
    }
    if let Some(what) = args.find.take() {
        if args.ask.is_some() {
            bail!("--find already is the question; drop --ask");
        }
        args.vision = true;
        args.ask = Some(what);
    }
    let modes = [args.image || args.vision, args.links, args.code].iter().filter(|m| **m).count();
    if modes > 1 {
        bail!("pick one of --image/--vision, --links, --code");
    }
    if args.precise && (args.image || args.vision || args.links) {
        bail!("--precise picks part of a block: use it with -q or --code -q, not images or links");
    }
    if args.precise && args.ask.is_none() {
        bail!("--precise needs a question: add -q \"…\"");
    }
    if args.follow.is_some() && (args.ask.is_none() || args.image || args.vision || args.links) {
        bail!("--follow looks for an answer: use it with -q (and --precise or --code), not images or links");
    }
    Ok(())
}

/// The page (or with --follow, the site) read in the mode `args` asks for: what the CLI prints and what `jurl mcp`
/// answers.
async fn page(args: &Args, cfg: &Config, client: &Client, key: &str, t: &mut Timer) -> Result<Rendered> {
    // Warm the API connections (TLS handshakes) while the page downloads.
    // Jev's host, or the one `JURL_JEV_URL` names.
    let mut hosts: Vec<String> = url::Url::parse(&decide::jev_url())
        .map(|u| format!("{}/", u.origin().ascii_serialization()))
        .into_iter()
        .collect();
    if args.vision {
        hosts.push("https://api.cloudflare.com/".to_string());
    }
    let warm = tokio::spawn({
        let c = client.clone();
        async move { join_all(hosts.into_iter().map(|h| c.head(h).send())).await }
    });
    let target: url::Url = args.url.parse().with_context(|| format!("bad url {}", args.url))?;
    read(args, cfg, client, key, target, warm, t).await
}

/// The page (or with --follow, the site) read in the mode asked for.
async fn read(
    args: &Args,
    cfg: &Config,
    client: &Client,
    key: &str,
    target: url::Url,
    warm: tokio::task::JoinHandle<impl Sized>,
    t: &mut Timer,
) -> Result<Rendered> {
    if args.follow.is_some() {
        let _ = warm.await;
        return follow::run(args, cfg, client, key, target, t).await;
    }
    let (url, ex) = load(args, cfg, client, &target, t).await?;
    let _ = warm.await;

    let ctx = Ctx::new(args, client, key, &url, &ex);
    if args.image || args.vision {
        images(&ctx, cfg, &ex, t).await
    } else if args.links {
        links::links(&ctx, &ex, t).await
    } else {
        blocks::blocks(&ctx, &ex, t).await
    }
}

/// Fetch a page (rendering it when it needs JavaScript) and cut it into blocks, links and images.
async fn load(
    args: &Args,
    cfg: &Config,
    client: &Client,
    target: &url::Url,
    t: &mut Timer,
) -> Result<(url::Url, Extracted)> {
    let page = if args.render {
        let bin = lightpanda::ensure(cfg.get("JURL_LIGHTPANDA")).await?;
        let page = fetch::render(&bin, target).await?;
        t.lap("render");
        page
    } else {
        let page = fetch::fetch(client, target.as_str()).await?;
        t.lap("fetch");
        page
    };
    let mut ex =
        if page.is_markdown { extract::markdown(&page.body, &page.url) } else { extract::html(&page.body, &page.url) };
    t.lap(if page.is_markdown { "extract(md)" } else { "extract" });
    decide::USAGE.pages.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // A JS app with (almost) no server-rendered text: render it instead of giving up.
    let text: usize = ex.blocks.iter().filter(|b| b.kind != Kind::Heading).map(|b| b.text.len()).sum();
    if !args.render && ex.app_shell && text < 300 {
        match lightpanda::ensure(cfg.get("JURL_LIGHTPANDA")).await {
            Ok(bin) => {
                eprintln!("jurl: no text without JavaScript, rendering with Lightpanda…");
                let rendered = fetch::render(&bin, &page.url).await?;
                ex = extract::html(&rendered.body, &rendered.url);
                t.lap("render");
            }
            Err(e) => eprintln!("jurl: {e:#}"),
        }
    }
    Ok((page.url, ex))
}

/// --image / --vision: content images, best first.
async fn images(ctx: &Ctx<'_>, cfg: &Config, ex: &Extracted, t: &mut Timer) -> Result<Rendered> {
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

    let looks: HashMap<usize, Result<f64>> = looks.into_iter().collect();
    let mut scored: Vec<(&Image, f64)> = ex
        .images
        .iter()
        .map(|img| {
            let p_text = a.noul(&format!("img{}", img.i)).unwrap_or(0.0);
            let p = match looks.get(&img.i) {
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
        && let Some(e) = looks.values().try_fold(None, |_, r| r.as_ref().err().map(Some)).flatten()
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

/// Clef on several images at once, each bounded by the vision deadline.
async fn look_all(
    client: &Client,
    clef_client: &Client,
    keys: Option<&(String, String)>,
    title: &str,
    imgs: Vec<&Image>,
    query: Option<&str>,
) -> Vec<(usize, Result<f64>)> {
    let Some((account, token)) = keys else { return Vec::new() };
    let deadline = vision_deadline();
    join_all(imgs.into_iter().map(|img| async move {
        let look = tokio::time::timeout(deadline, look(client, clef_client, account, token, title, img, query)).await;
        (img.i, look.unwrap_or_else(|_| Err(anyhow!("over {}ms (JURL_VISION_TIMEOUT_MS)", deadline.as_millis()))))
    }))
    .await
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

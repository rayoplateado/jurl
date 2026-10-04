mod config;
mod decide;
mod extract;
mod fetch;

use std::{
    collections::HashMap,
    io::{Write, stdout},
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use clap::Parser;
use futures::future::join_all;
use reqwest::Client;
use serde_json::{Map, Value, json};

use crate::{
    config::Config,
    decide::{Answers, choice, noul},
    extract::{Block, Extracted, Image, Kind},
};

/// curl, but it reads the page for you. Jev picks what matters; Clef looks at
/// the images. Everything printed is literally in the page.
#[derive(Parser)]
#[command(version)]
struct Args {
    url: String,
    /// Keep what helps answer this question instead of a general summary
    #[arg(short = 'q', long)]
    ask: Option<String>,
    /// Print the page's content images (one URL per line, best first)
    #[arg(short, long)]
    image: bool,
    /// With --image: let Clef look at the pixels (slower)
    #[arg(long)]
    vision: bool,
    /// Print the links worth following (one URL per line, best first)
    #[arg(short, long)]
    links: bool,
    /// Only code blocks: examples, commands, snippets
    #[arg(short, long)]
    code: bool,
    /// Run the page's JavaScript with Lightpanda first (automatic when a page has scripts but no text)
    #[arg(short, long)]
    render: bool,
    /// Max results [default: 12 blocks, 5 with --ask, 8 code blocks, 20 links, all images]
    #[arg(short = 'n', long)]
    max: Option<usize>,
    /// No max: keep everything that passes the threshold
    #[arg(short, long)]
    all: bool,
    /// Minimum probability to keep a result
    #[arg(long, default_value_t = 0.5)]
    threshold: f64,
    #[arg(long)]
    json: bool,
    /// Per-phase timings on stderr
    #[arg(short, long)]
    timing: bool,
}

impl Args {
    fn limit(&self, default: usize) -> usize {
        if self.all { usize::MAX } else { self.max.unwrap_or(default) }
    }

    /// The question asked about each candidate: the user's, or the mode's default.
    fn question(&self, target: &str, default: &str) -> Value {
        match &self.ask {
            Some(q) => noul(format!("{target} helps answer this question: {q}")),
            None => noul(format!("{target} {default}")),
        }
    }
}

/// One Jev request stays well under the ~64k token budget.
const MAX_STATE_CHARS: usize = 60_000;
const MAX_QUESTIONS: usize = 120;
const STATE_TEXT_CHARS: usize = 1_200;
/// --vision looks at this many images, starting while Jev is still thinking.
const VISION_MAX: usize = 12;
/// Past this, an image keeps its text-only score.
const VISION_DEADLINE: Duration = Duration::from_millis(2500);
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

struct Timer {
    start: Instant,
    last: Instant,
    phases: Vec<(String, Duration)>,
}

impl Timer {
    fn new() -> Self {
        let now = Instant::now();
        Self { start: now, last: now, phases: Vec::new() }
    }
    fn lap(&mut self, name: impl Into<String>) {
        let now = Instant::now();
        self.phases.push((name.into(), now - self.last));
        self.last = now;
    }
    fn report(&self) {
        let parts: Vec<_> = self.phases.iter().map(|(n, d)| format!("{n} {}ms", d.as_millis())).collect();
        eprintln!("⏱  {} · total {}ms", parts.join(" · "), self.start.elapsed().as_millis());
    }
}

/// Everything a mode needs to talk to Jev about one page.
struct Ctx<'a> {
    args: &'a Args,
    client: &'a Client,
    key: &'a str,
    url: &'a url::Url,
    title: &'a str,
}

/// A candidate for Jev: its entry in the state, plus a question if it is being judged
/// (headings ride along as context without one).
struct Item {
    id: String,
    state: Value,
    question: Option<Value>,
}

impl Ctx<'_> {
    /// Chunk items so each request fits the budget, ask all chunks in parallel, merge.
    async fn judge(&self, field: &str, items: Vec<Item>, extra: Map<String, Value>) -> Result<Answers> {
        let mut chunks: Vec<(Vec<Value>, Map<String, Value>)> = vec![(Vec::new(), extra)];
        let mut size = 0;
        for item in items {
            let len = item.state.to_string().len();
            let full = {
                let qs = &chunks.last().unwrap().1;
                !qs.is_empty() && (qs.len() >= MAX_QUESTIONS || size + len > MAX_STATE_CHARS)
            };
            if item.question.is_some() && full {
                chunks.push((Vec::new(), Map::new()));
                size = 0;
            }
            let (state, qs) = chunks.last_mut().unwrap();
            size += len;
            state.push(item.state);
            if let Some(q) = item.question {
                qs.insert(item.id, q);
            }
        }
        chunks.retain(|(_, qs)| !qs.is_empty());

        let n = chunks.len();
        let results = join_all(chunks.into_iter().map(|(entries, qs)| {
            let state = json!({ "title": self.title, "url": self.url.as_str(), field: entries });
            decide::jev(self.client, self.key, state, qs)
        }))
        .await;
        let mut merged = Answers { requests: n, ..Answers::default() };
        for r in results {
            let a = r?;
            merged.input_tokens += a.input_tokens;
            merged.answers.extend(a.answers);
        }
        Ok(merged)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jurl: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(mut args: Args) -> Result<()> {
    if !args.url.contains("://") {
        args.url = format!("https://{}", args.url);
    }
    let modes = [args.image || args.vision, args.links, args.code].iter().filter(|m| **m).count();
    if modes > 1 {
        bail!("pick one of --image/--vision, --links, --code");
    }
    let cfg = Config::load();
    let key = cfg.get("TYPESAFE_API_KEY").context("TYPESAFE_API_KEY not set (env or ~/.config/jurl/env)")?;
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0) AppleWebKit/605.1.15 (KHTML, like Gecko) jurl/0.1")
        .timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()?;

    let mut t = Timer::new();
    // Warm the API connections (TLS handshakes) while the page downloads.
    let mut hosts = vec!["https://api.typesafe.ai/"];
    if args.vision {
        hosts.push("https://api.cloudflare.com/");
    }
    let warm = tokio::spawn({
        let c = client.clone();
        async move { join_all(hosts.into_iter().map(|h| c.head(h).send())).await }
    });
    let lightpanda = fetch::lightpanda(cfg.get("JURL_LIGHTPANDA"));
    let target: url::Url = args.url.parse().with_context(|| format!("bad url {}", args.url))?;

    let page = if args.render {
        let bin = lightpanda.as_deref().context("--render needs Lightpanda (https://lightpanda.io) on PATH or JURL_LIGHTPANDA")?;
        let page = fetch::render(bin, &target).await?;
        t.lap("render");
        page
    } else {
        let page = fetch::fetch(&client, &args.url).await?;
        t.lap("fetch");
        page
    };
    let mut ex = if page.is_markdown {
        extract::markdown(&page.body, &page.url)
    } else {
        extract::html(&page.body, &page.url)
    };
    t.lap(if page.is_markdown { "extract(md)" } else { "extract" });

    // A JS app with (almost) no server-rendered text: render it instead of giving up.
    let text: usize = ex.blocks.iter().filter(|b| b.kind != Kind::Heading).map(|b| b.text.len()).sum();
    if !args.render && ex.app_shell && text < 300 {
        if let Some(bin) = &lightpanda {
            eprintln!("jurl: no text without JavaScript, rendering with Lightpanda…");
            let rendered = fetch::render(bin, &page.url).await?;
            ex = extract::html(&rendered.body, &rendered.url);
            t.lap("render");
        }
    }
    let _ = warm.await;

    let ctx = Ctx { args: &args, client: &client, key: &key, url: &page.url, title: &ex.title };
    if args.image || args.vision {
        images(&ctx, &cfg, &ex, &mut t).await?;
    } else if args.links {
        links(&ctx, &ex, &mut t).await?;
    } else {
        blocks(&ctx, &ex, &mut t).await?;
    }
    if args.timing {
        t.report();
    }
    Ok(())
}

/// Default mode and --code: pick blocks, print them in page order.
async fn blocks(ctx: &Ctx<'_>, ex: &Extracted, t: &mut Timer) -> Result<()> {
    let args = ctx.args;
    let is_candidate = |b: &Block| match b.kind {
        Kind::Heading => false,
        Kind::Code => !b.text.trim().is_empty(),
        _ => !args.code && b.text.chars().count() >= 25,
    };
    if !ex.blocks.iter().any(is_candidate) {
        if args.code {
            bail!("no code blocks in {}", ctx.url);
        }
        bail!("no readable content in {} (JS-rendered? install Lightpanda, https://lightpanda.io, and jurl renders it)", ctx.url);
    }

    let default = if args.code {
        "is a useful code example, command or snippet for this page's topic, not boilerplate."
    } else {
        "carries core information of this page — what a reader came here for — rather than navigation, \
         boilerplate, cookie or legal text, promos, sign-up prompts, author bios or comments."
    };
    let items = ex
        .blocks
        .iter()
        .map(|b| Item {
            id: format!("b{}", b.i),
            state: json!({ "i": b.i, "kind": b.kind, "text": b.text.chars().take(STATE_TEXT_CHARS).collect::<String>() }),
            question: is_candidate(b).then(|| args.question(&format!("The block in `blocks` with i={}", b.i), default)),
        })
        .collect();
    let extra = Map::from_iter([(
        "page_kind".to_string(),
        choice(
            "What kind of page is this?",
            json!({
                "article": "News, blog post, essay or story",
                "docs": "Technical documentation, reference, tutorial or README",
                "product": "A product, service or pricing page",
                "repo": "A code repository page",
                "listing": "Index, search results, feed or category page",
                "forum": "Discussion thread, Q&A or comments",
                "other": null,
            }),
        ),
    )]);
    let a = ctx.judge("blocks", items, extra).await?;
    t.lap(a.label());
    let kind = a.choice("page_kind");
    let scores: Vec<Option<f64>> = ex.blocks.iter().map(|b| a.noul(&format!("b{}", b.i))).collect();

    // Top-N by probability, printed in page order. Headings survive when their section does.
    let default_max = if args.code { 8 } else if args.ask.is_some() { 5 } else { 12 };
    let keep = top(&scores, args.threshold, args.limit(default_max));
    let mut pending_heading = None;
    let mut selected = Vec::new();
    for b in &ex.blocks {
        if b.kind == Kind::Heading {
            pending_heading = Some(b);
        } else if keep.contains_key(&b.i) {
            if let Some(h) = pending_heading.take() {
                selected.push(h);
            }
            selected.push(b);
        }
    }
    if selected.is_empty() && args.ask.is_some() {
        bail!("nothing in {} answers that (try a lower --threshold)", ctx.url);
    }

    let mut out = stdout().lock();
    if args.json {
        let blocks: Vec<_> = selected
            .iter()
            .map(|b| {
                let mut v = serde_json::to_value(b).unwrap();
                if let Some(p) = scores[b.i] {
                    v["p"] = json!(p);
                }
                v
            })
            .collect();
        let doc = json!({
            "url": ctx.url.as_str(),
            "title": ex.title,
            "ask": args.ask,
            "kind": kind.as_ref().map(|k| json!({ "choice": k.0, "confidence": k.1 })),
            "blocks": blocks,
        });
        writeln!(out, "{}", serde_json::to_string_pretty(&doc)?)?;
    } else {
        if !ex.title.is_empty() {
            writeln!(out, "# {}\n", ex.title)?;
        }
        let kind = kind.map(|(k, c)| format!(" · {k} ({c:.2})")).unwrap_or_default();
        writeln!(out, "<{}>{kind}\n", ctx.url)?;
        for b in selected {
            writeln!(out, "{}\n", b.markdown())?;
        }
    }
    Ok(())
}

/// --links: the links worth following, best first.
async fn links(ctx: &Ctx<'_>, ex: &Extracted, t: &mut Timer) -> Result<()> {
    if ex.links.is_empty() {
        bail!("no links found in {}", ctx.url);
    }
    let items = ex
        .links
        .iter()
        .map(|l| Item {
            id: format!("l{}", l.i),
            state: json!({ "i": l.i, "text": l.text, "context": l.context, "host": l.url.host_str() }),
            question: Some(ctx.args.question(
                &format!("The link in `links` with i={}", l.i),
                "points to something a reader of this page would want to follow — referenced articles, sources, \
                 docs, downloads or related content — not site navigation, login, social sharing, legal pages or ads.",
            )),
        })
        .collect();
    let a = ctx.judge("links", items, Map::new()).await?;
    t.lap(a.label());
    let scores: Vec<Option<f64>> = ex.links.iter().map(|l| a.noul(&format!("l{}", l.i))).collect();
    let mut kept: Vec<_> = top(&scores, ctx.args.threshold, ctx.args.limit(20)).into_iter().collect();
    kept.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut out = stdout().lock();
    if ctx.args.json {
        let v: Vec<_> = kept
            .iter()
            .map(|(i, p)| {
                let l = &ex.links[*i];
                json!({ "url": l.url.as_str(), "text": l.text, "p": p })
            })
            .collect();
        writeln!(out, "{}", serde_json::to_string_pretty(&json!({ "url": ctx.url.as_str(), "title": ex.title, "links": v }))?)?;
    } else {
        for (i, _) in kept {
            writeln!(out, "{}", ex.links[i].url)?;
        }
    }
    Ok(())
}

/// --image / --vision: content images, best first.
async fn images(ctx: &Ctx<'_>, cfg: &Config, ex: &Extracted, t: &mut Timer) -> Result<()> {
    if ex.images.is_empty() {
        bail!("no images found in {}", ctx.url);
    }
    let clef_keys = if ctx.args.vision {
        let account = cfg.get("CLOUDFLARE_ACCOUNT_ID").context("--vision needs CLOUDFLARE_ACCOUNT_ID")?;
        let token = cfg.get("CLOUDFLARE_AI_TOKEN").context("--vision needs CLOUDFLARE_AI_TOKEN")?;
        Some((account, token))
    } else {
        None
    };

    // Text-only judgement: alt, caption, file name and size are usually enough.
    let items = ex
        .images
        .iter()
        .map(|i| Item {
            id: format!("img{}", i.i),
            state: json!({
                "i": i.i,
                "file": i.url.path_segments().and_then(|mut s| s.next_back()).unwrap_or(""),
                "alt": i.alt,
                "caption": i.caption,
                "width": i.width,
                "height": i.height,
            }),
            question: Some(ctx.args.question(
                &format!("The image in `images` with i={}", i.i),
                "is meaningful content of this page (photo, diagram, chart, screenshot, illustration or product \
                 shot), not a logo, icon, avatar, ad, badge or decoration.",
            )),
        })
        .collect();

    // Clef starts at the same time as Jev, on the first images in page order (the noise
    // filter already dropped icons and trackers), so --vision costs max(jev, clef), not the sum.
    // One HTTP/1 connection per Clef call: multiplexing them all over a single HTTP/2
    // connection measured ~2x slower at the tail.
    let clef_client = Client::builder().http1_only().timeout(Duration::from_secs(10)).build()?;
    let looking = async {
        let Some((account, token)) = &clef_keys else { return Vec::new() };
        let clef_client = &clef_client;
        join_all(ex.images.iter().take(VISION_MAX).map(|img| async move {
            let look = tokio::time::timeout(VISION_DEADLINE, look(ctx.client, clef_client, account, token, &ex.title, img)).await;
            (img.i, look.unwrap_or_else(|_| Err(anyhow!("over {}ms", VISION_DEADLINE.as_millis()))))
        }))
        .await
    };
    let (a, looks) = tokio::join!(ctx.judge("images", items, Map::new()), looking);
    let a = a?;
    t.lap(if ctx.args.vision { format!("{} ‖ clef({} img)", a.label(), looks.len()) } else { a.label() });

    let looks: HashMap<usize, Result<f64>> = looks.into_iter().collect();
    let mut scored: Vec<(&Image, f64)> = ex
        .images
        .iter()
        .map(|img| {
            let p_text = a.noul(&format!("img{}", img.i)).unwrap_or(0.0);
            let p = match looks.get(&img.i) {
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

    let kept: Vec<_> = scored
        .into_iter()
        .filter(|(_, p)| *p >= ctx.args.threshold)
        .take(ctx.args.limit(usize::MAX))
        .collect();
    let mut out = stdout().lock();
    if ctx.args.json {
        let v: Vec<_> = kept
            .iter()
            .map(|(i, p)| json!({ "url": i.url.as_str(), "alt": i.alt, "caption": i.caption, "p": p }))
            .collect();
        writeln!(out, "{}", serde_json::to_string_pretty(&json!({ "url": ctx.url.as_str(), "title": ex.title, "images": v }))?)?;
    } else {
        for (i, _) in kept {
            writeln!(out, "{}", i.url)?;
        }
    }
    Ok(())
}

/// Clef's view of one image: probability that it shows content rather than chrome.
async fn look(client: &Client, clef_client: &Client, account: &str, token: &str, title: &str, img: &Image) -> Result<f64> {
    let data = thumbnail(client, &img.preview).await?;
    let state = json!({ "page_title": title, "alt": img.alt, "caption": img.caption });
    // Clef answers "what is this?" far better than "does this matter?", so ask the
    // factual question and add up the content classes here.
    let qs = Map::from_iter([("shows".to_string(), choice("What does the attached image show?", VISUAL_KINDS.clone()))]);
    let call = || decide::clef(clef_client, account, token, state.clone(), qs.clone(), vec![data.clone()]);
    let a = hedged(call, VISION_HEDGE).await?;
    let probs = a.probabilities("shows").context("clef returned no answer")?;
    Ok(CONTENT_KINDS.iter().filter_map(|k| probs.get(*k)).sum())
}

/// Start `call`; if it hasn't finished after `after`, start a second one and take whichever wins.
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
    tokio::select! {
        r = &mut first => r,
        r = &mut second => r,
    }
}

/// Indices of the `max` best scores at or above `threshold`.
fn top(scores: &[Option<f64>], threshold: f64, max: usize) -> HashMap<usize, f64> {
    let mut ranked: Vec<(usize, f64)> =
        scores.iter().enumerate().filter_map(|(i, p)| p.filter(|&p| p >= threshold).map(|p| (i, p))).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.truncate(max);
    ranked.into_iter().collect()
}

/// Download and shrink to a small JPEG: fewer vision tokens, faster Clef.
async fn thumbnail(client: &Client, url: &url::Url) -> Result<String> {
    let bytes = fetch::fetch_bytes(client, url).await?;
    tokio::task::spawn_blocking(move || -> Result<String> {
        let img = image::load_from_memory(&bytes)?;
        let img = if img.width() > VISION_PX || img.height() > VISION_PX { img.thumbnail(VISION_PX, VISION_PX) } else { img };
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 75).encode_image(&img.to_rgb8())?;
        Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
    })
    .await?
}

mod config;
mod decide;
mod extract;
mod fetch;
mod follow;
mod lightpanda;
mod links;
mod precise;
mod setup;
mod update;

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
    extract::{Block, Extracted, Image, Kind, Link},
};

/// curl, but it reads the page for you. Jev picks what matters; Clef looks at
/// the images. Everything printed is literally in the page.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// The page to read, `init` to set up your API keys, or `update` to install the latest jurl
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
    /// Find the images that show this ("a cathedral"): Clef looks at every candidate
    #[arg(short, long, value_name = "WHAT")]
    find: Option<String>,
    /// Print the links worth following (one URL per line, best first)
    #[arg(short, long)]
    links: bool,
    /// Only code blocks: examples, commands, snippets
    #[arg(short, long)]
    code: bool,
    /// With -q: print just the answer, in the page's own words, then the block it's in and a link to it.
    /// Exits with an error when no part of the page is exactly the answer
    #[arg(short, long)]
    precise: bool,
    /// With -q: when the page doesn't answer, follow its links within the same site, most promising first,
    /// reading up to this many pages in all [default: 5]
    #[arg(long, value_name = "PAGES", num_args = 0..=1, default_missing_value = "5")]
    follow: Option<usize>,
    /// Run the page's JavaScript with Lightpanda first (automatic when a page has scripts but no text)
    #[arg(short, long)]
    render: bool,
    /// Max results [default: 12 blocks, 5 with --ask, 8 code blocks, 20 links, all images]
    #[arg(short = 'n', long)]
    max: Option<usize>,
    /// No max: keep everything that passes the threshold
    #[arg(short, long)]
    all: bool,
    /// Minimum probability to keep a result [default: 0.5, or 0.4 for the --precise answer]
    #[arg(long)]
    threshold: Option<f64>,
    #[arg(long)]
    json: bool,
    /// Per-phase timings on stderr
    #[arg(short, long)]
    timing: bool,
}

impl Args {
    fn threshold(&self) -> f64 {
        self.threshold.unwrap_or(0.5)
    }

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

/// The answer's share of one choice over every span and "none". On 30 pricing pages every answer at or above
/// this was right; the wrong ones scored 0.37 or less.
const PRECISE_THRESHOLD: f64 = 0.4;
/// Blocks below this aren't searched for an answer at all.
const PRECISE_BLOCK_FLOOR: f64 = 0.1;
/// One Jev request stays well under the ~64k token budget.
const MAX_STATE_CHARS: usize = 60_000;
const MAX_QUESTIONS: usize = 120;
const STATE_TEXT_CHARS: usize = 1_200;
/// --links and --image don't send the blocks, so Jev sees this much page text to spot a block page.
const EXCERPT_CHARS: usize = 800;
/// Past this, the page is a block page (rate limit, bot check…) standing in for the real one.
const BLOCKED_P: f64 = 0.8;
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
    excerpt: String,
}

/// A candidate for Jev: its entry in the state, plus a question if it is being judged
/// (headings ride along as context without one).
struct Item {
    id: String,
    state: Value,
    question: Option<Value>,
}

impl<'a> Ctx<'a> {
    fn new(args: &'a Args, client: &'a Client, key: &'a str, url: &'a url::Url, ex: &'a Extracted) -> Self {
        let excerpt = ex.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
        let excerpt = excerpt.chars().take(EXCERPT_CHARS).collect();
        Ctx { args, client, key, url, title: &ex.title, excerpt }
    }

    /// Chunk items so each request fits the budget, ask all chunks in parallel, merge.
    /// A page that is really a rate-limit or bot-check interstitial fails instead of printing nothing.
    async fn judge(&self, field: &str, items: Vec<Item>, mut extra: Map<String, Value>) -> Result<Answers> {
        extra.insert(
            "blocked".to_string(),
            noul(
                "This page is an access-denied, rate-limit, CAPTCHA, bot-check or 'enable JavaScript/cookies' \
                 interstitial standing in for the real page, not a page whose content merely discusses those topics.",
            ),
        );
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
            let mut state = json!({ "title": self.title, "url": self.url.as_str(), field: entries });
            if field != "blocks" {
                state["page_text"] = json!(self.excerpt);
            }
            decide::jev(self.client, self.key, state, qs)
        }))
        .await;
        let mut merged = Answers { requests: n, ..Answers::default() };
        for r in results {
            let a = r?;
            merged.input_tokens += a.input_tokens;
            merged.answers.extend(a.answers);
        }
        if let Some(p) = merged.noul("blocked").filter(|&p| p >= BLOCKED_P) {
            let title = if self.title.is_empty() { String::new() } else { format!(": \"{}\"", self.title) };
            bail!(
                "{} served a block page (rate limit, bot check or access denied), not its content{title} (p={p:.2})",
                self.url
            );
        }
        Ok(merged)
    }
}

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
            ExitCode::FAILURE
        }
    }
}

async fn run(mut args: Args) -> Result<()> {
    let mut cfg = Config::load();
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0) AppleWebKit/605.1.15 (KHTML, like Gecko) jurl/0.1")
        .timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()?;
    if args.url == "update" {
        return update::run().await;
    }
    if args.url == "init" {
        return setup::init(&mut cfg, &client).await;
    }
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
    let key = setup::typesafe_key(&mut cfg, &client).await?;

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
    let target: url::Url = args.url.parse().with_context(|| format!("bad url {}", args.url))?;
    if args.follow.is_some() {
        let _ = warm.await;
        follow::run(&args, &cfg, &client, &key, target, &mut t).await?;
        if args.timing {
            t.report();
        }
        return Ok(());
    }

    let (url, ex) = load(&args, &cfg, &client, &target, &mut t).await?;
    let _ = warm.await;

    let ctx = Ctx::new(&args, &client, &key, &url, &ex);
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

/// Default mode and --code: pick blocks, print them in page order.
async fn blocks(ctx: &Ctx<'_>, ex: &Extracted, t: &mut Timer) -> Result<()> {
    let args = ctx.args;
    let (scores, kind) = score_blocks(ctx, ex, t).await?;

    // Top-N by probability, printed in page order. Headings survive when their section does.
    let default_max = if args.code {
        8
    } else if args.ask.is_some() {
        5
    } else {
        12
    };
    // --precise looks inside the best few blocks even when none of them answers on its own: whether a span of
    // them is the answer is decided next, at the span's own threshold.
    if args.precise {
        let keep = top(&scores, PRECISE_BLOCK_FLOOR, 3);
        if keep.is_empty() {
            bail!("nothing in {} answers that", ctx.url);
        }
        let pick = precise_pick(ctx, ex, &keep, t).await?;
        return print_precise(ctx, ex, &pick, None);
    }
    let keep = top(&scores, args.threshold(), args.limit(default_max));
    print_blocks(ctx, ex, &scores, &keep, kind, None)
}

/// Jev's probability for each block (None for headings and blocks too short to judge alone), and the page's kind.
async fn score_blocks(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    t: &mut Timer,
) -> Result<(Vec<Option<f64>>, Option<(String, f64)>)> {
    let args = ctx.args;
    let is_candidate = |b: &Block| match b.kind {
        Kind::Heading => false,
        Kind::Code => !b.text.trim().is_empty(),
        _ => !args.code && b.text.chars().count() >= extract::SHORT_BLOCK_CHARS,
    };
    if !ex.blocks.iter().any(is_candidate) {
        if args.code {
            bail!("no code blocks in {}", ctx.url);
        }
        if args.render {
            bail!("no readable content in {}", ctx.url);
        }
        bail!("no readable content in {} (if it needs JavaScript, try --render)", ctx.url);
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
    Ok((scores, kind))
}

/// The kept blocks in page order, each with its heading, as markdown or JSON. `path` is how --follow got here.
fn print_blocks(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    scores: &[Option<f64>],
    keep: &HashMap<usize, f64>,
    kind: Option<(String, f64)>,
    path: Option<&[url::Url]>,
) -> Result<()> {
    let args = ctx.args;
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
    if selected.is_empty() {
        if args.ask.is_some() {
            bail!("nothing in {} answers that (try a lower --threshold)", ctx.url);
        }
        bail!("nothing in {} looks like content (try a lower --threshold)", ctx.url);
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
        let mut doc = json!({
            "url": ctx.url.as_str(),
            "title": ex.title,
            "ask": args.ask,
            "kind": kind.as_ref().map(|k| json!({ "choice": k.0, "confidence": k.1 })),
            "blocks": blocks,
        });
        if let Some(path) = path {
            doc["path"] = json!(path.iter().map(url::Url::as_str).collect::<Vec<_>>());
        }
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

/// The --precise answer: a byte range of one block's text, and how sure Jev is that it's exactly the answer.
struct Pick {
    block: usize,
    range: std::ops::Range<usize>,
    p: f64,
}

/// --precise: Jev scores spans of the best blocks as the exact answer.
async fn precise_pick(ctx: &Ctx<'_>, ex: &Extracted, keep: &HashMap<usize, f64>, t: &mut Timer) -> Result<Pick> {
    let q = ctx.args.ask.as_deref().unwrap_or_default();
    let mut ranked: Vec<(&usize, &f64)> = keep.iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(a.1));
    let top: Vec<&Block> = ranked.iter().take(3).map(|(i, _)| &ex.blocks[**i]).collect();
    let spans = precise::candidates(&top);
    if spans.is_empty() {
        bail!("nothing in {} answers that exactly", ctx.url);
    }

    // One choice over every span, plus "none": Jev weighs the spans against each other, so the one that is
    // exactly the answer beats the sentence around it, and "none" wins when the page doesn't say it.
    let items: Vec<Item> = top
        .iter()
        .map(|b| Item {
            id: format!("ctx{}", b.i),
            state: json!({ "block": b.i, "text": b.text.chars().take(STATE_TEXT_CHARS).collect::<String>() }),
            question: None,
        })
        .collect();
    let mut criteria = Map::new();
    for (k, s) in spans.iter().enumerate() {
        criteria.insert(format!("s{k}"), json!(s.label.as_deref().unwrap_or(&top[s.block].text[s.range.clone()])));
    }
    criteria.insert("none".to_string(), json!("None of these is exactly the answer"));
    let pick = choice(
        &format!(
            "Which of these spans from the blocks is exactly the answer to this question, with nothing missing \
             and nothing extra? {q}"
        ),
        Value::Object(criteria),
    );
    let a = ctx.judge("blocks", items, Map::from_iter([("pick".to_string(), pick)])).await?;
    t.lap(a.label());

    // Most likely span wins; on a tie (to two decimals) the shorter one, which says the same with less.
    let probs = a.probabilities("pick").context("jev returned no answer")?;
    let mut scored: Vec<(&precise::Span, f64)> =
        spans.iter().enumerate().map(|(k, s)| (s, probs.get(&format!("s{k}")).copied().unwrap_or(0.0))).collect();
    let key = |(s, p): &(&precise::Span, f64)| ((-p * 100.0).round() as i64, s.range.len());
    scored.sort_by_key(key);
    let (mut best, own) = scored[0];
    // "$8", "$8 per user" and "$8 per user, per month" are one answer with more or less around it, and they split
    // the vote. How sure jurl is that the answer is there counts them together: the winner plus every span that
    // holds it or sits inside it.
    let nested = |s: &precise::Span| {
        s.block == best.block
            && s.range != best.range
            && ((s.range.start <= best.range.start && s.range.end >= best.range.end)
                || (s.range.start >= best.range.start && s.range.end <= best.range.end))
    };
    let p = (own + scored.iter().filter(|(s, _)| nested(s)).map(|(_, p)| p).sum::<f64>()).min(1.0);
    let p = (p * 100.0).round() / 100.0;
    let threshold = ctx.args.threshold.unwrap_or(PRECISE_THRESHOLD);

    // The winner can carry more than the answer ("2009; 17 years ago"). When shorter candidates sit inside it,
    // ask once more, among just those and the winner, which one is the answer with nothing extra.
    let inside: Vec<&precise::Span> = spans
        .iter()
        .filter(|s| s.block == best.block && s.range != best.range)
        .filter(|s| s.range.start >= best.range.start && s.range.end <= best.range.end)
        .collect();
    if p >= threshold && !inside.is_empty() {
        let options: Vec<&precise::Span> = std::iter::once(best).chain(inside).collect();
        let mut criteria = Map::new();
        for (k, s) in options.iter().enumerate() {
            criteria.insert(format!("o{k}"), json!(&top[s.block].text[s.range.clone()]));
        }
        let tighter = choice(
            &format!(
                "All of these say the answer to this question. Which one is exactly the answer, without any \
                 extra words around it? {q}"
            ),
            Value::Object(criteria),
        );
        let context = vec![Item {
            id: "ctx".to_string(),
            state: json!({ "block": top[best.block].i, "text": top[best.block].text.chars().take(STATE_TEXT_CHARS).collect::<String>() }),
            question: None,
        }];
        let b = ctx.judge("blocks", context, Map::from_iter([("tighter".to_string(), tighter)])).await?;
        t.lap(b.label());
        if let Some((k, c)) = b.choice("tighter")
            && c >= 0.5
            && let Some(i) = k.strip_prefix('o').and_then(|i| i.parse::<usize>().ok())
            && let Some(s) = options.get(i)
        {
            best = s;
        }
    }
    Ok(Pick { block: top[best.block].i, range: best.range.clone(), p })
}

/// The answer on its own line, then the block it's in and a link to it; JSON says how sure. `path` is how --follow got
/// here.
fn print_precise(ctx: &Ctx<'_>, ex: &Extracted, pick: &Pick, path: Option<&[url::Url]>) -> Result<()> {
    let q = ctx.args.ask.as_deref().unwrap_or_default();
    let threshold = ctx.args.threshold.unwrap_or(PRECISE_THRESHOLD);
    let block = &ex.blocks[pick.block];
    let answer = &block.text[pick.range.clone()];
    let link = precise::link(ctx.url, &block.text, &pick.range);
    let p = pick.p;

    let mut out = stdout().lock();
    if ctx.args.json {
        let mut doc = json!({
            "url": ctx.url.as_str(),
            "title": ex.title,
            "ask": q,
            "answer": (p >= threshold).then_some(answer),
            "closest": (p < threshold).then_some(answer),
            "p": p,
            "quote": block.text,
            "block": block.i,
            "link": link,
        });
        if let Some(path) = path {
            doc["path"] = json!(path.iter().map(url::Url::as_str).collect::<Vec<_>>());
        }
        writeln!(out, "{}", serde_json::to_string_pretty(&doc)?)?;
        return Ok(());
    }
    if p < threshold {
        bail!("no part of {} is exactly the answer (closest: \"{answer}\", p={p:.2})", ctx.url);
    }
    writeln!(out, "{answer}\n\n{}\n\n<{link}>", block.markdown())?;
    Ok(())
}

/// --links: the links worth following, best first. With -q, the links most likely to lead to the answer: what
/// --follow opens, menus and footers included (a nav bar's "Pricing" is often the way to a price).
async fn links(ctx: &Ctx<'_>, ex: &Extracted, t: &mut Timer) -> Result<()> {
    let (candidates, scores): (Vec<Link>, Vec<Option<f64>>) = if ctx.args.ask.is_some() {
        let candidates = links::candidates(ctx, ex, |_| true);
        if candidates.is_empty() {
            bail!("no links found in {}", ctx.url);
        }
        let scores = links::score(ctx, &candidates, "Following the link in `links`", false).await?;
        t.lap(format!("{} links", candidates.len()));
        (candidates, scores.into_iter().map(Some).collect())
    } else {
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
        (ex.links.clone(), ex.links.iter().map(|l| a.noul(&format!("l{}", l.i))).collect())
    };
    let mut kept: Vec<_> = top(&scores, ctx.args.threshold(), ctx.args.limit(20)).into_iter().collect();
    if kept.is_empty() {
        bail!("no links worth following in {} (try a lower --threshold)", ctx.url);
    }
    kept.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut out = stdout().lock();
    if ctx.args.json {
        let v: Vec<_> = kept
            .iter()
            .map(|(i, p)| {
                let l = &candidates[*i];
                json!({ "url": l.url.as_str(), "text": l.text, "p": p })
            })
            .collect();
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&json!({ "url": ctx.url.as_str(), "title": ex.title, "links": v }))?
        )?;
    } else {
        for (i, _) in kept {
            writeln!(out, "{}", candidates[i].url)?;
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
            id: format!("img{}", i.i),
            state: json!({
                "i": i.i,
                "file": i.url.path_segments().and_then(|mut s| s.next_back()).unwrap_or(""),
                "alt": i.alt,
                "caption": i.caption,
                "width": i.width,
                "height": i.height,
            }),
            question: Some(match &ctx.args.ask {
                Some(q) => noul(format!(
                    "Judging by its file name, alt text and caption, the image in `images` with i={} shows: {q}",
                    i.i
                )),
                None => noul(format!(
                    "The image in `images` with i={} is meaningful content of this page (photo, diagram, chart, \
                     screenshot, illustration or product shot), not a logo, icon, avatar, ad, badge or decoration.",
                    i.i
                )),
            }),
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
    if kept.is_empty()
        && let (Some(q), Some((url, p))) = (query, best)
    {
        bail!("no image in {} looks like \"{q}\" (closest: {url}, p={p:.2})", ctx.url);
    }
    if kept.is_empty() {
        bail!("no content images in {} (try a lower --threshold)", ctx.url);
    }
    let mut out = stdout().lock();
    if ctx.args.json {
        let v: Vec<_> = kept
            .iter()
            .map(|(i, p)| json!({ "url": i.url.as_str(), "alt": i.alt, "caption": i.caption, "p": p }))
            .collect();
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&json!({ "url": ctx.url.as_str(), "title": ex.title, "images": v }))?
        )?;
    } else {
        for (i, _) in kept {
            writeln!(out, "{}", i.url)?;
        }
    }
    Ok(())
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
        let img =
            if img.width() > VISION_PX || img.height() > VISION_PX { img.thumbnail(VISION_PX, VISION_PX) } else { img };
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 75).encode_image(&img.to_rgb8())?;
        Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
    })
    .await?
}

//! jurl cloud: which account a run reads with, and the read itself. The cloud reads the page on its servers and sends the
//! answer back, which is printed the way a local run prints one.

use std::{
    sync::atomic::Ordering::Relaxed,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::{Value, json};
use url::Url;

use crate::{
    blocks::{DEFAULT_ASK_BLOCKS, DEFAULT_BLOCKS, DEFAULT_CODE_BLOCKS},
    cli::Args,
    config::Config,
    decide,
    extract::{Block, Kind},
    follow,
    links::DEFAULT_LINKS,
    output::{Rendered, missed, not_found},
    timing::Timer,
};

/// Where jurl cloud is, when `JURL_CLOUD_URL` doesn't say.
pub(crate) const DEFAULT_BASE: &str = "https://cloud.jurl.dev";
/// A read can take a while (a `--follow` search reads up to 15 pages), so it waits longer than the 20 s every other
/// request gets.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// The `--follow` lengths jurl cloud reads.
const FOLLOW_PAGES: [usize; 3] = [5, 10, 15];
/// The most results a read asks for (`-n`). The server refuses more, so a run past it uses your own keys.
const MAX_RESULTS: usize = 50;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// The account a read is made with: where jurl cloud is, and the key.
pub(crate) struct Cloud {
    pub(crate) base: String,
    pub(crate) key: String,
}

/// The account this run reads with, when it's jurl cloud. None means your own keys.
pub(crate) fn in_use(cfg: &Config) -> Result<Option<Cloud>> {
    let env_key = Config::from_env("JURL_CLOUD_KEY");
    let saved_key = cfg.saved("JURL_CLOUD_KEY");
    let own_in_env = Config::from_env("TYPESAFE_API_KEY").is_some() || decide::custom_jev_url().is_some();
    if !chosen(env_key.is_some(), own_in_env, saved_key.is_some()) {
        return Ok(None);
    }
    let Some(key) = env_key.or(saved_key) else { return Ok(None) };
    Ok(Some(Cloud { base: base(cfg)?, key }))
}

/// Whether a run reads through jurl cloud, from where its settings come. A `JURL_CLOUD_KEY` in the environment means
/// cloud. `TYPESAFE_API_KEY` or `JURL_JEV_URL` in the environment means your own keys, whatever a login saved. Without
/// either, a saved cloud key (from `jurl login`) means cloud, and without one your own keys are used.
fn chosen(cloud_in_env: bool, own_in_env: bool, cloud_saved: bool) -> bool {
    cloud_in_env || (cloud_saved && !own_in_env)
}

/// Where jurl cloud is: `JURL_CLOUD_URL` from the environment or the saved login, else [`DEFAULT_BASE`].
pub(crate) fn base(cfg: &Config) -> Result<String> {
    checked_base(&cfg.get("JURL_CLOUD_URL").unwrap_or_else(|| DEFAULT_BASE.to_string()))
}

/// The address without a trailing slash, if a key may be sent to it: https, or plain http to this computer.
pub(crate) fn checked_base(url: &str) -> Result<String> {
    let base = url.trim().trim_end_matches('/').to_string();
    if Url::parse(&base).is_err() {
        bail!("JURL_CLOUD_URL is not a URL: {base}");
    }
    if decide::plain_http_off_loopback(&base) {
        bail!("not sending a key over plain http to {base}: use https, or localhost");
    }
    Ok(base)
}

/// What jurl cloud answered: the status, `Retry-After` in seconds when it sent one, and the body.
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    retry_after: Option<u64>,
    pub(crate) text: String,
}

/// Sends a request to jurl cloud and reads its whole answer. A status that isn't a success is an answer too: the caller
/// says what it means. Only a failure to reach jurl cloud, or to read what it sent, is an error here.
pub(crate) async fn send(req: RequestBuilder, base: &str) -> Result<Reply> {
    let res = req.send().await.map_err(|e| anyhow!("couldn't reach jurl cloud at {base}: {e}"))?;
    let status = res.status();
    let retry_after = res.headers().get("retry-after").and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok());
    let text = res.text().await.map_err(|e| anyhow!("couldn't read what jurl cloud sent: {e}"))?;
    Ok(Reply { status, retry_after, text })
}

/// The error for an answer that isn't a success: the server's own message (it is safe to show), and what to do about it.
pub(crate) fn refused(reply: &Reply) -> anyhow::Error {
    let body: Value = serde_json::from_str(&reply.text).unwrap_or(Value::Null);
    let said = match body["message"].as_str().or(body["error"].as_str()) {
        Some(message) => message.to_string(),
        None => format!("jurl cloud answered HTTP {}: {}", reply.status, decide::snippet(&reply.text)),
    };
    if reply.status == StatusCode::UNAUTHORIZED {
        return anyhow!("jurl cloud doesn't accept this key ({said}): sign in again with `jurl login`");
    }
    match reply.retry_after {
        Some(seconds) => anyhow!("{said} (try again in {seconds} s)"),
        None => anyhow!("{said}"),
    }
}

/// `GET {base}/api/v1/{path}` with the key. A 404 is None: the server doesn't have that endpoint, so there is nothing to
/// show from it.
pub(crate) async fn get<T: DeserializeOwned>(client: &Client, cloud: &Cloud, path: &str) -> Result<Option<T>> {
    let req = client.get(format!("{}/api/v1/{path}", cloud.base)).bearer_auth(&cloud.key);
    let reply = send(req, &cloud.base).await?;
    if reply.status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !reply.status.is_success() {
        return Err(refused(&reply));
    }
    serde_json::from_str(&reply.text)
        .map(Some)
        .with_context(|| format!("jurl cloud's {path} reply is not one this jurl understands"))
}

/// The request body for a read: the mode the flags pick, the question, how many pages `--follow` reads, and how many
/// results. A mode with a count sends it: `-n`, or the mode's local default, since the server's own defaults are fewer
/// than a local run prints. `-a` keeps every result above the threshold and wins over `-n`, as it does locally. The
/// threshold goes only when `--threshold` gives one, since the server's defaults are the local ones.
pub(crate) fn body(args: &Args) -> Value {
    let mode = mode_of(args);
    let mut body = json!({ "url": args.url, "mode": mode });
    if let Some(question) = &args.ask {
        body["question"] = json!(question);
    }
    if let Some(pages) = args.follow {
        body["follow"] = json!(pages);
    }
    if args.all {
        body["all"] = json!(true);
    } else if let Some(max) = args.max.or(default_max(mode)) {
        body["max"] = json!(max);
    }
    if let Some(threshold) = args.threshold {
        body["threshold"] = json!(threshold);
    }
    body
}

/// The mode the flags pick: `precise`, `code`, `links`, `ask` (with `-q`), or `gist`, the default blocks.
fn mode_of(args: &Args) -> &'static str {
    if args.precise {
        "precise"
    } else if args.code {
        "code"
    } else if args.links {
        "links"
    } else if args.ask.is_some() {
        "ask"
    } else {
        "gist"
    }
}

/// How many results a local run prints in a mode when `-n` doesn't say. Precise prints one answer, so it has no count.
fn default_max(mode: &str) -> Option<usize> {
    match mode {
        "gist" => Some(DEFAULT_BLOCKS),
        "ask" => Some(DEFAULT_ASK_BLOCKS),
        "code" => Some(DEFAULT_CODE_BLOCKS),
        "links" => Some(DEFAULT_LINKS),
        _ => None,
    }
}

/// Why jurl cloud can't do this read, when it can't: the things it doesn't read (images, rendering), a `-n` or
/// `--threshold` outside the range it takes, or a `--follow` it doesn't follow. A run that needs one of them uses the
/// own keys.
pub(crate) fn unsupported(args: &Args) -> Option<String> {
    if args.image || args.vision || args.find.is_some() {
        Some("jurl cloud doesn't read images yet (--image, --vision, --find)".into())
    } else if args.render {
        Some("jurl cloud doesn't render JavaScript yet (-r)".into())
    } else if args.threshold.is_some_and(|t| !(0.0..=1.0).contains(&t)) {
        Some("jurl cloud takes --threshold from 0 to 1".into())
    } else if !args.all && args.max.is_some_and(|n| !(1..=MAX_RESULTS).contains(&n)) {
        Some(format!("jurl cloud takes -n from 1 to {MAX_RESULTS}"))
    } else if args.code && args.precise {
        Some("jurl cloud doesn't take --code with --precise yet".into())
    } else if let Some(pages) = args.follow {
        if !args.precise {
            Some("jurl cloud follows links only with --precise".into())
        } else if !FOLLOW_PAGES.contains(&pages) {
            Some(format!("jurl cloud follows 5, 10 or 15 pages, not {pages}"))
        } else {
            None
        }
    } else {
        None
    }
}

/// Reads through jurl cloud. The answer comes back as a local run prints it; a read with nothing in it is a miss, which
/// is a `NotFound` (exit 1), as a local miss is.
pub(crate) async fn read(client: &Client, cloud: &Cloud, args: &Args, t: &mut Timer) -> Result<Rendered> {
    let req = client
        .post(format!("{}/api/v1/read", cloud.base))
        .bearer_auth(&cloud.key)
        .header("X-Jurl-Source", "cli")
        .header("X-Jurl-Client", concat!("jurl/", env!("CARGO_PKG_VERSION")))
        .timeout(READ_TIMEOUT)
        .json(&body(args));
    let reply = send(req, &cloud.base).await?;
    t.lap("cloud");
    if !reply.status.is_success() {
        return Err(refused(&reply));
    }
    let r: ReadReply =
        serde_json::from_str(&reply.text).context("jurl cloud's answer is not a read this jurl understands")?;
    // The pages count in `usage.pages`, as local pages do: a miss is billed too, since the page was read.
    decide::USAGE.pages.fetch_add(r.pages, Relaxed);
    let done = rendered(args, &r);
    if done.is_ok() && r.path.len() > 1 {
        let trail: Vec<Url> = r.path.iter().filter_map(|p| Url::parse(p).ok()).collect();
        let s = if r.pages == 1 { "" } else { "s" };
        eprintln!("jurl: found after reading {} page{s}: {}", r.pages, follow::trail(&trail));
    }
    done
}

/// What `POST /api/v1/read` answers. Fields the server doesn't send read as absent: a block without its `kind` prints as
/// text (as code in `--code`), and a precise answer without its block prints as the answer alone.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadReply {
    url: String,
    #[serde(default)]
    path: Vec<String>,
    #[serde(default)]
    pages: u64,
    title: Option<String>,
    answer: Option<String>,
    closest: Option<String>,
    quote: Option<String>,
    // The quoted block's index on the page, and its kind, level and language when the server sends them: a precise
    // answer prints its block with them, as a local run does. A page's kind is an object, `{choice, confidence}`, in
    // jurl's own blocks JSON; a reply that carries one reads as no kind.
    block: Option<usize>,
    #[serde(default, deserialize_with = "string_kind")]
    kind: Option<String>,
    level: Option<u8>,
    lang: Option<String>,
    link: Option<String>,
    p: Option<f64>,
    blocks: Option<Vec<ReplyBlock>>,
    links: Option<Vec<ReplyLink>>,
}

#[derive(Deserialize)]
struct ReplyBlock {
    text: String,
    p: Option<f64>,
    kind: Option<String>,
    level: Option<u8>,
    lang: Option<String>,
}

/// The kind a block has in the server's answer, when this jurl knows it. A kind it doesn't know prints as text, rather
/// than failing the read.
fn kind_of(name: &str) -> Option<Kind> {
    match name {
        "heading" => Some(Kind::Heading),
        "para" => Some(Kind::Para),
        "code" => Some(Kind::Code),
        "quote" => Some(Kind::Quote),
        "item" => Some(Kind::Item),
        "table" => Some(Kind::Table),
        _ => None,
    }
}

/// A kind as the server sends it: a string. Any other shape reads as no kind, so it can't fail the whole reply.
fn string_kind<'de, D: Deserializer<'de>>(de: D) -> Result<Option<String>, D::Error> {
    Ok(Value::deserialize(de)?.as_str().map(String::from))
}

#[derive(Deserialize)]
struct ReplyLink {
    url: String,
    #[serde(default)]
    text: String,
    p: Option<f64>,
}

/// The answer as a local run prints it: the text a person reads, and the JSON `--json` prints, with the same keys.
fn rendered(args: &Args, r: &ReadReply) -> Result<Rendered> {
    let path = (r.path.len() > 1).then(|| json!(r.path));
    if args.precise {
        precise_answer(args, r, path.as_ref())
    } else if args.links {
        link_list(args, r)
    } else {
        block_list(args, r, path.as_ref())
    }
}

/// The answer on its own line, then the block it's in as markdown, as a local run prints it, then a link to it. The block
/// and the link are there when the server sends them.
fn precise_answer(args: &Args, r: &ReadReply, path: Option<&Value>) -> Result<Rendered> {
    let doc = |answer: Option<&str>, closest: Option<&str>| {
        let mut doc = json!({
            "url": r.url,
            "title": r.title,
            "ask": args.ask,
            "answer": answer,
            "closest": closest,
            "p": r.p,
            "quote": r.quote,
            "block": r.block,
            "link": r.link,
        });
        // The quoted block's kind, level and language, as a local run's JSON has them: each only when the block has it.
        // A kind this jurl doesn't know is no kind, so it has no key.
        if let Some(kind) = r.kind.as_deref().and_then(kind_of) {
            doc["kind"] = json!(kind);
        }
        if let Some(level) = r.level {
            doc["level"] = json!(level);
        }
        if let Some(lang) = &r.lang {
            doc["lang"] = json!(lang);
        }
        if let Some(path) = path {
            doc["path"] = path.clone();
        }
        doc
    };
    match (&r.answer, &r.closest) {
        (Some(answer), _) => {
            let mut text = format!("{answer}\n");
            if let Some(quote) = &r.quote {
                // Printed as a local run prints the block: a heading with its level, code fenced with its language.
                // Without a kind from the server, it is text.
                let kind = r.kind.as_deref().and_then(kind_of).unwrap_or(Kind::Para);
                let block = Block {
                    level: r.level,
                    lang: r.lang.clone(),
                    ..Block::new(r.block.unwrap_or_default(), kind, quote.clone())
                };
                text += &format!("\n{}\n", block.markdown());
            }
            if let Some(link) = &r.link {
                text += &format!("\n<{link}>\n");
            }
            Ok(Rendered { text, json: doc(Some(answer), None) })
        }
        (None, Some(closest)) => {
            // The local miss says how sure jurl is of the closest candidate, too.
            let p = r.p.map(|p| format!(", p={p:.2}")).unwrap_or_default();
            Err(missed(
                format!("no part of {} is exactly the answer (closest: \"{closest}\"{p})", r.url),
                Rendered { text: String::new(), json: doc(None, Some(closest)) },
            ))
        }
        (None, None) => Err(not_found(format!("nothing in {} answers that", r.url))),
    }
}

/// `--links`, and `--links -q`: one URL per line, best first, as the server ranks them.
fn link_list(args: &Args, r: &ReadReply) -> Result<Rendered> {
    let links = r.links.as_deref().unwrap_or_default();
    if links.is_empty() {
        let what = if args.ask.is_some() { "no links worth following in" } else { "no links found in" };
        return Err(not_found(format!("{what} {}", r.url)));
    }
    let text: String = links.iter().map(|l| format!("{}\n", l.url)).collect();
    let v: Vec<Value> = links.iter().map(|l| json!({ "url": l.url, "text": l.text, "p": l.p })).collect();
    Ok(Rendered { text, json: json!({ "url": r.url, "title": r.title, "links": v }) })
}

/// The default mode, `-q` and `--code`: the blocks in page order, under the page's title, as a local run prints them.
fn block_list(args: &Args, r: &ReadReply, path: Option<&Value>) -> Result<Rendered> {
    let blocks = r.blocks.as_deref().unwrap_or_default();
    if blocks.is_empty() {
        let message = if args.code {
            format!("no code blocks in {}", r.url)
        } else if args.ask.is_some() {
            format!("nothing in {} answers that", r.url)
        } else {
            format!("nothing in {} looks like content", r.url)
        };
        return Err(not_found(message));
    }
    // A block without a kind from the server is text, or code in --code, where every block is code.
    let fallback = if args.code { Kind::Code } else { Kind::Para };
    let mut text = String::new();
    if let Some(title) = r.title.as_deref().filter(|t| !t.is_empty()) {
        text += &format!("# {title}\n\n");
    }
    text += &format!("<{}>\n\n", r.url);
    let mut v = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let kind = b.kind.as_deref().and_then(kind_of);
        let block =
            Block { level: b.level, lang: b.lang.clone(), ..Block::new(i, kind.unwrap_or(fallback), b.text.clone()) };
        text += &format!("{}\n\n", block.markdown());
        let mut item = json!({ "i": i, "kind": kind, "text": b.text, "p": b.p });
        if let Some(level) = b.level {
            item["level"] = json!(level);
        }
        if let Some(lang) = &b.lang {
            item["lang"] = json!(lang);
        }
        v.push(item);
    }
    let mut doc = json!({ "url": r.url, "title": r.title, "ask": args.ask, "kind": null, "blocks": v });
    if let Some(path) = path {
        doc["path"] = path.clone();
    }
    Ok(Rendered { text, json: doc })
}

/// The organization and key a jurl cloud key belongs to (`GET /api/v1/whoami`).
#[derive(Deserialize)]
pub(crate) struct Whoami {
    pub(crate) organization: Option<Named>,
    pub(crate) key: Option<Named>,
}

#[derive(Deserialize)]
pub(crate) struct Named {
    pub(crate) name: String,
}

/// The period's page reads, against the plan's (`GET /api/v1/usage`). Read leniently: the plan can be a name or an object
/// with one, and the overage a number or a rule's name.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Usage {
    #[serde(default)]
    plan: Value,
    #[serde(default)]
    period_end: Value,
    #[serde(default)]
    included: Value,
    #[serde(default)]
    used: u64,
    #[serde(default)]
    overage: Value,
}

impl Usage {
    pub(crate) fn plan_name(&self) -> Option<&str> {
        self.plan.as_str().or_else(|| self.plan["name"].as_str())
    }
}

/// `1,240 of 20,000 page reads used · 18 days left`: the period's reads so far, the plan's, and how long the period has.
pub(crate) fn usage_line(u: &Usage, now: u64) -> String {
    let used = match u.included.as_u64() {
        Some(plan) => format!("{} of {} page reads used", grouped(u.used), grouped(plan)),
        None => format!("{} page reads used", grouped(u.used)),
    };
    let over = match u.overage.as_u64().filter(|n| *n > 0) {
        Some(n) => format!(" ({} over the plan)", grouped(n)),
        None => String::new(),
    };
    let left = match u.period_end.as_u64() {
        Some(end) => format!(" · {}", days_left(end, now)),
        None => String::new(),
    };
    format!("{used}{over}{left}")
}

/// How long until the period ends, in words: "18 days left", "1 day left", or "period over".
fn days_left(end: u64, now: u64) -> String {
    if end <= now {
        return "period over".into();
    }
    match (end - now).div_ceil(DAY_MS) {
        1 => "1 day left".into(),
        n => format!("{} days left", grouped(n)),
    }
}

/// 1234567 as "1,234,567".
pub(crate) fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The time in milliseconds since the epoch, which is how the server counts a period's end.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{mock, output::stdout_for};

    fn args(argv: &[&str]) -> Args {
        Args::parse_from(argv)
    }

    fn cloud_at(base: String) -> Cloud {
        Cloud { base, key: "jurl_test".into() }
    }

    #[test]
    fn the_cloud_is_used_when_the_environment_or_a_saved_login_says_so() {
        assert!(chosen(true, false, false), "JURL_CLOUD_KEY in the environment");
        assert!(chosen(true, true, false), "a cloud key in the environment beats own keys in it");
        assert!(!chosen(false, true, true), "own keys in the environment beat a saved login");
        assert!(chosen(false, false, true), "a saved login");
        assert!(!chosen(false, false, false), "nothing saved: your own keys");
    }

    #[test]
    fn each_flag_picks_its_mode_and_the_question_and_follow_go_along() {
        assert_eq!(body(&args(&["jurl", "x.com"])), json!({ "url": "x.com", "mode": "gist", "max": 12 }));
        assert_eq!(
            body(&args(&["jurl", "-q", "price?", "x.com"])),
            json!({ "url": "x.com", "mode": "ask", "question": "price?", "max": 5 })
        );
        assert_eq!(
            body(&args(&["jurl", "-p", "-q", "price?", "x.com"])),
            json!({ "url": "x.com", "mode": "precise", "question": "price?" })
        );
        assert_eq!(
            body(&args(&["jurl", "-p", "-q", "price?", "x.com", "--follow"])),
            json!({ "url": "x.com", "mode": "precise", "question": "price?", "follow": 5 })
        );
        assert_eq!(body(&args(&["jurl", "-c", "x.com"])), json!({ "url": "x.com", "mode": "code", "max": 8 }));
        assert_eq!(
            body(&args(&["jurl", "-l", "-q", "pricing?", "x.com"])),
            json!({ "url": "x.com", "mode": "links", "question": "pricing?", "max": 20 })
        );
    }

    #[test]
    fn the_count_is_the_local_one_unless_n_or_a_says_otherwise() {
        // -n replaces a mode's count. Precise prints one answer, so it has no count unless -n gives one.
        assert_eq!(body(&args(&["jurl", "-n", "3", "x.com"]))["max"], 3);
        assert_eq!(body(&args(&["jurl", "-p", "-q", "x?", "x.com"])).get("max"), None);
        assert_eq!(body(&args(&["jurl", "-p", "-q", "x?", "-n", "7", "x.com"]))["max"], 7);
        // -a keeps every result above the threshold, so no count goes with it, and it wins over -n.
        let all = body(&args(&["jurl", "-a", "-n", "3", "x.com"]));
        assert_eq!((all["all"].as_bool(), all.get("max")), (Some(true), None));
        // The threshold goes only when it is given: the server's defaults are the local ones.
        assert_eq!(body(&args(&["jurl", "--threshold", "0.7", "x.com"]))["threshold"], 0.7);
        assert_eq!(body(&args(&["jurl", "x.com"])).get("threshold"), None);
    }

    #[test]
    fn what_the_cloud_cannot_read_says_why() {
        assert!(unsupported(&args(&["jurl", "-p", "-q", "x?", "x.com"])).is_none());
        assert!(unsupported(&args(&["jurl", "-p", "-q", "x?", "--follow", "10", "x.com"])).is_none());
        // -n, -a and --threshold are read within the range the server takes. Outside it, a run uses your own keys.
        assert!(unsupported(&args(&["jurl", "-n", "50", "x.com"])).is_none());
        assert!(unsupported(&args(&["jurl", "-a", "-n", "60", "x.com"])).is_none(), "-a wins, so -n isn't checked");
        assert!(unsupported(&args(&["jurl", "--threshold", "0", "x.com"])).is_none());
        assert!(unsupported(&args(&["jurl", "-n", "60", "x.com"])).unwrap().contains("-n from 1 to 50"));
        assert!(unsupported(&args(&["jurl", "-n", "0", "x.com"])).unwrap().contains("-n from 1 to 50"));
        assert!(unsupported(&args(&["jurl", "--threshold", "1.5", "x.com"])).unwrap().contains("0 to 1"));
        assert!(unsupported(&args(&["jurl", "--threshold", "nan", "x.com"])).unwrap().contains("--threshold"));
        assert!(unsupported(&args(&["jurl", "-i", "x.com"])).unwrap().contains("images"));
        assert!(unsupported(&args(&["jurl", "--find", "a cathedral", "x.com"])).unwrap().contains("images"));
        assert!(unsupported(&args(&["jurl", "-r", "x.com"])).unwrap().contains("render"));
        assert!(unsupported(&args(&["jurl", "-q", "x?", "x.com", "--follow"])).unwrap().contains("--precise"));
        assert!(unsupported(&args(&["jurl", "-p", "-q", "x?", "--follow", "7", "x.com"])).unwrap().contains("not 7"));
        assert!(unsupported(&args(&["jurl", "-p", "-c", "-q", "x?", "x.com"])).unwrap().contains("--code"));
    }

    #[tokio::test]
    async fn a_precise_read_is_sent_with_the_key_and_prints_the_answer_alone() {
        let (base, seen) = mock::serve(vec![(
            200,
            "",
            r#"{"answered":true,"url":"https://x.com/","path":["https://x.com/"],"answer":"5,000 requests per hour","closest":null,"blocks":null,"links":null,"pages":1,"durationMs":900,"usage":{"pageReads":1}}"#,
        )]);
        let a = args(&["jurl", "-p", "-q", "rate limit?", "x.com"]);
        let r = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        assert_eq!(r.text, "5,000 requests per hour\n");
        assert_eq!(r.json["answer"], "5,000 requests per hour");
        assert_eq!(r.json["ask"], "rate limit?");
        assert!(r.json.get("path").is_none(), "one page is no trail");
        let seen = seen.lock().unwrap();
        let req = &seen[0];
        assert_eq!((req.method.as_str(), req.path.as_str()), ("POST", "/api/v1/read"));
        let header =
            |name: &str| req.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
        assert_eq!(header("authorization").as_deref(), Some("Bearer jurl_test"));
        assert_eq!(header("x-jurl-source").as_deref(), Some("cli"));
        assert!(header("x-jurl-client").unwrap().starts_with("jurl/"));
        assert!(req.body.contains("\"mode\":\"precise\"") && req.body.contains("\"question\":\"rate limit?\""));
    }

    #[tokio::test]
    async fn a_precise_answer_prints_its_block_as_a_local_run_does() {
        // A heading with its level, and code fenced with its language, when the server sends the kind.
        let heading = r#"{"url":"https://x.com/","title":"Pricing","answer":"Pro","quote":"Pro","block":4,"kind":"heading","level":3,"link":"https://x.com/#:~:text=Pro","pages":1,"path":["https://x.com/"]}"#;
        let (base, _) = mock::serve(vec![(200, "", heading)]);
        let a = args(&["jurl", "-p", "-q", "plan?", "x.com"]);
        let r = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        assert_eq!(r.text, "Pro\n\n### Pro\n\n<https://x.com/#:~:text=Pro>\n");
        assert_eq!((r.json["block"].as_u64(), r.json["title"].as_str()), (Some(4), Some("Pricing")));

        let code = r#"{"url":"https://x.com/","answer":"cargo install jurl","quote":"cargo install jurl","block":2,"kind":"code","lang":"sh","link":null,"pages":1,"path":["https://x.com/"]}"#;
        let (base, _) = mock::serve(vec![(200, "", code)]);
        let r = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        assert_eq!(r.text, "cargo install jurl\n\n```sh\ncargo install jurl\n```\n");

        // Without a kind (the server doesn't know this block's), the quote is text, and no link is no line.
        let plain = r#"{"url":"https://x.com/","answer":"5,000 requests per hour","quote":"Authenticated requests get 5,000 requests per hour.","block":0,"link":null,"pages":1,"path":["https://x.com/"]}"#;
        let (base, _) = mock::serve(vec![(200, "", plain)]);
        let r = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        assert_eq!(r.text, "5,000 requests per hour\n\nAuthenticated requests get 5,000 requests per hour.\n");
    }

    #[tokio::test]
    async fn a_precise_answers_json_has_its_block_kind_level_and_lang_as_a_local_run_does() {
        // The kind when the server sends one this jurl knows; the level and the language only when the block has them.
        let a = args(&["jurl", "-p", "-q", "plan?", "x.com"]);
        let heading = r#"{"url":"https://x.com/","title":"Pricing","answer":"Pro","quote":"Pro","block":4,"kind":"heading","level":3,"lang":null,"link":null,"pages":1,"path":["https://x.com/"]}"#;
        let (base, _) = mock::serve(vec![(200, "", heading)]);
        let r = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        assert_eq!((r.json["kind"].as_str(), r.json["level"].as_u64()), (Some("heading"), Some(3)));
        assert!(r.json.get("lang").is_none(), "no language, no key");

        let code = r#"{"url":"https://x.com/","answer":"cargo install jurl","quote":"cargo install jurl","block":2,"kind":"code","level":null,"lang":"sh","link":null,"pages":1,"path":["https://x.com/"]}"#;
        let (base, _) = mock::serve(vec![(200, "", code)]);
        let r = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        assert_eq!((r.json["kind"].as_str(), r.json["lang"].as_str()), (Some("code"), Some("sh")));
        assert!(r.json.get("level").is_none(), "no level, no key");

        // A kind this jurl doesn't know has no key either, and the quote prints as text.
        let unknown = r#"{"url":"https://x.com/","answer":"5,000 requests per hour","quote":"Authenticated requests get 5,000 requests per hour.","block":0,"kind":"video","link":null,"pages":1,"path":["https://x.com/"]}"#;
        let (base, _) = mock::serve(vec![(200, "", unknown)]);
        let r = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        assert!(r.json.get("kind").is_none(), "an unknown kind is no kind");
        assert_eq!(r.text, "5,000 requests per hour\n\nAuthenticated requests get 5,000 requests per hour.\n");

        // A miss carries the closest candidate's block the same way.
        let miss = r#"{"url":"https://x.com/","answer":null,"closest":"$8 a month","quote":"$8 a month","block":3,"kind":"para","p":0.31,"link":null,"pages":1,"path":["https://x.com/"]}"#;
        let (base, _) = mock::serve(vec![(200, "", miss)]);
        let a = args(&["jurl", "--json", "-p", "-q", "price?", "x.com"]);
        let err = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap_err();
        let out = stdout_for(&a, &Err(err), &decide::Usage::new()).unwrap().expect("JSON on a miss");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!((v["kind"].as_str(), v["block"].as_u64()), (Some("para"), Some(3)));
        assert!(v.get("level").is_none() && v.get("lang").is_none());
    }

    #[test]
    fn a_reply_reads_its_kind_only_when_the_kind_is_a_string() {
        // A precise reply's kind is a block's kind, a string.
        let precise = r#"{"url":"https://x.com/","answer":"Pro","kind":"heading","level":3,"pages":1}"#;
        assert_eq!(serde_json::from_str::<ReadReply>(precise).unwrap().kind.as_deref(), Some("heading"));

        // jurl's own blocks JSON has the page's kind as an object. That is no block kind, and the reply still reads.
        let page = r#"{"url":"https://x.com/","blocks":[{"text":"Run it.","p":0.8}],"kind":{"choice":"docs","confidence":0.84}}"#;
        let r: ReadReply = serde_json::from_str(page).unwrap();
        assert_eq!((r.kind, r.blocks.map(|b| b.len())), (None, Some(1)));

        // null, and no kind at all, are no kind either.
        let null: ReadReply = serde_json::from_str(r#"{"url":"https://x.com/","kind":null}"#).unwrap();
        let absent: ReadReply = serde_json::from_str(r#"{"url":"https://x.com/"}"#).unwrap();
        assert_eq!((null.kind, absent.kind), (None, None));
    }

    #[tokio::test]
    async fn the_count_and_the_threshold_are_sent_with_the_read() {
        let reply = r#"{"url":"https://x.com/","blocks":[{"text":"Run it.","p":0.8}]}"#;
        let (base, seen) = mock::serve(vec![(200, "", reply)]);
        let a = args(&["jurl", "-n", "3", "--threshold", "0.7", "x.com"]);
        read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        let sent: Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
        let got = (sent["mode"].as_str(), sent["max"].as_u64(), sent["threshold"].as_f64());
        assert_eq!(got, (Some("gist"), Some(3), Some(0.7)));
    }

    #[tokio::test]
    async fn a_precise_miss_is_not_found_and_its_json_has_the_closest_candidate() {
        let (base, _) = mock::serve(vec![(
            200,
            "",
            r#"{"answered":false,"url":"https://x.com/","path":["https://x.com/"],"answer":null,"closest":"$8 a month","p":0.31,"blocks":null,"links":null,"pages":2,"durationMs":1,"usage":{"pageReads":2}}"#,
        )]);
        let a = args(&["jurl", "--json", "-p", "-q", "price?", "x.com"]);
        let done: Result<Rendered> = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await;
        let err = done.unwrap_err();
        assert!(crate::output::is_not_found(&err));
        // The same message a local miss gives, with how sure jurl is of the closest candidate.
        assert!(format!("{err:#}").contains("(closest: \"$8 a month\", p=0.31)"), "{err:#}");
        let out = stdout_for(&a, &Err(err), &decide::Usage::new()).unwrap().expect("JSON on a miss");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["answer"], Value::Null);
        assert_eq!(v["closest"], "$8 a month");
        assert_eq!(v["p"], 0.31);
        assert_eq!(v["usage"]["jev"]["requests"], 0);
    }

    #[tokio::test]
    async fn gist_blocks_print_their_kinds_and_code_blocks_are_fenced() {
        let blocks = r#"{"url":"https://x.com/","blocks":[{"text":"Installation","kind":"heading","level":3,"p":0.9},{"text":"Run it.","p":0.8}]}"#;
        let (base, _) = mock::serve(vec![(200, "", blocks)]);
        let r = read(&Client::new(), &cloud_at(base), &args(&["jurl", "x.com"]), &mut Timer::new()).await.unwrap();
        assert_eq!(r.text, "<https://x.com/>\n\n### Installation\n\nRun it.\n\n");
        assert_eq!(r.json["blocks"][0]["kind"], "heading");
        assert_eq!(r.json["blocks"][1]["kind"], Value::Null);

        let code = r#"{"url":"https://x.com/","blocks":[{"text":"cargo install jurl","p":0.9}]}"#;
        let (base, _) = mock::serve(vec![(200, "", code)]);
        let r =
            read(&Client::new(), &cloud_at(base), &args(&["jurl", "-c", "x.com"]), &mut Timer::new()).await.unwrap();
        assert_eq!(r.text, "<https://x.com/>\n\n```\ncargo install jurl\n```\n\n");

        // A kind this jurl doesn't know is no reason to fail the read: the block prints as text.
        let unknown = r#"{"url":"https://x.com/","blocks":[{"text":"Watch it","kind":"video"}]}"#;
        let (base, _) = mock::serve(vec![(200, "", unknown)]);
        let r = read(&Client::new(), &cloud_at(base), &args(&["jurl", "x.com"]), &mut Timer::new()).await.unwrap();
        assert_eq!(r.text, "<https://x.com/>\n\nWatch it\n\n");
        assert_eq!(r.json["blocks"][0]["kind"], Value::Null);
        assert_eq!(kind_of("heading"), Some(Kind::Heading));
        assert_eq!(kind_of("video"), None);
    }

    #[tokio::test]
    async fn links_print_one_url_per_line_and_a_trail_is_kept_in_json() {
        let links = r#"{"url":"https://x.com/","links":[{"url":"https://a.com/","text":"A","p":0.9},{"url":"https://b.com/","text":"B","p":0.5}],"path":["https://x.com/","https://x.com/pricing"],"pages":2}"#;
        let (base, _) = mock::serve(vec![(200, "", links)]);
        let r =
            read(&Client::new(), &cloud_at(base), &args(&["jurl", "-l", "x.com"]), &mut Timer::new()).await.unwrap();
        assert_eq!(r.text, "https://a.com/\nhttps://b.com/\n");
        assert_eq!(r.json["links"][0]["p"], 0.9);

        let found = r#"{"url":"https://x.com/pricing","path":["https://x.com/","https://x.com/pricing"],"pages":2,"answer":"$10 per user/month"}"#;
        let (base, _) = mock::serve(vec![(200, "", found)]);
        let a = args(&["jurl", "--json", "-p", "-q", "price?", "x.com", "--follow"]);
        let r = read(&Client::new(), &cloud_at(base), &a, &mut Timer::new()).await.unwrap();
        assert_eq!(r.json["path"], json!(["https://x.com/", "https://x.com/pricing"]));
    }

    #[tokio::test]
    async fn refusals_say_what_to_do_and_are_errors_not_misses() {
        let cases: [(u16, &'static str, &'static str, &str); 4] = [
            (401, "", r#"{"error":"missing_api_key"}"#, "sign in again"),
            (
                402,
                "",
                r#"{"error":"quota_exceeded","message":"The page read limit for this period is used up."}"#,
                "used up",
            ),
            (422, "", r#"{"error":"url_not_public","message":"That address isn't public."}"#, "isn't public"),
            (502, "", r#"{"error":"read_failed","message":"The page could not be read."}"#, "could not be read"),
        ];
        for (status, extra, body, want) in cases {
            let (base, _) = mock::serve(vec![(status, extra, body)]);
            let err =
                read(&Client::new(), &cloud_at(base), &args(&["jurl", "x.com"]), &mut Timer::new()).await.unwrap_err();
            assert!(format!("{err:#}").contains(want), "{status}: {err:#}");
            assert!(!crate::output::is_not_found(&err), "{status} is a failure, exit 2");
        }
        let (base, _) =
            mock::serve(vec![(429, "Retry-After: 12\r\n", r#"{"error":"rate_limited","message":"Too many reads."}"#)]);
        let err =
            read(&Client::new(), &cloud_at(base), &args(&["jurl", "x.com"]), &mut Timer::new()).await.unwrap_err();
        assert_eq!(format!("{err:#}"), "Too many reads. (try again in 12 s)");
    }

    #[tokio::test]
    async fn a_404_for_usage_or_whoami_is_none_and_a_reply_is_read() {
        let usage = r#"{"plan":"Starter","periodEnd":1700000000000,"included":20000,"used":1240,"overage":0}"#;
        let (base, seen) = mock::serve(vec![(200, "", usage), (404, "", r#"{"error":"not_found"}"#)]);
        let cloud = cloud_at(base);
        let u: Option<Usage> = get(&Client::new(), &cloud, "usage").await.unwrap();
        assert_eq!(u.as_ref().and_then(Usage::plan_name), Some("Starter"));
        let w: Option<Whoami> = get(&Client::new(), &cloud, "whoami").await.unwrap();
        assert!(w.is_none());
        assert_eq!(seen.lock().unwrap()[0].path, "/api/v1/usage");
    }

    #[test]
    fn usage_says_what_the_period_has_used_and_how_long_it_has() {
        let now = 1_000 * DAY_MS;
        let u: Usage = serde_json::from_value(json!({
            "plan": "Starter", "periodEnd": now + 18 * DAY_MS, "included": 20000, "used": 1240, "overage": 0,
        }))
        .unwrap();
        assert_eq!(usage_line(&u, now), "1,240 of 20,000 page reads used · 18 days left");
        // An object plan, an overage, and a period with a day left.
        let u: Usage = serde_json::from_value(json!({
            "plan": { "name": "Growth" }, "periodEnd": now + DAY_MS / 2, "included": 20000, "used": 20300, "overage": 300,
        }))
        .unwrap();
        assert_eq!(usage_line(&u, now), "20,300 of 20,000 page reads used (300 over the plan) · 1 day left");
        assert_eq!(u.plan_name(), Some("Growth"));
        // No plan limit, a rule's name for the overage, and a period that is over.
        let u: Usage = serde_json::from_value(json!({
            "periodEnd": now - 1, "included": null, "used": 5, "overage": "allow",
        }))
        .unwrap();
        assert_eq!(usage_line(&u, now), "5 page reads used · period over");
    }

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1240), "1,240");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn a_key_goes_only_to_https_or_this_computer() {
        assert_eq!(checked_base("https://cloud.jurl.dev/").unwrap(), "https://cloud.jurl.dev");
        assert_eq!(checked_base("http://127.0.0.1:3211").unwrap(), "http://127.0.0.1:3211");
        assert!(checked_base("http://cloud.jurl.dev").is_err());
        assert!(checked_base("cloud.jurl.dev").is_err());
    }
}

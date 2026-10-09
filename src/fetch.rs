use std::{
    collections::{HashMap, hash_map::Entry},
    path::Path,
    process::Stdio,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError, atomic::Ordering::Relaxed},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use encoding_rs::Encoding;
use futures::{Stream, StreamExt};
use reqwest::{Client, Response, StatusCode, header};
use serde_json::Value;
use tokio::process::Command;
use url::Url;

pub struct Page {
    pub url: Url,
    pub body: String,
    pub is_markdown: bool,
    /// Read with the browser client: after a retry, or because the host was already on it.
    pub via_browser: bool,
}

/// The largest page read: a body past this is an error, not read on.
const PAGE_MAX: usize = 8 << 20;

/// How long a host stays on the browser client after a retry showed that it needs one, counted from that retry. The memo
/// lives as long as the process, and `jurl mcp` lives on: after this a host is asked plain first again, which costs one
/// plain request per host per window.
pub(crate) const STICKY_TTL: Duration = Duration::from_secs(10 * 60);

/// The hosts that need the browser client, each with the moment a retry first showed it. One memo per process, so the hops
/// of a `--follow` search and the calls of a `jurl mcp` server share it.
#[derive(Default)]
pub(crate) struct Memo {
    learned: Mutex<HashMap<String, Instant>>,
}

impl Memo {
    fn entries(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        self.learned.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `host` is on the browser client at `now`. Entries older than STICKY_TTL are dropped first.
    pub(crate) fn on_browser(&self, host: &str, now: Instant) -> bool {
        let mut entries = self.entries();
        entries.retain(|_, learned| now.saturating_duration_since(*learned) < STICKY_TTL);
        entries.contains_key(host)
    }

    /// Puts `host` on the browser client as of `now`. Returns whether that is news: the host was not on it. A host that
    /// is already on it keeps the moment it was learned, so its window is not extended. Concurrent requests to one host
    /// race on this lock, and exactly one of them is told it was news.
    fn learn(&self, host: &str, now: Instant) -> bool {
        let mut entries = self.entries();
        entries.retain(|_, learned| now.saturating_duration_since(*learned) < STICKY_TTL);
        match entries.entry(host.to_string()) {
            Entry::Vacant(entry) => {
                entry.insert(now);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn learn_url(&self, url: &Url, now: Instant) -> bool {
        host_key(url).is_some_and(|host| self.learn(&host, now))
    }
}

/// What a fetch may do about bot protection: whether the browser-fingerprint retry is on for the run, the memo of hosts that
/// need the browser client, and whether a host switching to the browser client is said on stderr (`-t`).
#[derive(Clone, Copy)]
pub(crate) struct Retry<'a> {
    pub(crate) on: bool,
    pub(crate) memo: &'a Memo,
    pub(crate) timing: bool,
}

static MEMO: LazyLock<Memo> = LazyLock::new(Memo::default);

impl Retry<'static> {
    /// The retry for a run: on unless the run opts out with `--no-browser-retry` or `JURL_NO_BROWSER_RETRY`.
    pub(crate) fn for_run(flag: bool, timing: bool) -> Self {
        let env_set = std::env::var_os("JURL_NO_BROWSER_RETRY").is_some();
        Retry { on: browser_retry_allowed(flag, env_set), memo: &MEMO, timing }
    }
}

impl Retry<'_> {
    /// Whether `url`'s host is asked with the browser client alone: the retry is on, and a retry showed the host needs it
    /// within STICKY_TTL of `now`.
    pub(crate) fn sticky(&self, url: &Url, now: Instant) -> bool {
        self.on && host_key(url).is_some_and(|host| self.memo.on_browser(&host, now))
    }

    /// Puts `url`'s host on the browser client, as a browser reply that was a success just showed. The first host to be put
    /// there is said on stderr under `-t`, once.
    fn learn(&self, url: &Url, now: Instant) {
        if let Some(host) = host_key(url)
            && self.memo.learn(&host, now)
            && self.timing
        {
            eprintln!("jurl: {host}: using the browser client");
        }
    }
}

/// The memo's name for a host: the host, with the port when the URL names one.
fn host_key(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// The page at `url`. When the normal client is refused and the retry is on, the same URL is asked once more by the
/// browser client, whose TLS and HTTP/2 fingerprint some bot protection accepts where it refuses the normal client. A host
/// that a retry showed needs the browser client is asked there first, for STICKY_TTL. Nothing else is retried.
pub(crate) async fn fetch(client: &Client, url: &str, retry: Retry<'_>) -> Result<Page> {
    fetch_capped(client, url, PAGE_MAX, retry, Instant::now()).await
}

/// Whether a refused page is retried with a browser's fingerprint. A run opts out with `--no-browser-retry`, or with
/// `JURL_NO_BROWSER_RETRY` set to anything (as `JURL_NO_DOWNLOAD` is).
pub(crate) fn browser_retry_allowed(flag: bool, env_set: bool) -> bool {
    !flag && !env_set
}

/// The refusals that get the retry. A 403 always does. A 503 does unless it carries a Retry-After, in either form: that is
/// maintenance or backoff, which is respected as a 429 is. A 429 never does.
fn retried(status: StatusCode, retry_after: bool) -> bool {
    match status {
        StatusCode::FORBIDDEN => true,
        StatusCode::SERVICE_UNAVAILABLE => !retry_after,
        _ => false,
    }
}

/// What the error of a refusal says about the client that refused, when it says anything.
const VIA_BROWSER: &str = "with a browser's TLS fingerprint";
const VIA_BROWSER_TOO: &str = "also with a browser's TLS fingerprint";

/// `fetch`, refusing a page larger than `max` bytes, and judging the host's memo as of `now`.
async fn fetch_capped(client: &Client, url: &str, max: usize, retry: Retry<'_>, now: Instant) -> Result<Page> {
    let parsed = Url::parse(url).ok();
    if parsed.as_ref().is_some_and(|u| retry.sticky(u, now)) {
        // This host needed the browser client within STICKY_TTL: ask only that. A refusal from it is the answer.
        return match get_browser(url, max).await? {
            Reply::Page { page, .. } => Ok(Page { via_browser: true, ..page }),
            Reply::Refused(refused) => Err(refused.error(url, Some(VIA_BROWSER))),
        };
    }
    let refused = match get(client, url, max).await? {
        Reply::Page { page, .. } => return Ok(page),
        Reply::Refused(refused) => refused,
    };
    if !retry.on || !retried(refused.status, refused.retry_after.is_some()) {
        return Err(refused.error(url, None));
    }
    let Ok(res) = browser_send(url).await else {
        // The browser client got no reply at all: the plain refusal stands.
        return Err(refused.error(url, None));
    };
    // A success from the browser client means it got past the bot check: the host is on the client from now on.
    if let Some(parsed) = &parsed
        && res.status().is_success()
    {
        retry.learn(parsed, now);
    }
    // Once the browser client has a reply, that reply is the answer: a page too large to read is an error, not a fallback.
    match browser_reply(res, url, max).await? {
        Reply::Page { page, .. } => Ok(Page { via_browser: true, ..page }),
        Reply::Refused(again) => Err(again.error(url, Some(VIA_BROWSER_TOO))),
    }
}

/// A small text file (robots.txt, llms.txt, a sitemap) at `url`, or None. It follows the retry rule for pages, with the same
/// memo: a host on the browser client is read from there alone; otherwise the plain client is asked first, and a refusal the
/// rule retries (a 403, or a 503 without a Retry-After) is asked once more of the browser client, whose success teaches the
/// host. Its body is decoded as a page is; which bodies count as the site's file is for the caller to say.
pub(crate) async fn fetch_small(
    client: &Client,
    url: &Url,
    max: usize,
    timeout: Duration,
    retry: Retry<'_>,
    now: Instant,
) -> Option<String> {
    if retry.sticky(url, now) {
        crate::decide::USAGE.browser_requests.fetch_add(1, Relaxed);
        let res = browser_client().ok()?.get(url.as_str()).timeout(timeout).send().await.ok()?;
        return small_from_browser(res, max).await;
    }
    let mut res = client.get(url.as_str()).timeout(timeout).send().await.ok()?;
    let status = res.status();
    if status.is_success() {
        let content_type =
            res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let bytes = read_capped(&mut res, max).await.ok().flatten()?;
        return Some(decode(&content_type, &bytes));
    }
    crate::decide::USAGE.plain_refusals.fetch_add(1, Relaxed);
    let retry_after = res.headers().get(header::RETRY_AFTER).is_some();
    if !retry.on || !retried(status, retry_after) {
        return None;
    }
    crate::decide::USAGE.browser_requests.fetch_add(1, Relaxed);
    let res = browser_client().ok()?.get(url.as_str()).timeout(timeout).send().await.ok()?;
    if res.status().is_success() {
        retry.learn(url, now);
    }
    small_from_browser(res, max).await
}

/// The body of a small file from a browser client's reply: decoded as a page is, or None on a refusal or a body larger than
/// `max`.
async fn small_from_browser(res: wreq::Response, max: usize) -> Option<String> {
    if !res.status().is_success() {
        return None;
    }
    let content_type =
        res.headers().get(wreq::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let bytes = read_stream_capped(res.content_length(), res.bytes_stream(), max).await.ok().flatten()?;
    Some(decode(&content_type, &bytes))
}

/// What a request for a page came back with: the page, or the refusal to give one.
enum Reply {
    Page {
        // Only the bench reports it: a page is read whatever its 2xx status.
        #[cfg_attr(not(test), allow(dead_code))]
        status: StatusCode,
        page: Page,
    },
    Refused(Refusal),
}

/// A reply that is not a success: its status, and its Retry-After when it has one (in either form, as text).
struct Refusal {
    status: StatusCode,
    retry_after: Option<String>,
}

impl Refusal {
    /// The error for this refusal. `via` says which client refused, when the error should say.
    fn error(&self, url: &str, via: Option<&str>) -> anyhow::Error {
        let mut notes: Vec<String> = via.map(String::from).into_iter().collect();
        if let Some(r) = &self.retry_after {
            notes.push(if r.is_empty() { "retry after".to_string() } else { format!("retry after {r}") });
        }
        if notes.is_empty() {
            anyhow!("{url} returned HTTP {}", self.status)
        } else {
            anyhow!("{url} returned HTTP {} ({})", self.status, notes.join("; "))
        }
    }
}

/// A Retry-After header's value as text. A value that is not text is kept, lossily: the header is there either way.
fn retry_after_text(value: &header::HeaderValue) -> String {
    String::from_utf8_lossy(value.as_bytes()).into_owned()
}

/// One GET of `url` with the normal client.
async fn get(client: &Client, url: &str, max: usize) -> Result<Reply> {
    let mut res = client
        .get(url)
        .header(header::ACCEPT, "text/markdown, text/html;q=0.9, */*;q=0.5")
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    let status = res.status();
    if !status.is_success() {
        crate::decide::USAGE.plain_refusals.fetch_add(1, Relaxed);
        let retry_after = res.headers().get(header::RETRY_AFTER).map(retry_after_text);
        return Ok(Reply::Refused(Refusal { status, retry_after }));
    }
    let final_url = res.url().clone();
    let ct = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let links: Vec<String> =
        res.headers().get_all(header::LINK).iter().filter_map(|v| v.to_str().ok()).map(String::from).collect();
    let Some(bytes) = read_capped(&mut res, max).await? else {
        bail!("{url}: page larger than {}", size_label(max));
    };
    let mut page = to_page(final_url, &ct, &bytes);
    // The body is read as its markdown, but the page a person reads is the HTML page it is the alternate of.
    if page.is_markdown {
        let md = page.url.clone();
        page.url = html_twin(client, &md, &links).await.unwrap_or(md);
    }
    Ok(Reply::Page { status, page })
}

/// One GET of `url` with the browser client, which asks with its own headers: the page, or the refusal. No reply at all,
/// or a reply that cannot be read, is an error.
async fn get_browser(url: &str, max: usize) -> Result<Reply> {
    browser_reply(browser_send(url).await?, url, max).await
}

/// The browser client's request for `url`, sent. An error here means no reply came.
async fn browser_send(url: &str) -> Result<wreq::Response> {
    crate::decide::USAGE.browser_requests.fetch_add(1, Relaxed);
    browser_client()?.get(url).send().await.with_context(|| format!("fetching {url} with a browser's fingerprint"))
}

/// The browser client's reply to `url`: the page, or the refusal. A reply that cannot be read is an error.
async fn browser_reply(res: wreq::Response, url: &str, max: usize) -> Result<Reply> {
    let status = res.status();
    if !status.is_success() {
        let retry_after = res.headers().get(wreq::header::RETRY_AFTER).map(retry_after_text);
        return Ok(Reply::Refused(Refusal { status, retry_after }));
    }
    let final_url = Url::parse(&res.uri().to_string()).with_context(|| format!("the address {url} ended at"))?;
    let ct =
        res.headers().get(wreq::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let Some(bytes) = read_stream_capped(res.content_length(), res.bytes_stream(), max).await? else {
        bail!("{url}: page larger than {}", size_label(max));
    };
    Ok(Reply::Page { status, page: to_page(final_url, &ct, &bytes) })
}

/// The browser client, built on first use and then shared, so its connections are pooled across hops and files. It asks like
/// Chrome 149: its TLS and HTTP/2 fingerprint, headers and user agent. Proxies from the environment apply, as they do to the
/// normal client, and the redirects, timeout and idle timeout are the normal client's.
static BROWSER: LazyLock<std::result::Result<wreq::Client, String>> = LazyLock::new(|| {
    let builder = wreq::Client::builder()
        .emulation(wreq_util::Emulation::Chrome149)
        .default_headers(browser_language_headers())
        // wreq follows no redirects unless asked; the normal client follows up to 10, and so does this one.
        .redirect(wreq::redirect::Policy::limited(10))
        .timeout(crate::HTTP_TIMEOUT)
        .pool_idle_timeout(crate::POOL_IDLE_TIMEOUT);
    // The loopback servers in the tests are reached directly, as the normal client reaches them.
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder.build().map_err(|e| format!("building the browser client: {e}"))
});

fn browser_client() -> Result<wreq::Client> {
    BROWSER.as_ref().cloned().map_err(|e| anyhow!("{e}"))
}

/// The page a body makes: its text as its Content-Type's charset says, and whether it is markdown.
fn to_page(url: Url, content_type: &str, bytes: &[u8]) -> Page {
    let body = decode(content_type, bytes);
    let is_markdown = served_markdown(content_type, &body);
    Page { url, body, is_markdown, via_browser: false }
}

/// The body of `res`, or None once it is larger than `max` bytes. The Content-Length is checked first, but the bytes
/// that arrive are what is counted: a compressed or streamed body may have no length, or one that is not its size.
pub async fn read_capped(res: &mut Response, max: usize) -> Result<Option<Vec<u8>>> {
    if res.content_length().is_some_and(|n| n > max as u64) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        if bytes.len() + chunk.len() > max {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}

/// `read_capped` for a reply from the browser client, whose body comes as a stream of chunks.
async fn read_stream_capped<B: AsRef<[u8]>>(
    declared: Option<u64>,
    body: impl Stream<Item = wreq::Result<B>>,
    max: usize,
) -> Result<Option<Vec<u8>>> {
    if declared.is_some_and(|n| n > max as u64) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    let mut body = std::pin::pin!(body);
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        let chunk = chunk.as_ref();
        if bytes.len() + chunk.len() > max {
            return Ok(None);
        }
        bytes.extend_from_slice(chunk);
    }
    Ok(Some(bytes))
}

/// A byte count for a message: "8 MB", "1 KB" for a whole number of KB, or the bytes. A multiple of 2^k has at least k
/// trailing zero bits.
pub fn size_label(bytes: usize) -> String {
    if bytes.trailing_zeros() >= 20 {
        format!("{} MB", bytes >> 20)
    } else if bytes.trailing_zeros() >= 10 {
        format!("{} KB", bytes >> 10)
    } else {
        format!("{bytes} bytes")
    }
}

/// A body's text, decoded as reqwest's text() decodes it: the charset the Content-Type names, UTF-8 if it names none,
/// and a byte order mark over both.
pub(crate) fn decode(content_type: &str, bytes: &[u8]) -> String {
    charset_of(content_type).decode(bytes).0.into_owned()
}

/// The encoding a Content-Type names, as reqwest's text() reads it: the `charset` parameter's label, or UTF-8 when there
/// is no such parameter or encoding_rs knows no such label.
fn charset_of(content_type: &str) -> &'static Encoding {
    content_type
        .split(';')
        .skip(1)
        .filter_map(|param| param.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("charset"))
        .and_then(|(_, label)| Encoding::for_label(label.trim().trim_matches('"').as_bytes()))
        .unwrap_or(encoding_rs::UTF_8)
}

/// Some sites answer `Accept: text/markdown` with markdown served as text/plain. Read as HTML, its fenced code
/// collapses into one line; plain text read as markdown loses nothing.
fn served_markdown(content_type: &str, body: &str) -> bool {
    content_type.contains("markdown") || (content_type.starts_with("text/plain") && !body.trim_start().starts_with('<'))
}

/// The Accept-Language every page request sends, when the run names one. jurl does not detect the language of the question (a
/// precise answer has no `lang`), so it sends none unless `JURL_ACCEPT_LANGUAGE` is set, e.g. `es` for a question in Spanish:
/// a site that answers in the language it is asked for then answers in it. The caller chooses the value.
pub(crate) fn accept_language() -> Option<String> {
    accept_language_of(std::env::var("JURL_ACCEPT_LANGUAGE").ok())
}

/// [`accept_language`]'s rule for the value of `JURL_ACCEPT_LANGUAGE` (or none): the value, trimmed, when it is a header
/// value; none when it is unset, empty or not a header value.
pub(crate) fn accept_language_of(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty() && header::HeaderValue::from_str(v).is_ok())
}

/// The Accept-Language as a header map for the plain client: one line when the run names a language, none otherwise.
pub(crate) fn language_headers() -> header::HeaderMap {
    language_headers_of(accept_language())
}

fn language_headers_of(language: Option<String>) -> header::HeaderMap {
    let mut headers = header::HeaderMap::new();
    if let Some(value) = language.and_then(|v| header::HeaderValue::from_str(&v).ok()) {
        headers.insert(header::ACCEPT_LANGUAGE, value);
    }
    headers
}

/// The same for the browser client. With none named it adds no line of its own: its emulation profile keeps Chrome's
/// `en-US,en;q=0.9`, as it always has (see [`BROWSER`]).
fn browser_language_headers() -> wreq::header::HeaderMap {
    browser_language_headers_of(accept_language())
}

fn browser_language_headers_of(language: Option<String>) -> wreq::header::HeaderMap {
    let mut headers = wreq::header::HeaderMap::new();
    if let Some(value) = language.and_then(|v| wreq::header::HeaderValue::from_str(&v).ok()) {
        headers.insert(wreq::header::ACCEPT_LANGUAGE, value);
    }
    headers
}

/// How long a check that a page's HTML twin exists may take. It reads the response's headers and no more.
const TWIN_TIMEOUT: Duration = Duration::from_secs(4);

/// The HTML page a markdown page is the alternate of, if there is one. A `Link: <…>; rel="canonical"` header names it
/// (RFC 8288; RFC 6596 defines the relation). Failing that, a `…/x.md` URL is the markdown of `…/x`, as the llms.txt
/// proposal (llmstxt.org) has it: a page's markdown is at the same URL with `.md` appended, and a directory's at
/// `index.md`. A candidate counts only if it answers with an HTML page on the same host, so a page the site doesn't
/// have is never named.
async fn html_twin(client: &Client, url: &Url, links: &[String]) -> Option<Url> {
    let canonical = links.iter().filter_map(|l| canonical_link(l)).filter_map(|c| url.join(c).ok());
    for candidate in canonical.chain(md_stem(url)) {
        if candidate == *url || candidate.host_str() != url.host_str() {
            continue;
        }
        if let Some(page) = serves_html(client, &candidate).await {
            return Some(page);
        }
    }
    None
}

/// The target of the first link in a `Link` header value whose `rel` is `canonical`. A link is `<uri>` and then its
/// `;`-separated parameters; links are separated by commas.
fn canonical_link(value: &str) -> Option<&str> {
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let close = after.find('>')?;
        let params = after[close + 1..].split(',').next().unwrap_or_default();
        let canonical = params.split(';').any(|p| {
            p.split_once('=').is_some_and(|(name, rel)| {
                name.trim().eq_ignore_ascii_case("rel")
                    && rel.trim().trim_matches('"').split_whitespace().any(|r| r.eq_ignore_ascii_case("canonical"))
            })
        });
        if canonical {
            return Some(&after[..close]);
        }
        rest = &after[close + 1..];
    }
    None
}

/// The page a `…/x.md` URL is the markdown of: `…/x`, or for `…/index.md` the directory `…/`. None for any other URL.
fn md_stem(url: &Url) -> Option<Url> {
    let path = url.path().strip_suffix(".md").filter(|p| !p.is_empty() && !p.ends_with('/'))?;
    let stem = match path.rsplit_once('/') {
        Some((dir, last)) if last == "index" || last == "index.html" => format!("{dir}/"),
        _ => path.to_string(),
    };
    let mut page = url.clone();
    page.set_path(&stem);
    Some(page)
}

/// The URL of the page at `url` if it answers with an HTML page on the same host, after any redirects. The request asks
/// for HTML outright: a site that answers every request with its markdown must not pass for its own HTML.
async fn serves_html(client: &Client, url: &Url) -> Option<Url> {
    let res = client.get(url.as_str()).header(header::ACCEPT, "text/html").timeout(TWIN_TIMEOUT).send().await.ok()?;
    let ct = res.headers().get(header::CONTENT_TYPE)?.to_str().ok()?;
    let html = ct.starts_with("text/html") || ct.starts_with("application/xhtml+xml");
    let same_host = res.url().host_str() == url.host_str();
    (res.status().is_success() && html && same_host).then(|| res.url().clone())
}

/// Run the page's JavaScript in Lightpanda and return the resulting DOM.
/// Waits for the network to settle and for real visible text to appear, capped at 8s:
/// SPAs keep background traffic going and often paint content after the network calms down.
pub async fn render(bin: &Path, url: &Url) -> Result<Page> {
    let run = Command::new(bin)
        .args(["fetch", "--json", "--dump", "html", "--wait-until", "networkalmostidle", "--wait-ms", "8000"])
        .args(["--wait-script", "document.body && (document.body.innerText || '').trim().length > 1500"])
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(12), run)
        .await
        .context("lightpanda timed out")?
        .with_context(|| format!("running {}", bin.display()))?;
    // --json wraps the dump with the page's HTTP status and headers, so a 429 fails here too.
    let v: Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("lightpanda rendered nothing for {url} (exit {})", out.status))?;
    let status = v["http_status"].as_u64().unwrap_or(0);
    if status >= 400 {
        let retry = v["headers"].as_array().into_iter().flatten().find_map(|h| {
            h["name"].as_str().filter(|n| n.eq_ignore_ascii_case("retry-after"))?;
            h["value"].as_str()
        });
        match retry {
            Some(r) => bail!("{url} returned HTTP {status} (retry after {r})"),
            None => bail!("{url} returned HTTP {status}"),
        }
    }
    let body = v["content"].as_str().unwrap_or_default();
    if body.is_empty() {
        let error = v["error"].as_str().unwrap_or("no output");
        bail!("lightpanda rendered nothing for {url} ({error})");
    }
    Ok(Page { url: url.clone(), body: body.to_string(), is_markdown: false, via_browser: false })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use test_server::{Kind, chunked_reply, client, reply, retry_off, retry_on, serve, serve_replies, serve_routed};

    /// `<p>Hello, page.</p>` as gzip, as a server sends it under Content-Encoding: gzip.
    const GZIPPED: &[u8] = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\xb3\x29\xb0\xf3\x48\xcd\xc9\xc9\xd7\x51\x28\x48\x4c\x4f\xd5\xb3\xd1\x2f\xb0\x03\x00\x04\x6d\x98\xda\x13\x00\x00\x00";

    /// The memo's name for the server at `url`.
    fn host_of(url: &str) -> String {
        host_key(&Url::parse(url).expect("a URL")).expect("a host")
    }

    /// A server that answers the plain client with `plain` and the browser client with `browser`, whatever the path.
    fn serve_by_client(plain: Vec<u8>, browser: Vec<u8>) -> (String, test_server::Served) {
        serve_routed(move |head| Some(if head.contains("Chrome/") { browser.clone() } else { plain.clone() }))
    }

    /// The URL of a path on the server at `base`.
    fn file_at(base: &str, path: &str) -> Url {
        Url::parse(base).expect("a URL").join(path).expect("a path")
    }

    /// How long a small file may take in these tests.
    const WAIT: Duration = Duration::from_secs(4);

    #[test]
    fn markdown_served_as_plain_text_is_markdown() {
        assert!(served_markdown("text/markdown; charset=utf-8", "# Hi"));
        assert!(served_markdown("text/plain; charset=utf-8", "---\ntitle: useEffect\n---\n\n```js\nx\n```"));
        assert!(!served_markdown("text/plain", "<!DOCTYPE html><html></html>"));
        assert!(!served_markdown("text/html; charset=utf-8", "# not markdown"));
    }

    /// A cap small enough to pass in a test.
    const SMALL: usize = 1024;

    #[tokio::test]
    async fn a_page_whose_length_says_it_is_too_big_is_refused_unread() {
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 5000\r\n\r\n".to_string();
        let url = serve(head, Vec::new(), false);
        let err = fetch_capped(&client(), &url, SMALL, retry_off(), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn a_streamed_page_is_stopped_once_it_passes_the_cap() {
        // No Content-Length: the bytes that arrive are what is counted.
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let url = serve(head, vec![b'a'; 4 * SMALL], true);
        let err = fetch_capped(&client(), &url, SMALL, retry_off(), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn a_page_at_the_cap_is_read_whole_without_its_byte_order_mark() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let mut text = "\u{feff}<p>Hello, page.</p>".to_string();
        text.push_str(&" ".repeat(SMALL - text.len()));
        assert_eq!(text.len(), SMALL);
        let url = serve(head, text.clone().into_bytes(), true);
        let page =
            fetch_capped(&client(), &url, SMALL, retry_off(), Instant::now()).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, text.trim_start_matches('\u{feff}'));
    }

    /// The body as fetch_capped reads it, and the body reqwest's own text() reads from the same response.
    async fn both_ways(head: &str, body: Vec<u8>) -> (String, String) {
        let url = serve(head.to_string(), body.clone(), true);
        let ours = fetch_capped(&client(), &url, SMALL, retry_off(), Instant::now())
            .await
            .unwrap_or_else(|e| panic!("{e:#}"))
            .body;
        let res = client().get(serve(head.to_string(), body, true)).send().await.expect("a response");
        (ours, res.text().await.expect("text"))
    }

    #[tokio::test]
    async fn a_windows_1252_page_is_decoded_as_its_charset_says() {
        // 0xE9 is é and 0x80 is € in windows-1252; neither byte is valid UTF-8 on its own.
        let head =
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=windows-1252\r\nTransfer-Encoding: chunked\r\n\r\n";
        let (ours, theirs) = both_ways(head, b"Caf\xE9 costs \x80 5".to_vec()).await;
        assert_eq!(ours, "Café costs € 5");
        assert_eq!(ours, theirs);
    }

    #[tokio::test]
    async fn a_utf8_page_without_a_charset_is_read_as_utf8() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n";
        let (ours, theirs) = both_ways(head, "Café costs € 5".as_bytes().to_vec()).await;
        assert_eq!(ours, "Café costs € 5");
        assert_eq!(ours, theirs);
    }

    #[tokio::test]
    async fn a_byte_order_mark_decides_over_the_charset_the_header_names() {
        // UTF-16LE with its byte order mark, under a header that says windows-1252.
        let head =
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=windows-1252\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut body = vec![0xFF, 0xFE];
        body.extend("Café".encode_utf16().flat_map(u16::to_le_bytes));
        let (ours, theirs) = both_ways(head, body).await;
        assert_eq!(ours, "Café");
        assert_eq!(ours, theirs);
    }

    #[test]
    fn the_charset_is_the_labelled_one_or_utf8() {
        assert_eq!(charset_of("text/html; charset=windows-1252").name(), "windows-1252");
        assert_eq!(charset_of("text/html; Charset=\"Shift_JIS\"").name(), "Shift_JIS");
        assert_eq!(charset_of("text/html").name(), "UTF-8");
        assert_eq!(charset_of("text/html; charset=no-such-label").name(), "UTF-8");
    }

    #[test]
    fn the_accept_language_is_none_unless_the_run_names_one() {
        assert_eq!(accept_language_of(None), None);
        assert_eq!(accept_language_of(Some(String::new())), None);
        assert_eq!(accept_language_of(Some("  ".to_string())), None);
        assert_eq!(accept_language_of(Some(" es-ES, es;q=0.9 ".to_string())).as_deref(), Some("es-ES, es;q=0.9"));
        assert_eq!(accept_language_of(Some("es\nx".to_string())), None, "not a header value");
    }

    /// The Accept-Language lines (lower-cased) of the request that `served` recorded for `path`.
    fn accept_language_lines(heads: &[String], path: &str) -> Vec<String> {
        let head = heads
            .iter()
            .find(|h| h.lines().next().is_some_and(|l| l.split(' ').nth(1) == Some(path)))
            .expect("a request for the path");
        head.lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("accept-language:"))
            .map(|l| l.to_ascii_lowercase())
            .collect()
    }

    #[tokio::test]
    async fn a_named_language_is_the_one_line_each_client_sends_and_none_adds_no_line_of_jurls_own() {
        let (base, served) = serve_routed(|_| Some(reply("200 OK", "Content-Type: text/plain\r\n", b"ok")));
        let plain = |language: Option<String>| {
            reqwest::Client::builder()
                .default_headers(language_headers_of(language))
                .no_proxy()
                .build()
                .expect("a client")
        };
        let browser = |language: Option<String>| {
            wreq::Client::builder()
                .emulation(wreq_util::Emulation::Chrome149)
                .default_headers(browser_language_headers_of(language))
                .no_proxy()
                .build()
                .expect("a client")
        };
        let bare =
            wreq::Client::builder().emulation(wreq_util::Emulation::Chrome149).no_proxy().build().expect("a client");
        plain(Some("es".to_string())).get(format!("{base}/plain-es")).send().await.expect("a reply");
        browser(Some("es".to_string())).get(format!("{base}/browser-es")).send().await.expect("a reply");
        plain(None).get(format!("{base}/plain-none")).send().await.expect("a reply");
        browser(None).get(format!("{base}/browser-none")).send().await.expect("a reply");
        bare.get(format!("{base}/browser-bare")).send().await.expect("a reply");
        let heads = served.heads();
        assert_eq!(accept_language_lines(&heads, "/plain-es"), ["accept-language: es"]);
        assert_eq!(accept_language_lines(&heads, "/browser-es"), ["accept-language: es"]);
        assert!(accept_language_lines(&heads, "/plain-none").is_empty(), "the plain client sends none");
        assert_eq!(
            accept_language_lines(&heads, "/browser-none"),
            accept_language_lines(&heads, "/browser-bare"),
            "with none, the browser client sends only its emulation's own line"
        );
    }

    #[test]
    fn a_size_is_named_in_the_units_a_message_uses() {
        assert_eq!(size_label(8 << 20), "8 MB");
        assert_eq!(size_label(1 << 10), "1 KB");
        assert_eq!(size_label(1500), "1500 bytes");
    }

    #[test]
    fn a_403_is_retried_and_a_503_only_when_it_gives_no_retry_after() {
        assert!(retried(StatusCode::FORBIDDEN, false));
        assert!(retried(StatusCode::FORBIDDEN, true));
        assert!(retried(StatusCode::SERVICE_UNAVAILABLE, false));
        assert!(!retried(StatusCode::SERVICE_UNAVAILABLE, true));
        for code in [200, 301, 400, 401, 404, 408, 429, 500, 502, 504] {
            assert!(!retried(StatusCode::from_u16(code).unwrap(), false), "{code} was retried");
        }
    }

    #[test]
    fn the_retry_is_on_unless_the_run_or_the_environment_opts_out() {
        assert!(browser_retry_allowed(false, false));
        assert!(!browser_retry_allowed(true, false));
        assert!(!browser_retry_allowed(false, true));
    }

    #[tokio::test]
    async fn a_403_is_asked_again_with_a_browser_fingerprint_and_read() {
        let (url, served) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Hello, page.</p>")),
        ]);
        let memo = Memo::default();
        let now = Instant::now();
        let page =
            fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, "<p>Hello, page.</p>");
        assert!(page.via_browser);
        assert!(memo.on_browser(&host_of(&url), now), "the host is learned");
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser]);
    }

    #[tokio::test]
    async fn a_503_without_a_retry_after_is_asked_again_too() {
        let (url, served) = serve_replies(vec![
            Some(reply("503 Service Unavailable", "", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Hello, page.</p>")),
        ]);
        let memo = Memo::default();
        let page = fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), Instant::now())
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert!(page.via_browser);
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser]);
    }

    #[tokio::test]
    async fn a_503_that_says_when_to_come_back_is_final_whatever_the_form_of_retry_after() {
        // A delta in seconds, an HTTP date, and a value that is not one of those: each is a Retry-After, so each is final.
        for (header, value) in [
            ("Retry-After: 120\r\n", "120"),
            ("Retry-After: Wed, 21 Oct 2015 07:28:00 GMT\r\n", "Wed, 21 Oct 2015 07:28:00 GMT"),
            ("Retry-After: soon\r\n", "soon"),
        ] {
            let (url, served) = serve_replies(vec![Some(reply("503 Service Unavailable", header, b""))]);
            let memo = Memo::default();
            let err =
                fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
            assert!(
                format!("{err:#}").ends_with(&format!("returned HTTP 503 Service Unavailable (retry after {value})")),
                "{header:?}: {err:#}"
            );
            assert_eq!(served.kinds(), [Kind::Plain], "{header:?} was asked again");
        }
    }

    #[tokio::test]
    async fn a_refusal_on_the_retry_is_the_answer_and_nothing_more_is_asked() {
        let (url, served) =
            serve_replies(vec![Some(reply("403 Forbidden", "", b"")), Some(reply("403 Forbidden", "", b""))]);
        let memo = Memo::default();
        let err =
            fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
        assert!(
            format!("{err:#}").ends_with("returned HTTP 403 Forbidden (also with a browser's TLS fingerprint)"),
            "{err:#}"
        );
        assert_eq!(served.count(), 2);
        assert!(
            memo.learn_url(&Url::parse(&url).unwrap(), Instant::now()),
            "a refused retry is not a reply that teaches"
        );
    }

    #[tokio::test]
    async fn a_retry_that_gets_no_answer_leaves_the_first_refusal() {
        let (url, served) = serve_replies(vec![Some(reply("403 Forbidden", "", b"")), None]);
        let memo = Memo::default();
        let err =
            fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 403 Forbidden"), "{err:#}");
        assert_eq!(served.count(), 2);
    }

    #[tokio::test]
    async fn a_429_is_final_and_keeps_its_retry_after() {
        let (url, served) = serve_replies(vec![Some(reply("429 Too Many Requests", "Retry-After: 30\r\n", b""))]);
        let memo = Memo::default();
        let err =
            fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 429 Too Many Requests (retry after 30)"), "{err:#}");
        assert_eq!(served.kinds(), [Kind::Plain]);
    }

    #[tokio::test]
    async fn no_other_refusal_is_asked_again() {
        for status in ["401 Unauthorized", "404 Not Found", "500 Internal Server Error", "502 Bad Gateway"] {
            let (url, served) = serve_replies(vec![Some(reply(status, "", b""))]);
            let memo = Memo::default();
            let err =
                fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
            assert!(format!("{err:#}").ends_with(&format!("returned HTTP {status}")), "{status}: {err:#}");
            assert_eq!(served.kinds(), [Kind::Plain], "{status} was asked again");
        }
    }

    #[tokio::test]
    async fn with_the_retry_off_a_403_is_final() {
        let (url, served) = serve_replies(vec![Some(reply("403 Forbidden", "", b""))]);
        let err = fetch_capped(&client(), &url, PAGE_MAX, retry_off(), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 403 Forbidden"), "{err:#}");
        assert_eq!(served.kinds(), [Kind::Plain]);
    }

    #[tokio::test]
    async fn a_retried_page_keeps_the_size_limit_whether_or_not_it_says_its_length() {
        let memo = Memo::default();
        let (url, _) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\n", &[b'a'; 5000])),
        ]);
        let err = fetch_capped(&client(), &url, SMALL, retry_on(&memo), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");

        let (url, _) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(chunked_reply("200 OK", "Content-Type: text/html\r\n", &[b'a'; 4 * SMALL])),
        ]);
        let err = fetch_capped(&client(), &url, SMALL, retry_on(&memo), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn the_retry_follows_redirects_and_the_page_is_its_final_address() {
        let (url, served) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(reply("302 Found", "Location: /moved\r\n", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Hello, page.</p>")),
        ]);
        let memo = Memo::default();
        let page = fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), Instant::now())
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, "<p>Hello, page.</p>");
        assert_eq!(page.url.path(), "/moved");
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser, Kind::Browser]);
    }

    #[tokio::test]
    async fn a_retried_page_is_decompressed_when_the_browser_client_asked_for_it() {
        // The browser client asks for gzip, deflate, br and zstd, so the page it gets back may be compressed.
        let (url, _) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\nContent-Encoding: gzip\r\n", GZIPPED)),
        ]);
        let memo = Memo::default();
        let page = fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), Instant::now())
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, "<p>Hello, page.</p>");
    }

    /// A page the browser client answers, as a whole reply.
    fn ok_page(body: &str) -> Vec<u8> {
        reply("200 OK", "Content-Type: text/html\r\n", body.as_bytes())
    }

    #[tokio::test]
    async fn a_host_a_retry_showed_needs_the_browser_client_is_asked_there_first_after_that() {
        let (url, served) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(ok_page("<p>One.</p>")),
            Some(ok_page("<p>Two.</p>")),
        ]);
        let memo = Memo::default();
        let now = Instant::now();
        fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(memo.on_browser(&host_of(&url), now));
        let second =
            fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(second.body, "<p>Two.</p>");
        assert!(second.via_browser);
        // The second request never went to the plain client.
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser, Kind::Browser]);
    }

    #[tokio::test]
    async fn a_sticky_host_that_the_browser_client_refuses_is_refused_with_no_plain_request() {
        let (url, served) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(ok_page("<p>One.</p>")),
            Some(reply("403 Forbidden", "", b"")),
        ]);
        let memo = Memo::default();
        let now = Instant::now();
        fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        let err = fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), now).await.err().expect("refused");
        assert!(
            format!("{err:#}").ends_with("returned HTTP 403 Forbidden (with a browser's TLS fingerprint)"),
            "{err:#}"
        );
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser, Kind::Browser]);
    }

    #[tokio::test]
    async fn other_hosts_are_still_asked_plain_first() {
        let (sticky, _) = serve_replies(vec![Some(reply("403 Forbidden", "", b"")), Some(ok_page("<p>One.</p>"))]);
        let (other, other_served) = serve_replies(vec![Some(ok_page("<p>Other.</p>"))]);
        let memo = Memo::default();
        let now = Instant::now();
        fetch_capped(&client(), &sticky, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        let page =
            fetch_capped(&client(), &other, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(!page.via_browser);
        assert_eq!(other_served.kinds(), [Kind::Plain]);
    }

    #[tokio::test]
    async fn the_memo_forgets_a_host_after_the_ttl_in_a_long_lived_process() {
        // One memo for the whole life of the process (as `jurl mcp` keeps one), and time moved by the test.
        let (url, served) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(ok_page("<p>One.</p>")),
            Some(ok_page("<p>Two.</p>")),
            Some(reply("403 Forbidden", "", b"")),
            Some(ok_page("<p>Three.</p>")),
        ]);
        let memo = Memo::default();
        let host = host_of(&url);
        let learned_at = Instant::now();
        let just_before = learned_at + STICKY_TTL - Duration::from_secs(1);
        let after = learned_at + STICKY_TTL + Duration::from_secs(1);

        fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), learned_at).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(memo.on_browser(&host, learned_at), "learned by the first retry");
        let within = fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), just_before)
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert!(within.via_browser, "still on the browser client just before the TTL");
        let expired =
            fetch_capped(&client(), &url, PAGE_MAX, retry_on(&memo), after).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(expired.via_browser);
        assert!(memo.on_browser(&host, after), "after the TTL the host is tried plain again, and learned again");
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser, Kind::Browser, Kind::Plain, Kind::Browser]);
    }

    #[tokio::test]
    async fn the_opt_out_disables_the_memo_as_well_as_the_retry() {
        let (url, served) = serve_replies(vec![Some(reply("403 Forbidden", "", b""))]);
        let memo = Memo::default();
        let now = Instant::now();
        // The host is on the browser client, as a retry would have left it; the run has opted out.
        assert!(memo.learn_url(&Url::parse(&url).unwrap(), now));
        let retry = Retry { on: false, memo: &memo, timing: false };
        let err = fetch_capped(&client(), &url, PAGE_MAX, retry, now).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 403 Forbidden"), "{err:#}");
        assert_eq!(served.kinds(), [Kind::Plain]);
    }

    #[tokio::test]
    async fn a_small_file_refused_on_the_plain_client_is_read_from_the_browser_client_and_teaches_the_host() {
        let file = "User-agent: *\nDisallow: /private\n";
        let (base, served) = serve_by_client(
            reply("403 Forbidden", "", b""),
            reply("200 OK", "Content-Type: text/plain\r\n", file.as_bytes()),
        );
        let memo = Memo::default();
        let now = Instant::now();
        let body = fetch_small(&client(), &file_at(&base, "/robots.txt"), PAGE_MAX, WAIT, retry_on(&memo), now).await;
        assert_eq!(body.as_deref(), Some(file));
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser]);
        assert!(memo.on_browser(&host_of(&base), now), "the host is learned");
    }

    #[tokio::test]
    async fn a_503_without_a_retry_after_is_retried_for_a_small_file() {
        let (base, served) = serve_by_client(
            reply("503 Service Unavailable", "", b""),
            reply("200 OK", "Content-Type: text/xml\r\n", b"<urlset/>"),
        );
        let memo = Memo::default();
        let body =
            fetch_small(&client(), &file_at(&base, "/sitemap.xml"), PAGE_MAX, WAIT, retry_on(&memo), Instant::now())
                .await;
        assert_eq!(body.as_deref(), Some("<urlset/>"));
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser]);
    }

    #[tokio::test]
    async fn a_503_with_a_retry_after_is_not_retried_for_a_small_file_in_either_form() {
        for header in ["Retry-After: 120\r\n", "Retry-After: Wed, 21 Oct 2015 07:28:00 GMT\r\n"] {
            let (base, served) = serve_by_client(
                reply("503 Service Unavailable", header, b""),
                reply("200 OK", "Content-Type: text/xml\r\n", b"<urlset/>"),
            );
            let memo = Memo::default();
            let now = Instant::now();
            let body =
                fetch_small(&client(), &file_at(&base, "/sitemap.xml"), PAGE_MAX, WAIT, retry_on(&memo), now).await;
            assert_eq!(body, None, "{header:?}");
            assert_eq!(served.kinds(), [Kind::Plain], "{header:?} was retried");
            assert!(!memo.on_browser(&host_of(&base), now), "{header:?} taught the host");
        }
    }

    #[tokio::test]
    async fn a_429_or_401_or_404_is_not_retried_for_a_small_file() {
        for status in ["429 Too Many Requests", "401 Unauthorized", "404 Not Found"] {
            let (base, served) = serve_by_client(reply(status, "", b""), reply("200 OK", "", b"never"));
            let memo = Memo::default();
            let body =
                fetch_small(&client(), &file_at(&base, "/robots.txt"), PAGE_MAX, WAIT, retry_on(&memo), Instant::now())
                    .await;
            assert_eq!(body, None, "{status}");
            assert_eq!(served.kinds(), [Kind::Plain], "{status} was retried");
        }
    }

    #[tokio::test]
    async fn with_the_retry_off_a_small_file_refused_on_the_plain_client_is_not_retried() {
        let (base, served) = serve_by_client(reply("403 Forbidden", "", b""), reply("200 OK", "", b"never"));
        let memo = Memo::default();
        let now = Instant::now();
        // The host is on the memo, but the run has opted out: the memo is not read, and the file is not retried.
        assert!(memo.learn_url(&Url::parse(&base).unwrap(), now));
        let off = Retry { on: false, memo: &memo, timing: false };
        assert_eq!(fetch_small(&client(), &file_at(&base, "/robots.txt"), PAGE_MAX, WAIT, off, now).await, None);
        assert_eq!(served.kinds(), [Kind::Plain]);
    }

    #[tokio::test]
    async fn a_small_file_on_a_learned_host_comes_from_the_browser_client_alone() {
        let (base, served) = serve_by_client(
            reply("403 Forbidden", "", b""),
            reply("200 OK", "Content-Type: text/plain\r\n", b"User-agent: *\n"),
        );
        let memo = Memo::default();
        let now = Instant::now();
        assert!(memo.learn_url(&Url::parse(&base).unwrap(), now));
        let body = fetch_small(&client(), &file_at(&base, "/robots.txt"), PAGE_MAX, WAIT, retry_on(&memo), now).await;
        assert_eq!(body.as_deref(), Some("User-agent: *\n"));
        assert_eq!(served.kinds(), [Kind::Browser]);
    }

    #[tokio::test]
    async fn three_parallel_refused_files_of_one_host_each_retry_once_and_the_host_is_learned() {
        // The start host's robots.txt, llms.txt and sitemap.xml, fetched at once, each refused on the plain client.
        let (base, served) = serve_routed(|head| {
            let path = head.split(' ').nth(1).unwrap_or("/");
            if head.contains("Chrome/") {
                Some(reply("200 OK", "Content-Type: text/plain\r\n", format!("file {path}").as_bytes()))
            } else {
                Some(reply("403 Forbidden", "", b""))
            }
        });
        let memo = Memo::default();
        let now = Instant::now();
        let (plain, robots_url, llms_url, sitemap_url) =
            (client(), file_at(&base, "/robots.txt"), file_at(&base, "/llms.txt"), file_at(&base, "/sitemap.xml"));
        let (robots, llms, sitemap) = tokio::join!(
            fetch_small(&plain, &robots_url, PAGE_MAX, WAIT, retry_on(&memo), now),
            fetch_small(&plain, &llms_url, PAGE_MAX, WAIT, retry_on(&memo), now),
            fetch_small(&plain, &sitemap_url, PAGE_MAX, WAIT, retry_on(&memo), now),
        );
        assert_eq!(robots.as_deref(), Some("file /robots.txt"));
        assert_eq!(llms.as_deref(), Some("file /llms.txt"));
        assert_eq!(sitemap.as_deref(), Some("file /sitemap.xml"));
        let kinds = served.kinds();
        assert_eq!(kinds.iter().filter(|k| **k == Kind::Plain).count(), 3, "one plain request per file");
        assert_eq!(kinds.iter().filter(|k| **k == Kind::Browser).count(), 3, "one browser retry per file");
        assert!(memo.on_browser(&host_of(&base), now), "the host is learned, once");
    }

    #[test]
    fn a_host_is_named_by_its_host_and_port_when_it_has_one() {
        let named = |u: &str| host_key(&Url::parse(u).unwrap());
        assert_eq!(named("https://www.eleconomista.es/a").as_deref(), Some("www.eleconomista.es"));
        assert_eq!(named("https://www.eleconomista.es:443/a").as_deref(), Some("www.eleconomista.es"));
        assert_eq!(named("http://127.0.0.1:8080/a").as_deref(), Some("127.0.0.1:8080"));
    }

    /// The bench behind bench/browser-retry.md. Run it with `cargo test --release bench_browser_retry -- --ignored
    /// --nocapture`: each site in bench/browser-retry/sites.json is asked by the normal client, by the browser client,
    /// and by `fetch` with the retry on, and a JSON line per site is printed (prefixed `BENCH `). It needs the network,
    /// so it is not part of `cargo test`.
    #[tokio::test]
    #[ignore]
    async fn bench_browser_retry() {
        #[derive(serde::Deserialize)]
        struct Site {
            category: String,
            url: String,
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/bench/browser-retry/sites.json");
        let sites: Vec<Site> =
            serde_json::from_str(&std::fs::read_to_string(path).expect("the site list")).expect("a JSON list of sites");
        let client = crate::http_client().expect("a client");
        let memo = Memo::default();
        for site in sites {
            let started = Instant::now();
            let normal = get(&client, &site.url, PAGE_MAX).await;
            let normal_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let browser = get_browser(&site.url, PAGE_MAX).await;
            let browser_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let retry = fetch(&client, &site.url, retry_on(&memo)).await;
            let retry_ms = started.elapsed().as_millis();
            let retry = match &retry {
                Ok(page) => json!({ "ok": true, "bytes": page.body.len(), "via_browser": page.via_browser }),
                Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
            };
            let row = json!({
                "category": site.category,
                "url": site.url,
                "normal": outcome(&normal), "normal_ms": normal_ms,
                "browser": outcome(&browser), "browser_ms": browser_ms,
                "retry": retry, "retry_ms": retry_ms,
            });
            println!("BENCH {row}");
        }
    }

    /// A reply as the bench records it: the status and the decoded body length of a page, the status of a refusal, or
    /// the error the request ended in.
    fn outcome(reply: &Result<Reply>) -> Value {
        match reply {
            Ok(Reply::Page { status, page }) => json!({ "status": status.as_u16(), "bytes": page.body.len() }),
            Ok(Reply::Refused(refusal)) => json!({ "status": refusal.status.as_u16() }),
            Err(e) => json!({ "error": format!("{e:#}") }),
        }
    }

    /// A test site: each `(path, Content-Type, extra header lines, body)` is served at its path, and any other path is a
    /// 404. Returns its base URL.
    fn twin_site(routes: Vec<(&'static str, &'static str, &'static str, &'static str)>) -> String {
        serve_routed(move |head| {
            let path = head.split_whitespace().nth(1).unwrap_or_default();
            Some(match routes.iter().find(|route| route.0 == path) {
                Some((_, content_type, extra, body)) => {
                    reply("200 OK", &format!("Content-Type: {content_type}\r\n{extra}"), body.as_bytes())
                }
                None => reply("404 Not Found", "", b""),
            })
        })
        .0
    }

    #[test]
    fn a_canonical_link_is_found_among_the_others() {
        let preload = "</fonts/a.woff2>; rel=preload; as=font, </x.css>; rel=preload";
        assert_eq!(canonical_link(preload), None);
        let both = format!("{preload}, <https://site.test/page>; rel=\"alternate canonical\"");
        assert_eq!(canonical_link(&both), Some("https://site.test/page"));
    }

    #[test]
    fn a_md_url_names_the_page_it_is_the_markdown_of() {
        let stem = |u: &str| md_stem(&Url::parse(u).expect("a URL")).map(|s| s.to_string());
        assert_eq!(stem("https://linear.app/docs/saml.md").as_deref(), Some("https://linear.app/docs/saml"));
        assert_eq!(stem("https://site.test/docs/page.html.md").as_deref(), Some("https://site.test/docs/page.html"));
        assert_eq!(stem("https://site.test/docs/index.md").as_deref(), Some("https://site.test/docs/"));
        assert_eq!(stem("https://site.test/docs/saml"), None);
        assert_eq!(stem("https://site.test/.md"), None);
    }

    #[tokio::test]
    async fn a_markdown_page_at_md_is_read_as_the_html_page_it_is_the_alternate_of() {
        let md = "# Pricing\n\nThe Enterprise plan includes SSO.\n";
        let base = twin_site(vec![
            ("/docs/saml.md", "text/markdown; charset=utf-8", "", md),
            ("/docs/saml", "text/html", "", "<h1>Pricing</h1>"),
        ]);
        let page =
            fetch(&client(), &format!("{base}/docs/saml.md"), retry_off()).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(page.is_markdown);
        assert_eq!(page.url.as_str(), format!("{base}/docs/saml"));
        assert_eq!(page.body, md);
    }

    #[tokio::test]
    async fn a_markdown_page_whose_twin_is_missing_or_not_html_keeps_its_md_url() {
        // `a`'s twin is missing (404); `b`'s answers with markdown again, which is not an HTML page.
        let base = twin_site(vec![
            ("/docs/a.md", "text/markdown", "", "# A\n"),
            ("/docs/b.md", "text/markdown", "", "# B\n"),
            ("/docs/b", "text/markdown", "", "# B\n"),
        ]);
        for path in ["/docs/a.md", "/docs/b.md"] {
            let url = format!("{base}{path}");
            let page = fetch(&client(), &url, retry_off()).await.unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(page.url.as_str(), url);
        }
    }

    #[tokio::test]
    async fn a_canonical_link_names_the_html_page() {
        let base = twin_site(vec![
            ("/docs/one", "text/markdown", "Link: </docs/canon>; rel=\"canonical\"\r\n", "# One\n"),
            ("/docs/canon", "text/html", "", "<p>One</p>"),
        ]);
        let page = fetch(&client(), &format!("{base}/docs/one"), retry_off()).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.url.as_str(), format!("{base}/docs/canon"));
    }
}

/// Loopback servers for the tests that need a body of a given size or shape, a count of the requests made, or the client
/// that made each one.
#[cfg(test)]
pub(crate) mod test_server {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, LazyLock, Mutex},
        thread,
    };

    use super::{Memo, Retry};

    /// Answers one request with `head` (the status line and headers, ending in a blank line) and then `body`, in pieces
    /// when `chunked`. The client may hang up part way: the server just stops. Returns the URL to fetch.
    pub(crate) fn serve(head: String, body: Vec<u8>, chunked: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("the loopback address");
        thread::spawn(move || {
            let Ok((mut conn, _)) = listener.accept() else { return };
            let mut request = [0u8; 4096];
            let _ = conn.read(&mut request);
            if conn.write_all(head.as_bytes()).is_err() {
                return;
            }
            if !chunked {
                let _ = conn.write_all(&body);
                return;
            }
            for piece in body.chunks(16 << 10) {
                let mut frame = format!("{:x}\r\n", piece.len()).into_bytes();
                frame.extend_from_slice(piece);
                frame.extend_from_slice(b"\r\n");
                if conn.write_all(&frame).is_err() {
                    return;
                }
            }
            let _ = conn.write_all(b"0\r\n\r\n");
        });
        format!("http://{addr}/page")
    }

    /// Which client sent a request. The browser client's User-Agent is Chrome's; jurl's, and the test client's, are not.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Kind {
        Plain,
        Browser,
    }

    /// The requests a test server has taken, as their heads, in order.
    #[derive(Clone, Default)]
    pub(crate) struct Served {
        heads: Arc<Mutex<Vec<String>>>,
    }

    impl Served {
        pub(crate) fn count(&self) -> usize {
            self.heads.lock().expect("the request log").len()
        }

        /// The requests' heads, in the order they came.
        pub(crate) fn heads(&self) -> Vec<String> {
            self.heads.lock().expect("the request log").clone()
        }

        /// Which client made each request, in order.
        pub(crate) fn kinds(&self) -> Vec<Kind> {
            let heads = self.heads.lock().expect("the request log");
            heads.iter().map(|head| if head.contains("Chrome/") { Kind::Browser } else { Kind::Plain }).collect()
        }
    }

    /// A loopback server that answers each connection in turn with the next of `replies`: a whole reply (see `reply`),
    /// or None to hang up with no reply. Past the last one, each request gets a 500. Returns its URL and what it was sent.
    pub(crate) fn serve_replies(replies: Vec<Option<Vec<u8>>>) -> (String, Served) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("the loopback address");
        let served = Served::default();
        let log = served.heads.clone();
        thread::spawn(move || {
            let mut replies = replies.into_iter();
            loop {
                let Ok((mut conn, _)) = listener.accept() else { return };
                let head = read_head(&mut conn);
                log.lock().expect("the request log").push(head);
                let answer = replies.next().unwrap_or_else(|| Some(reply("500 Internal Server Error", "", b"")));
                if let Some(bytes) = answer {
                    let _ = conn.write_all(&bytes);
                }
            }
        });
        (format!("http://{addr}/page"), served)
    }

    /// A loopback server that answers each request by its head (the request line and headers): `route` gives the whole
    /// reply, or None to hang up with no reply. Returns the server's base URL and what it was sent.
    pub(crate) fn serve_routed(route: impl Fn(&str) -> Option<Vec<u8>> + Send + 'static) -> (String, Served) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("the loopback address");
        let served = Served::default();
        let log = served.heads.clone();
        thread::spawn(move || {
            loop {
                let Ok((mut conn, _)) = listener.accept() else { return };
                let head = read_head(&mut conn);
                let answer = route(&head);
                log.lock().expect("the request log").push(head);
                if let Some(bytes) = answer {
                    let _ = conn.write_all(&bytes);
                }
            }
        });
        (format!("http://{addr}"), served)
    }

    /// A whole HTTP/1.1 reply: the status line, the headers (each ending in CRLF), and the body, with the Content-Length
    /// and the Connection: close that a client needs to read it to its end.
    pub(crate) fn reply(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let head = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    /// A whole reply with a chunked body, sent in pieces of 16 KB, with no Content-Length.
    pub(crate) fn chunked_reply(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let head = format!("HTTP/1.1 {status}\r\n{headers}Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
        let mut bytes = head.into_bytes();
        for piece in body.chunks(16 << 10) {
            bytes.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            bytes.extend_from_slice(piece);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"0\r\n\r\n");
        bytes
    }

    /// The retry on, with `memo` as the run's memo.
    pub(crate) fn retry_on(memo: &Memo) -> Retry<'_> {
        Retry { on: true, memo, timing: false }
    }

    /// The retry off: no memo is ever read or written.
    pub(crate) fn retry_off() -> Retry<'static> {
        static NO_MEMO: LazyLock<Memo> = LazyLock::new(Memo::default);
        Retry { on: false, memo: &NO_MEMO, timing: false }
    }

    /// Reads a request up to its blank line, so the client has sent all of it before the reply comes. Returns the head.
    fn read_head(conn: &mut TcpStream) -> String {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match conn.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                _ => break,
            }
        }
        String::from_utf8_lossy(&head).into_owned()
    }

    /// A client for the loopback server, which ignores any proxy in the environment.
    pub(crate) fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().expect("a client")
    }
}

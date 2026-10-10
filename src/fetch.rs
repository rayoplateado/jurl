use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    path::Path,
    process::Stdio,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError, atomic::Ordering::Relaxed},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use encoding_rs::Encoding;
use futures::{Stream, StreamExt};
use reqwest::{
    Client, Response, StatusCode,
    cookie::{CookieStore, Jar},
    header,
};
use serde_json::Value;
use tokio::process::Command;
use url::Url;

use crate::{
    fallback::{self, Fallback},
    reach::{self, NotPublic, Reach},
    stealth::{self, Sidecar},
};

pub struct Page {
    pub url: Url,
    pub body: String,
    pub is_markdown: bool,
    /// The rung of the read ladder the page was read on.
    pub route: Route,
}

impl Page {
    /// Whether the page was read with the browser client: after a retry, or because the host was already on it.
    pub(crate) fn via_browser(&self) -> bool {
        self.route == Route::Browser
    }
}

/// The rung of the read ladder a page was read on (see the README): the plain client, the browser client's retry, the fallback
/// proxy, or the stealth sidecar. The usage's `route` names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Route {
    #[default]
    Direct,
    Browser,
    Proxy,
    Stealth,
}

impl Route {
    /// The route's name in the usage: `direct`, `browser`, `proxy` or `stealth`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Route::Direct => "direct",
            Route::Browser => "browser",
            Route::Proxy => "proxy",
            Route::Stealth => "stealth",
        }
    }
}

/// How a page was served, for the usage's `route`: the rung it was read on, and whether Lightpanda rendered it afterwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Served {
    pub(crate) route: Route,
    pub(crate) rendered: bool,
}

impl Served {
    /// The usage's name for the page: its route, with `+render` when it was rendered.
    pub(crate) fn name(self) -> String {
        if self.rendered { format!("{}+render", self.route.name()) } else { self.route.name().to_string() }
    }
}

/// The largest page read: a body past this is an error, not read on.
const PAGE_MAX: usize = 8 << 20;

/// How long a host stays on the browser client after a retry showed that it needs one, counted from that retry. The memo
/// lives as long as the process, and `jurl mcp` lives on: after this a host is asked plain first again, which costs one
/// plain request per host per window.
pub(crate) const STICKY_TTL: Duration = Duration::from_secs(10 * 60);

/// The hosts that need the browser client, each with the moment a retry first showed it. One memo per process, so the hops
/// of a `--follow` search and the calls of a `jurl mcp` server share it. The stealth sidecar keeps its own memo per run (see
/// `stealth.rs`), of the hosts whose call failed: that is the other half of this type.
#[derive(Default)]
pub(crate) struct Memo {
    learned: Mutex<HashMap<String, Instant>>,
    /// The hosts whose stealth-sidecar call failed. Kept for the memo's whole life, with no window: a run's memo lives as long as
    /// the run.
    sidecar_failed: Mutex<HashSet<String>>,
}

impl Memo {
    fn entries(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        self.learned.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the stealth sidecar's call for `host` failed in this memo's run.
    pub(crate) fn sidecar_failed(&self, host: &str) -> bool {
        self.sidecar_failed.lock().unwrap_or_else(PoisonError::into_inner).contains(host)
    }

    /// Records that the stealth sidecar's call for `host` failed: the host is not asked again in this memo's run.
    pub(crate) fn fail_sidecar(&self, host: &str) {
        self.sidecar_failed.lock().unwrap_or_else(PoisonError::into_inner).insert(host.to_string());
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
/// need the browser client, whether a host switching to the browser client is said on stderr (`-t`), and the run's stealth
/// sidecar for rung 5.
#[derive(Clone, Copy)]
pub(crate) struct Retry<'a> {
    pub(crate) on: bool,
    pub(crate) memo: &'a Memo,
    pub(crate) timing: bool,
    /// What the run may read (see [`Reach`]): under a public run, the guarded clients are the only ones a page goes through.
    pub(crate) reach: &'a Reach,
    /// The run's cookies (see [`Jar`]): each request carries the ones its address is due, and stores the ones its reply sets. One store per run, shared by both clients, so a host the retry moved to the browser client keeps them.
    pub(crate) cookies: &'a Jar,
    /// The run's stealth sidecar, when it is set up (see `stealth.rs`). Asked only for a page a host refuses, last of all.
    pub(crate) stealth: Option<&'a Sidecar>,
    /// The run's fallback proxy (see `fallback.rs`): the hosts on it, and the client that goes through it.
    pub(crate) fallback: &'a Fallback,
}

static MEMO: LazyLock<Memo> = LazyLock::new(Memo::default);

impl<'a> Retry<'a> {
    /// The retry for a run: on unless the run opts out with `--no-browser-retry` or `JURL_NO_BROWSER_RETRY`. The reach, the
    /// cookies, the stealth sidecar and the fallback proxy are the run's own.
    pub(crate) fn for_run(
        flag: bool,
        timing: bool,
        reach: &'a Reach,
        cookies: &'a Jar,
        stealth: Option<&'a Sidecar>,
        fallback: &'a Fallback,
    ) -> Self {
        let env_set = std::env::var_os("JURL_NO_BROWSER_RETRY").is_some();
        Retry { on: browser_retry_allowed(flag, env_set), memo: &MEMO, timing, reach, cookies, stealth, fallback }
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
pub(crate) fn host_key(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// The page at `url`. When the normal client is refused and the retry is on, the same URL is asked once more by the
/// browser client, whose TLS and HTTP/2 fingerprint some bot protection accepts where it refuses the normal client. A host
/// that a retry showed needs the browser client is asked there first, for STICKY_TTL. Nothing else is retried. A page that a host
/// refuses with 401, 403 or 429 is read again through the fallback proxy, when the run has one (rung 3), and a page still refused
/// with 401, 403 or 429 is last asked of the stealth sidecar, when the run has one (see `refused_page`).
pub(crate) async fn fetch(url: &str, retry: Retry<'_>) -> Result<Page> {
    fetch_capped(url, PAGE_MAX, retry, Instant::now()).await
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
const VIA_PROXY: &str = "through the proxy";

/// `fetch`, refusing a page larger than `max` bytes, and judging the host's memo as of `now`.
async fn fetch_capped(url: &str, max: usize, retry: Retry<'_>, now: Instant) -> Result<Page> {
    let parsed = Url::parse(url).ok();
    if let Some(u) = &parsed {
        reach::admit(u, retry.reach)?;
    }
    // A host on the fallback proxy is read through it from its first request, and only through it (rung 3). Its refusal is the
    // page's, and `refused_page` decides what follows it.
    if parsed.as_ref().is_some_and(|u| retry.fallback.uses(u)) {
        return match get(plain_for(retry.reach), url, max, retry).await? {
            Reply::Page { page, .. } => Ok(page),
            Reply::Refused(refused) => refused_page(url, refused, None, max, retry).await,
        };
    }
    if parsed.as_ref().is_some_and(|u| retry.sticky(u, now)) {
        // This host needed the browser client within STICKY_TTL: ask only that. Its refusal is the page's refusal.
        return match get_browser(url, max, retry).await? {
            Reply::Page { page, .. } => Ok(Page { route: Route::Browser, ..page }),
            Reply::Refused(refused) => refused_page(url, refused, Some(VIA_BROWSER), max, retry).await,
        };
    }
    let refused = match get(plain_for(retry.reach), url, max, retry).await? {
        Reply::Page { page, .. } => return Ok(page),
        Reply::Refused(refused) => refused,
    };
    // A refusal that came through the proxy is not retried with the browser client: it is the proxy's answer.
    if refused.via_proxy || !retry.on || !retried(refused.status, refused.retry_after.is_some()) {
        return refused_page(url, refused, None, max, retry).await;
    }
    let res = match browser_send(url, retry).await {
        Ok(res) => res,
        // The guard's refusal is the answer, not the plain refusal.
        Err(e) if e.downcast_ref::<NotPublic>().is_some() => return Err(e),
        // The browser client got no reply at all: the plain refusal stands, and the page is still refused.
        Err(_) => return refused_page(url, refused, None, max, retry).await,
    };
    // A success from the browser client means it got past the bot check: the host is on the client from now on.
    if let Some(parsed) = &parsed
        && res.status().is_success()
    {
        retry.learn(parsed, now);
    }
    // Once the browser client has a reply, that reply is the answer: a page too large to read is an error, not a fallback.
    match browser_reply(res, url, max, retry).await? {
        Reply::Page { page, .. } => Ok(Page { route: Route::Browser, ..page }),
        Reply::Refused(again) => refused_page(url, again, Some(VIA_BROWSER_TOO), max, retry).await,
    }
}

/// What a page that a host refused is answered with, once the direct clients (and the browser client's retry, where it
/// applies) have refused it. A direct refusal with 401, 403 or 429 puts the page's host on the fallback proxy, and the page is
/// read again through it (rung 3): that is the host's switch, said on stderr under `-t`. A refusal through the proxy, or one
/// that does not switch a host, goes to the sidecar (rung 5) and else stands as the error (see `sidecar_or_error`).
async fn refused_page(url: &str, refused: Refusal, via: Option<&str>, max: usize, retry: Retry<'_>) -> Result<Page> {
    if let Some(target) = Url::parse(url).ok().filter(|_| retry.fallback.has_proxy())
        && !refused.via_proxy
        && fallback::refused_enough(refused.status)
    {
        retry.fallback.put_on_proxy(&target, &switch_why(refused.status, via), retry.timing);
        return match get(plain_for(retry.reach), url, max, retry).await? {
            Reply::Page { page, .. } => Ok(page),
            Reply::Refused(again) => {
                let via = again.via_proxy.then_some(VIA_PROXY);
                sidecar_or_error(url, again, via, max, retry).await
            }
        };
    }
    let via = if refused.via_proxy { Some(VIA_PROXY) } else { via };
    sidecar_or_error(url, refused, via, max, retry).await
}

/// Why a host is put on the proxy, for the `-t` line: the status that refused it, and which client did.
fn switch_why(status: StatusCode, via: Option<&str>) -> String {
    let from = match via {
        Some(VIA_BROWSER) => "browser",
        Some(VIA_BROWSER_TOO) => "direct and browser",
        _ => "direct",
    };
    format!("{} from {from}", status.as_u16())
}

/// The stealth sidecar's answer to a refused page (rung 5), or else the refusal as the error. Only 401, 403 and 429 are asked of
/// the sidecar (see [`stealth::asked_for`]), and a run with no sidecar never asks.
async fn sidecar_or_error(
    url: &str,
    refused: Refusal,
    via: Option<&str>,
    max: usize,
    retry: Retry<'_>,
) -> Result<Page> {
    if let Some(sidecar) = retry.stealth
        && stealth::asked_for(refused.status)
        && let Ok(target) = Url::parse(url)
        && let Some(body) = sidecar.read(&target, retry.reach, max, retry.timing).await?
    {
        return Ok(Page { url: target, body, is_markdown: false, route: Route::Stealth });
    }
    Err(refused.error(url, via))
}

/// A small text file (robots.txt, llms.txt, a sitemap) at `url`, or None. It follows the retry rule for pages, with the same
/// memo: a host on the browser client is read from there alone; otherwise the plain client is asked first, and a refusal the
/// rule retries (a 403, or a 503 without a Retry-After) is asked once more of the browser client, whose success teaches the
/// host. Its body is decoded as a page is; which bodies count as the site's file is for the caller to say. It never asks the
/// stealth sidecar: rung 5 is for pages, and these files are not pages. A host on the fallback proxy has its files read through
/// it, as its pages are, and a refused file does not put a host on the proxy: only a refused page does (see `refused_page`).
pub(crate) async fn fetch_small(
    url: &Url,
    max: usize,
    timeout: Duration,
    retry: Retry<'_>,
    now: Instant,
) -> Option<String> {
    if reach::admit(url, retry.reach).is_err() {
        return small_refused(url, retry.timing);
    }
    if !retry.fallback.uses(url) && retry.sticky(url, now) {
        let res = match send_browser_get(url, retry, Some(timeout), &[]).await {
            Ok(res) => res,
            Err(e) if e.downcast_ref::<NotPublic>().is_some() => return small_refused(url, retry.timing),
            Err(_) => return None,
        };
        return small_from_browser(res, max, retry).await;
    }
    let (mut res, via_proxy) = match send_get(plain_for(retry.reach), url.as_str(), retry, &[], Some(timeout)).await {
        Ok(sent) => sent,
        Err(e) if e.downcast_ref::<NotPublic>().is_some() => return small_refused(url, retry.timing),
        Err(_) => return None,
    };
    let status = res.status();
    if status.is_success() {
        let content_type =
            res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let bytes = read_capped(&mut res, max).await.ok().flatten()?;
        retry.fallback.count(via_proxy, bytes.len());
        return Some(decode(&content_type, &bytes));
    }
    crate::decide::USAGE.plain_refusals.fetch_add(1, Relaxed);
    let retry_after = res.headers().get(header::RETRY_AFTER).is_some();
    if via_proxy || !retry.on || !retried(status, retry_after) {
        return None;
    }
    let res = match send_browser_get(url, retry, Some(timeout), &[]).await {
        Ok(res) => res,
        Err(e) if e.downcast_ref::<NotPublic>().is_some() => return small_refused(url, retry.timing),
        Err(_) => return None,
    };
    if res.status().is_success() {
        retry.learn(url, now);
    }
    small_from_browser(res, max, retry).await
}

/// A small file the guard refused: None, said on stderr under `-t` as a refused page is.
fn small_refused(url: &Url, timing: bool) -> Option<String> {
    if timing {
        eprintln!("jurl: skipped {url}: {}", NotPublic);
    }
    None
}

/// The body of a small file from a browser client's reply: decoded as a page is, or None on a refusal or a body larger than
/// `max`.
async fn small_from_browser(res: wreq::Response, max: usize, retry: Retry<'_>) -> Option<String> {
    if !res.status().is_success() {
        return None;
    }
    let content_type =
        res.headers().get(wreq::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let bytes = read_stream_capped(res.content_length(), res.bytes_stream(), max).await.ok().flatten()?;
    retry.fallback.count(false, bytes.len());
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

/// A reply that is not a success: its status, its Retry-After when it has one (in either form, as text), and whether the request
/// went through the fallback proxy.
struct Refusal {
    status: StatusCode,
    retry_after: Option<String>,
    via_proxy: bool,
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

/// One GET of `url` with the plain `client`, or through the fallback proxy where its host is on it (see `send_get`): the page,
/// or the refusal.
async fn get(client: &Client, url: &str, max: usize, retry: Retry<'_>) -> Result<Reply> {
    let (mut res, via_proxy) =
        send_get(client, url, retry, &[("accept", "text/markdown, text/html;q=0.9, */*;q=0.5".to_string())], None)
            .await?;
    let status = res.status();
    if !status.is_success() {
        crate::decide::USAGE.plain_refusals.fetch_add(1, Relaxed);
        let retry_after = res.headers().get(header::RETRY_AFTER).map(retry_after_text);
        return Ok(Reply::Refused(Refusal { status, retry_after, via_proxy }));
    }
    let final_url = res.url().clone();
    let ct = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let links: Vec<String> =
        res.headers().get_all(header::LINK).iter().filter_map(|v| v.to_str().ok()).map(String::from).collect();
    let Some(bytes) = read_capped(&mut res, max).await.map_err(|e| retry.fallback.scrubbed(via_proxy, e))? else {
        bail!("{url}: page larger than {}", size_label(max));
    };
    retry.fallback.count(via_proxy, bytes.len());
    let mut page = to_page(final_url, &ct, &bytes);
    page.route = if via_proxy { Route::Proxy } else { Route::Direct };
    // The body is read as its markdown, but the page a person reads is the HTML page it is the alternate of.
    if page.is_markdown {
        let md = page.url.clone();
        page.url = html_twin(client, retry, &md, &links).await.unwrap_or(md);
    }
    Ok(Reply::Page { status, page })
}

/// One GET of `url` with the browser client, which asks with its own headers: the page, or the refusal. No reply at all,
/// or a reply that cannot be read, is an error.
async fn get_browser(url: &str, max: usize, retry: Retry<'_>) -> Result<Reply> {
    browser_reply(browser_send(url, retry).await?, url, max, retry).await
}

/// The browser client's request for `url`, sent. An error here means no reply came; the guard's refusal is its own error.
async fn browser_send(url: &str, retry: Retry<'_>) -> Result<wreq::Response> {
    let target = Url::parse(url).map_err(|e| anyhow!("{url}: {e}"))?;
    send_browser_get(&target, retry, None, &[]).await
}

/// The browser client's reply to `url`: the page, or the refusal. A reply that cannot be read is an error.
async fn browser_reply(res: wreq::Response, url: &str, max: usize, retry: Retry<'_>) -> Result<Reply> {
    let status = res.status();
    if !status.is_success() {
        let retry_after = res.headers().get(wreq::header::RETRY_AFTER).map(retry_after_text);
        return Ok(Reply::Refused(Refusal { status, retry_after, via_proxy: false }));
    }
    let final_url = Url::parse(&res.uri().to_string()).with_context(|| format!("the address {url} ended at"))?;
    let ct =
        res.headers().get(wreq::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let Some(bytes) = read_stream_capped(res.content_length(), res.bytes_stream(), max).await? else {
        bail!("{url}: page larger than {}", size_label(max));
    };
    retry.fallback.count(false, bytes.len());
    Ok(Reply::Page { status, page: to_page(final_url, &ct, &bytes) })
}

/// What both browser clients share: Chrome 149's emulation (its TLS and HTTP/2 fingerprint, headers and user agent), the
/// Accept-Language a run names, and the timeouts. Each client adds its own redirects and resolver.
fn browser_builder() -> wreq::ClientBuilder {
    wreq::Client::builder()
        .emulation(wreq_util::Emulation::Chrome149)
        .default_headers(browser_language_headers())
        .timeout(crate::HTTP_TIMEOUT)
        .pool_idle_timeout(crate::POOL_IDLE_TIMEOUT)
}

/// The browser client, built on first use and then shared, so its connections are pooled across hops and files. Proxies from
/// the environment apply, as they do to the normal client. It follows no redirects: [`send_browser_get`] follows them, so
/// that each hop carries the run's cookies.
static BROWSER: LazyLock<std::result::Result<wreq::Client, String>> = LazyLock::new(|| {
    let builder = browser_builder().redirect(wreq::redirect::Policy::none());
    // The loopback servers in the tests are reached directly, as the normal client reaches them.
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder.build().map_err(|e| format!("building the browser client: {e}"))
});

/// The browser client of a public run: the same, with no redirects of its own (see [`send_browser_get`]) and every connection
/// checked by the resolver. It uses the environment's proxy, if any, as the normal client does: then each target is checked
/// before its request.
static GUARDED_BROWSER: LazyLock<std::result::Result<wreq::Client, String>> = LazyLock::new(|| {
    guarded_browser_builder(reach::proxy_hosts(env_var))
        .build()
        .map_err(|e| format!("building the guarded browser client: {e}"))
});

fn guarded_browser_builder(exempt: Vec<String>) -> wreq::ClientBuilder {
    let builder =
        browser_builder().redirect(wreq::redirect::Policy::none()).dns_resolver(reach::GlobalOnly::new(exempt));
    // The tests' loopback servers are reached directly, as the normal client reaches them; a proxy test adds its own.
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The browser client a request of a run with `reach` goes through.
fn browser_for(reach: &Reach) -> Result<wreq::Client> {
    let shared = match reach {
        Reach::Private => &BROWSER,
        Reach::Public { .. } => &GUARDED_BROWSER,
    };
    shared.as_ref().cloned().map_err(|e| anyhow!("{e}"))
}

/// The plain client of a private run's pages: jurl's own settings, with no redirects of its own (see [`send_get`]). It uses
/// the environment's proxy, as the normal client does.
static PRIVATE_PLAIN: LazyLock<Client> = LazyLock::new(|| {
    let builder = crate::client_builder().redirect(reqwest::redirect::Policy::none());
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder.build().expect("the private plain client builds from fixed settings")
});

/// The plain client of a public run: jurl's own settings, with no redirects of its own (see [`send_get`]) and every connection
/// checked by the resolver. It uses the environment's proxy, if any: then each target is checked before its request.
static GUARDED_PLAIN: LazyLock<Client> = LazyLock::new(|| {
    guarded_plain_builder(reach::proxy_hosts(env_var)).build().expect("the guarded client builds from fixed settings")
});

fn guarded_plain_builder(exempt: Vec<String>) -> reqwest::ClientBuilder {
    let builder = crate::client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(reach::GlobalOnly::new(exempt));
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder
}

/// The plain client a page request of a run with `reach` goes through: the guarded one under a public run, otherwise the
/// private one.
pub(crate) fn plain_for(reach: &Reach) -> &'static Client {
    match reach {
        Reach::Private => &PRIVATE_PLAIN,
        Reach::Public { .. } => &GUARDED_PLAIN,
    }
}

/// The address a redirect answer names, when it is one a client follows (301, 302, 303, 307, 308) and it names one.
fn redirect_target(status: u16, location: Option<&str>, from: &Url) -> Option<Url> {
    match status {
        301 | 302 | 303 | 307 | 308 => from.join(location?).ok(),
        _ => None,
    }
}

/// One GET of `url` under the run's reach and cookies, with the plain `client`, or through the fallback proxy where its host is on
/// it (see [`fallback::Fallback::uses`]). Every hop is made here, so each one carries the cookies its address is due and stores
/// the Set-Cookie of its reply: a site that sets a cookie and redirects until the cookie comes back is followed. Under a public
/// run each address is checked before its request (see [`reach::check`]), and so is each redirect target, because a proxy
/// resolves names itself. A chain keeps the client it started with, and a hop to a host on the proxy switches the rest of the
/// chain to it. The bool is whether the final reply came through the proxy.
pub(crate) async fn send_get(
    client: &Client,
    url: &str,
    retry: Retry<'_>,
    headers: &[(&str, String)],
    timeout: Option<Duration>,
) -> Result<(Response, bool)> {
    let mut target = Url::parse(url).map_err(|e| anyhow!("{url}: {e}"))?;
    let mut via_proxy = false;
    for _ in 0..=reach::MAX_REDIRECTS {
        reach::check(&target, retry.reach).await?;
        if retry.fallback.uses(&target) {
            // A host that `JURL_PROXY_FIRST` puts on the proxy is said once; a host already on it is not news.
            retry.fallback.put_on_proxy(&target, "JURL_PROXY_FIRST=1", retry.timing);
            via_proxy = true;
        }
        let hop = match retry.fallback.client().filter(|_| via_proxy) {
            Some(proxied) => proxied,
            None => client,
        };
        let mut request = hop.get(target.as_str());
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if let Some(cookie) = retry.cookies.cookies(&target) {
            request = request.header(header::COOKIE, cookie);
        }
        let res = match request.send().await {
            Ok(res) => res,
            Err(e) if reach::refused(&e) => return Err(NotPublic.into()),
            Err(e) => {
                let err = anyhow::Error::new(e).context(format!("fetching {target}"));
                return Err(retry.fallback.scrubbed(via_proxy, err));
            }
        };
        retry.cookies.set_cookies(&mut res.headers().get_all(header::SET_COOKIE).iter(), &target);
        let location = res.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()).map(str::to_string);
        match redirect_target(res.status().as_u16(), location.as_deref(), &target) {
            Some(next) => target = next,
            None => return Ok((res, via_proxy)),
        }
    }
    bail!("{url}: too many redirects")
}

/// [`send_get`] for the browser client: the same rules, and each request it makes is counted. The browser client has no proxy,
/// so a hop to a host on the fallback proxy is not made with it: the browser's request fails, and the refusal it was retrying
/// stands (see `fetch_capped`).
async fn send_browser_get(
    url: &Url,
    retry: Retry<'_>,
    timeout: Option<Duration>,
    headers: &[(&str, String)],
) -> Result<wreq::Response> {
    let client = browser_for(retry.reach)?;
    let mut target = url.clone();
    for _ in 0..=reach::MAX_REDIRECTS {
        reach::check(&target, retry.reach).await?;
        if retry.fallback.uses(&target) {
            bail!("{target}: on the fallback proxy, which the browser client does not use");
        }
        crate::decide::USAGE.browser_requests.fetch_add(1, Relaxed);
        let mut request = client.get(target.as_str());
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if let Some(cookie) = retry.cookies.cookies(&target) {
            request = request.header(wreq::header::COOKIE, cookie);
        }
        let res = match request.send().await {
            Ok(res) => res,
            Err(e) if reach::refused(&e) => return Err(NotPublic.into()),
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("fetching {target} with a browser's fingerprint")));
            }
        };
        retry.cookies.set_cookies(&mut res.headers().get_all(wreq::header::SET_COOKIE).iter(), &target);
        let location = res.headers().get(wreq::header::LOCATION).and_then(|v| v.to_str().ok()).map(str::to_string);
        match redirect_target(res.status().as_u16(), location.as_deref(), &target) {
            Some(next) => target = next,
            None => return Ok(res),
        }
    }
    bail!("{url}: too many redirects")
}

/// The Accept of an image request: a browser's for an `<img>`, without AVIF, which jurl's decoder does not read. A CDN that
/// negotiates by Accept sends AVIF to a request that lists it first.
const IMAGE_ACCEPT: &str = "image/webp,image/apng,image/*,*/*;q=0.8";

/// The headers a browser sends with an image it shows on `page`: the Referer, the image Accept, and the Sec-Fetch-* of an
/// image request, with Sec-Fetch-Site computed from the two addresses.
fn image_headers(page: &Url, image: &Url) -> Vec<(&'static str, String)> {
    vec![
        ("referer", page.as_str().to_string()),
        ("accept", IMAGE_ACCEPT.to_string()),
        ("sec-fetch-dest", "image".to_string()),
        ("sec-fetch-mode", "no-cors".to_string()),
        ("sec-fetch-site", fetch_site(page, image).to_string()),
    ]
}

/// `Sec-Fetch-Site` of a request from `page` for `target`: same-origin when the scheme, host and port match; same-site when
/// the scheme matches and the hosts share a registrable domain (see `hosts`); cross-site otherwise.
fn fetch_site(page: &Url, target: &Url) -> &'static str {
    if page.origin() == target.origin() {
        "same-origin"
    } else if page.scheme() == target.scheme() && crate::hosts::same_registrable(page, target) {
        "same-site"
    } else {
        "cross-site"
    }
}

/// A host's refusal of an image: a reply that is not a success, after the browser retry if one was made. Vision reports the
/// image as refused, and asks no more of that host's images until its first one has been answered (see `vision`).
#[derive(Debug)]
pub(crate) struct ImageRefused(pub(crate) String);

impl std::fmt::Display for ImageRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ImageRefused {}

/// The bytes of the image `url` that `page` shows, at most `max`, asked the way a browser's `<img>` asks (see
/// [`image_headers`]). A 401, 403 or 429 from the plain client is asked once more of the browser client, as the page is,
/// and a host that such a retry showed needs the browser client is asked there first, through the memo pages share. A host on
/// the fallback proxy has its images read through it, and a refusal is not retried. An image never puts a host on the proxy: only
/// a page does. A refusal is an [`ImageRefused`]. A guard refusal, an image too large to read, and no reply are ordinary errors.
pub(crate) async fn image_bytes(page: &Url, url: &Url, retry: Retry<'_>, max: usize) -> Result<Vec<u8>> {
    let now = Instant::now();
    let headers = image_headers(page, url);
    if !retry.fallback.uses(url) && retry.sticky(url, now) {
        let res = send_browser_get(url, retry, None, &headers).await?;
        return browser_image(res, url, max, VIA_BROWSER, retry).await;
    }
    let (mut res, via_proxy) = send_get(plain_for(retry.reach), url.as_str(), retry, &headers, None).await?;
    if res.status().is_success() {
        let bytes = read_capped(&mut res, max)
            .await
            .map_err(|e| retry.fallback.scrubbed(via_proxy, e))?
            .ok_or_else(|| anyhow!("{url}: image larger than {}", size_label(max)))?;
        retry.fallback.count(via_proxy, bytes.len());
        return Ok(bytes);
    }
    crate::decide::USAGE.plain_refusals.fetch_add(1, Relaxed);
    let refused = Refusal {
        status: res.status(),
        retry_after: res.headers().get(header::RETRY_AFTER).map(retry_after_text),
        via_proxy,
    };
    let asked_again =
        matches!(refused.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS);
    // A refusal through the proxy is the answer too: an image never puts a host on the proxy, and is not retried with the browser.
    if via_proxy || !retry.on || !asked_again {
        let via = via_proxy.then_some(VIA_PROXY);
        return Err(ImageRefused(refused.error(url.as_str(), via).to_string()).into());
    }
    let res = match send_browser_get(url, retry, None, &headers).await {
        Ok(res) => res,
        // The guard's refusal is the answer, not the plain refusal.
        Err(e) if e.downcast_ref::<NotPublic>().is_some() => return Err(e),
        // The browser client got no reply at all: the plain refusal stands.
        Err(_) => return Err(ImageRefused(refused.error(url.as_str(), None).to_string()).into()),
    };
    // A success from the browser client means it got past the bot check: the host is on the browser client from now on.
    if res.status().is_success() {
        retry.learn(url, now);
    }
    browser_image(res, url, max, VIA_BROWSER_TOO, retry).await
}

/// The bytes of an image from a browser client's reply, or its refusal, which says `via` in its error.
async fn browser_image(res: wreq::Response, url: &Url, max: usize, via: &str, retry: Retry<'_>) -> Result<Vec<u8>> {
    if !res.status().is_success() {
        let retry_after = res.headers().get(wreq::header::RETRY_AFTER).map(retry_after_text);
        let refused = Refusal { status: res.status(), retry_after, via_proxy: false };
        return Err(ImageRefused(refused.error(url.as_str(), Some(via)).to_string()).into());
    }
    let bytes = read_stream_capped(res.content_length(), res.bytes_stream(), max)
        .await?
        .ok_or_else(|| anyhow!("{url}: image larger than {}", size_label(max)))?;
    retry.fallback.count(false, bytes.len());
    Ok(bytes)
}

/// Whether `url` may be rendered under `reach`, checked before the browser starts (see the README). A private run renders as
/// before, and a public run renders a page only if its own address passes the check. Under `JURL_PUBLIC_ONLY=1` the browser's
/// own requests (its redirects, the page's scripts) are not checked, so it does not render at all, unless the network outside
/// jurl confines it (`sandboxed`, from `JURL_RENDER_SANDBOXED=1`): then the page's address is checked, as it always is, and
/// the browser runs.
pub(crate) async fn render_allowed(url: &Url, reach: &Reach, public_only: bool, sandboxed: bool) -> Result<()> {
    if matches!(reach, Reach::Private) {
        return Ok(());
    }
    if public_only && !sandboxed {
        bail!("rendering is refused under JURL_PUBLIC_ONLY=1: the browser's own requests are not checked");
    }
    reach::check(url, reach).await
}

/// The page a body makes: its text as its Content-Type's charset says, and whether it is markdown.
fn to_page(url: Url, content_type: &str, bytes: &[u8]) -> Page {
    let body = decode(content_type, bytes);
    let is_markdown = served_markdown(content_type, &body);
    Page { url, body, is_markdown, route: Route::Direct }
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
async fn html_twin(client: &Client, retry: Retry<'_>, url: &Url, links: &[String]) -> Option<Url> {
    let canonical = links.iter().filter_map(|l| canonical_link(l)).filter_map(|c| url.join(c).ok());
    for candidate in canonical.chain(md_stem(url)) {
        if candidate == *url || candidate.host_str() != url.host_str() {
            continue;
        }
        if let Some(page) = serves_html(client, retry, &candidate).await {
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
async fn serves_html(client: &Client, retry: Retry<'_>, url: &Url) -> Option<Url> {
    let (res, _) =
        send_get(client, url.as_str(), retry, &[("accept", "text/html".to_string())], Some(TWIN_TIMEOUT)).await.ok()?;
    let ct = res.headers().get(header::CONTENT_TYPE)?.to_str().ok()?;
    let html = ct.starts_with("text/html") || ct.starts_with("application/xhtml+xml");
    let same_host = res.url().host_str() == url.host_str();
    (res.status().is_success() && html && same_host).then(|| res.url().clone())
}

/// Run the page's JavaScript in Lightpanda and return the resulting DOM.
/// Waits for the network to settle and for real visible text to appear, capped at 8s:
/// SPAs keep background traffic going and often paint content after the network calms down.
/// A host on the fallback proxy is rendered through it: Lightpanda is given the proxy, and every request of the render goes
/// through it, the page's scripts and images included (see the README). An error of such a render is scrubbed of the proxy.
pub async fn render(bin: &Path, url: &Url, fallback: &Fallback) -> Result<Page> {
    let proxy = fallback.lightpanda_args(url);
    let proxied = !proxy.is_empty();
    if proxied {
        // A render is billed by the proxy whatever it returns, so it is counted as it starts.
        fallback.count_render();
    }
    let body = dom_of(bin, url, &proxy).await.map_err(|e| fallback.scrubbed(proxied, e))?;
    // The render's own requests (its scripts, its images) are not counted: only the DOM it returned is.
    fallback.count(proxied, body.len());
    Ok(Page { url: url.clone(), body, is_markdown: false, route: if proxied { Route::Proxy } else { Route::Direct } })
}

/// The DOM Lightpanda returns for `url`, run with the arguments `proxy` (none for a direct render).
async fn dom_of(bin: &Path, url: &Url, proxy: &[String]) -> Result<String> {
    let run = Command::new(bin)
        .args(["fetch", "--json", "--dump", "html", "--wait-until", "networkalmostidle", "--wait-ms", "8000"])
        .args(["--wait-script", "document.body && (document.body.innerText || '').trim().length > 1500"])
        .args(proxy)
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
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use test_server::{
        Kind, NO_COOKIES, NO_FALLBACK, PRIVATE, chunked_reply, client, reply, retry_at, retry_off, retry_on, serve,
        serve_replies, serve_routed,
    };

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
        let err = fetch_capped(&url, SMALL, retry_off(), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn a_streamed_page_is_stopped_once_it_passes_the_cap() {
        // No Content-Length: the bytes that arrive are what is counted.
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let url = serve(head, vec![b'a'; 4 * SMALL], true);
        let err = fetch_capped(&url, SMALL, retry_off(), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn a_page_at_the_cap_is_read_whole_without_its_byte_order_mark() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let mut text = "\u{feff}<p>Hello, page.</p>".to_string();
        text.push_str(&" ".repeat(SMALL - text.len()));
        assert_eq!(text.len(), SMALL);
        let url = serve(head, text.clone().into_bytes(), true);
        let page = fetch_capped(&url, SMALL, retry_off(), Instant::now()).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, text.trim_start_matches('\u{feff}'));
    }

    /// The body as fetch_capped reads it, and the body reqwest's own text() reads from the same response.
    async fn both_ways(head: &str, body: Vec<u8>) -> (String, String) {
        let url = serve(head.to_string(), body.clone(), true);
        let ours =
            fetch_capped(&url, SMALL, retry_off(), Instant::now()).await.unwrap_or_else(|e| panic!("{e:#}")).body;
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
        let page = fetch_capped(&url, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, "<p>Hello, page.</p>");
        assert!(page.via_browser());
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
        let page =
            fetch_capped(&url, PAGE_MAX, retry_on(&memo), Instant::now()).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(page.via_browser());
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
            let err = fetch_capped(&url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
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
        let err = fetch_capped(&url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
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
        let err = fetch_capped(&url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 403 Forbidden"), "{err:#}");
        assert_eq!(served.count(), 2);
    }

    #[tokio::test]
    async fn a_429_is_final_and_keeps_its_retry_after() {
        let (url, served) = serve_replies(vec![Some(reply("429 Too Many Requests", "Retry-After: 30\r\n", b""))]);
        let memo = Memo::default();
        let err = fetch_capped(&url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 429 Too Many Requests (retry after 30)"), "{err:#}");
        assert_eq!(served.kinds(), [Kind::Plain]);
    }

    #[tokio::test]
    async fn no_other_refusal_is_asked_again() {
        for status in ["401 Unauthorized", "404 Not Found", "500 Internal Server Error", "502 Bad Gateway"] {
            let (url, served) = serve_replies(vec![Some(reply(status, "", b""))]);
            let memo = Memo::default();
            let err = fetch_capped(&url, PAGE_MAX, retry_on(&memo), Instant::now()).await.err().expect("refused");
            assert!(format!("{err:#}").ends_with(&format!("returned HTTP {status}")), "{status}: {err:#}");
            assert_eq!(served.kinds(), [Kind::Plain], "{status} was asked again");
        }
    }

    #[tokio::test]
    async fn with_the_retry_off_a_403_is_final() {
        let (url, served) = serve_replies(vec![Some(reply("403 Forbidden", "", b""))]);
        let err = fetch_capped(&url, PAGE_MAX, retry_off(), Instant::now()).await.err().expect("refused");
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
        let err = fetch_capped(&url, SMALL, retry_on(&memo), Instant::now()).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");

        let (url, _) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(chunked_reply("200 OK", "Content-Type: text/html\r\n", &[b'a'; 4 * SMALL])),
        ]);
        let err = fetch_capped(&url, SMALL, retry_on(&memo), Instant::now()).await.err().expect("refused");
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
        let page =
            fetch_capped(&url, PAGE_MAX, retry_on(&memo), Instant::now()).await.unwrap_or_else(|e| panic!("{e:#}"));
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
        let page =
            fetch_capped(&url, PAGE_MAX, retry_on(&memo), Instant::now()).await.unwrap_or_else(|e| panic!("{e:#}"));
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
        fetch_capped(&url, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(memo.on_browser(&host_of(&url), now));
        let second = fetch_capped(&url, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(second.body, "<p>Two.</p>");
        assert!(second.via_browser());
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
        fetch_capped(&url, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        let err = fetch_capped(&url, PAGE_MAX, retry_on(&memo), now).await.err().expect("refused");
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
        fetch_capped(&sticky, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        let page = fetch_capped(&other, PAGE_MAX, retry_on(&memo), now).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(!page.via_browser());
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

        fetch_capped(&url, PAGE_MAX, retry_on(&memo), learned_at).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(memo.on_browser(&host, learned_at), "learned by the first retry");
        let within =
            fetch_capped(&url, PAGE_MAX, retry_on(&memo), just_before).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(within.via_browser(), "still on the browser client just before the TTL");
        let expired = fetch_capped(&url, PAGE_MAX, retry_on(&memo), after).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(expired.via_browser());
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
        let retry = Retry {
            on: false,
            memo: &memo,
            timing: false,
            reach: &PRIVATE,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        let err = fetch_capped(&url, PAGE_MAX, retry, now).await.err().expect("refused");
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
        let body = fetch_small(&file_at(&base, "/robots.txt"), PAGE_MAX, WAIT, retry_on(&memo), now).await;
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
        let body = fetch_small(&file_at(&base, "/sitemap.xml"), PAGE_MAX, WAIT, retry_on(&memo), Instant::now()).await;
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
            let body = fetch_small(&file_at(&base, "/sitemap.xml"), PAGE_MAX, WAIT, retry_on(&memo), now).await;
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
                fetch_small(&file_at(&base, "/robots.txt"), PAGE_MAX, WAIT, retry_on(&memo), Instant::now()).await;
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
        let off = Retry {
            on: false,
            memo: &memo,
            timing: false,
            reach: &PRIVATE,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        assert_eq!(fetch_small(&file_at(&base, "/robots.txt"), PAGE_MAX, WAIT, off, now).await, None);
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
        let body = fetch_small(&file_at(&base, "/robots.txt"), PAGE_MAX, WAIT, retry_on(&memo), now).await;
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
        let (robots_url, llms_url, sitemap_url) =
            (file_at(&base, "/robots.txt"), file_at(&base, "/llms.txt"), file_at(&base, "/sitemap.xml"));
        let (robots, llms, sitemap) = tokio::join!(
            fetch_small(&robots_url, PAGE_MAX, WAIT, retry_on(&memo), now),
            fetch_small(&llms_url, PAGE_MAX, WAIT, retry_on(&memo), now),
            fetch_small(&sitemap_url, PAGE_MAX, WAIT, retry_on(&memo), now),
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
        let memo = Memo::default();
        for site in sites {
            let started = Instant::now();
            let normal = get(plain_for(&PRIVATE), &site.url, PAGE_MAX, retry_at(&PRIVATE)).await;
            let normal_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let browser = get_browser(&site.url, PAGE_MAX, retry_at(&PRIVATE)).await;
            let browser_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let retry = fetch(&site.url, retry_on(&memo)).await;
            let retry_ms = started.elapsed().as_millis();
            let retry = match &retry {
                Ok(page) => json!({ "ok": true, "bytes": page.body.len(), "via_browser": page.via_browser() }),
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
        let page = fetch(&format!("{base}/docs/saml.md"), retry_off()).await.unwrap_or_else(|e| panic!("{e:#}"));
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
            let page = fetch(&url, retry_off()).await.unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(page.url.as_str(), url);
        }
    }

    #[tokio::test]
    async fn a_canonical_link_names_the_html_page() {
        let base = twin_site(vec![
            ("/docs/one", "text/markdown", "Link: </docs/canon>; rel=\"canonical\"\r\n", "# One\n"),
            ("/docs/canon", "text/html", "", "<p>One</p>"),
        ]);
        let page = fetch(&format!("{base}/docs/one"), retry_off()).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.url.as_str(), format!("{base}/docs/canon"));
    }

    /// A public run whose own address is the loopback test server's: its start is admitted, and an address that is not
    /// public is not, unless the run names it (see [`Reach`]).
    fn public_run() -> Reach {
        Reach::Public { allowed: std::collections::HashSet::from([std::net::IpAddr::from([127, 0, 0, 1])]) }
    }

    #[tokio::test]
    async fn a_public_run_refuses_a_literal_private_address_before_asking_it() {
        let reach = public_run();
        let memo = Memo::default();
        let retry = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &reach,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        for url in ["http://169.254.169.254/latest/meta-data/", "http://10.0.0.1/", "http://[::ffff:127.0.0.2]/"] {
            let err = fetch_capped(url, PAGE_MAX, retry, Instant::now()).await.err().expect("refused");
            assert_eq!(format!("{err:#}"), "not a public address", "{url}");
        }
    }

    #[tokio::test]
    async fn a_public_run_reads_its_start_and_refuses_a_redirect_to_a_private_address() {
        let (base, served) = serve_routed(|head| {
            let path = head.split_whitespace().nth(1).unwrap_or_default();
            Some(match path {
                "/start" => reply("302 Found", "Location: http://127.0.0.2:1/secret\r\n", b""),
                "/page" => reply("200 OK", "Content-Type: text/html\r\n", b"<p>Start</p>"),
                _ => reply("404 Not Found", "", b""),
            })
        });
        let reach = public_run();
        let memo = Memo::default();
        let retry = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &reach,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        let page = fetch_capped(&format!("{base}/page"), PAGE_MAX, retry, Instant::now())
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, "<p>Start</p>");
        let err = fetch_capped(&format!("{base}/start"), PAGE_MAX, retry, Instant::now()).await.err().expect("refused");
        assert_eq!(format!("{err:#}"), "not a public address", "{err:#}");
        assert_eq!(served.count(), 2, "the redirect's target is never asked");
    }

    #[tokio::test]
    async fn an_image_is_asked_the_way_a_browser_asks_for_an_img_and_without_avif() {
        let (base, served) = serve_routed(|head| {
            Some(match path_of(head) {
                "/photo.jpg" => reply("200 OK", "Content-Type: image/jpeg\r\n", b"JPEG"),
                _ => reply("404 Not Found", "", b""),
            })
        });
        let page = Url::parse(&format!("{base}/article")).expect("a URL");
        let image = Url::parse(&format!("{base}/photo.jpg")).expect("a URL");
        let memo = Memo::default();
        let bytes = image_bytes(&page, &image, retry_on(&memo), PAGE_MAX).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(bytes, b"JPEG");
        let head = served.heads().remove(0).to_ascii_lowercase();
        assert!(head.contains(&format!("referer: {base}/article")), "{head}");
        assert!(head.contains("sec-fetch-dest: image") && head.contains("sec-fetch-mode: no-cors"), "{head}");
        assert!(head.contains("sec-fetch-site: same-origin"), "{head}");
        let accept = head.lines().find(|l| l.starts_with("accept:")).expect("an accept line");
        assert!(accept.starts_with("accept: image/webp") && !accept.contains("avif"), "{accept}");
    }

    #[tokio::test]
    async fn a_403_on_an_image_is_asked_once_more_by_the_browser_client_and_the_host_is_learned() {
        let (base, served) = serve_routed(|head| {
            Some(if head.contains("Chrome/") {
                reply("200 OK", "Content-Type: image/png\r\n", b"PNG")
            } else {
                reply("403 Forbidden", "", b"")
            })
        });
        let image = Url::parse(&format!("{base}/photo.png")).expect("a URL");
        let memo = Memo::default();
        let now = Instant::now();
        let retry = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &PRIVATE,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        let bytes = image_bytes(&image, &image, retry, PAGE_MAX).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(bytes, b"PNG");
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser]);
        assert!(memo.on_browser(&host_of(&base), now), "the host is learned, as a page's retry teaches it");
    }

    #[tokio::test]
    async fn an_image_the_host_refuses_is_an_image_refused_and_a_404_is_not_asked_again() {
        let (url, served) = serve_replies(vec![Some(reply("404 Not Found", "", b""))]);
        let image = Url::parse(&url).expect("a URL");
        let memo = Memo::default();
        let err = image_bytes(&image, &image, retry_on(&memo), PAGE_MAX).await.expect_err("refused");
        assert!(err.downcast_ref::<ImageRefused>().is_some(), "{err:#}");
        assert_eq!(served.kinds(), [Kind::Plain]);
    }

    #[test]
    fn sec_fetch_site_is_same_origin_same_site_or_cross_site() {
        let u = |s: &str| Url::parse(s).expect("a URL");
        let page = u("https://www.example.es/article");
        assert_eq!(fetch_site(&page, &u("https://www.example.es/photo.jpg")), "same-origin");
        assert_eq!(fetch_site(&page, &u("https://cdn.example.es/photo.jpg")), "same-site");
        assert_eq!(
            fetch_site(&page, &u("http://cdn.example.es/photo.jpg")),
            "cross-site",
            "a scheme change is not same-site"
        );
        assert_eq!(fetch_site(&page, &u("https://img.other.net/photo.jpg")), "cross-site");
    }

    #[tokio::test]
    async fn a_public_run_refuses_a_name_that_resolves_to_loopback() {
        let (base, _served) = serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Named</p>")));
        let named = base.replacen("127.0.0.1", "localhost", 1);
        let reach = public_run();
        let memo = Memo::default();
        let retry = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &reach,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        let err = fetch_capped(&format!("{named}/x"), PAGE_MAX, retry, Instant::now()).await.err().expect("refused");
        assert_eq!(format!("{err:#}"), "not a public address", "{err:#}");
    }

    #[tokio::test]
    async fn a_public_run_refuses_a_browser_redirect_to_a_private_address() {
        let (base, served) = serve_routed(|head| {
            let path = head.split_whitespace().nth(1).unwrap_or_default();
            Some(if head.contains("Chrome/") {
                reply("302 Found", "Location: http://127.0.0.2:1/secret\r\n", b"")
            } else if path == "/start" {
                reply("403 Forbidden", "", b"")
            } else {
                reply("404 Not Found", "", b"")
            })
        });
        let reach = public_run();
        let memo = Memo::default();
        let retry = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &reach,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        let err = fetch_capped(&format!("{base}/start"), PAGE_MAX, retry, Instant::now()).await.err().expect("refused");
        assert_eq!(format!("{err:#}"), "not a public address", "{err:#}");
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser]);
    }

    #[tokio::test]
    async fn a_public_run_reads_no_small_file_at_a_private_address() {
        let reach = public_run();
        let memo = Memo::default();
        let retry = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &reach,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        let private = Url::parse("http://127.0.0.2:1/robots.txt").unwrap();
        assert_eq!(fetch_small(&private, PAGE_MAX, WAIT, retry, Instant::now()).await, None);
    }

    #[tokio::test]
    async fn a_public_run_renders_a_page_at_a_public_address_and_nothing_under_public_only() {
        let reach = Reach::Public { allowed: std::collections::HashSet::new() };
        let page = Url::parse("http://93.184.216.34/app").unwrap();
        render_allowed(&page, &reach, false, false).await.expect("a public page may be rendered");
        let err = render_allowed(&page, &reach, true, false).await.expect_err("refused under JURL_PUBLIC_ONLY");
        assert!(format!("{err:#}").starts_with("rendering is refused under JURL_PUBLIC_ONLY=1"), "{err:#}");
        let private = Url::parse("http://10.0.0.1/app").unwrap();
        let err = render_allowed(&private, &reach, false, false).await.expect_err("a private page is refused");
        assert_eq!(format!("{err:#}"), "not a public address");
        render_allowed(&private, &Reach::Private, false, false).await.expect("a private run renders as before");
    }

    #[tokio::test]
    async fn a_sandboxed_public_run_renders_under_public_only_after_the_page_address_is_checked() {
        let reach = Reach::Public { allowed: std::collections::HashSet::new() };
        let page = Url::parse("http://93.184.216.34/app").unwrap();
        render_allowed(&page, &reach, true, true).await.expect("a public page renders, sandboxed, under PUBLIC_ONLY");
        // The page's own address is still checked first: a private one is never rendered, sandboxed or not.
        let private = Url::parse("http://10.0.0.1/app").unwrap();
        let err = render_allowed(&private, &reach, true, true).await.expect_err("a private page is refused");
        assert_eq!(format!("{err:#}"), "not a public address");
    }

    #[tokio::test]
    async fn a_public_run_through_a_proxy_checks_each_target_before_the_proxy_is_asked() {
        // The test server is the proxy: it is sent each request with its absolute address.
        let (proxy, served) = serve_routed(|head| {
            let target = head.split_whitespace().nth(1).unwrap_or_default();
            Some(match target {
                "http://93.184.216.34/page" => reply("200 OK", "Content-Type: text/html\r\n", b"<p>Via the proxy</p>"),
                _ => reply("404 Not Found", "", b""),
            })
        });
        let client = guarded_plain_builder(Vec::new())
            .proxy(reqwest::Proxy::http(proxy.as_str()).expect("a proxy"))
            .build()
            .expect("a client");
        let reach = Reach::Public { allowed: std::collections::HashSet::new() };
        match get(&client, "http://93.184.216.34/page", PAGE_MAX, retry_at(&reach))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"))
        {
            Reply::Page { page, .. } => assert_eq!(page.body, "<p>Via the proxy</p>"),
            Reply::Refused(_) => panic!("the proxy answered with a page"),
        }
        assert_eq!(served.count(), 1, "the proxy is asked for the public target");
        // A private address, and a name that resolves to loopback, are refused before the proxy is asked.
        for url in ["http://10.0.0.1/page", "http://localhost:1/page"] {
            let err = get(&client, url, PAGE_MAX, retry_at(&reach)).await.err().expect("refused");
            assert_eq!(format!("{err:#}"), "not a public address", "{url}");
        }
        assert_eq!(served.count(), 1, "the proxy is never asked for those");
    }

    // The fallback proxy (rung 3). The test server standing for the proxy is sent each request with its absolute address (see
    // `path_of`), and answers by it. A site that refuses direct requests is refused with the status a test names.

    /// The proxy's URL for the test server at `base`, with a credential in it.
    fn proxy_url(base: &str) -> String {
        base.replacen("http://", "http://user:s3cret@", 1)
    }

    /// A proxy that answers every request with `body`, and what it was sent.
    fn proxy_answering(body: &'static [u8]) -> (String, test_server::Served) {
        serve_routed(move |_| Some(reply("200 OK", "Content-Type: text/html\r\n", body)))
    }

    /// A site that refuses every request with `status`, and what it was sent.
    fn refusing_site(status: &'static str) -> (String, test_server::Served) {
        serve_routed(move |_| Some(reply(status, "", b"denied")))
    }

    /// The retry of a run with `fallback` as its proxy, and the reach given.
    fn proxied<'a>(memo: &'a Memo, reach: &'a Reach, fallback: &'a Fallback) -> Retry<'a> {
        Retry { on: true, memo, timing: false, reach, cookies: &NO_COOKIES, stealth: None, fallback }
    }

    /// The memo name of a test server's base address (its host and port).
    fn name_of(base: &str) -> String {
        host_of(base)
    }

    #[tokio::test]
    async fn a_403_from_direct_and_browser_puts_the_host_on_the_proxy_and_keeps_it_there() {
        let (site, site_served) = refusing_site("403 Forbidden");
        let (proxy, proxy_served) = proxy_answering(b"<p>Via the proxy</p>");
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), false);
        let memo = Memo::default();
        let retry = proxied(&memo, &PRIVATE, &fallback);
        let first = fetch(&format!("{site}/precio"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(first.route, Route::Proxy);
        assert_eq!(first.body, "<p>Via the proxy</p>");
        // The site was asked directly twice: plain, then with the browser's fingerprint. Never again.
        assert_eq!(site_served.count(), 2);
        let second = fetch(&format!("{site}/otra"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(second.route, Route::Proxy);
        assert_eq!(site_served.count(), 2, "a host on the proxy is not asked directly again");
        let asked: Vec<String> = proxy_served.heads().iter().map(|h| path_of(h).to_string()).collect();
        assert_eq!(asked, [format!("{site}/precio"), format!("{site}/otra")]);
        assert_eq!(fallback.hosts(), [name_of(&site)]);
    }

    #[tokio::test]
    async fn a_429_from_direct_also_opens_the_proxy_and_is_not_retried_with_the_browser() {
        let (site, site_served) = refusing_site("429 Too Many Requests");
        let (proxy, _) = proxy_answering(b"<p>Via the proxy</p>");
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), false);
        let memo = Memo::default();
        let page = fetch(&format!("{site}/precio"), proxied(&memo, &PRIVATE, &fallback))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, Route::Proxy);
        assert_eq!(site_served.count(), 1, "a 429 is not retried with the browser: only the plain request was made");
    }

    #[tokio::test]
    async fn other_hosts_stay_direct_while_a_refusing_host_is_on_the_proxy() {
        let (refusing, _) = refusing_site("403 Forbidden");
        let (open, open_served) =
            serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Direct</p>")));
        let (proxy, proxy_served) = proxy_answering(b"<p>Via the proxy</p>");
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), false);
        let memo = Memo::default();
        let retry = proxied(&memo, &PRIVATE, &fallback);
        let proxied_page = fetch(&format!("{refusing}/precio"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(proxied_page.route, Route::Proxy);
        let direct = fetch(&format!("{open}/pagina"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(direct.route, Route::Direct);
        assert_eq!(direct.body, "<p>Direct</p>");
        assert_eq!(open_served.count(), 1);
        assert_eq!(proxy_served.count(), 1, "the proxy is not asked for the host that reads direct");
        assert_eq!(fallback.hosts(), [name_of(&refusing)]);
    }

    #[tokio::test]
    async fn proxy_first_reads_a_host_through_the_proxy_from_its_first_request() {
        let (site, site_served) =
            serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Direct</p>")));
        let (proxy, proxy_served) = proxy_answering(b"<p>Via the proxy</p>");
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), true);
        let memo = Memo::default();
        let page = fetch(&format!("{site}/precio"), proxied(&memo, &PRIVATE, &fallback))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, Route::Proxy);
        assert_eq!(page.body, "<p>Via the proxy</p>");
        assert_eq!(site_served.count(), 0, "the site is never asked directly under JURL_PROXY_FIRST");
        assert_eq!(proxy_served.count(), 1);
        assert_eq!(fallback.hosts(), [name_of(&site)], "the host is recorded as proxied from its first request");
    }

    #[tokio::test]
    async fn a_refusal_through_the_proxy_goes_to_the_sidecar_and_not_back_to_the_browser() {
        const CONTENT: &str = r#"{"outcome":"content","status":200,"html":"<html><body><p>Precio: 5 €</p></body></html>","text":"Precio: 5 €","title":"Tienda","reason":null,"wall_s":3.1,"robots":"allowed"}"#;
        let (site, site_served) = refusing_site("403 Forbidden");
        let (proxy, proxy_served) = refusing_site("403 Forbidden");
        let (side, side_seen) = crate::mock::serve(vec![(200, "", CONTENT)]);
        let sidecar = crate::stealth::Sidecar::configured(
            |key| match key {
                "JURL_STEALTH_URL" => Some(side.clone()),
                "JURL_STEALTH_TOKEN" => Some("s3cret-token".to_string()),
                _ => None,
            },
            false,
            false,
            false,
        )
        .expect("a sidecar");
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), false);
        let memo = Memo::default();
        let retry = Retry { stealth: Some(&sidecar), ..proxied(&memo, &PRIVATE, &fallback) };
        let page = fetch(&format!("{site}/precio"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, Route::Stealth, "the proxy's refusal is the sidecar's to answer");
        assert!(page.body.contains("Precio: 5"), "{}", page.body);
        assert_eq!(site_served.count(), 2, "the site: plain, then with the browser's fingerprint");
        assert_eq!(
            proxy_served.count(),
            1,
            "the proxy is asked once: a refusal through it is not retried with the browser"
        );
        assert_eq!(side_seen.lock().unwrap().len(), 1, "the sidecar is asked once");
    }

    #[tokio::test]
    async fn a_public_run_refuses_a_private_target_before_the_proxy_is_asked() {
        let (proxy, proxy_served) = proxy_answering(b"<p>Via the proxy</p>");
        // Every host is on the proxy, so each request would go to it if it were not checked first.
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), true);
        let memo = Memo::default();
        let reach = Reach::Public { allowed: HashSet::new() };
        let retry = proxied(&memo, &reach, &fallback);
        for url in ["http://127.0.0.1:9/page", "http://localhost:9/page"] {
            let err = fetch(url, retry).await.err().expect("a private target is refused");
            assert_eq!(format!("{err:#}"), "not a public address", "{url}");
        }
        assert_eq!(proxy_served.count(), 0, "the proxy is never asked for a private target");
    }

    #[tokio::test]
    async fn a_public_target_is_read_through_the_proxy_once_its_address_is_checked() {
        let (proxy, proxy_served) = proxy_answering(b"<p>Via the proxy</p>");
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), true);
        let memo = Memo::default();
        let reach = Reach::Public { allowed: HashSet::new() };
        let page = fetch("http://93.184.216.34/precio", proxied(&memo, &reach, &fallback))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, Route::Proxy);
        assert_eq!(path_of(&proxy_served.heads()[0]), "http://93.184.216.34/precio");
    }

    #[tokio::test]
    async fn a_redirect_from_a_host_on_the_proxy_stays_on_the_proxy() {
        // The proxy stands for the site here, and answers as the site would: /a redirects to /b. The site is never asked.
        let (site, site_served) =
            serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Direct</p>")));
        let (proxy, proxy_served) = serve_routed(|head| {
            Some(match path_of(head) {
                p if p.ends_with("/a") => reply("301 Moved Permanently", "Location: /b\r\n", b""),
                _ => reply("200 OK", "Content-Type: text/html\r\n", b"<p>Via the proxy</p>"),
            })
        });
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), true);
        let memo = Memo::default();
        let page =
            fetch(&format!("{site}/a"), proxied(&memo, &PRIVATE, &fallback)).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, Route::Proxy);
        assert_eq!(page.body, "<p>Via the proxy</p>");
        assert_eq!(site_served.count(), 0, "the redirect is followed through the proxy");
        let asked: Vec<String> = proxy_served.heads().iter().map(|h| path_of(h).to_string()).collect();
        assert_eq!(asked, [format!("{site}/a"), format!("{site}/b")]);
    }

    #[tokio::test]
    async fn a_redirect_into_a_host_on_the_proxy_switches_the_rest_of_the_chain_to_it() {
        let (target, target_served) =
            serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Target direct</p>")));
        let target_at = format!("{target}/final");
        let (start, start_served) =
            serve_routed(move |_| Some(reply("301 Moved Permanently", &format!("Location: {target_at}\r\n"), b"")));
        let (proxy, proxy_served) = proxy_answering(b"<p>Via the proxy</p>");
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), false);
        fallback.put_on_proxy(&Url::parse(&target).expect("a URL"), "test", false);
        let memo = Memo::default();
        let page =
            fetch(&format!("{start}/a"), proxied(&memo, &PRIVATE, &fallback)).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, Route::Proxy);
        assert_eq!(start_served.count(), 1, "the start host is read direct");
        assert_eq!(target_served.count(), 0, "the host on the proxy is never asked directly");
        assert_eq!(path_of(&proxy_served.heads()[0]), format!("{target}/final"));
    }

    #[tokio::test]
    async fn robots_and_sitemaps_of_a_host_on_the_proxy_go_through_the_proxy() {
        let (site, site_served) = serve_routed(|_| Some(reply("200 OK", "", b"direct")));
        let (proxy, proxy_served) = serve_routed(|head| {
            let body: &[u8] = match path_of(head) {
                p if p.ends_with("/robots.txt") => b"User-agent: *\nDisallow: /privado\n",
                p if p.ends_with("/sitemap.xml") => b"<urlset/>",
                _ => b"",
            };
            Some(reply("200 OK", "Content-Type: text/plain\r\n", body))
        });
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), true);
        let memo = Memo::default();
        let retry = proxied(&memo, &PRIVATE, &fallback);
        let robots = fetch_small(&file_at(&site, "/robots.txt"), PAGE_MAX, WAIT, retry, Instant::now()).await;
        assert_eq!(robots.as_deref(), Some("User-agent: *\nDisallow: /privado\n"));
        let sitemap = fetch_small(&file_at(&site, "/sitemap.xml"), PAGE_MAX, WAIT, retry, Instant::now()).await;
        assert_eq!(sitemap.as_deref(), Some("<urlset/>"));
        assert_eq!(site_served.count(), 0, "the site's files are read through the proxy");
        assert_eq!(proxy_served.count(), 2);
    }

    #[tokio::test]
    async fn a_refused_robots_file_does_not_put_its_host_on_the_proxy() {
        let (site, _) = refusing_site("403 Forbidden");
        let (proxy, proxy_served) = proxy_answering(b"should not be asked");
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), false);
        let memo = Memo::default();
        let retry = proxied(&memo, &PRIVATE, &fallback);
        assert_eq!(fetch_small(&file_at(&site, "/robots.txt"), PAGE_MAX, WAIT, retry, Instant::now()).await, None);
        assert!(fallback.hosts().is_empty(), "only a refused page puts a host on the proxy");
        assert_eq!(proxy_served.count(), 0);
    }

    #[tokio::test]
    async fn an_image_of_a_host_on_the_proxy_goes_through_the_proxy_with_its_referer() {
        let (site, site_served) = serve_routed(|_| Some(reply("200 OK", "Content-Type: image/png\r\n", b"direct")));
        let (proxy, proxy_served) =
            serve_routed(|_| Some(reply("200 OK", "Content-Type: image/png\r\n", b"\x89PNG-bytes")));
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), true);
        let memo = Memo::default();
        let retry = proxied(&memo, &PRIVATE, &fallback);
        let page = Url::parse(&format!("{site}/producto")).expect("a URL");
        let image = Url::parse(&format!("{site}/foto.png")).expect("a URL");
        let bytes = image_bytes(&page, &image, retry, PAGE_MAX).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(bytes, b"\x89PNG-bytes".to_vec());
        assert_eq!(site_served.count(), 0, "the image is read through the proxy");
        assert_eq!(proxy_served.count(), 1);
        let head = proxy_served.heads().remove(0).to_ascii_lowercase();
        assert!(head.contains(&format!("referer: {page}")), "{head}");
    }

    #[tokio::test]
    async fn the_run_counts_the_bytes_it_read_direct_and_through_the_proxy() {
        let (site, _) = refusing_site("403 Forbidden");
        let (proxy, _) = proxy_answering(b"<p>Via the proxy</p>");
        let (open, _) = serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Direct</p>")));
        let fallback = Fallback::with(Some(&proxy_url(&proxy)), false);
        let memo = Memo::default();
        let retry = proxied(&memo, &PRIVATE, &fallback);
        fetch(&format!("{site}/precio"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        fetch(&format!("{open}/pagina"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(fallback.bytes(true), "<p>Via the proxy</p>".len() as u64);
        assert_eq!(
            fallback.bytes(false),
            "<p>Direct</p>".len() as u64,
            "the refusals' bodies are not read, so not counted"
        );
    }

    #[tokio::test]
    async fn a_proxy_first_read_through_load_reports_its_route_bytes_and_hosts() {
        use clap::Parser;
        let body = format!("<html><body><p>{}</p></body></html>", "Precio y envío a domicilio. ".repeat(20));
        let (site, site_served) = serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", b"direct")));
        let (proxy, proxy_served) =
            serve_routed(move |_| Some(reply("200 OK", "Content-Type: text/html\r\n", body.as_bytes())));
        let mut args = crate::cli::Args::parse_from(["jurl", &format!("{site}/precio")]);
        args.fallback = Fallback::with(Some(&proxy_url(&proxy)), true);
        let target = Url::parse(&args.url).expect("a URL");
        let mut t = crate::timing::Timer::new();
        let (_, ex, served) = crate::load(&args, &crate::config::Config::default(), &target, &mut t)
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(served.name(), "proxy");
        assert!(ex.blocks.iter().any(|b| b.text.contains("Precio y envío")), "the proxy's page is what is read");
        assert_eq!(site_served.count(), 0);
        assert_eq!(proxy_served.count(), 1);
        let usage = crate::decide::Usage::new();
        usage.record_route(served);
        let json = usage.json(&args.fallback);
        assert_eq!(json["route"], "proxy");
        assert_eq!(json["bytes"]["proxy"], serde_json::json!(args.fallback.bytes(true)));
        assert_eq!(json["bytes"]["direct"], 0);
        assert_eq!(json["proxied_hosts"], serde_json::json!([name_of(&site)]));
    }

    /// The proxy's URL for the test server at `base`, with the given credential in it.
    fn proxy_url_with(base: &str, user: &str, pass: &str) -> String {
        base.replacen("http://", &format!("http://{user}:{pass}@"), 1)
    }

    #[tokio::test]
    async fn a_proxy_that_hangs_up_is_an_error_that_shows_neither_the_credential_nor_the_user() {
        let (site, site_served) = serve_routed(|_| Some(reply("200 OK", "", b"direct")));
        let (proxy, proxy_served) = serve_routed(|_| None);
        let fallback = Fallback::with(Some(&proxy_url_with(&proxy, "jurlcred", "s3cret-pw")), true);
        let memo = Memo::default();
        let err = fetch(&format!("{site}/precio"), proxied(&memo, &PRIVATE, &fallback))
            .await
            .err()
            .expect("a proxy that hangs up is an error");
        let text = format!("{err:#}");
        assert!(!text.contains("s3cret-pw") && !text.contains("jurlcred"), "{text}");
        assert!(text.contains("fetching"), "the request is still named: {text}");
        assert_eq!(proxy_served.count(), 1);
        assert_eq!(site_served.count(), 0, "the site is not asked directly");
    }

    #[tokio::test]
    async fn a_407_from_the_proxy_is_a_refusal_through_the_proxy_with_no_credential() {
        let (site, _) = serve_routed(|_| Some(reply("200 OK", "", b"direct")));
        let (proxy, _) = serve_routed(|_| {
            Some(reply("407 Proxy Authentication Required", "Proxy-Authenticate: Basic realm=\"proxy\"\r\n", b""))
        });
        let fallback = Fallback::with(Some(&proxy_url_with(&proxy, "jurlcred", "s3cret-pw")), true);
        let memo = Memo::default();
        let err = fetch(&format!("{site}/precio"), proxied(&memo, &PRIVATE, &fallback))
            .await
            .err()
            .expect("a 407 is an error");
        let text = format!("{err:#}");
        assert!(text.contains("HTTP 407") && text.contains("through the proxy"), "{text}");
        assert!(!text.contains("s3cret-pw") && !text.contains("jurlcred"), "{text}");
    }

    #[tokio::test]
    async fn a_direct_transport_error_keeps_its_type() {
        let (site, _) = serve_routed(|_| None);
        let memo = Memo::default();
        let err = fetch(&format!("{site}/page"), retry_on(&memo)).await.err().expect("the site hangs up");
        assert!(err.downcast_ref::<reqwest::Error>().is_some(), "{err:#}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_render_through_the_proxy_whose_error_shows_its_arguments_shows_no_credential() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("jurl-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a test directory");
        let bin = dir.join("lightpanda");
        // A stand-in Lightpanda that fails and says its arguments, as the browser's own error might.
        std::fs::write(&bin, "#!/bin/sh\nprintf '{\"http_status\":0,\"content\":\"\",\"error\":\"%s\"}' \"$*\"\n")
            .expect("the stand-in");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("executable");
        let fallback = Fallback::with(Some("http://jurlcred:s3cret-pw@proxy.test:8080"), true);
        let url = Url::parse("http://93.184.216.34/precio").expect("a URL");
        let err = render(&bin, &url, &fallback).await.err().expect("the stand-in fails");
        let text = format!("{err:#}");
        assert!(!text.contains("s3cret-pw") && !text.contains("jurlcred"), "{text}");
        assert!(text.contains("proxy"), "the proxy is named by its place-holder: {text}");
        assert_eq!(fallback.renders(), 1, "the render was counted as it started");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_proxy_host_resolves_as_the_system_resolves_it_and_any_other_name_is_checked() {
        let (base, _served) =
            serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Proxy host</p>")));
        let named = format!("{}/", base.replacen("127.0.0.1", "localhost", 1));
        let exempt = guarded_plain_builder(vec!["localhost".to_string()]).build().expect("a client");
        let body =
            exempt.get(named.as_str()).send().await.expect("the proxy host connects").text().await.expect("a body");
        assert_eq!(body, "<p>Proxy host</p>");
        let checked = guarded_plain_builder(Vec::new()).build().expect("a client");
        let err = checked.get(named.as_str()).send().await.expect_err("refused");
        assert!(reach::refused(&err), "{err:?}");
    }

    /// A page whose header block is 31 KB, as backmarket's /es-es sends (a Link header of preloads), over HTTP/2 with the
    /// client's own settings. A client that advertises hyper's 16 KB header-list limit has the stream reset before the page
    /// is read: "http2 error: stream error detected: unspecific protocol error detected".
    #[tokio::test]
    async fn a_page_with_a_31_kb_header_block_is_read_over_http2() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind a loopback port");
        let addr = listener.local_addr().expect("the loopback address");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("a connection");
            let mut conn = h2::server::handshake(socket).await.expect("the HTTP/2 handshake");
            while let Some(Ok((_request, mut respond))) = conn.accept().await {
                let preload = format!("<https://cdn.test/{}>; rel=preload", "a".repeat(31_000));
                let response = http::Response::builder()
                    .status(200)
                    .header("content-type", "text/html")
                    .header("link", preload)
                    .body(())
                    .expect("a response");
                let mut send = respond.send_response(response, false).expect("the response is sent");
                send.send_data(bytes::Bytes::from_static(b"<p>Hello, page.</p>"), true).expect("the body is sent");
            }
        });
        // Prior knowledge: the server speaks HTTP/2 over plain TCP, as a TLS connection's ALPN would have it.
        let client = crate::client_builder().http2_prior_knowledge().no_proxy().build().expect("a client");
        let res = client.get(format!("http://{addr}/es-es")).send().await.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(res.version(), reqwest::Version::HTTP_2);
        assert_eq!(res.text().await.expect("the body"), "<p>Hello, page.</p>");
    }

    /// The path a request head asks for.
    fn path_of(head: &str) -> &str {
        head.split_whitespace().nth(1).unwrap_or_default()
    }

    /// Whether a request head's Cookie header carries `cookie`, a `name=value` pair.
    fn sends_cookie(head: &str, cookie: &str) -> bool {
        head.lines().any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.trim().eq_ignore_ascii_case("cookie") && value.split(';').any(|pair| pair.trim() == cookie)
            })
        })
    }

    /// Reads `url` with `reach`, the run's cookies being `jar`.
    async fn read_with(url: &str, reach: &Reach, jar: &Jar) -> Result<Page> {
        let memo = Memo::default();
        let retry =
            Retry { on: false, memo: &memo, timing: false, reach, cookies: jar, stealth: None, fallback: &NO_FALLBACK };
        fetch_capped(url, PAGE_MAX, retry, Instant::now()).await
    }

    #[tokio::test]
    async fn a_redirect_loop_that_sets_a_cookie_is_followed_until_the_cookie_comes_back() {
        // `/loop` answers 307 to itself, with a cookie, until the cookie is sent back. `/check` says whether it was sent.
        let (base, _served) = serve_routed(|head| {
            Some(match path_of(head) {
                "/loop" if sends_cookie(head, "session=ok") => {
                    reply("200 OK", "Content-Type: text/html\r\n", b"<p>Cookied</p>")
                }
                "/loop" => {
                    let headers = format!("Location: {}\r\nSet-Cookie: session=ok; Path=/\r\n", path_of(head));
                    reply("307 Temporary Redirect", &headers, b"")
                }
                _ => {
                    let body: &[u8] = if sends_cookie(head, "session=ok") { b"sent" } else { b"not sent" };
                    reply("200 OK", "Content-Type: text/html\r\n", body)
                }
            })
        });
        let public = public_run();
        for reach in [&PRIVATE, &public] {
            let run = Jar::default();
            let page = read_with(&format!("{base}/loop"), reach, &run).await.unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(page.body, "<p>Cookied</p>");
            // The run keeps the cookie for its own later reads...
            let page = read_with(&format!("{base}/check"), reach, &run).await.unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(page.body, "sent");
            // ...and another run, with a jar of its own, has none.
            let other = Jar::default();
            let page = read_with(&format!("{base}/check"), reach, &other).await.unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(page.body, "not sent");
        }
    }

    #[tokio::test]
    async fn the_browser_client_is_sent_the_cookies_the_plain_client_stored() {
        let (base, served) = serve_routed(|head| {
            Some(match path_of(head) {
                "/set" => {
                    reply("200 OK", "Content-Type: text/html\r\nSet-Cookie: session=ok; Path=/\r\n", b"<p>Set</p>")
                }
                "/browser" if head.contains("Chrome/") && sends_cookie(head, "session=ok") => {
                    reply("200 OK", "Content-Type: text/html\r\n", b"<p>Browser saw it</p>")
                }
                _ => reply("403 Forbidden", "", b""),
            })
        });
        let memo = Memo::default();
        let jar = Jar::default();
        let retry = Retry {
            on: false,
            memo: &memo,
            timing: false,
            reach: &PRIVATE,
            cookies: &jar,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        fetch(&format!("{base}/set"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        match get_browser(&format!("{base}/browser"), PAGE_MAX, retry).await.unwrap_or_else(|e| panic!("{e:#}")) {
            Reply::Page { page, .. } => assert_eq!(page.body, "<p>Browser saw it</p>"),
            Reply::Refused(refused) => panic!("refused with {}", refused.status),
        }
        assert_eq!(served.kinds(), [Kind::Plain, Kind::Browser]);
    }

    #[tokio::test]
    async fn a_cookie_is_sent_only_to_the_host_and_path_that_set_it() {
        let (base, _served) = serve_routed(|head| {
            Some(match path_of(head) {
                "/set" => {
                    reply("200 OK", "Content-Type: text/html\r\nSet-Cookie: scoped=1; Path=/private\r\n", b"<p>Set</p>")
                }
                // A redirect to the same server under another name: the cookie is for 127.0.0.1 alone.
                "/hop" => {
                    let host = head.lines().find(|l| l.to_ascii_lowercase().starts_with("host:")).unwrap_or_default();
                    let host = host.split_once(':').map_or("", |(_, v)| v.trim()).replacen("127.0.0.1", "localhost", 1);
                    reply("302 Found", &format!("Location: http://{host}/private/page\r\n"), b"")
                }
                // The body says whether the request carried the cookie.
                _ => {
                    let body: &[u8] = if sends_cookie(head, "scoped=1") { b"sent" } else { b"not sent" };
                    reply("200 OK", "Content-Type: text/html\r\n", body)
                }
            })
        });
        let localhost = base.replacen("127.0.0.1", "localhost", 1);
        let memo = Memo::default();
        let jar = Jar::default();
        let retry = Retry {
            on: false,
            memo: &memo,
            timing: false,
            reach: &PRIVATE,
            cookies: &jar,
            stealth: None,
            fallback: &NO_FALLBACK,
        };
        fetch(&format!("{base}/set"), retry).await.unwrap_or_else(|e| panic!("{e:#}"));
        for (url, expected) in [
            (format!("{base}/private/page"), "sent"),
            (format!("{base}/public"), "not sent"),
            (format!("{localhost}/private/page"), "not sent"),
            (format!("{base}/hop"), "not sent"),
        ] {
            let page = fetch(&url, retry).await.unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(page.body, expected, "{url}");
        }
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

    use super::{Jar, Memo, Reach, Retry};
    use crate::fallback::Fallback;

    /// The reach of a run that reads any address, as a run from a private start does.
    pub(crate) static PRIVATE: Reach = Reach::Private;

    /// A run with no fallback proxy, for the tests that do not use one.
    pub(crate) static NO_FALLBACK: Fallback = Fallback::new();

    /// The cookie store of the tests that involve no cookie: none of their servers sets one.
    pub(crate) static NO_COOKIES: LazyLock<Jar> = LazyLock::new(Jar::default);

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
        Retry {
            on: true,
            memo,
            timing: false,
            reach: &PRIVATE,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        }
    }

    /// The retry off, for a run that reads with `reach`: no memo is ever read or written.
    pub(crate) fn retry_at(reach: &Reach) -> Retry<'_> {
        static NO_MEMO: LazyLock<Memo> = LazyLock::new(Memo::default);
        Retry {
            on: false,
            memo: &NO_MEMO,
            timing: false,
            reach,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        }
    }

    /// The retry off: no memo is ever read or written.
    pub(crate) fn retry_off() -> Retry<'static> {
        static NO_MEMO: LazyLock<Memo> = LazyLock::new(Memo::default);
        Retry {
            on: false,
            memo: &NO_MEMO,
            timing: false,
            reach: &PRIVATE,
            cookies: &NO_COOKIES,
            stealth: None,
            fallback: &NO_FALLBACK,
        }
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

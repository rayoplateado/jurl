use std::{
    path::Path,
    process::Stdio,
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
    /// Set when the normal client was refused and the page came from the retry with a browser's fingerprint.
    pub browser_retry: Option<BrowserRetry>,
}

/// The retry that read a page: the status the normal client was refused with, and how long the retry took.
#[derive(Clone, Copy)]
pub struct BrowserRetry {
    pub refused: StatusCode,
    pub took: Duration,
}

/// The largest page read: a body past this is an error, not read on.
const PAGE_MAX: usize = 8 << 20;

/// The page at `url`. When the normal client is refused with a 403 or 503 and `browser_retry` is on, the same URL is
/// asked once more by a client with a browser's TLS and HTTP/2 fingerprint, which some bot protection accepts where it
/// refuses the normal client. That is the only retry: any other reply is final, a 429 rate limit included.
pub async fn fetch(client: &Client, url: &str, browser_retry: bool) -> Result<Page> {
    fetch_capped(client, url, PAGE_MAX, browser_retry).await
}

/// Whether a refused page is retried with a browser's fingerprint. A run opts out with `--no-browser-retry`, or with
/// `JURL_NO_BROWSER_RETRY` set to anything (as `JURL_NO_DOWNLOAD` is).
pub(crate) fn browser_retry_allowed(flag: bool, env_set: bool) -> bool {
    !flag && !env_set
}

/// The statuses that get the retry: 403 and 503. Only the status decides, and a 429 never does: it is a rate limit,
/// to be respected rather than gone round.
fn retried(status: StatusCode) -> bool {
    matches!(status, StatusCode::FORBIDDEN | StatusCode::SERVICE_UNAVAILABLE)
}

/// `fetch`, refusing a page larger than `max` bytes.
async fn fetch_capped(client: &Client, url: &str, max: usize, browser_retry: bool) -> Result<Page> {
    let refused = match get(client, url, max).await? {
        Reply::Page { page, .. } => return Ok(page),
        Reply::Refused(refused) => refused,
    };
    if !browser_retry || !retried(refused.status) {
        return Err(refused.error(url, false));
    }
    let started = Instant::now();
    let Ok(res) = browser_send(url).await else {
        // The retry got no reply at all: the first refusal stands.
        return Err(refused.error(url, false));
    };
    // Once the retry has a reply, that reply is the answer: a page too large to read is an error, not a fallback.
    match browser_reply(res, url, max).await? {
        Reply::Page { page, .. } => {
            Ok(Page { browser_retry: Some(BrowserRetry { refused: refused.status, took: started.elapsed() }), ..page })
        }
        Reply::Refused(again) => Err(again.error(url, true)),
    }
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

/// A reply that is not a success: its status, and the Retry-After it sent, if it sent one.
struct Refusal {
    status: StatusCode,
    retry_after: Option<String>,
}

impl Refusal {
    /// The error for this refusal. `also` says the browser's fingerprint was refused too.
    fn error(&self, url: &str, also: bool) -> anyhow::Error {
        let mut notes = Vec::new();
        if also {
            notes.push("also with a browser's TLS fingerprint".to_string());
        }
        if let Some(r) = &self.retry_after {
            notes.push(format!("retry after {r}"));
        }
        if notes.is_empty() {
            anyhow!("{url} returned HTTP {}", self.status)
        } else {
            anyhow!("{url} returned HTTP {} ({})", self.status, notes.join("; "))
        }
    }
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
        let retry_after = res.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()).map(String::from);
        return Ok(Reply::Refused(Refusal { status, retry_after }));
    }
    let final_url = res.url().clone();
    let ct = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let Some(bytes) = read_capped(&mut res, max).await? else {
        bail!("{url}: page larger than {}", size_label(max));
    };
    Ok(Reply::Page { status, page: to_page(final_url, &ct, &bytes) })
}

/// The browser client's request for `url`, sent. An error here means no reply came.
async fn browser_send(url: &str) -> Result<wreq::Response> {
    browser_client()?
        .get(url)
        .timeout(crate::HTTP_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("fetching {url} with a browser's fingerprint"))
}

/// The browser client's reply to `url`: the page, or the refusal. A reply that cannot be read is an error.
async fn browser_reply(res: wreq::Response, url: &str, max: usize) -> Result<Reply> {
    let status = res.status();
    if !status.is_success() {
        let retry_after = res.headers().get(wreq::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).map(String::from);
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

/// A client that asks like Chrome 149: its TLS and HTTP/2 fingerprint, headers and user agent. Proxies from the
/// environment apply, as they do to the normal client.
fn browser_client() -> Result<wreq::Client> {
    let builder = wreq::Client::builder()
        .emulation(wreq_util::Emulation::Chrome149)
        // wreq follows no redirects unless asked; the normal client follows up to 10, and so does this one.
        .redirect(wreq::redirect::Policy::limited(10));
    // The loopback servers in the tests are reached directly, as the normal client reaches them.
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder.build().context("building the browser client")
}

/// The page a body makes: its text as its Content-Type's charset says, and whether it is markdown.
fn to_page(url: Url, content_type: &str, bytes: &[u8]) -> Page {
    let body = decode(content_type, bytes);
    let is_markdown = served_markdown(content_type, &body);
    Page { url, body, is_markdown, browser_retry: None }
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
    Ok(Page { url: url.clone(), body: body.to_string(), is_markdown: false, browser_retry: None })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use serde_json::json;

    use super::*;
    use test_server::{chunked_reply, client, reply, serve, serve_replies};

    /// `<p>Hello, page.</p>` as gzip, as a server sends it under Content-Encoding: gzip.
    const GZIPPED: &[u8] = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\xb3\x29\xb0\xf3\x48\xcd\xc9\xc9\xd7\x51\x28\x48\x4c\x4f\xd5\xb3\xd1\x2f\xb0\x03\x00\x04\x6d\x98\xda\x13\x00\x00\x00";

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
        let err = fetch_capped(&client(), &url, SMALL, false).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn a_streamed_page_is_stopped_once_it_passes_the_cap() {
        // No Content-Length: the bytes that arrive are what is counted.
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let url = serve(head, vec![b'a'; 4 * SMALL], true);
        let err = fetch_capped(&client(), &url, SMALL, false).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn a_page_at_the_cap_is_read_whole_without_its_byte_order_mark() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let mut text = "\u{feff}<p>Hello, page.</p>".to_string();
        text.push_str(&" ".repeat(SMALL - text.len()));
        assert_eq!(text.len(), SMALL);
        let url = serve(head, text.clone().into_bytes(), true);
        let page = fetch_capped(&client(), &url, SMALL, false).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, text.trim_start_matches('\u{feff}'));
    }

    /// The body as fetch_capped reads it, and the body reqwest's own text() reads from the same response.
    async fn both_ways(head: &str, body: Vec<u8>) -> (String, String) {
        let url = serve(head.to_string(), body.clone(), true);
        let ours = fetch_capped(&client(), &url, SMALL, false).await.unwrap_or_else(|e| panic!("{e:#}")).body;
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
    fn a_size_is_named_in_the_units_a_message_uses() {
        assert_eq!(size_label(8 << 20), "8 MB");
        assert_eq!(size_label(1 << 10), "1 KB");
        assert_eq!(size_label(1500), "1500 bytes");
    }

    #[test]
    fn only_403_and_503_are_retried_with_a_browser_fingerprint() {
        assert!(retried(StatusCode::FORBIDDEN));
        assert!(retried(StatusCode::SERVICE_UNAVAILABLE));
        for code in [200, 301, 400, 401, 404, 408, 429, 500, 502, 504] {
            assert!(!retried(StatusCode::from_u16(code).unwrap()), "{code} was retried");
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
        let (url, taken) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Hello, page.</p>")),
        ]);
        let page = fetch_capped(&client(), &url, PAGE_MAX, true).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, "<p>Hello, page.</p>");
        assert_eq!(page.browser_retry.map(|r| r.refused), Some(StatusCode::FORBIDDEN));
        assert_eq!(taken.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_503_is_asked_again_too() {
        let (url, taken) = serve_replies(vec![
            Some(reply("503 Service Unavailable", "", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Hello, page.</p>")),
        ]);
        let page = fetch_capped(&client(), &url, PAGE_MAX, true).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.browser_retry.map(|r| r.refused), Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(taken.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_refusal_on_the_retry_is_the_answer_and_nothing_more_is_asked() {
        let (url, taken) =
            serve_replies(vec![Some(reply("403 Forbidden", "", b"")), Some(reply("403 Forbidden", "", b""))]);
        let err = fetch_capped(&client(), &url, PAGE_MAX, true).await.err().expect("refused");
        assert!(
            format!("{err:#}").ends_with("returned HTTP 403 Forbidden (also with a browser's TLS fingerprint)"),
            "{err:#}"
        );
        assert_eq!(taken.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_retry_that_gets_no_answer_leaves_the_first_refusal() {
        let (url, taken) = serve_replies(vec![Some(reply("403 Forbidden", "", b"")), None]);
        let err = fetch_capped(&client(), &url, PAGE_MAX, true).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 403 Forbidden"), "{err:#}");
        assert_eq!(taken.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_429_is_final_and_keeps_its_retry_after() {
        let (url, taken) = serve_replies(vec![Some(reply("429 Too Many Requests", "Retry-After: 30\r\n", b""))]);
        let err = fetch_capped(&client(), &url, PAGE_MAX, true).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 429 Too Many Requests (retry after 30)"), "{err:#}");
        assert_eq!(taken.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn no_other_refusal_is_asked_again() {
        for status in ["401 Unauthorized", "404 Not Found", "500 Internal Server Error", "502 Bad Gateway"] {
            let (url, taken) = serve_replies(vec![Some(reply(status, "", b""))]);
            let err = fetch_capped(&client(), &url, PAGE_MAX, true).await.err().expect("refused");
            assert!(format!("{err:#}").ends_with(&format!("returned HTTP {status}")), "{status}: {err:#}");
            assert_eq!(taken.load(Ordering::SeqCst), 1, "{status} was asked again");
        }
    }

    #[tokio::test]
    async fn with_the_retry_off_a_403_is_final() {
        let (url, taken) = serve_replies(vec![Some(reply("403 Forbidden", "", b""))]);
        let err = fetch_capped(&client(), &url, PAGE_MAX, false).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("returned HTTP 403 Forbidden"), "{err:#}");
        assert_eq!(taken.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_retried_page_keeps_the_size_limit_whether_or_not_it_says_its_length() {
        let (url, _) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\n", &[b'a'; 5000])),
        ]);
        let err = fetch_capped(&client(), &url, SMALL, true).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");

        let (url, _) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(chunked_reply("200 OK", "Content-Type: text/html\r\n", &[b'a'; 4 * SMALL])),
        ]);
        let err = fetch_capped(&client(), &url, SMALL, true).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn the_retry_follows_redirects_and_the_page_is_its_final_address() {
        let (url, taken) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(reply("302 Found", "Location: /moved\r\n", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Hello, page.</p>")),
        ]);
        let page = fetch_capped(&client(), &url, PAGE_MAX, true).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, "<p>Hello, page.</p>");
        assert_eq!(page.url.path(), "/moved");
        assert_eq!(taken.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_retried_page_is_decompressed_when_the_browser_client_asked_for_it() {
        // The browser client asks for gzip, deflate, br and zstd, so the page it gets back may be compressed.
        let (url, _) = serve_replies(vec![
            Some(reply("403 Forbidden", "", b"")),
            Some(reply("200 OK", "Content-Type: text/html\r\nContent-Encoding: gzip\r\n", GZIPPED)),
        ]);
        let page = fetch_capped(&client(), &url, PAGE_MAX, true).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, "<p>Hello, page.</p>");
    }

    /// The browser client's answer to `url`: its reply, or an error if it had none or the reply could not be read.
    async fn get_browser(url: &str) -> Result<Reply> {
        browser_reply(browser_send(url).await?, url, PAGE_MAX).await
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
        for site in sites {
            let started = Instant::now();
            let normal = get(&client, &site.url, PAGE_MAX).await;
            let normal_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let browser = get_browser(&site.url).await;
            let browser_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let retry = fetch(&client, &site.url, true).await;
            let retry_ms = started.elapsed().as_millis();
            let retry = match &retry {
                Ok(page) => {
                    json!({ "ok": true, "bytes": page.body.len(), "refused": page.browser_retry.map(|r| r.refused.as_u16()) })
                }
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
}

/// Loopback servers for the tests that need a body of a given size or shape, or a count of the requests made.
#[cfg(test)]
pub(crate) mod test_server {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };

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

    /// A loopback server that answers each connection in turn with the next of `replies`: a whole reply (see `reply`),
    /// or None to hang up with no reply. Past the last one, each request gets a 500. Returns its URL and the count of
    /// requests it has taken, so a test can tell how many times a client asked.
    pub(crate) fn serve_replies(replies: Vec<Option<Vec<u8>>>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("the loopback address");
        let taken = Arc::new(AtomicUsize::new(0));
        let counter = taken.clone();
        thread::spawn(move || {
            let mut replies = replies.into_iter();
            loop {
                let Ok((mut conn, _)) = listener.accept() else { return };
                counter.fetch_add(1, Ordering::SeqCst);
                read_head(&mut conn);
                let answer = replies.next().unwrap_or_else(|| Some(reply("500 Internal Server Error", "", b"")));
                if let Some(bytes) = answer {
                    let _ = conn.write_all(&bytes);
                }
            }
        });
        (format!("http://{addr}/page"), taken)
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

    /// Reads a request up to its blank line, so the client has sent all of it before the reply comes.
    fn read_head(conn: &mut TcpStream) {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match conn.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                _ => return,
            }
        }
    }

    /// A client for the loopback server, which ignores any proxy in the environment.
    pub(crate) fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().expect("a client")
    }
}

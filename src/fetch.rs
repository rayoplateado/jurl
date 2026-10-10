use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use encoding_rs::Encoding;
use reqwest::{Client, Response, header};
use serde_json::Value;
use tokio::process::Command;
use url::Url;

pub struct Page {
    pub url: Url,
    pub body: String,
    pub is_markdown: bool,
}

/// The largest page read: a body past this is an error, not read on.
const PAGE_MAX: usize = 8 << 20;

pub async fn fetch(client: &Client, url: &str) -> Result<Page> {
    fetch_capped(client, url, PAGE_MAX).await
}

/// `fetch`, refusing a page larger than `max` bytes.
async fn fetch_capped(client: &Client, url: &str, max: usize) -> Result<Page> {
    let mut res = client
        .get(url)
        .header(header::ACCEPT, "text/markdown, text/html;q=0.9, */*;q=0.5")
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    let status = res.status();
    if !status.is_success() {
        let retry = res.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok());
        match retry {
            Some(r) => bail!("{url} returned HTTP {status} (retry after {r})"),
            None => bail!("{url} returned HTTP {status}"),
        }
    }
    let final_url = res.url().clone();
    let ct = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let links: Vec<String> =
        res.headers().get_all(header::LINK).iter().filter_map(|v| v.to_str().ok()).map(String::from).collect();
    let Some(bytes) = read_capped(&mut res, max).await? else {
        bail!("{url}: page larger than {}", size_label(max));
    };
    let body = decode(&ct, &bytes);
    let is_markdown = served_markdown(&ct, &body);
    // The body is read as its markdown, but the page a person reads is the HTML page it is the alternate of.
    let url = if is_markdown { html_twin(client, &final_url, &links).await.unwrap_or(final_url) } else { final_url };
    Ok(Page { url, body, is_markdown })
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
    Ok(Page { url: url.clone(), body: body.to_string(), is_markdown: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_server::{client, serve, serve_routes};

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
        let err = fetch_capped(&client(), &url, SMALL).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn a_streamed_page_is_stopped_once_it_passes_the_cap() {
        // No Content-Length: the bytes that arrive are what is counted.
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let url = serve(head, vec![b'a'; 4 * SMALL], true);
        let err = fetch_capped(&client(), &url, SMALL).await.err().expect("refused");
        assert!(format!("{err:#}").ends_with("page larger than 1 KB"), "{err:#}");
    }

    #[tokio::test]
    async fn a_page_at_the_cap_is_read_whole_without_its_byte_order_mark() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n".to_string();
        let mut text = "\u{feff}<p>Hello, page.</p>".to_string();
        text.push_str(&" ".repeat(SMALL - text.len()));
        assert_eq!(text.len(), SMALL);
        let url = serve(head, text.clone().into_bytes(), true);
        let page = fetch_capped(&client(), &url, SMALL).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.body, text.trim_start_matches('\u{feff}'));
    }

    /// The body as fetch_capped reads it, and the body reqwest's own text() reads from the same response.
    async fn both_ways(head: &str, body: Vec<u8>) -> (String, String) {
        let url = serve(head.to_string(), body.clone(), true);
        let ours = fetch_capped(&client(), &url, SMALL).await.unwrap_or_else(|e| panic!("{e:#}")).body;
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

    /// A 200 response for `path`: its Content-Type, any extra header lines (each ending in CRLF), and its body.
    fn route(path: &'static str, content_type: &str, extra: &str, body: &str) -> (&'static str, String, Vec<u8>) {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
            body.len()
        );
        (path, head, body.as_bytes().to_vec())
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
        let stem = |u: &str| md_stem(&Url::parse(u).unwrap()).map(|s| s.to_string());
        assert_eq!(stem("https://linear.app/docs/saml.md").as_deref(), Some("https://linear.app/docs/saml"));
        assert_eq!(stem("https://site.test/docs/page.html.md").as_deref(), Some("https://site.test/docs/page.html"));
        assert_eq!(stem("https://site.test/docs/index.md").as_deref(), Some("https://site.test/docs/"));
        assert_eq!(stem("https://site.test/docs/saml"), None);
        assert_eq!(stem("https://site.test/.md"), None);
    }

    #[tokio::test]
    async fn a_markdown_page_at_md_is_read_as_the_html_page_it_is_the_alternate_of() {
        let md = "# Pricing\n\nThe Enterprise plan includes SSO.\n";
        let base = serve_routes(
            vec![
                route("/docs/saml.md", "text/markdown; charset=utf-8", "", md),
                route("/docs/saml", "text/html", "", "<h1>Pricing</h1>"),
            ],
            2,
        );
        let page =
            fetch_capped(&client(), &format!("{base}/docs/saml.md"), SMALL).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert!(page.is_markdown);
        assert_eq!(page.url.as_str(), format!("{base}/docs/saml"));
        assert_eq!(page.body, md);
    }

    #[tokio::test]
    async fn a_markdown_page_whose_twin_is_missing_or_not_html_keeps_its_md_url() {
        // `a`'s twin is missing (404); `b`'s answers with markdown again, which is not an HTML page.
        let base = serve_routes(
            vec![
                route("/docs/a.md", "text/markdown", "", "# A\n"),
                route("/docs/b.md", "text/markdown", "", "# B\n"),
                route("/docs/b", "text/markdown", "", "# B\n"),
            ],
            4,
        );
        for path in ["/docs/a.md", "/docs/b.md"] {
            let url = format!("{base}{path}");
            let page = fetch_capped(&client(), &url, SMALL).await.unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(page.url.as_str(), url);
        }
    }

    #[tokio::test]
    async fn a_canonical_link_names_the_html_page() {
        let base = serve_routes(
            vec![
                route("/docs/one", "text/markdown", "Link: </docs/canon>; rel=\"canonical\"\r\n", "# One\n"),
                route("/docs/canon", "text/html", "", "<p>One</p>"),
            ],
            2,
        );
        let page =
            fetch_capped(&client(), &format!("{base}/docs/one"), SMALL).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.url.as_str(), format!("{base}/docs/canon"));
    }
}

/// A one-shot server on 127.0.0.1, for the tests that need a body of a given size or shape.
#[cfg(test)]
pub(crate) mod test_server {
    use std::{
        io::{Read, Write},
        net::TcpListener,
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

    /// Answers each request with the route for its path (its head and body), or a 404 for a path with no route; it
    /// serves `requests` requests and then stops. Returns the base URL to fetch from.
    pub(crate) fn serve_routes(routes: Vec<(&'static str, String, Vec<u8>)>, requests: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("the loopback address");
        thread::spawn(move || {
            for _ in 0..requests {
                let Ok((mut conn, _)) = listener.accept() else { return };
                let mut request = [0u8; 4096];
                let n = conn.read(&mut request).unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..n]).to_string();
                let path = line.split_whitespace().nth(1).unwrap_or_default();
                let (head, body) = match routes.iter().find(|(p, _, _)| *p == path) {
                    Some((_, head, body)) => (head.clone(), body.clone()),
                    None => (
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                        Vec::new(),
                    ),
                };
                if conn.write_all(head.as_bytes()).is_ok() {
                    let _ = conn.write_all(&body);
                }
            }
        });
        format!("http://{addr}")
    }

    /// A client for the loopback server, which ignores any proxy in the environment.
    pub(crate) fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().expect("a client")
    }
}

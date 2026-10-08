//! Clients for System One–style decision models. Jev (TypeSafe) and Clef
//! (Cloudflare Workers AI) share the `{state, questions} → {answers}` contract.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::Duration,
};

use anyhow::Result;
use reqwest::{Client, StatusCode};
use serde_json::{Map, Value, json};

pub const JEV_MODEL: &str = "jev-1.13.0";

pub const CLEF_MODEL: &str = "clef-flash";

/// Everything this run asked of Jev and Clef, and the pages it read: what it cost, as `usage` in `--json`. Counted
/// per process, so `jurl mcp` (calls side by side) doesn't report it.
pub static USAGE: Usage = Usage::new();

/// Requests that came back with answers. A hedged Clef call that lost the race isn't counted: its answer never came.
#[derive(Debug, Default)]
pub struct Usage {
    pub pages: AtomicU64,
    pub jev_requests: AtomicU64,
    pub jev_tokens: AtomicU64,
    pub clef_requests: AtomicU64,
    pub clef_tokens: AtomicU64,
    pub clef_images: AtomicU64,
}

impl Usage {
    pub const fn new() -> Self {
        Usage {
            pages: AtomicU64::new(0),
            jev_requests: AtomicU64::new(0),
            jev_tokens: AtomicU64::new(0),
            clef_requests: AtomicU64::new(0),
            clef_tokens: AtomicU64::new(0),
            clef_images: AtomicU64::new(0),
        }
    }

    pub fn jev(&self, a: &Answers) {
        self.jev_requests.fetch_add(a.requests as u64, Relaxed);
        self.jev_tokens.fetch_add(a.input_tokens, Relaxed);
    }

    pub fn clef(&self, a: &Answers, images: usize) {
        self.clef_requests.fetch_add(a.requests as u64, Relaxed);
        self.clef_tokens.fetch_add(a.input_tokens, Relaxed);
        self.clef_images.fetch_add(images as u64, Relaxed);
    }

    pub fn json(&self) -> Value {
        let n = |c: &AtomicU64| c.load(Relaxed);
        json!({
            "pages": n(&self.pages),
            "jev": { "requests": n(&self.jev_requests), "input_tokens": n(&self.jev_tokens) },
            "clef": { "requests": n(&self.clef_requests), "input_tokens": n(&self.clef_tokens), "images": n(&self.clef_images) },
        })
    }
}

#[derive(Debug, Default)]
pub struct Answers {
    pub answers: HashMap<String, Value>,
    pub input_tokens: u64,
    pub requests: usize,
}

impl Answers {
    pub fn label(&self) -> String {
        format!("jev({} req, {} tok)", self.requests, self.input_tokens)
    }

    pub fn noul(&self, id: &str) -> Option<f64> {
        self.answers.get(id)?.get("noul")?.as_f64()
    }

    pub fn probabilities(&self, id: &str) -> Option<HashMap<String, f64>> {
        let p = self.answers.get(id)?.get("probabilities")?.as_object()?;
        Some(p.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect())
    }

    pub fn choice(&self, id: &str) -> Option<(String, f64)> {
        let a = self.answers.get(id)?;
        Some((a.get("choice")?.as_str()?.to_string(), a.get("confidence").and_then(Value::as_f64).unwrap_or(0.0)))
    }
}

pub fn noul(instructions: impl Into<String>) -> Value {
    json!({ "type": "noul", "instructions": instructions.into() })
}

pub fn choice(instructions: &str, criteria: Value) -> Value {
    json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
}

/// `JURL_JEV_URL`: another server with Jev's contract (a self-hosted model), when set and not empty.
pub fn custom_jev_url() -> Option<String> {
    non_empty(std::env::var("JURL_JEV_URL").ok())
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|u| !u.trim().is_empty())
}

/// Jev's endpoint, or the one in `JURL_JEV_URL`.
pub fn jev_url() -> String {
    custom_jev_url().unwrap_or_else(|| JEV_URL.to_string())
}

const JEV_URL: &str = "https://api.typesafe.ai/v1/systemone";

pub async fn jev(client: &Client, key: &str, state: Value, questions: Map<String, Value>) -> Result<Answers> {
    let body = json!({ "state": state, "model": JEV_MODEL, "questions": questions });
    let v = post(client, &jev_url(), key, &body).await?;
    let a = parse(&v);
    USAGE.jev(&a);
    Ok(a)
}

pub async fn clef(
    client: &Client,
    account: &str,
    token: &str,
    state: Value,
    questions: Map<String, Value>,
    images: Vec<String>,
) -> Result<Answers> {
    let url = format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/@cf/cloudflare/{CLEF_MODEL}");
    let mut body = json!({ "model": CLEF_MODEL, "state": state, "questions": questions });
    if !images.is_empty() {
        body["images"] = json!(&images);
    }
    let v = post(client, &url, token, &body).await?;
    let a = clef_answers(&v);
    USAGE.clef(&a, images.len());
    Ok(a)
}

/// Workers AI wraps the model's reply in `result`; its usage may sit inside it or beside it.
fn clef_answers(v: &Value) -> Answers {
    let mut a = parse(v.get("result").unwrap_or(v));
    if a.input_tokens == 0 {
        a.input_tokens = v.pointer("/usage/input_tokens").and_then(Value::as_u64).unwrap_or(0);
    }
    a
}

fn parse(v: &Value) -> Answers {
    let answers = v
        .get("answers")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let input_tokens = v.pointer("/usage/input_tokens").and_then(Value::as_u64).unwrap_or(0);
    Answers { answers, input_tokens, requests: 1 }
}

/// Jev or Clef couldn't be asked (a bad key, no credits, the API down): never mistaken for a page with nothing on it.
#[derive(Debug)]
pub struct ApiError(String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ApiError {}

pub fn is_api_error(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<ApiError>())
}

/// Plain http to another host: anyone on the network path could read a key sent there.
fn plain_http_off_loopback(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let loopback = match u.host() {
        Some(url::Host::Domain(d)) => d == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    u.scheme() == "http" && !loopback
}

/// POST with a short backoff on 429/529, honouring `Retry-After`.
async fn post(client: &Client, url: &str, bearer: &str, body: &Value) -> Result<Value> {
    if !bearer.is_empty() && plain_http_off_loopback(url) {
        let msg = format!("not sending a key over plain http to {url}: use https, or localhost");
        return Err(ApiError(msg).into());
    }
    let mut attempt = 0;
    loop {
        let mut req = client.post(url).json(body);
        if !bearer.is_empty() {
            req = req.bearer_auth(bearer);
        }
        let res = req.send().await.map_err(|e| ApiError(format!("{url}: {e}")))?;
        let status = res.status();
        if status.is_success() {
            // A cut-off or garbled reply is a failed call; --follow would otherwise read it as a page with no answer.
            let text = res.text().await.map_err(|e| ApiError(format!("{url}: reading the reply: {e}")))?;
            return parse_reply(url, &text);
        }
        let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.as_u16() == 529;
        if retryable && attempt < 2 {
            let wait = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok()?.parse::<f64>().ok())
                .map_or(Duration::from_millis(300 << attempt), Duration::from_secs_f64);
            tokio::time::sleep(wait.min(Duration::from_secs(5))).await;
            attempt += 1;
            continue;
        }
        let text = res.text().await.unwrap_or_default();
        return Err(ApiError(format!("{url} → HTTP {status}: {}", snippet(&text))).into());
    }
}

/// A 200's body as JSON. One that doesn't parse is an API failure, not a page with nothing on it.
fn parse_reply(url: &str, text: &str) -> Result<Value> {
    serde_json::from_str(text).map_err(|e| ApiError(format!("{url}: bad reply ({e}): {}", snippet(text))).into())
}

/// The start of a reply, for an error message.
fn snippet(text: &str) -> String {
    text.chars().take(400).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_adds_up_jev_and_clef() {
        let u = Usage::new();
        u.pages.fetch_add(2, Relaxed);
        u.jev(&parse(&json!({ "answers": {}, "usage": { "input_tokens": 1200 } })));
        u.jev(&Answers { requests: 3, input_tokens: 800, ..Answers::default() });
        u.clef(&clef_answers(&json!({ "result": { "answers": {}, "usage": { "input_tokens": 90 } } })), 1);
        u.clef(&clef_answers(&json!({ "result": { "answers": {} }, "usage": { "input_tokens": 70 } })), 1);
        assert_eq!(
            u.json(),
            json!({
                "pages": 2,
                "jev": { "requests": 4, "input_tokens": 2000 },
                "clef": { "requests": 2, "input_tokens": 160, "images": 2 },
            })
        );
    }

    #[test]
    fn jurl_jev_url_counts_only_when_not_empty() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some(String::new())), None);
        assert_eq!(non_empty(Some("  ".into())), None);
        let url = "http://127.0.0.1:8000/v1/systemone";
        assert_eq!(non_empty(Some(url.into())).as_deref(), Some(url));
    }

    #[test]
    fn a_key_goes_only_over_https_or_to_loopback() {
        for url in [
            "https://api.typesafe.ai/v1/systemone",
            "https://10.0.0.5:8000/v1",
            "http://127.0.0.1:8000/v1/systemone",
            "http://127.0.0.2:8000/v1",
            "http://localhost:18100/v1",
            "http://LOCALHOST:8000/v1",
            "http://[::1]:8000/v1",
            "not a url", // unparseable: reqwest fails it before sending
        ] {
            assert!(!plain_http_off_loopback(url), "{url}");
        }
        for url in [
            "http://10.0.0.5:8000/v1/systemone",
            "http://example.com/v1",
            "http://localhost.example.com/v1",
            "http://127.0.0.1.nip.io/v1",
            "http://[2001:db8::1]:8000/v1",
        ] {
            assert!(plain_http_off_loopback(url), "{url}");
        }
    }

    #[test]
    fn a_reply_that_does_not_parse_is_an_api_error() {
        let e = parse_reply("https://x/v1", "{\"answers\":").unwrap_err();
        assert!(is_api_error(&e), "{e:#}");
        assert!(parse_reply("https://x/v1", r#"{"answers":{}}"#).is_ok());
    }

    #[tokio::test]
    async fn a_reply_cut_off_mid_body_is_an_api_error() {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            // Read the whole request first, so closing the socket doesn't reset the connection.
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut len = 0;
            for line in r.by_ref().lines().map_while(Result::ok) {
                if line.is_empty() {
                    break;
                }
                if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = n.trim().parse().unwrap();
                }
            }
            r.read_exact(&mut vec![0; len]).unwrap();
            // Promises 100 bytes, sends 10, then closes.
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"answers\"").unwrap();
        });
        let e = post(&Client::new(), &url, "", &json!({})).await.unwrap_err();
        assert!(is_api_error(&e), "{e:#}");
        assert!(format!("{e:#}").contains("reading the reply"), "{e:#}");
    }
}

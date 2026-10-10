//! Rung 5 of the read ladder: a stealth sidecar that loads one public page in a stealth browser (Camoufox, MPL-2.0) and
//! returns the rendered HTML, with its own verdict on what it found. jurl asks it only for a page that a host still refuses
//! with 401, 403 or 429 after rungs 1–2 (see `fetch::fetch_capped`), and only when `JURL_STEALTH_URL` and
//! `JURL_STEALTH_TOKEN` are both set. jurl decides nothing from the page's text: the sidecar says `content`, `captcha`,
//! `blocked`, `challenge` or `error`. A CAPTCHA is never solved: the page is an error, and it goes to human review.
//!
//! The sidecar is `jurl-cloud/infra/stealth/`. Robots.txt, sitemaps and llms.txt never go to it (see `fetch::fetch_small`).

use std::{
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Duration,
};

use reqwest::{Client, StatusCode};
use serde_json::{Map, Value, json};
use url::Url;

use crate::{
    fetch::{self, Memo},
    reach::{self, Reach},
};

/// How long one stealth render may take. The sidecar answers within about 100 s at its worst: a queue of 20 s, a host gate of
/// 20 s, navigation of 45 s and a settle window of 15 s. 110 s covers that, with room for the reply itself. A call that runs
/// longer is a failure, and the refusal stands.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(110);

/// The calls one run may make to the sidecar, when `JURL_STEALTH_MAX_PAGES` says nothing else.
const MAX_PAGES: usize = 3;

/// The most of a sidecar's reason that a trace or an error shows, in characters.
const REASON_CHARS: usize = 200;

/// A page the sidecar found a CAPTCHA on. jurl does not solve it: the page goes to human review. Under `--follow` it is skipped
/// like any page that failed to load; the start page fails the run.
#[derive(Debug)]
pub(crate) struct Captcha(pub(crate) String);

impl std::fmt::Display for Captcha {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: an interactive CAPTCHA, not solved; for human review", self.0)
    }
}

impl std::error::Error for Captcha {}

/// Whether a page that a host refused with `status` is asked of the sidecar: the statuses an anti-bot wall answers with.
pub(crate) fn asked_for(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS)
}

/// What a call to the sidecar came back with when it is not a page.
enum Failed {
    /// A CAPTCHA: the page is for human review.
    Captcha,
    /// Anything else. `memo` says whether the host is then not asked again in the run: a busy sidecar is not memoized.
    Other { reason: String, memo: bool },
}

/// The stealth sidecar a run is set up with (see `cli::Args`): where it is, its token, the language and country a page is read
/// as, and the run's own memo and budget. Per run, not per process: `fetch::MEMO` lives as long as `jurl mcp`, and the budget of
/// one run would starve every later call of it.
pub(crate) struct Sidecar {
    /// `JURL_STEALTH_URL`, without a trailing slash.
    base: String,
    /// `JURL_STEALTH_TOKEN`, the bearer. Never shown: see [`Sidecar::shown`].
    token: String,
    /// The first language tag of the run's Accept-Language, when it names one (see [`lang_of`]).
    lang: Option<String>,
    /// `JURL_STEALTH_COUNTRY`, when it is two uppercase letters (see [`country_of`]).
    country: Option<String>,
    /// The run's public-only setting and sandbox flag (see `reach.rs`). The sidecar runs the page's JavaScript, which
    /// `JURL_PUBLIC_ONLY=1` allows only when the network outside jurl confines the browser, as `fetch::render_allowed` says.
    public_only: bool,
    sandboxed: bool,
    /// How many calls the run may make: `JURL_STEALTH_MAX_PAGES`.
    max_pages: usize,
    /// Its calls go through this client, built with [`TIMEOUT`].
    client: Client,
    /// The hosts whose call failed in this run, which are not asked again. Only the memo's sidecar facet is used.
    memo: Memo,
    /// The calls made so far. Reserved before each call (see [`Sidecar::reserve`]).
    calls: AtomicUsize,
}

impl Sidecar {
    /// The sidecar this run is set up with, from the settings `get` gives (the environment, then the saved file, as
    /// `Config::get` does), or None when it is not set up. Under `timing`, says why when it is named but cannot be used.
    pub(crate) fn configured(
        get: impl Fn(&str) -> Option<String>,
        public_only: bool,
        sandboxed: bool,
        timing: bool,
    ) -> Option<Sidecar> {
        let base = get("JURL_STEALTH_URL").filter(|v| !v.is_empty())?;
        let Some(token) = get("JURL_STEALTH_TOKEN").filter(|v| !v.is_empty()) else {
            note(
                timing,
                "JURL_STEALTH_URL is set but JURL_STEALTH_TOKEN is not: the stealth sidecar is not asked".into(),
            );
            return None;
        };
        let country_setting = get("JURL_STEALTH_COUNTRY").filter(|v| !v.is_empty());
        let country = country_of(country_setting.as_deref());
        if country_setting.is_some() && country.is_none() {
            note(timing, "JURL_STEALTH_COUNTRY is not two uppercase letters, so it is not sent".into());
        }
        let max_pages = get("JURL_STEALTH_MAX_PAGES").and_then(|v| v.trim().parse().ok()).unwrap_or(MAX_PAGES);
        let client = match Client::builder().no_proxy().timeout(TIMEOUT).build() {
            Ok(client) => client,
            Err(e) => {
                note(timing, format!("the stealth sidecar's client could not be built: {e}"));
                return None;
            }
        };
        Some(Sidecar {
            base: base.trim_end_matches('/').to_string(),
            token,
            lang: lang_of(fetch::accept_language().as_deref()),
            country,
            public_only,
            sandboxed,
            max_pages,
            client,
            memo: Memo::default(),
            calls: AtomicUsize::new(0),
        })
    }

    /// Asks the sidecar for `url`, a page a host refused (see [`asked_for`]), when the run may. Returns the rendered HTML when the
    /// sidecar has content, and None when the refusal stands: the run may not ask, the host failed earlier in the run, the budget
    /// is spent, or the sidecar could not read the page. A CAPTCHA is an error. `max` is the largest page the run reads.
    pub(crate) async fn read(
        &self,
        url: &Url,
        reach: &Reach,
        max: usize,
        timing: bool,
    ) -> Result<Option<String>, Captcha> {
        if let Some(why) = self.not_asked(url, reach).await {
            note(timing, format!("{url}: not asked the stealth sidecar: {why}"));
            return Ok(None);
        }
        note(timing, format!("{url}: asking the stealth sidecar"));
        let host = fetch::host_key(url);
        match self.call(url, max).await {
            Ok(html) => Ok(Some(html)),
            Err(Failed::Captcha) => {
                self.fail(host.as_deref());
                Err(Captcha(url.to_string()))
            }
            Err(Failed::Other { reason, memo }) => {
                if memo {
                    self.fail(host.as_deref());
                }
                note(timing, format!("{url}: the stealth sidecar could not read it: {reason}"));
                Ok(None)
            }
        }
    }

    /// Why the sidecar may not be asked for `url` now, or None when it may. The checks are the run's guard, the host's memo, and
    /// then the budget, which is reserved only when every other check passed.
    async fn not_asked(&self, url: &Url, reach: &Reach) -> Option<String> {
        // As `fetch::render_allowed` does: a run that reads any address asks the sidecar without a check.
        if !matches!(reach, Reach::Private) {
            if self.public_only && !self.sandboxed {
                return Some(
                    "JURL_PUBLIC_ONLY=1 needs JURL_RENDER_SANDBOXED=1, since the sidecar runs the page's JavaScript unchecked"
                        .into(),
                );
            }
            // The page's own address, resolved here. The sidecar resolves it again when it loads the page, so this does not
            // stop a name that changes its answer between the two.
            if let Err(e) = reach::check(url, reach).await {
                return Some(e.to_string());
            }
        }
        if fetch::host_key(url).is_some_and(|host| self.memo.sidecar_failed(&host)) {
            return Some("its call failed earlier in this run".into());
        }
        if !self.reserve() {
            return Some(format!("the run's {} calls are used up (JURL_STEALTH_MAX_PAGES)", self.max_pages));
        }
        None
    }

    /// Takes one of the run's calls, if one is left. Checked and counted in one step, so pages read at once cannot both take
    /// the last call.
    fn reserve(&self) -> bool {
        let mut seen = self.calls.load(Relaxed);
        loop {
            if seen >= self.max_pages {
                return false;
            }
            match self.calls.compare_exchange_weak(seen, seen + 1, Relaxed, Relaxed) {
                Ok(_) => return true,
                Err(now) => seen = now,
            }
        }
    }

    /// Records that the sidecar's call for `host` failed: it is not asked for that host again in this run.
    fn fail(&self, host: Option<&str>) {
        if let Some(host) = host {
            self.memo.fail_sidecar(host);
        }
    }

    /// One render call for `url`. The token travels only in the bearer header, and what the sidecar says is shown without it.
    async fn call(&self, url: &Url, max: usize) -> Result<String, Failed> {
        let mut body = Map::new();
        body.insert("url".into(), json!(url.as_str()));
        if let Some(lang) = &self.lang {
            body.insert("lang".into(), json!(lang));
        }
        if let Some(country) = &self.country {
            body.insert("country".into(), json!(country));
        }
        let sent = self.client.post(format!("{}/render", self.base)).bearer_auth(&self.token).json(&body).send().await;
        let res = sent.map_err(|e| self.unreachable(&e))?;
        let status = res.status();
        let text = res.text().await.map_err(|e| self.unreachable(&e))?;
        let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status != StatusCode::OK {
            // A busy sidecar may have room later: it is not memoized, though its call still counts against the budget.
            let busy = status == StatusCode::SERVICE_UNAVAILABLE;
            let code = answer["error"].as_str().unwrap_or("no reply");
            return Err(Failed::Other { reason: self.shown(&format!("HTTP {status}: {code}")), memo: !busy });
        }
        match answer["outcome"].as_str() {
            Some("content") => {
                let html = answer["html"].as_str().unwrap_or_default();
                if html.trim().is_empty() {
                    Err(self.other("no content"))
                } else if html.len() > max {
                    Err(self.other(&format!("larger than {}", fetch::size_label(max))))
                } else {
                    Ok(html.to_string())
                }
            }
            Some("captcha") => Err(Failed::Captcha),
            outcome => Err(self.other(answer["reason"].as_str().or(outcome).unwrap_or("no answer"))),
        }
    }

    /// A failure the host is not asked again for.
    fn other(&self, reason: &str) -> Failed {
        Failed::Other { reason: self.shown(reason), memo: true }
    }

    /// A call that got no reply: a timeout or a connection error. The error's own text is not shown: it names the sidecar's URL.
    fn unreachable(&self, e: &reqwest::Error) -> Failed {
        let reason = if e.is_timeout() { "timed out" } else { "unreachable" };
        Failed::Other { reason: reason.into(), memo: true }
    }

    /// `text` as a trace or an error may show it: without the token, and at most [`REASON_CHARS`] characters.
    fn shown(&self, text: &str) -> String {
        text.replace(&self.token, "[token]").chars().take(REASON_CHARS).collect()
    }
}

/// The first language tag of an Accept-Language value, without its `;q=` weight: `es-ES,es;q=0.9` is `es-ES`. None when there is
/// no such tag, or its first tag is not letters, digits and hyphens (such as `*`).
pub(crate) fn lang_of(accept_language: Option<&str>) -> Option<String> {
    let first = accept_language?.split(',').next()?.split(';').next()?.trim();
    let plain = !first.is_empty() && first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    plain.then(|| first.to_string())
}

/// The country a page is read as, when `JURL_STEALTH_COUNTRY` is two uppercase letters (ISO 3166-1 alpha-2). Any other value is
/// not sent.
pub(crate) fn country_of(value: Option<&str>) -> Option<String> {
    let v = value?;
    (v.len() == 2 && v.bytes().all(|b| b.is_ascii_uppercase())).then(|| v.to_string())
}

/// Says `text` on stderr as a trace line, under `-t`.
fn note(timing: bool, text: String) {
    if timing {
        eprintln!("jurl: {text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: (&str, &str) = ("JURL_STEALTH_URL", "https://sidecar.example:8091");
    const TOKEN: (&str, &str) = ("JURL_STEALTH_TOKEN", "s3cret-token");

    /// The settings the pairs name, as `get` reads them.
    fn settings<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
    }

    fn sidecar(pairs: &[(&str, &str)]) -> Option<Sidecar> {
        Sidecar::configured(settings(pairs), false, false, false)
    }

    #[test]
    fn the_sidecar_is_set_up_only_with_a_url_and_a_token() {
        assert!(sidecar(&[]).is_none());
        assert!(sidecar(&[URL]).is_none(), "no token, no call");
        assert!(sidecar(&[URL, ("JURL_STEALTH_TOKEN", "")]).is_none(), "an empty token is no token");
        assert!(sidecar(&[URL, TOKEN]).is_some());
    }

    #[test]
    fn the_budget_is_three_calls_unless_the_run_names_another() {
        assert_eq!(sidecar(&[URL, TOKEN]).map(|s| s.max_pages), Some(3));
        assert_eq!(sidecar(&[URL, TOKEN, ("JURL_STEALTH_MAX_PAGES", "5")]).map(|s| s.max_pages), Some(5));
        assert_eq!(sidecar(&[URL, TOKEN, ("JURL_STEALTH_MAX_PAGES", "lots")]).map(|s| s.max_pages), Some(3));
    }

    #[test]
    fn the_first_language_tag_is_the_one_sent() {
        assert_eq!(lang_of(Some("es-ES,es;q=0.9,en;q=0.8")), Some("es-ES".into()));
        assert_eq!(lang_of(Some(" fr ;q=1")), Some("fr".into()));
        assert_eq!(lang_of(Some(";q=0.5")), None, "no first tag");
        assert_eq!(lang_of(Some("*")), None, "not a language tag");
        assert_eq!(lang_of(None), None);
    }

    #[test]
    fn a_country_is_sent_only_as_two_uppercase_letters() {
        assert_eq!(country_of(Some("ES")), Some("ES".into()));
        for bad in ["es", " US ", "ESP", "E", "E1", ""] {
            assert_eq!(country_of(Some(bad)), None, "{bad:?}");
        }
        assert_eq!(country_of(None), None);
    }

    #[test]
    fn a_reason_shows_no_token_and_at_most_200_characters() {
        let s = sidecar(&[URL, TOKEN]).expect("set up");
        assert_eq!(s.shown("blocked, as s3cret-token says"), "blocked, as [token] says");
        let long = "ñ".repeat(500);
        assert_eq!(s.shown(&long).chars().count(), REASON_CHARS, "cut on a character, not a byte");
    }

    #[test]
    fn a_captcha_names_the_page_and_says_it_is_not_solved() {
        let err = Captcha("https://shop.example/p".into());
        assert_eq!(err.to_string(), "https://shop.example/p: an interactive CAPTCHA, not solved; for human review");
    }

    // The rest run the sidecar against local servers: a page server that refuses, and a mock sidecar that answers in order. A
    // mock that holds more replies than a test should use shows a call that should not have been made: the page would succeed.

    use std::{
        collections::HashSet,
        net::{IpAddr, TcpListener},
        sync::Arc,
        thread,
        time::Instant,
    };

    use crate::{
        fetch::{
            Memo, Retry, test_server::NO_COOKIES, test_server::PRIVATE, test_server::reply, test_server::serve_routed,
        },
        mock,
    };

    const SECRET_TOKEN: &str = "s3cret-token";

    const CONTENT: &str = r#"{"outcome":"content","status":200,"html":"<html><body><p>Precio: 5 €</p></body></html>","text":"Precio: 5 €","title":"Tienda","reason":null,"wall_s":3.1,"robots":"allowed"}"#;
    const CAPTCHA_ANSWER: &str = r#"{"outcome":"captcha","status":403,"html":"","text":"","title":"","reason":"geo.captcha-delivery.com","wall_s":4.0,"robots":"allowed"}"#;
    const BLOCKED: &str = r#"{"outcome":"blocked","status":403,"html":"","text":"","title":"","reason":"datadome wall","wall_s":2.0,"robots":"allowed"}"#;
    const CHALLENGE: &str = r#"{"outcome":"challenge","status":503,"html":"","text":"","title":"","reason":"js challenge","wall_s":8.0,"robots":"allowed"}"#;
    const ERROR: &str = r#"{"outcome":"error","status":null,"html":"","text":"","title":"","reason":"navigation timeout","wall_s":45.0,"robots":"allowed"}"#;
    const EMPTY_CONTENT: &str = r#"{"outcome":"content","status":200,"html":"  ","text":"","title":"","reason":null,"wall_s":2.0,"robots":"allowed"}"#;
    const BUSY: &str = r#"{"error":"busy"}"#;

    /// A sidecar at `base`, as a run sets one up, with its client's timeout and budget as given.
    fn sidecar_with(base: &str, timeout: Duration, max_pages: usize) -> Sidecar {
        Sidecar {
            base: base.to_string(),
            token: SECRET_TOKEN.to_string(),
            lang: None,
            country: None,
            public_only: false,
            sandboxed: false,
            max_pages,
            client: Client::builder().no_proxy().timeout(timeout).build().expect("a client"),
            memo: Memo::default(),
            calls: AtomicUsize::new(0),
        }
    }

    fn sidecar_at(base: &str) -> Sidecar {
        sidecar_with(base, TIMEOUT, MAX_PAGES)
    }

    /// The retry of a test run: the browser retry on, the run's reach, and `stealth` as its sidecar.
    fn retry_for<'a>(memo: &'a Memo, reach: &'a Reach, stealth: Option<&'a Sidecar>) -> Retry<'a> {
        Retry { on: true, memo, timing: false, reach, cookies: &NO_COOKIES, stealth }
    }

    /// A reach that reads loopback, as a run from a public address reads only public ones and the start's own.
    fn public_run() -> Reach {
        Reach::Public { allowed: HashSet::from([IpAddr::from([127, 0, 0, 1])]) }
    }

    /// A header the request sent, by its name in any case.
    fn header(seen: &mock::Seen, name: &str) -> Option<String> {
        seen.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    }

    /// A site that refuses every request with `status`, and its base URL.
    fn refusing(status: &'static str) -> String {
        serve_routed(move |_| Some(reply(status, "", b"denied"))).0
    }

    /// The error text of a refused page.
    fn refusal_of(url: &str) -> String {
        format!("{url} returned HTTP 403 Forbidden (also with a browser's TLS fingerprint)")
    }

    #[tokio::test]
    async fn a_page_refused_with_403_is_read_from_the_sidecar_as_html() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        let url = format!("{base}/precio");
        let page =
            fetch::fetch(&url, retry_for(&memo, &PRIVATE, Some(&sidecar))).await.unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, fetch::Route::Stealth);
        assert!(page.body.contains("Precio: 5"), "{}", page.body);
        assert!(!page.is_markdown, "the sidecar's HTML is read as HTML");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("POST", "/render"));
        assert_eq!(header(&seen[0], "authorization").as_deref(), Some("Bearer s3cret-token"));
        let body: Value = serde_json::from_str(&seen[0].body).expect("a JSON request");
        assert_eq!(body["url"], url.as_str());
    }

    #[tokio::test]
    async fn a_401_and_a_429_are_asked_of_the_sidecar_too() {
        let (base, _) = serve_routed(|head| {
            let path = head.split_whitespace().nth(1).unwrap_or_default();
            Some(match path {
                "/login" => reply("401 Unauthorized", "", b"sign in"),
                _ => reply("429 Too Many Requests", "", b"slow down"),
            })
        });
        let (side, seen) = mock::serve(vec![(200, "", CONTENT), (200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        for path in ["/login", "/busy-day"] {
            let page = fetch::fetch(&format!("{base}{path}"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
                .await
                .unwrap_or_else(|e| panic!("{path}: {e:#}"));
            assert_eq!(page.route, fetch::Route::Stealth, "{path}");
        }
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_404_or_a_500_is_not_asked_of_the_sidecar() {
        let (base, _) = serve_routed(|head| {
            let path = head.split_whitespace().nth(1).unwrap_or_default();
            Some(match path {
                "/gone" => reply("404 Not Found", "", b""),
                _ => reply("500 Internal Server Error", "", b""),
            })
        });
        let (side, seen) = mock::serve(vec![(200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        for (path, status) in [("/gone", "404"), ("/broken", "500")] {
            let err = fetch::fetch(&format!("{base}{path}"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
                .await
                .err()
                .expect("refused");
            assert!(format!("{err:#}").contains(&format!("returned HTTP {status}")), "{err:#}");
        }
        assert!(seen.lock().unwrap().is_empty(), "neither status is an anti-bot wall");
    }

    #[tokio::test]
    async fn a_captcha_is_an_error_that_names_the_page_and_is_not_solved() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", CAPTCHA_ANSWER), (200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        let url = format!("{base}/tienda");
        let err =
            fetch::fetch(&url, retry_for(&memo, &PRIVATE, Some(&sidecar))).await.err().expect("a CAPTCHA is an error");
        assert_eq!(format!("{err:#}"), format!("{url}: an interactive CAPTCHA, not solved; for human review"));
        assert!(err.downcast_ref::<Captcha>().is_some());
        // The host is not asked again: the next page of it is refused, and the sidecar's content stays unread.
        let err = fetch::fetch(&format!("{base}/otra"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .err()
            .expect("refused");
        assert!(format!("{err:#}").starts_with(&format!("{base}/otra returned HTTP 403")), "{err:#}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_sidecar_that_cannot_read_the_page_leaves_the_refusal_standing() {
        let (side, seen) = mock::serve(vec![(200, "", BLOCKED), (200, "", CHALLENGE), (200, "", ERROR)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        for _ in 0..3 {
            // A host of its own for each page, so each one is asked.
            let base = refusing("403 Forbidden");
            let url = format!("{base}/p");
            let err = fetch::fetch(&url, retry_for(&memo, &PRIVATE, Some(&sidecar))).await.err().expect("refused");
            assert_eq!(format!("{err:#}"), refusal_of(&url));
        }
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn sidecar_content_with_no_html_is_a_failure_for_the_host() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", EMPTY_CONTENT), (200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        let url = format!("{base}/a");
        let err = fetch::fetch(&url, retry_for(&memo, &PRIVATE, Some(&sidecar))).await.err().expect("refused");
        assert_eq!(format!("{err:#}"), refusal_of(&url));
        let err = fetch::fetch(&format!("{base}/b"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .err()
            .expect("refused");
        assert!(format!("{err:#}").contains("returned HTTP 403"), "{err:#}");
        assert_eq!(seen.lock().unwrap().len(), 1, "the host failed, so it is not asked again");
    }

    #[tokio::test]
    async fn a_sidecar_call_that_times_out_leaves_the_refusal_and_is_not_asked_again() {
        // A sidecar that answers after the test's timeout, and counts the calls it gets.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let side = format!("http://{}", listener.local_addr().expect("the loopback address"));
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                counted.fetch_add(1, Relaxed);
                thread::sleep(Duration::from_secs(3));
                drop(conn);
            }
        });
        let sidecar = sidecar_with(&side, Duration::from_millis(300), MAX_PAGES);
        let memo = Memo::default();
        let base = refusing("403 Forbidden");
        let err = fetch::fetch(&format!("{base}/a"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .err()
            .expect("refused");
        assert!(format!("{err:#}").contains("returned HTTP 403 Forbidden"), "{err:#}");
        let err = fetch::fetch(&format!("{base}/b"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .err()
            .expect("refused");
        assert!(format!("{err:#}").contains("returned HTTP 403 Forbidden"), "{err:#}");
        assert_eq!(calls.load(Relaxed), 1, "a timeout is a failure, and the host is not asked again");
    }

    #[tokio::test]
    async fn a_host_whose_sidecar_call_failed_is_not_asked_again_in_the_run() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", BLOCKED), (200, "", CONTENT), (200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        for path in ["/a", "/b", "/c"] {
            let url = format!("{base}{path}");
            let err = fetch::fetch(&url, retry_for(&memo, &PRIVATE, Some(&sidecar))).await.err().expect("refused");
            assert_eq!(format!("{err:#}"), refusal_of(&url));
        }
        assert_eq!(seen.lock().unwrap().len(), 1, "asked once for the host, then not again");
    }

    #[tokio::test]
    async fn a_new_run_asks_again_because_the_memo_is_the_runs_own() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", BLOCKED), (200, "", CONTENT)]);
        let memo = Memo::default();
        let first = sidecar_at(&side);
        let _ = fetch::fetch(&format!("{base}/a"), retry_for(&memo, &PRIVATE, Some(&first))).await.err();
        let second = sidecar_at(&side);
        let page = fetch::fetch(&format!("{base}/b"), retry_for(&Memo::default(), &PRIVATE, Some(&second)))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, fetch::Route::Stealth);
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_budget_caps_the_calls_a_run_makes() {
        let (base_a, base_b) = (refusing("403 Forbidden"), refusing("403 Forbidden"));
        let (side, seen) = mock::serve(vec![(200, "", CONTENT), (200, "", CONTENT)]);
        let sidecar = sidecar_with(&side, TIMEOUT, 1);
        let memo = Memo::default();
        let first = fetch::fetch(&format!("{base_a}/p"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(first.route, fetch::Route::Stealth);
        let url = format!("{base_b}/p");
        let err = fetch::fetch(&url, retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .err()
            .expect("refused: the budget is spent");
        assert_eq!(format!("{err:#}"), refusal_of(&url));
        assert_eq!(seen.lock().unwrap().len(), 1, "the second host is not asked");
    }

    #[tokio::test]
    async fn a_busy_sidecar_is_not_memoized_but_its_call_counts() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(503, "", BUSY), (200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        let url = format!("{base}/a");
        let err = fetch::fetch(&url, retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .err()
            .expect("busy: the refusal stands");
        assert_eq!(format!("{err:#}"), refusal_of(&url));
        let page = fetch::fetch(&format!("{base}/b"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, fetch::Route::Stealth, "asked again, and read");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_public_only_run_without_the_sandbox_does_not_ask_the_sidecar() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", CONTENT)]);
        let mut sidecar = sidecar_at(&side);
        sidecar.public_only = true;
        let reach = public_run();
        let memo = Memo::default();
        let url = format!("{base}/p");
        let err = fetch::fetch(&url, retry_for(&memo, &reach, Some(&sidecar))).await.err().expect("refused");
        assert_eq!(format!("{err:#}"), refusal_of(&url));
        assert!(seen.lock().unwrap().is_empty(), "the sidecar runs the page's JavaScript unchecked");
    }

    #[tokio::test]
    async fn a_public_only_run_with_the_sandbox_asks_the_sidecar_after_the_page_is_checked() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", CONTENT)]);
        let mut sidecar = sidecar_at(&side);
        sidecar.public_only = true;
        sidecar.sandboxed = true;
        let reach = public_run();
        let memo = Memo::default();
        let page = fetch::fetch(&format!("{base}/p"), retry_for(&memo, &reach, Some(&sidecar)))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(page.route, fetch::Route::Stealth);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_request_carries_the_language_and_country_the_run_names() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", CONTENT), (200, "", CONTENT)]);
        let mut named = sidecar_at(&side);
        named.lang = Some("es-ES".into());
        named.country = Some("ES".into());
        let memo = Memo::default();
        fetch::fetch(&format!("{base}/a"), retry_for(&memo, &PRIVATE, Some(&named)))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        let bare = sidecar_at(&side);
        fetch::fetch(&format!("{base}/b"), retry_for(&Memo::default(), &PRIVATE, Some(&bare)))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        let seen = seen.lock().unwrap();
        let with: Value = serde_json::from_str(&seen[0].body).expect("JSON");
        assert_eq!((with["lang"].as_str(), with["country"].as_str()), (Some("es-ES"), Some("ES")));
        let without: Value = serde_json::from_str(&seen[1].body).expect("JSON");
        assert!(without.get("lang").is_none() && without.get("country").is_none(), "{without}");
    }

    #[tokio::test]
    async fn robots_and_sitemaps_never_reach_the_sidecar() {
        let base = refusing("403 Forbidden");
        let (side, seen) = mock::serve(vec![(200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        let url = Url::parse(&format!("{base}/robots.txt")).expect("a URL");
        let text = fetch::fetch_small(
            &url,
            1 << 20,
            Duration::from_secs(5),
            retry_for(&memo, &PRIVATE, Some(&sidecar)),
            Instant::now(),
        )
        .await;
        assert!(text.is_none());
        assert!(seen.lock().unwrap().is_empty(), "small files are not pages");
    }

    #[tokio::test]
    async fn a_token_never_appears_in_an_error() {
        // A sidecar nobody listens on: the call gets no reply at all.
        let closed = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
            format!("http://{}", listener.local_addr().expect("the loopback address"))
        };
        let base = refusing("403 Forbidden");
        let memo = Memo::default();
        let err = fetch::fetch(&format!("{base}/p"), retry_for(&memo, &PRIVATE, Some(&sidecar_at(&closed))))
            .await
            .err()
            .expect("refused");
        assert!(!format!("{err:#}").contains(SECRET_TOKEN), "{err:#}");
        // A sidecar that names the token in its reason: the reason is shown without it.
        let (side, _) =
            mock::serve(vec![(200, "", r#"{"outcome":"blocked","html":"","reason":"rejected s3cret-token"}"#)]);
        let sidecar = sidecar_at(&side);
        let err = fetch::fetch(&format!("{base}/q"), retry_for(&Memo::default(), &PRIVATE, Some(&sidecar)))
            .await
            .err()
            .expect("refused");
        assert!(!format!("{err:#}").contains(SECRET_TOKEN), "{err:#}");
        assert_eq!(sidecar.shown("rejected s3cret-token"), "rejected [token]");
    }

    #[tokio::test]
    async fn a_host_on_the_browser_client_whose_browser_refuses_is_asked_of_the_sidecar() {
        // Plain requests are refused. The first browser request is read, and the browser client refuses after that.
        let browser_requests = Arc::new(AtomicUsize::new(0));
        let counted = browser_requests.clone();
        let (base, _) = serve_routed(move |head| {
            if !head.contains("Chrome/") {
                return Some(reply("403 Forbidden", "", b"denied"));
            }
            if counted.fetch_add(1, Relaxed) == 0 {
                Some(reply("200 OK", "Content-Type: text/html\r\n", b"<p>Primera pagina</p>"))
            } else {
                Some(reply("403 Forbidden", "", b"denied"))
            }
        });
        let (side, seen) = mock::serve(vec![(200, "", CONTENT)]);
        let sidecar = sidecar_at(&side);
        let memo = Memo::default();
        let first = fetch::fetch(&format!("{base}/a"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(first.route, fetch::Route::Browser);
        let second = fetch::fetch(&format!("{base}/b"), retry_for(&memo, &PRIVATE, Some(&sidecar)))
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(second.route, fetch::Route::Stealth, "the sticky host's refusal is the sidecar's to read");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

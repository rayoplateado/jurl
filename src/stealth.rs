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
    decide,
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
    /// How long a call may take: [`TIMEOUT`], except in the tests.
    timeout: Duration,
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
        if decide::plain_http_off_loopback(&base) {
            note(timing, "JURL_STEALTH_URL is plain http off this computer: the stealth sidecar is not asked, its token would travel in clear".into());
            return None;
        }
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
            timeout: TIMEOUT,
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
        self.calls.fetch_update(Relaxed, Relaxed, |n| (n < self.max_pages).then_some(n + 1)).is_ok()
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
    let v = value?.trim();
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
    fn a_token_is_not_sent_over_plain_http_off_this_computer() {
        assert!(sidecar(&[("JURL_STEALTH_URL", "http://sidecar.example:8091"), TOKEN]).is_none());
        assert!(sidecar(&[("JURL_STEALTH_URL", "http://127.0.0.1:8091"), TOKEN]).is_some());
        assert!(sidecar(&[("JURL_STEALTH_URL", "https://sidecar.example:8091/"), TOKEN]).is_some());
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
        assert_eq!(country_of(Some(" US ")), Some("US".into()));
        for bad in ["es", "ESP", "E", "E1", ""] {
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
}

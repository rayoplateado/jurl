//! Rung 3 of the read ladder: a residential fallback proxy, used only for a host that refused the direct clients in this run
//! with HTTP 401, 403 or 429 (or from its first request under `JURL_PROXY_FIRST=1`).
//!
//! The credential is `JURL_FALLBACK_PROXY=http://user:pass@host:port`. jurl does not print it: a `-t` line names the host only,
//! and an error from a request through the proxy has the URL and credential taken out of it (see [`Fallback::scrubbed`]).
//! Lightpanda is the one other process that is given the URL, in its arguments (see [`Fallback::lightpanda_args`]). Under
//! `JURL_PUBLIC_ONLY=1` the proxy is allowed, unlike `HTTP(S)_PROXY`: each target is checked public before its request (see
//! `reach::check`), but the proxy resolves the name itself, so that check is not rebinding-proof (see the README).

use std::{
    collections::BTreeSet,
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};

use anyhow::{Result, anyhow, bail};
use reqwest::{Client, StatusCode, redirect::Policy};
use url::Url;

use crate::{
    fetch::{RenderRefused, host_key},
    reach,
};

/// What a credential becomes in an error or a trace.
const REDACTED: &str = "proxy";

/// The proxy a run reads through: its URL, and the one client that goes through it.
struct Proxy {
    /// `JURL_FALLBACK_PROXY`, as given. Never printed.
    url: String,
    /// The client every request through the proxy uses, built once per run and pooled. It resolves only the proxy's own host:
    /// a request's own host is the proxy's to resolve, and `reach::check` checks it before the request is sent.
    client: Client,
}

/// The run's fallback proxy, set up from the settings a run reads (see [`Fallback::configured`]), and the hosts this run has
/// put on it. One per run, not per process: `jurl mcp` serves calls side by side, and a host that one call put on the proxy is
/// not put there for the next. Each fetch reaches it through `fetch::Retry`.
pub(crate) struct Fallback {
    /// The proxy, when `JURL_FALLBACK_PROXY` names one.
    proxy: Option<Proxy>,
    /// `JURL_PROXY_FIRST=1`: every host starts on the proxy.
    first: bool,
    /// The hosts on the proxy in this run, by their memo name (see `fetch::host_key`).
    hosts: Mutex<BTreeSet<String>>,
    /// The decoded body bytes read directly, and through the proxy, and the renders made through it (Lightpanda's, and each
    /// stealth sidecar call's): the usage's `bytes` and `proxied_renders`. Counted per run, so a run's usage is its own.
    bytes_direct: AtomicU64,
    bytes_proxy: AtomicU64,
    renders: AtomicU64,
}

impl Default for Fallback {
    fn default() -> Self {
        Self::new()
    }
}

impl Fallback {
    /// No proxy: every request is direct.
    pub(crate) const fn new() -> Self {
        Fallback {
            proxy: None,
            first: false,
            hosts: Mutex::new(BTreeSet::new()),
            bytes_direct: AtomicU64::new(0),
            bytes_proxy: AtomicU64::new(0),
            renders: AtomicU64::new(0),
        }
    }

    /// The run's fallback, from the settings `get` gives (the environment, then the saved file, as `Config::get` does). A
    /// `JURL_FALLBACK_PROXY` that is not an http or https proxy address is an error, and the error does not show its value.
    pub(crate) fn configured(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let mut fallback = Self::new();
        fallback.first = proxy_first(&get);
        let Some(url) = get("JURL_FALLBACK_PROXY").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
            return Ok(fallback);
        };
        let plain = Url::parse(&url).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
        if !plain {
            bail!("JURL_FALLBACK_PROXY is not an http or https proxy address (http://user:password@host:port)");
        }
        let proxy = reqwest::Proxy::all(url.as_str())
            .map_err(|_| anyhow!("JURL_FALLBACK_PROXY is not a proxy address that jurl can use"))?;
        // `proxy` turns off the environment's proxies, so this is the only one a request goes through. No redirects of its
        // own: `fetch::send_get` follows them, so that each hop is checked.
        let client = crate::client_builder()
            .redirect(Policy::none())
            .proxy(proxy)
            .build()
            .map_err(|_| anyhow!("the fallback proxy's client could not be built"))?;
        fallback.proxy = Some(Proxy { url, client });
        Ok(fallback)
    }

    /// Whether this run has a proxy at all.
    pub(crate) fn has_proxy(&self) -> bool {
        self.proxy.is_some()
    }

    /// Whether a request to `url` goes through the proxy now: its host is on it, or every host is (`JURL_PROXY_FIRST=1`).
    pub(crate) fn uses(&self, url: &Url) -> bool {
        self.proxy.is_some() && (self.first || host_key(url).is_some_and(|host| self.lock().contains(&host)))
    }

    /// The client that goes through the proxy, when this run has one.
    pub(crate) fn client(&self) -> Option<&Client> {
        self.proxy.as_ref().map(|proxy| &proxy.client)
    }

    /// The error of a request, scrubbed of the proxy's URL and credential when the request went through the proxy (see
    /// [`scrub_with`]). A direct request's error is returned as it is, and so is the guard's refusal, which names no address and
    /// is looked for by its type. So is a render's refusal ([`RenderRefused`]): its message is the page's URL, status and
    /// Retry-After, never the proxy's.
    pub(crate) fn scrubbed(&self, via_proxy: bool, err: anyhow::Error) -> anyhow::Error {
        match &self.proxy {
            Some(proxy) if via_proxy && !reach::refused(err.as_ref()) && !err.is::<RenderRefused>() => {
                anyhow!(scrub_with(&proxy.url, &format!("{err:#}")))
            }
            _ => err,
        }
    }

    /// Puts `url`'s host on the proxy for the rest of the run, for `why` (the refusal that switched it, or `JURL_PROXY_FIRST`).
    /// Returns whether the host was not on the proxy already. The first host put there is said on stderr under `timing`.
    pub(crate) fn put_on_proxy(&self, url: &Url, why: &str, timing: bool) -> bool {
        if self.proxy.is_none() {
            return false;
        }
        let Some(host) = host_key(url) else { return false };
        let news = self.lock().insert(host.clone());
        if news && timing {
            eprintln!("{}", switch_note(&host, why));
        }
        news
    }

    /// The hosts on the proxy in this run, in order.
    pub(crate) fn hosts(&self) -> Vec<String> {
        self.lock().iter().cloned().collect()
    }

    fn lock(&self) -> MutexGuard<'_, BTreeSet<String>> {
        self.hosts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts `n` decoded body bytes, read directly or through the proxy. The wire size is not measured: a body is counted as
    /// jurl decodes it, and a refusal's body is not read at all.
    pub(crate) fn count(&self, via_proxy: bool, n: usize) {
        let counter = if via_proxy { &self.bytes_proxy } else { &self.bytes_direct };
        counter.fetch_add(n as u64, Relaxed);
    }

    /// The body bytes counted for the run, read directly or through the proxy.
    pub(crate) fn bytes(&self, via_proxy: bool) -> u64 {
        let counter = if via_proxy { &self.bytes_proxy } else { &self.bytes_direct };
        counter.load(Relaxed)
    }

    /// Counts one render made through the proxy: Lightpanda's, or a stealth sidecar's call through it (see `stealth.rs`).
    pub(crate) fn count_render(&self) {
        self.renders.fetch_add(1, Relaxed);
    }

    /// The renders the run made through the proxy.
    pub(crate) fn renders(&self) -> u64 {
        self.renders.load(Relaxed)
    }

    /// The Lightpanda arguments that render `url` through the proxy, or none when `url` is direct.
    pub(crate) fn lightpanda_args(&self, url: &Url) -> Vec<String> {
        match &self.proxy {
            Some(proxy) if self.uses(url) => vec!["--http-proxy".to_string(), proxy.url.clone()],
            _ => Vec::new(),
        }
    }

    /// A run's fallback with the proxy and `JURL_PROXY_FIRST` given, for tests.
    #[cfg(test)]
    pub(crate) fn with(proxy: Option<&str>, first: bool) -> Self {
        let first = if first { "1" } else { "0" };
        Self::configured(|key| match key {
            "JURL_FALLBACK_PROXY" => proxy.map(String::from),
            "JURL_PROXY_FIRST" => Some(first.to_string()),
            _ => None,
        })
        .expect("a valid test proxy")
    }
}

/// `text` with the proxy's URL taken out, and its credential taken out in the forms an error can show it: the userinfo as the
/// URL spells it and as it decodes, as `user:password@` and as `user:password`. With no password, only `user@`. A bare user or
/// password is not replaced: a short one would mangle unrelated text, and no message from reqwest shows one alone.
pub(crate) fn scrub_with(proxy_url: &str, text: &str) -> String {
    let mut out = text.replace(proxy_url, REDACTED);
    let Ok(parsed) = Url::parse(proxy_url) else { return out };
    let raw = (parsed.username().to_string(), parsed.password().unwrap_or_default().to_string());
    let decoded = (percent_decoded(&raw.0), percent_decoded(&raw.1));
    let mut forms = Vec::new();
    for (user, pass) in [raw, decoded] {
        if pass.is_empty() {
            forms.push(format!("{user}@"));
        } else {
            forms.push(format!("{user}:{pass}@"));
            forms.push(format!("{user}:{pass}"));
        }
    }
    for form in forms.iter().filter(|form| form.len() > 1) {
        out = out.replace(form.as_str(), REDACTED);
    }
    out
}

/// `raw` with its `%XX` escapes decoded, as reqwest decodes a proxy's credential. A `+` stays a `+`.
fn percent_decoded(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `JURL_PROXY_FIRST=1`: every host starts on the proxy. Without it the proxy is a fallback after a refusal.
fn proxy_first(get: &impl Fn(&str) -> Option<String>) -> bool {
    get("JURL_PROXY_FIRST").is_some_and(|v| v.trim() == "1")
}

/// Whether a refusal with `status` puts a host on the proxy. Decided on the status alone (the project's no-ad-hoc-heuristics
/// rule).
pub(crate) fn refused_enough(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS)
}

/// The trace line that says a host is on the proxy. It names the host only, never the URL.
fn switch_note(host: &str, why: &str) -> String {
    format!("jurl: {host}: using the proxy ({why})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a test URL")
    }

    #[test]
    fn a_host_is_direct_until_it_is_put_on_the_proxy() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), false);
        let page = url("https://shop.example/page");
        assert!(!fb.uses(&page));
        assert!(fb.put_on_proxy(&page, "403 from direct", false), "the first switch is news");
        assert!(fb.uses(&page));
        assert!(!fb.put_on_proxy(&page, "403 from direct", false), "the second switch is not news");
    }

    #[test]
    fn proxy_first_puts_every_host_on_the_proxy() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), true);
        assert!(fb.uses(&url("https://a.example/")));
        assert!(fb.uses(&url("https://b.example/")));
    }

    #[test]
    fn without_a_proxy_nothing_uses_it_and_nothing_is_put_on_it() {
        let fb = Fallback::with(None, true);
        assert!(!fb.has_proxy());
        assert!(!fb.uses(&url("https://a.example/")));
        assert!(!fb.put_on_proxy(&url("https://a.example/"), "403 from direct", false));
    }

    #[test]
    fn a_bad_proxy_address_is_an_error_that_does_not_show_it() {
        let err = Fallback::configured(|key| {
            (key == "JURL_FALLBACK_PROXY").then(|| "socks5://user:s3cret@proxy.test".into())
        })
        .err()
        .expect("a SOCKS address is refused");
        assert!(!format!("{err:#}").contains("s3cret"), "{err:#}");
    }

    #[test]
    fn the_switch_note_names_the_host_and_never_the_url() {
        assert_eq!(
            switch_note("shop.example", "403 from direct and browser"),
            "jurl: shop.example: using the proxy (403 from direct and browser)"
        );
    }

    #[test]
    fn statuses_that_switch_are_401_403_429() {
        assert!(refused_enough(StatusCode::UNAUTHORIZED));
        assert!(refused_enough(StatusCode::FORBIDDEN));
        assert!(refused_enough(StatusCode::TOO_MANY_REQUESTS));
        assert!(!refused_enough(StatusCode::NOT_FOUND));
        assert!(!refused_enough(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!refused_enough(StatusCode::OK));
    }

    #[test]
    fn the_proxy_url_and_its_userinfo_are_scrubbed_raw_and_decoded_but_no_bare_user() {
        let proxy = "http://jurl%40user:p%40ss@proxy.test:8080";
        let text = "connect to http://jurl%40user:p%40ss@proxy.test:8080 failed; jurl@user:p@ss@proxy.test, jurl%40user:p%40ss, \
                    and a user agent";
        let clean = scrub_with(proxy, text);
        for secret in ["jurl%40user", "p%40ss", "jurl@user", "p@ss"] {
            assert!(!clean.contains(secret), "{secret} in {clean}");
        }
        assert!(clean.contains("a user agent"), "a bare word is not mangled: {clean}");
        assert!(clean.contains("proxy.test"), "the host is not a credential: {clean}");
    }

    #[test]
    fn a_direct_error_is_kept_as_it_is_and_a_refusal_through_the_proxy_is_not_scrubbed() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), false);
        let direct = fb.scrubbed(false, anyhow!("via http://user:s3cret@proxy.test:8080"));
        assert!(format!("{direct:#}").contains("s3cret"), "a direct error is not the proxy's to scrub");
        let refused = fb.scrubbed(true, reach::NotPublic.into());
        assert!(refused.downcast_ref::<reach::NotPublic>().is_some(), "the guard's refusal keeps its type");
        let proxied = fb.scrubbed(true, anyhow!("via http://user:s3cret@proxy.test:8080"));
        assert!(!format!("{proxied:#}").contains("s3cret"), "{proxied:#}");
    }

    #[test]
    fn a_render_refusal_keeps_its_type_through_the_proxy_scrub() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), false);
        let refused =
            RenderRefused { url: "https://shop.example/p".into(), status: StatusCode::FORBIDDEN, retry_after: None };
        let kept = fb.scrubbed(true, refused.into());
        assert!(kept.downcast_ref::<RenderRefused>().is_some(), "{kept:#}");
    }

    #[test]
    fn lightpanda_gets_the_proxy_url_for_a_proxied_host_only() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), false);
        let page = url("https://shop.example/");
        assert!(fb.lightpanda_args(&page).is_empty());
        fb.put_on_proxy(&page, "403 from direct", false);
        assert_eq!(fb.lightpanda_args(&page), ["--http-proxy", "http://user:s3cret@proxy.test:8080"]);
    }
}

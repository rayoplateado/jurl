//! Rung 3 of the read ladder: a residential fallback proxy, used only for a host that refused the direct clients in this run
//! with HTTP 202, 401, 403 or 429, or after the server reset its stream (or from its first request under `JURL_PROXY_FIRST=1`).
//!
//! The credential is `JURL_FALLBACK_PROXY=http://user:pass@host:port`. jurl does not print it: a `-t` line names the host only,
//! and an error from a request through the proxy has the URL and credential taken out of it (see [`Fallback::scrubbed`]).
//! Lightpanda and the run's client are given a loopback address instead of the proxy's (see `meter.rs`): a forwarder relays
//! every byte to the proxy unchanged and counts it, so the usage's `bytes.proxy_wire` is what crosses the proxy. The URL
//! Lightpanda is given keeps the credential in its arguments (see [`Fallback::render_meter`]). Under
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
    fetch::{RenderRefused, ServerReset, host_key},
    meter::{Meter, Wire},
    reach,
};

/// What a credential becomes in an error or a trace.
const REDACTED: &str = "proxy";

/// The proxy a run reads through: its address, and the one client that goes through it, which the run's meter carries.
struct Proxy {
    /// `JURL_FALLBACK_PROXY`, as given. Never printed.
    url: String,
    /// The same address, parsed: a render's meter is started on it.
    upstream: Url,
    /// The run's own meter, which the client's connections go through: plain reads, robots files, sitemaps and images.
    meter: Meter,
    /// The client every request through the proxy uses, built once per run and pooled. It resolves only the proxy's own host:
    /// a request's own host is the proxy's to resolve, and `reach::check` checks it before the request is sent. It is given the
    /// meter's loopback URL, so its bytes are counted.
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
    /// stealth sidecar call's): the usage's `bytes.direct`, `bytes.proxy` and `proxied_renders`. Counted per run, so a run's usage
    /// is its own. The wire bytes through the proxy are the meters' (see `meter.rs`): the run's client's, and each render's,
    /// which `render_up` and `render_down` add up.
    bytes_direct: AtomicU64,
    bytes_proxy: AtomicU64,
    renders: AtomicU64,
    render_up: AtomicU64,
    render_down: AtomicU64,
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
            render_up: AtomicU64::new(0),
            render_down: AtomicU64::new(0),
        }
    }

    /// The run's fallback, from the settings `get` gives (the environment, then the saved file, as `Config::get` does). A
    /// `JURL_FALLBACK_PROXY` that is not an `http://` proxy address is an error, and the error does not show its value. So is
    /// one that https would need: the meter speaks plain TCP to the proxy, so a TLS proxy would be read uncounted.
    pub(crate) fn configured(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let mut fallback = Self::new();
        fallback.first = proxy_first(&get);
        let Some(url) = get("JURL_FALLBACK_PROXY").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
            return Ok(fallback);
        };
        let upstream = Url::parse(&url).ok().filter(|u| u.scheme() == "http" && u.host_str().is_some());
        let Some(upstream) = upstream else {
            bail!("JURL_FALLBACK_PROXY is not an http proxy address (http://user:password@host:port)");
        };
        let meter = Meter::start(&upstream)?;
        // The client is given the meter's loopback address, which keeps the proxy's credential: `proxy` turns off the
        // environment's proxies, so this is the only one a request goes through. No redirects of its own: `fetch::send_get`
        // follows them, so that each hop is checked.
        let proxy = reqwest::Proxy::all(meter.url())
            .map_err(|_| anyhow!("JURL_FALLBACK_PROXY is not a proxy address that jurl can use"))?;
        let client = crate::client_builder()
            .redirect(Policy::none())
            .proxy(proxy)
            .build()
            .map_err(|_| anyhow!("the fallback proxy's client could not be built"))?;
        fallback.proxy = Some(Proxy { url, upstream, meter, client });
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
    /// Retry-After, never the proxy's. So is a reset of the stream ([`ServerReset`]), whose message is its reason alone.
    pub(crate) fn scrubbed(&self, via_proxy: bool, err: anyhow::Error) -> anyhow::Error {
        self.scrubbed_urls(via_proxy, err, &[])
    }

    /// [`Fallback::scrubbed`], with each of `also` taken out too: a render's own loopback URL, which carries the same credential.
    pub(crate) fn scrubbed_urls(&self, via_proxy: bool, err: anyhow::Error, also: &[String]) -> anyhow::Error {
        match &self.proxy {
            Some(proxy)
                if via_proxy
                    && !reach::refused(err.as_ref())
                    && !err.is::<RenderRefused>()
                    && !err.is::<ServerReset>() =>
            {
                let text =
                    also.iter().fold(scrub_with(&proxy.url, &format!("{err:#}")), |text, url| scrub_with(url, &text));
                anyhow!(text)
            }
            _ => err,
        }
    }

    /// The meter a render of `url` goes through, when the run sends the render through the proxy (see `fetch::render`). Each
    /// render has a meter of its own, so its bytes are its own. None for a direct render.
    pub(crate) fn render_meter(&self, url: &Url) -> Result<Option<Meter>> {
        match &self.proxy {
            Some(proxy) if self.uses(url) => Meter::start(&proxy.upstream).map(Some),
            _ => Ok(None),
        }
    }

    /// Counts the wire bytes of one render through the proxy, or of one stealth sidecar load, as its meter (or the sidecar) counted them.
    pub(crate) fn count_render_wire(&self, wire: Wire) {
        self.render_up.fetch_add(wire.up, Relaxed);
        self.render_down.fetch_add(wire.down, Relaxed);
    }

    /// The wire bytes of the renders the run made through the proxy: Lightpanda's, and the stealth sidecar's loads.
    pub(crate) fn render_wire(&self) -> Wire {
        Wire { up: self.render_up.load(Relaxed), down: self.render_down.load(Relaxed) }
    }

    /// The wire bytes the run sent through the proxy, both directions: the run's own client's, and every render's.
    pub(crate) fn proxy_wire(&self) -> Wire {
        let own = self.proxy.as_ref().map(|proxy| proxy.meter.wire()).unwrap_or_default();
        own.plus(self.render_wire())
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

    /// Counts `n` decoded body bytes, read directly or through the proxy. This is not the wire size: a body is counted as jurl
    /// decodes it, and a refusal's body is not read at all. The wire size through the proxy is `proxy_wire` (see `meter.rs`).
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
/// rule). A 202 Accepted is included: it is no page (RFC 9110 §15.3.3), so it is refused as a 403 is.
pub(crate) fn refused_enough(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::ACCEPTED | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
    )
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
    fn statuses_that_switch_are_202_401_403_429() {
        assert!(refused_enough(StatusCode::ACCEPTED));
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
    fn a_stream_reset_keeps_its_type_through_the_proxy_scrub() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), false);
        let reset = ServerReset { reason: h2::Reason::INTERNAL_ERROR, via_proxy: true };
        let kept = fb.scrubbed(true, reset.into());
        assert!(kept.downcast_ref::<ServerReset>().is_some(), "{kept:#}");
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
    fn a_render_gets_a_meter_for_a_proxied_host_only() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), false);
        let page = url("https://shop.example/");
        assert!(fb.render_meter(&page).expect("no error").is_none());
        fb.put_on_proxy(&page, "403 from direct", false);
        let meter = fb.render_meter(&page).expect("a meter").expect("the render goes through the proxy");
        let given = meter.url();
        assert!(given.starts_with("http://user:s3cret@127.0.0.1:"), "{given}");
        assert!(!given.contains("proxy.test"), "the proxy's host is not given to the render: {given}");
    }

    #[test]
    fn a_proxy_that_is_not_an_http_address_is_refused_without_showing_it() {
        for proxy in ["https://user:s3cret@proxy.test", "socks5://user:s3cret@proxy.test"] {
            let err = Fallback::configured(|key| (key == "JURL_FALLBACK_PROXY").then(|| proxy.into()))
                .err()
                .expect("refused");
            let text = format!("{err:#}");
            assert!(text.contains("http proxy address"), "{text}");
            assert!(!text.contains("s3cret"), "{text}");
        }
    }

    #[test]
    fn the_run_counts_its_renders_wire_bytes_on_top_of_its_own_clients() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), false);
        assert_eq!(fb.proxy_wire(), Wire::default());
        fb.count_render_wire(Wire { up: 10, down: 20 });
        fb.count_render_wire(Wire { up: 1, down: 2 });
        assert_eq!(fb.render_wire(), Wire { up: 11, down: 22 });
        assert_eq!(fb.proxy_wire(), Wire { up: 11, down: 22 }, "no client of the run has sent anything yet");
        let direct = Fallback::default();
        assert_eq!(direct.proxy_wire(), Wire::default(), "a run with no proxy has no proxy bytes");
    }

    #[test]
    fn a_render_error_scrubs_its_own_loopback_url_as_well() {
        let fb = Fallback::with(Some("http://user:s3cret@proxy.test:8080"), false);
        let err = anyhow!(
            "ran with --http-proxy http://user:s3cret@127.0.0.1:5555 and via http://user:s3cret@proxy.test:8080"
        );
        let text = format!("{:#}", fb.scrubbed_urls(true, err, &["http://user:s3cret@127.0.0.1:5555".to_string()]));
        assert!(!text.contains("s3cret"), "{text}");
        assert!(text.contains("proxy"), "{text}");
    }
}

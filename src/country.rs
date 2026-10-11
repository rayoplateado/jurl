//! The country a run reads as (`JURL_COUNTRY`), and the start page read again through the fallback proxy when the site sent its
//! direct read to a page of another country. The country puts a site's pages of its region first (see `follow.rs`), and it is the
//! stealth sidecar's country too (see `stealth.rs`).
//!
//! The re-read is rung 3 for the start page: the host goes on the proxy, as a refused host does (see `fallback.rs`), and the
//! start page is read through it. Only the start page: the pages after it carry the country in their paths. The decisions are
//! pure; the read is [`reread`].

use url::Url;

use crate::{
    cli::Args,
    fetch::{self, Page, Retry, Route},
    langtag, stealth,
    timing::Timer,
};

/// A setting's country: two uppercase letters that the registry lists as an ISO 3166-1 alpha-2 code (see [`stealth::country_of`]
/// and [`langtag::is_country_code`]). None for anything else.
fn country_of_setting(value: Option<&str>) -> Option<String> {
    stealth::country_of(value).filter(|code| langtag::is_country_code(code))
}

/// The country a run reads as, from the settings `get` gives: `JURL_COUNTRY`, else `JURL_STEALTH_COUNTRY`, each when it is an
/// ISO 3166-1 alpha-2 code (see [`country_of_setting`]). None when neither is.
pub(crate) fn setting(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let valid = |key: &str| country_of_setting(get(key).filter(|v| !v.is_empty()).as_deref());
    valid("JURL_COUNTRY").or_else(|| valid("JURL_STEALTH_COUNTRY"))
}

/// [`setting`] for a run. Under `timing`, says on stderr each of the two settings that is set and is not a country code, which
/// is not used.
pub(crate) fn configured(get: impl Fn(&str) -> Option<String>, timing: bool) -> Option<String> {
    if timing {
        for key in ["JURL_COUNTRY", "JURL_STEALTH_COUNTRY"] {
            let set = get(key).filter(|v| !v.is_empty());
            if set.is_some() && country_of_setting(set.as_deref()).is_none() {
                eprintln!("jurl: {key} is not an ISO 3166-1 country code (two capital letters), so it is not used");
            }
        }
    }
    setting(get)
}

/// Whether the URL declares a country other than `country` (two uppercase letters). Its path names a region (see
/// [`langtag::path_locale`]: `/at/de/`, `/es-ar/`, `/at/`), and a region other than the country's is another country. Or the
/// path names a language alone (`/de/`) that is not the run's own language (`run_language`, the primary subtag of its
/// Accept-Language; with none, every language is another), and whose likely region (CLDR, see [`langtag::likely_region`]) is
/// not the country's. A path that names neither declares no country.
pub(crate) fn declares_other_country(country: &str, url: &Url, run_language: Option<&str>) -> bool {
    let locale = langtag::path_locale(url);
    if let Some(region) = locale.region {
        return !region.eq_ignore_ascii_case(country);
    }
    let Some(language) = locale.language else { return false };
    if run_language.is_some_and(|run| run.eq_ignore_ascii_case(&language)) {
        return false;
    }
    langtag::likely_region(&language).is_some_and(|region| !region.eq_ignore_ascii_case(country))
}

/// Whether the start page is read again through the fallback proxy (see [`reread`]): the run names a country and has a
/// fallback proxy, the direct read (on the plain or the browser client, not through the proxy or the sidecar) went to a path
/// the URL did not ask for, and the page it went to declares another country (see [`declares_other_country`]). A URL that
/// asked for a country's page is not read again: the site sent nothing.
pub(crate) fn reread_through_proxy(
    country: Option<&str>,
    has_proxy: bool,
    route: Route,
    asked: &Url,
    page: &Url,
    run_language: Option<&str>,
) -> bool {
    let Some(country) = country else { return false };
    has_proxy
        && matches!(route, Route::Direct | Route::Browser)
        && page.path() != asked.path()
        && declares_other_country(country, page, run_language)
}

/// The start page's direct read `page` (of `asked`), read again through the run's fallback proxy when it is another country's
/// (see [`reread_through_proxy`]). The proxy's read is the start page the run goes on from, and its host is on the proxy from then
/// on, as a host put there is (see `fallback::Fallback::put_on_proxy`). A proxy read that fails leaves the direct read, which was
/// a page, and `-t` says so. Under `-t`, one line says the site sent the run elsewhere and the run reads as the country.
pub(crate) async fn reread(args: &Args, retry: Retry<'_>, asked: &Url, page: Page, t: &mut Timer) -> Page {
    let Some(country) = args.country.as_deref() else { return page };
    let language = stealth::lang_of(fetch::accept_language().as_deref());
    let run_language = language.as_deref().map(langtag::primary);
    if !reread_through_proxy(Some(country), retry.fallback.has_proxy(), page.route, asked, &page.url, run_language) {
        return page;
    }
    let host = asked.host_str().unwrap_or_default();
    if args.timing {
        eprintln!("jurl: {host} sent us to {}; reading it as from {country}", page.url.path());
    }
    retry.fallback.put_on_proxy(asked, &format!("as from {country}"), false);
    match fetch::fetch(asked.as_str(), retry).await {
        Ok(read) => {
            t.lap("fetch through the proxy");
            read
        }
        Err(e) => {
            if args.timing {
                eprintln!("jurl: {host} could not be read as from {country} ({e:#}); reading it as the site sent it");
            }
            page
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{
        extract::Extracted,
        fallback::Fallback,
        fetch::{Served, test_server, test_server::reply, test_server::serve_routed},
    };

    fn url(s: &str) -> Url {
        Url::parse(s).expect("a test URL")
    }

    /// The settings a test gives, as the environment would.
    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_run_reads_as_jurl_country_or_the_stealth_one_when_it_is_two_uppercase_letters() {
        assert_eq!(setting(env(&[("JURL_COUNTRY", "ES"), ("JURL_STEALTH_COUNTRY", "AT")])).as_deref(), Some("ES"));
        assert_eq!(setting(env(&[("JURL_STEALTH_COUNTRY", "AT")])).as_deref(), Some("AT"), "the legacy setting works");
        assert_eq!(setting(env(&[("JURL_COUNTRY", "es"), ("JURL_STEALTH_COUNTRY", "AT")])).as_deref(), Some("AT"));
        let unset: [&'static [(&'static str, &'static str)]; 5] = [
            &[("JURL_COUNTRY", "ESP")],
            &[("JURL_COUNTRY", "")],
            &[("JURL_COUNTRY", "XX")],
            &[("JURL_COUNTRY", "QQ")],
            &[],
        ];
        for pairs in unset {
            assert_eq!(setting(env(pairs)), None, "{pairs:?}");
        }
    }

    #[test]
    fn a_declared_region_other_than_the_countrys_is_another_country() {
        // `/at/de` is Austria in German: a region, and not Spain.
        assert!(declares_other_country("ES", &url("https://x.com/at/de/"), Some("en")));
        assert!(declares_other_country("ES", &url("https://x.com/es-ar/aea"), Some("en")));
        assert!(declares_other_country("ES", &url("https://x.com/at/flights"), Some("en")), "a region alone");
        assert!(
            declares_other_country("ES", &url("https://x.com/us/"), None),
            "a region alone, with no language asked"
        );
        assert!(!declares_other_country("ES", &url("https://x.com/es-es/pricing"), Some("en")));
        assert!(
            !declares_other_country("ES", &url("https://x.com/es/pricing"), Some("en")),
            "a language, not a region"
        );
        assert!(
            !declares_other_country("ES", &url("https://x.com/ar/es/aea"), Some("en")),
            "ambiguous: no region claimed"
        );
    }

    #[test]
    fn a_language_alone_declares_the_country_its_likely_region_is() {
        // Iberia's `/de/`, read as from Spain in English: German is Germany's, not Spain's.
        assert!(declares_other_country("ES", &url("https://x.com/de/"), Some("en")));
        assert!(!declares_other_country("ES", &url("https://x.com/es/"), Some("en")), "Spanish is Spain's");
        assert!(
            !declares_other_country("ES", &url("https://x.com/de/"), Some("de")),
            "a page in the run's own language declares no country"
        );
        assert!(
            declares_other_country("ES", &url("https://x.com/de/"), None),
            "no language asked: any language is another"
        );
        assert!(!declares_other_country("ES", &url("https://x.com/"), Some("en")));
        assert!(!declares_other_country("ES", &url("https://x.com/pricing"), Some("en")));
    }

    #[test]
    fn the_start_page_is_read_again_only_when_the_run_has_a_proxy_and_the_site_sent_it_elsewhere() {
        let asked = url("https://x.com/");
        let at = url("https://x.com/at/de");
        let again =
            |country, has_proxy, route| reread_through_proxy(country, has_proxy, route, &asked, &at, Some("en"));
        assert!(again(Some("ES"), true, Route::Direct));
        assert!(again(Some("ES"), true, Route::Browser), "a browser read is a direct read too");
        assert!(!again(None, true, Route::Direct), "no country named");
        assert!(!again(Some("ES"), false, Route::Direct), "no proxy");
        assert!(!again(Some("ES"), true, Route::Proxy), "already read through the proxy");
        assert!(!again(Some("ES"), true, Route::Stealth), "read by the sidecar");
        assert!(
            !reread_through_proxy(Some("ES"), true, Route::Direct, &at, &at, Some("en")),
            "a URL that asked for the other country's page is not read again"
        );
        assert!(
            !reread_through_proxy(Some("ES"), true, Route::Direct, &asked, &asked, Some("en")),
            "no redirect, no re-read"
        );
    }

    /// The proxy URL a test's run is given: the test server's address, with a credential in it.
    fn proxy_url(base: &str) -> String {
        base.replacen("http://", "http://user:s3cret@", 1)
    }

    /// A run's start page (or a page a search follows, with `start` false) read as from `country`, through the fallback proxy at
    /// `proxy` (none when None), with `first` every host on the proxy from its first request (`JURL_PROXY_FIRST`). Also returns the
    /// hosts the run put on the proxy (the usage's `proxied_hosts`).
    async fn read_page_as(
        page: &str,
        country: Option<&str>,
        proxy: Option<&str>,
        first: bool,
        start: bool,
    ) -> anyhow::Result<(Url, Extracted, Served, Vec<String>)> {
        let mut args = crate::cli::Args::parse_from(["jurl", page]);
        args.country = country.map(str::to_string);
        args.fallback = Fallback::with(proxy.map(proxy_url).as_deref(), first);
        let target = Url::parse(page).expect("a URL");
        let mut t = crate::timing::Timer::new();
        let config = crate::config::Config::default();
        let read = if start {
            crate::load_start(&args, &config, &target, &mut t).await
        } else {
            crate::load(&args, &config, &target, &mut t).await
        };
        read.map(|(url, ex, served)| (url, ex, served, args.fallback.hosts()))
    }

    const AT_PAGE: &str =
        "<html lang=\"de-AT\"><body><h1>Preise</h1><p>Handgepäck: 10 kg. Aufgabe: 23 kg Gepäck.</p></body></html>";
    const ES_PAGE: &str =
        "<html lang=\"es-ES\"><body><h1>Precios</h1><p>Equipaje de mano: 10 kg. Facturado: 23 kg.</p></body></html>";

    /// A site as the test sends it: `/` redirects to the Austrian page, which is German. Returns its base address and its requests.
    fn site_sending_to_austria() -> (String, test_server::Served) {
        serve_routed(|head| {
            let path = head.split_whitespace().nth(1).unwrap_or_default();
            Some(match path {
                "/" => reply("302 Found", "Location: /at/de\r\n", b""),
                "/at/de" => reply("200 OK", "Content-Type: text/html\r\n", AT_PAGE.as_bytes()),
                _ => reply("404 Not Found", "", b"no such page"),
            })
        })
    }

    /// A proxy that answers every request with the Spanish page. Returns its base address and its requests.
    fn proxy_in_spain() -> (String, test_server::Served) {
        serve_routed(|_| Some(reply("200 OK", "Content-Type: text/html\r\n", ES_PAGE.as_bytes())))
    }

    fn text_of(ex: &Extracted) -> String {
        ex.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    #[tokio::test]
    async fn a_start_page_the_site_sends_to_another_country_is_read_through_the_proxy_as_from_the_run_s_country() {
        let (site, site_served) = site_sending_to_austria();
        let (proxy, proxy_served) = proxy_in_spain();
        let (url, ex, served, hosts) = read_page_as(&format!("{site}/"), Some("ES"), Some(&proxy), false, true)
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(served.name(), "proxy", "the start page is the proxy's read");
        assert!(text_of(&ex).contains("Equipaje de mano"), "the Spanish page is what is read: {}", text_of(&ex));
        assert_eq!(url.path(), "/", "the page is the one the run asked for");
        let site_host = crate::fetch::host_key(&Url::parse(&site).expect("a URL")).expect("a host");
        assert_eq!(hosts, [site_host], "the site's host is on the proxy, as the usage's proxied_hosts says");
        assert_eq!(
            site_served.count(),
            2,
            "the site is asked twice directly: `/`, then the Austrian page it sent us to"
        );
        let asked = proxy_served.heads();
        assert_eq!(asked.len(), 1, "the proxy is asked once");
        assert!(
            asked[0].contains(&format!("GET {site}/ HTTP/1.1")),
            "the proxy is asked for the URL the run asked: {}",
            asked[0]
        );
    }

    #[tokio::test]
    async fn without_a_country_the_austrian_read_stands_whatever_the_proxy() {
        let (site, site_served) = site_sending_to_austria();
        let (proxy, proxy_served) = proxy_in_spain();
        let (_, ex, served, _) = read_page_as(&format!("{site}/"), None, Some(&proxy), false, true)
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(served.name(), "direct", "a run that names no country reads the page the site gives");
        assert!(text_of(&ex).contains("Handgepäck"), "{}", text_of(&ex));
        assert_eq!(site_served.count(), 2);
        assert_eq!(proxy_served.count(), 0);
    }

    #[tokio::test]
    async fn a_page_a_search_follows_is_not_read_again() {
        // `load` reads the pages a search follows: the country is the start page's to be read as.
        let (site, site_served) = site_sending_to_austria();
        let (proxy, proxy_served) = proxy_in_spain();
        let (_, ex, served, _) = read_page_as(&format!("{site}/"), Some("ES"), Some(&proxy), false, false)
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(served.name(), "direct");
        assert!(text_of(&ex).contains("Handgepäck"));
        assert_eq!(site_served.count(), 2);
        assert_eq!(proxy_served.count(), 0);
    }

    #[tokio::test]
    async fn a_url_that_asks_for_the_other_countrys_page_is_read_as_it_is() {
        let (site, site_served) = site_sending_to_austria();
        let (proxy, proxy_served) = proxy_in_spain();
        let (_, ex, served, _) = read_page_as(&format!("{site}/at/de"), Some("ES"), Some(&proxy), false, true)
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(served.name(), "direct");
        assert!(text_of(&ex).contains("Handgepäck"));
        assert_eq!(site_served.count(), 1, "the page is asked once");
        assert_eq!(proxy_served.count(), 0);
    }

    #[tokio::test]
    async fn a_host_on_the_proxy_from_its_first_request_is_read_through_it_once() {
        let (site, site_served) = site_sending_to_austria();
        let (proxy, proxy_served) = proxy_in_spain();
        let (_, _, served, _) = read_page_as(&format!("{site}/"), Some("ES"), Some(&proxy), true, true)
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(served.name(), "proxy");
        assert_eq!(proxy_served.count(), 1, "one read through the proxy, not two");
        assert_eq!(site_served.count(), 0, "the site is not asked directly");
    }

    #[tokio::test]
    async fn a_proxy_read_that_is_refused_leaves_the_direct_page_standing() {
        let (site, site_served) = site_sending_to_austria();
        let (proxy, proxy_served) = serve_routed(|_| Some(reply("403 Forbidden", "", b"denied")));
        let (_, ex, served, _) = read_page_as(&format!("{site}/"), Some("ES"), Some(&proxy), false, true)
            .await
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(served.name(), "direct", "the page the site gave is the run's");
        assert!(text_of(&ex).contains("Handgepäck"));
        assert_eq!(site_served.count(), 2);
        assert_eq!(proxy_served.count(), 1);
    }
}

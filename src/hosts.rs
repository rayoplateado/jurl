//! Hosts and registrable domains. The Public Suffix List (publicsuffix.org, compiled into the `psl` crate) says where a
//! registrable domain starts: `docs.stripe.com` and `stripe.com` share `stripe.com`; `a.github.io` and `b.github.io` do not.
//! Also the URLs a text writes out in full, which a page links to as much as an `<a href>` does.

use url::Url;

/// The most hosts whose `llms.txt` is read beside a front door's: each costs one request, and a front door links a few.
pub const LINKED_HOSTS: usize = 3;

/// The registrable domain of `host` (its public suffix plus one label), or None for a host with no known suffix (an IP
/// address, `localhost`).
pub fn registrable(host: &str) -> Option<&str> {
    // The list has no say over an address: `127.0.0.1` would read as the domain `0.1`.
    if host.starts_with('[') || host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    psl::domain_str(host)
}

/// Whether two URLs' hosts share a registrable domain. Hosts with none (an address, `localhost`) share one only when they
/// are the same host.
pub fn same_registrable(a: &Url, b: &Url) -> bool {
    match (a.host_str(), b.host_str()) {
        (Some(x), Some(y)) => match (registrable(x), registrable(y)) {
            (Some(p), Some(q)) => p == q,
            _ => x == y,
        },
        _ => false,
    }
}

/// The origins (scheme, host and port, as `/`) of the hosts among `urls` that share `start`'s registrable domain but are
/// not `start`'s own host: each host once, in the order the links name them, at most `max`. The links are the page's own.
pub fn linked_hosts<'a>(start: &Url, urls: impl IntoIterator<Item = &'a Url>, max: usize) -> Vec<Url> {
    let mut hosts: Vec<Url> = Vec::new();
    for u in urls {
        if !matches!(u.scheme(), "http" | "https") || u.host_str() == start.host_str() || !same_registrable(start, u) {
            continue;
        }
        if hosts.iter().any(|h| h.host_str() == u.host_str()) {
            continue;
        }
        let mut origin = u.clone();
        origin.set_path("/");
        origin.set_query(None);
        origin.set_fragment(None);
        hosts.push(origin);
        if hosts.len() == max {
            break;
        }
    }
    hosts
}

/// The URLs a text writes out in full: each word that starts with `http://` or `https://` and parses as a URL, without the
/// punctuation a sentence puts after one (a full stop, a comma, a closing bracket the URL doesn't open). Nothing is made up.
pub fn bare_urls(text: &str) -> Vec<Url> {
    text.split_whitespace()
        .filter_map(|word| {
            // From the scheme on: a bracket or quote before it is not part of the URL.
            let at = ["https://", "http://"].iter().filter_map(|scheme| word.find(scheme)).min()?;
            Url::parse(trim_sentence(&word[at..])).ok()
        })
        .collect()
}

/// `word` without the punctuation that ends a sentence, and a closing bracket its URL does not open.
fn trim_sentence(word: &str) -> &str {
    let mut w = word;
    loop {
        w = match w.chars().last() {
            Some('.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '»' | '”' | '’') => &w[..w.len() - 1],
            Some(')') if w.matches(')').count() > w.matches('(').count() => &w[..w.len() - 1],
            _ => return w,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).expect("a URL")
    }

    #[test]
    fn a_registrable_domain_is_the_public_suffix_plus_one_label() {
        assert_eq!(registrable("docs.stripe.com"), Some("stripe.com"));
        assert_eq!(registrable("stripe.com"), Some("stripe.com"));
        assert_eq!(registrable("shopify.dev"), Some("shopify.dev"));
        assert_eq!(registrable("docs.gitlab.com"), Some("gitlab.com"));
        assert_eq!(registrable("www.co.uk.example.co.uk"), Some("example.co.uk"));
        // Private suffixes: each GitHub Pages site is its own registrable domain.
        assert_eq!(registrable("a.github.io"), Some("a.github.io"));
        assert_eq!(registrable("127.0.0.1"), None);
        assert_eq!(registrable("localhost"), None);
    }

    #[test]
    fn hosts_share_a_registrable_domain_or_they_do_not() {
        assert!(same_registrable(&u("https://stripe.com/"), &u("https://docs.stripe.com/rate-limits")));
        assert!(!same_registrable(&u("https://shopify.com/"), &u("https://shopify.dev/docs")));
        assert!(!same_registrable(&u("https://a.github.io/"), &u("https://b.github.io/")));
        assert!(same_registrable(&u("http://127.0.0.1:8080/x"), &u("http://127.0.0.1:9090/y")));
        assert!(!same_registrable(&u("http://127.0.0.1/"), &u("https://example.com/")));
    }

    #[test]
    fn the_linked_hosts_are_the_same_registrable_domain_each_once() {
        let start = u("https://stripe.com/");
        let links = [
            u("https://stripe.com/docs"),
            u("https://docs.stripe.com/rate-limits"),
            u("https://docs.stripe.com/api"),
            u("https://twitter.com/stripe"),
            u("https://dashboard.stripe.com/login"),
            u("mailto:hi@stripe.com"),
            u("https://status.stripe.com/x?y=1#z"),
        ];
        let hosts = linked_hosts(&start, links.iter(), LINKED_HOSTS);
        let shown: Vec<String> = hosts.iter().map(Url::to_string).collect();
        assert_eq!(shown, ["https://docs.stripe.com/", "https://dashboard.stripe.com/", "https://status.stripe.com/"]);
    }

    #[test]
    fn the_linked_hosts_are_capped() {
        let start = u("https://x.com/");
        let links = [u("https://a.x.com/"), u("https://b.x.com/"), u("https://c.x.com/"), u("https://d.x.com/")];
        assert_eq!(linked_hosts(&start, links.iter(), 2).len(), 2);
        assert_eq!(linked_hosts(&start, links.iter(), LINKED_HOSTS).len(), 3);
    }

    #[test]
    fn a_host_with_no_registrable_domain_is_never_a_linked_host() {
        let start = u("http://127.0.0.1:8000/");
        let links = [u("http://127.0.0.1:9000/docs")];
        assert!(linked_hosts(&start, links.iter(), 3).is_empty());
    }

    #[test]
    fn bare_urls_are_the_urls_a_text_writes_out_without_its_punctuation() {
        let text = "The limits are documented at https://shopify.dev/api/usage/rate-limits. See (https://en.wikipedia.org/wiki/Foo_(bar)) and http://x.test/a, or ask: https://x.test/b?c=d!";
        let urls: Vec<String> = bare_urls(text).iter().map(Url::to_string).collect();
        assert_eq!(
            urls,
            [
                "https://shopify.dev/api/usage/rate-limits",
                "https://en.wikipedia.org/wiki/Foo_(bar)",
                "http://x.test/a",
                "https://x.test/b?c=d"
            ]
        );
    }

    #[test]
    fn a_word_that_is_not_a_full_url_is_not_one() {
        assert!(bare_urls("shopify.dev/api and https:// and mailto:me@x.com and ftp://x.test/").is_empty());
    }
}

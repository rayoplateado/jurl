//! The site's own search, where the site declares one with a standard: an OpenSearch description (`<link rel="search">`), a
//! schema.org SearchAction (JSON-LD), or the WordPress REST search (announced by `<link rel="https://api.w.org/">`). The
//! question goes to the search as it was asked; what it finds becomes candidate pages for `--follow`, ranked with the site's
//! other pages (see `Search::start` in follow.rs). A site that declares none gets none: nothing is guessed.

use std::time::{Duration, Instant};

use scraper::{Html, Selector};
use serde_json::Value;
use url::Url;

use crate::extract::Link;
use crate::fetch::{self, Retry};

/// The most results one WordPress search adds: the first few, as the site ranks them.
const MAX_RESULTS: usize = 10;
/// The most a search's answer may be, and how long it may take, the body included.
const SEARCH_MAX: usize = 1_000_000;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(6);

/// The candidate pages the site's own search gives for `question`: the WordPress results as pages of their own, and the
/// results page of an OpenSearch description or a SearchAction, as one page (its links are then leads like any page's).
/// Only --precise asks it, and only with a question.
pub async fn candidates(start: &Url, question: &str, precise: bool, retry: Retry<'_>) -> Vec<Link> {
    if !precise || question.trim().is_empty() {
        return Vec::new();
    }
    // The start page is read again here for the site's declarations, and the stealth sidecar is not asked for that second read:
    // `load` has asked it already, and its budget is for the pages a search reads.
    let plain = Retry { stealth: None, ..retry };
    let Ok(page) = fetch::fetch(start.as_str(), plain).await else { return Vec::new() };
    let html = Html::parse_document(&page.body);
    let mut found: Vec<(Url, String)> = Vec::new();
    let api = wp_root(&html, start)
        .and_then(|root| root.join(&format!("wp/v2/search?search={}&per_page={MAX_RESULTS}", encode(question))).ok());
    let wp = match api {
        Some(api) => fetch::fetch_small(&api, SEARCH_MAX, SEARCH_TIMEOUT, retry, Instant::now()).await,
        None => None,
    };
    found.extend(wp.map(|body| wp_results(&body)).unwrap_or_default());
    let results = match opensearch_description(&html, start) {
        Some(description) => fetch::fetch_small(&description, SEARCH_MAX, SEARCH_TIMEOUT, retry, Instant::now())
            .await
            .and_then(|xml| opensearch_template(&xml))
            .map(|t| (t, "searchTerms".to_string())),
        None => search_action_template(&html),
    };
    if let Some((template, name)) = results {
        found.extend(
            Url::parse(&fill(&template, &name, question))
                .ok()
                .map(|url| (url, format!("search results for {question}"))),
        );
    }
    found
        .into_iter()
        // A result at an address the run may not read is not a candidate (see `reach.rs`).
        .filter(|(url, _)| crate::reach::admit(url, retry.reach).is_ok())
        .enumerate()
        .map(|(i, (url, text))| Link { i, url, text, context: String::new(), marginal: false })
        .collect()
}

/// The question's words as a query value: spaces and punctuation are percent-encoded, as a form submits them.
fn encode(question: &str) -> String {
    url::form_urlencoded::byte_serialize(question.as_bytes()).collect()
}

/// `template` with its `{name}` placeholder filled by the encoded question.
fn fill(template: &str, name: &str, question: &str) -> String {
    template.replace(&format!("{{{name}}}"), &encode(question))
}

/// The root of the WordPress REST API the page announces: `<link rel="https://api.w.org/" href="…">`.
pub fn wp_root(html: &Html, base: &Url) -> Option<Url> {
    let sel = Selector::parse(r#"link[rel="https://api.w.org/"]"#).expect("static selector");
    let href = html.select(&sel).find_map(|el| el.value().attr("href"))?;
    base.join(href).ok()
}

/// The WordPress search's results: each one's address and title (a title is text or `{"rendered": …}`).
pub fn wp_results(json: &str) -> Vec<(Url, String)> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let url = Url::parse(item.get("url")?.as_str()?).ok()?;
            let title = match item.get("title") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Object(o)) => o.get("rendered").and_then(Value::as_str).unwrap_or_default().to_string(),
                _ => String::new(),
            };
            Some((url, title))
        })
        .take(MAX_RESULTS)
        .collect()
}

/// The address of the OpenSearch description the page links: `<link rel="search" type="…opensearchdescription+xml">`.
pub fn opensearch_description(html: &Html, base: &Url) -> Option<Url> {
    let sel = Selector::parse(r#"link[rel="search"]"#).expect("static selector");
    html.select(&sel)
        .find(|el| el.value().attr("type").is_some_and(|t| t.contains("opensearchdescription")))
        .and_then(|el| el.value().attr("href"))
        .and_then(|href| base.join(href).ok())
}

/// The results address an OpenSearch description gives: its text/html `<Url>` template (with `{searchTerms}`).
pub fn opensearch_template(xml: &str) -> Option<String> {
    let doc = Html::parse_document(xml);
    let sel = Selector::parse(r#"url[type="text/html"]"#).expect("static selector");
    doc.select(&sel).find_map(|el| el.value().attr("template").map(str::to_string))
}

/// The results address of a schema.org SearchAction in the page's JSON-LD, with the name of its placeholder: the query
/// input's name (`required name=search_term_string`), else the template's only `{…}`.
pub fn search_action_template(html: &Html) -> Option<(String, String)> {
    let sel = Selector::parse(r#"script[type="application/ld+json"]"#).expect("static selector");
    html.select(&sel).find_map(|el| {
        let body: String = el.text().collect();
        let doc = serde_json::from_str::<Value>(&body).ok()?;
        find_search_action(&doc)
    })
}

fn find_search_action(value: &Value) -> Option<(String, String)> {
    match value {
        Value::Array(items) => items.iter().find_map(find_search_action),
        Value::Object(map) => {
            let is_action = match map.get("@type") {
                Some(Value::String(t)) => t == "SearchAction",
                Some(Value::Array(ts)) => ts.iter().any(|t| t.as_str() == Some("SearchAction")),
                _ => false,
            };
            if is_action {
                let template = match map.get("target") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Object(o)) => o.get("urlTemplate")?.as_str()?.to_string(),
                    _ => return None,
                };
                let named = map
                    .get("query-input")
                    .and_then(Value::as_str)
                    .and_then(|q| q.split("name=").nth(1))
                    .and_then(|n| n.split_whitespace().next())
                    .map(str::to_string);
                let placeholder = named.or_else(|| {
                    let open = template.find('{')?;
                    let close = template[open..].find('}')? + open;
                    Some(template[open + 1..close].to_string())
                })?;
                return Some((template, placeholder));
            }
            map.values().find_map(find_search_action)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(html: &str) -> Html {
        Html::parse_document(html)
    }

    #[test]
    fn a_wordpress_site_announces_its_api_and_its_search_lists_the_matching_pages() {
        let base = Url::parse("https://example.org/").unwrap();
        let html = page(r#"<link rel="https://api.w.org/" href="https://example.org/wp-json/">"#);
        assert_eq!(wp_root(&html, &base).unwrap().as_str(), "https://example.org/wp-json/");
        let json = r#"[{"id":1,"title":"Hot desk prices","url":"https://example.org/hot-desk/","type":"page"},
                       {"id":2,"title":{"rendered":"Day pass"},"url":"https://example.org/day-pass/","type":"post"}]"#;
        let results = wp_results(json);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.as_str(), "https://example.org/hot-desk/");
        assert_eq!(results[1].1, "Day pass");
        assert!(wp_results("not json").is_empty());
    }

    #[test]
    fn an_opensearch_description_gives_its_html_results_template() {
        let base = Url::parse("https://example.org/").unwrap();
        let html = page(r#"<link rel="search" type="application/opensearchdescription+xml" href="/osd.xml">"#);
        assert_eq!(opensearch_description(&html, &base).unwrap().as_str(), "https://example.org/osd.xml");
        let xml = r#"<OpenSearchDescription><Url type="application/rss+xml" template="https://example.org/feed?q={searchTerms}"/><Url type="text/html" template="https://example.org/search?s={searchTerms}"/></OpenSearchDescription>"#;
        assert_eq!(opensearch_template(xml).as_deref(), Some("https://example.org/search?s={searchTerms}"));
    }

    #[test]
    fn a_schema_org_search_action_gives_its_target_with_the_query_inputs_placeholder() {
        let string_target = page(
            r#"<script type="application/ld+json">{"@type":"WebSite","potentialAction":{"@type":"SearchAction","target":"https://example.org/search?q={search_term_string}","query-input":"required name=search_term_string"}}</script>"#,
        );
        assert_eq!(
            search_action_template(&string_target),
            Some(("https://example.org/search?q={search_term_string}".to_string(), "search_term_string".to_string()))
        );
        let entry_point = page(
            r#"<script type="application/ld+json">{"@graph":[{"@type":"SearchAction","target":{"@type":"EntryPoint","urlTemplate":"https://example.org/find/{term}"}}]}</script>"#,
        );
        assert_eq!(
            search_action_template(&entry_point),
            Some(("https://example.org/find/{term}".to_string(), "term".to_string()))
        );
        assert_eq!(search_action_template(&page("<p>no search here</p>")), None);
    }

    #[test]
    fn the_question_is_encoded_as_a_form_submits_it() {
        assert_eq!(
            fill("https://example.org/search?s={searchTerms}", "searchTerms", "How much is a day pass?"),
            "https://example.org/search?s=How+much+is+a+day+pass%3F"
        );
    }

    /// The loopback test server's start page with `head` in it, and what the server was asked.
    fn site_with_head(head: &'static str) -> (Url, crate::fetch::test_server::Served) {
        let (base, served) = crate::fetch::test_server::serve_routed(move |request| {
            let path = request.split_whitespace().nth(1).unwrap_or_default();
            Some(if path == "/" {
                crate::fetch::test_server::reply("200 OK", "Content-Type: text/html\r\n", head.as_bytes())
            } else {
                crate::fetch::test_server::reply("404 Not Found", "", b"")
            })
        });
        (Url::parse(&format!("{base}/")).expect("a URL"), served)
    }

    /// The reach of a public run from the loopback test server, which is its own start address.
    fn public_run() -> crate::reach::Reach {
        crate::reach::Reach::Public {
            allowed: std::collections::HashSet::from([std::net::IpAddr::from([127, 0, 0, 1])]),
        }
    }

    #[tokio::test]
    async fn a_search_description_at_a_private_address_is_not_asked_under_a_public_run() {
        let (start, served) = site_with_head(
            r#"<html><head><link rel="search" type="application/opensearchdescription+xml" href="http://127.0.0.2:1/opensearch.xml"></head><body>Home</body></html>"#,
        );
        let reach = public_run();
        let memo = crate::fetch::Memo::default();
        let retry = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &reach,
            cookies: &crate::fetch::test_server::NO_COOKIES,
            stealth: None,
            fallback: &crate::fetch::test_server::NO_FALLBACK,
        };
        let found = candidates(&start, "pricing", true, retry).await;
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(served.count(), 1, "only the start page is asked");
    }

    #[tokio::test]
    async fn a_search_action_at_a_private_address_gives_no_candidate_under_a_public_run() {
        let head = r#"<html><head><script type="application/ld+json">{"@type":"WebSite","potentialAction":{"@type":"SearchAction","target":"http://127.0.0.2:1/search?q={search_term_string}","query-input":"required name=search_term_string"}}</script></head><body>Home</body></html>"#;
        let (start, _served) = site_with_head(head);
        let reach = public_run();
        let memo = crate::fetch::Memo::default();
        let public = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &reach,
            cookies: &crate::fetch::test_server::NO_COOKIES,
            stealth: None,
            fallback: &crate::fetch::test_server::NO_FALLBACK,
        };
        let found = candidates(&start, "pricing", true, public).await;
        assert!(found.is_empty(), "the public run lists no result at 127.0.0.2: {found:?}");
        // The same site under a private run lists its result, so the empty list above is the guard's doing.
        let private = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &crate::fetch::test_server::PRIVATE,
            cookies: &crate::fetch::test_server::NO_COOKIES,
            stealth: None,
            fallback: &crate::fetch::test_server::NO_FALLBACK,
        };
        let found = candidates(&start, "pricing", true, private).await;
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].url.as_str(), "http://127.0.0.2:1/search?q=pricing");
    }
}

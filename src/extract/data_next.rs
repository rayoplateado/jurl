//! The page after this one as a page's own data names it. Many lists load their next things with a script: the page's app
//! state, or the JSON it fetches, holds a link to the next batch (`next`, `next_url`, `nextPage`, `links.next.href`…), and the
//! page links none. That link is the site's own, so `--pages` may read it as it reads a `rel="next"` link: the batch it gives
//! is JSON of the same things, read as the page's data (`document`), and it names the batch after it in the same way.
//!
//! Nothing here knows what a thing is. A link counts only where its data also holds a list of things (see `sources`), and the
//! batch it gives counts only when its data holds things with about the same keys (`holds_like`): data can name the next page
//! of something else, such as an analytics feed, and that batch holds none of the list's things.

use std::collections::HashSet;

use scraper::{Html, Selector};
use serde_json::{Map, Value};
use url::Url;

use super::sources::{self, Applied, Data, Recipe};

/// What a key that names the next page is, with its case, `_` and `-` dropped.
const NEXT_KEYS: [&str; 6] = ["next", "nexturl", "nextpage", "nextpageurl", "nextlink", "nexthref"];
/// The keys of a list's things that are kept to tell its next batch by: the things' own keys, from their first elements.
const KEYED_ELEMENTS: usize = 5;
/// The most lists of a page whose keys are kept for `holds_like`.
const MAX_SHAPES: usize = 16;

/// The most links a page's data names next that are tried, nearest its list first: each one tried is a page loaded.
const MAX_LINKS: usize = 3;

/// The pages after this one, as the page's data names them (nearest its list first), and the keys of the things its list holds.
#[derive(Debug, Clone, PartialEq)]
pub struct DataNext {
    pub urls: Vec<Url>,
    pub keys: Vec<String>,
}

/// The pages after `base` as its data names them: the links of the data that holds the largest list, those sharing more of
/// that list's path first, then the shorter. A link counts when it is a web address on the page's own site, and not the page.
/// The list is of 3 things or more (`sources::MIN_LIST`), unless the page is a `batch` a data link gave: a batch may hold one.
pub fn next_of(data: &Data, base: &Url, batch: bool) -> Option<DataNext> {
    let min = if batch { 1 } else { sources::MIN_LIST };
    let mut best: Option<(usize, DataNext)> = None;
    for (_, root) in data.entries() {
        let Some((score, list_path, keys)) = largest_list(root, min) else { continue };
        let mut links = Vec::new();
        links_in(root, &mut Vec::new(), base, &mut links);
        let near = |path: &Vec<String>| path.iter().zip(&list_path).take_while(|(a, b)| a == b).count();
        links.sort_by_key(|(path, _)| (std::cmp::Reverse(near(path)), path.len()));
        let mut urls: Vec<Url> = Vec::new();
        for (_, url) in links {
            if !urls.contains(&url) && urls.len() < MAX_LINKS {
                urls.push(url);
            }
        }
        if !urls.is_empty() && best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, DataNext { urls, keys }));
        }
    }
    best.map(|(_, next)| next)
}

/// The keys of each list of things the data holds, a list of one thing included: a list's last batch may hold only one.
pub fn shapes_of(data: &Data) -> Vec<Vec<String>> {
    let mut shapes: Vec<(usize, Vec<String>)> = Vec::new();
    for (_, root) in data.entries() {
        sources::walk_min(root, 1, &mut Vec::new(), &mut |_, objects| shapes.push((objects.len(), keys_of(&objects))));
    }
    shapes.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    shapes.into_iter().take(MAX_SHAPES).map(|(_, keys)| keys).collect()
}

/// Whether one of a page's lists (`shapes_of`) holds things like those `keys` came from: half of their keys, or more.
pub fn holds_like(shapes: &[Vec<String>], keys: &[String]) -> bool {
    let wanted: HashSet<&String> = keys.iter().collect();
    !wanted.is_empty()
        && shapes.iter().any(|shape| 2 * shape.iter().filter(|k| wanted.contains(k)).count() >= wanted.len())
}

/// A page that is itself JSON, such as a batch a data link names, as the page's data: one piece, named as the JSON the page
/// would fetch is named when it is rendered (`json:fetch:PATH`, see render_capture.js), so a recipe written for one reads the
/// other. A browser that shows JSON (a render, the stealth route) wraps it in a `<pre>` of an otherwise empty page, which is
/// read the same. None for a page that is no JSON.
pub fn document(body: &str, url: &Url) -> Option<Data> {
    let text = body.trim_start();
    let value: Value = if text.starts_with(['{', '[']) {
        serde_json::from_str(text).ok()?
    } else if text.starts_with('<') && text.contains("<pre") {
        let doc = Html::parse_document(text);
        let pre = Selector::parse("body > pre").expect("static selector");
        let mut shown = doc.select(&pre);
        let (Some(pre), None) = (shown.next(), shown.next()) else { return None };
        let inner: String = pre.text().collect();
        let all: String = doc.root_element().text().collect();
        if all.trim() != inner.trim() {
            return None;
        }
        serde_json::from_str(inner.trim()).ok()?
    } else {
        return None;
    };
    Some(Data::from_entries(vec![(format!("json:fetch:{}", url.path()), value)]))
}

/// The rows a recipe takes from a page that is JSON, when the recipe's own path is not in it: its list may sit at another
/// path than in the page the recipe was written from (in an app's state under its section, in the batch at its root). The
/// list read is the one whose first element gives the most of the recipe's fields, half of them or more.
pub fn apply_by_shape(data: &Data, recipe: &Recipe) -> Applied {
    let mut best: Option<(usize, Vec<&Map<String, Value>>)> = None;
    for (_, root) in data.entries() {
        sources::walk_min(root, 1, &mut Vec::new(), &mut |_, objects| {
            let given = recipe.fields.values().filter(|path| sources::scalar_at(objects[0], path)).count();
            if 2 * given >= recipe.fields.len()
                && given > 0
                && best.as_ref().is_none_or(|(g, o)| (given, objects.len()) > (*g, o.len()))
            {
                best = Some((given, objects));
            }
        });
    }
    match best {
        Some((_, objects)) => sources::rows_from(&objects, recipe),
        None => Applied::RecipeError(format!("no list like {} {} in the page", recipe.source, recipe.path.join(" "))),
    }
}

/// The largest list of `min` things or more in a piece of data (as `sources::lists` weighs them), its path and its things' keys.
fn largest_list(root: &Value, min: usize) -> Option<(usize, Vec<String>, Vec<String>)> {
    let mut best: Option<(usize, Vec<String>, Vec<String>)> = None;
    sources::walk_min(root, min, &mut Vec::new(), &mut |path, objects| {
        let keys = keys_of(&objects);
        let score = objects.len() * keys.len().min(15);
        if best.as_ref().is_none_or(|(s, _, _)| score > *s) {
            best = Some((score, path.to_vec(), keys));
        }
    });
    best
}

fn keys_of(objects: &[&Map<String, Value>]) -> Vec<String> {
    let mut keys: Vec<String> = objects.iter().take(KEYED_ELEMENTS).flat_map(|o| o.keys().cloned()).collect();
    keys.sort();
    keys.dedup();
    keys
}

/// The links the data names as the next page, with their paths: a text under a next key, or an object's `href` or `url`
/// there. Only links on the page's site that are not the page itself.
fn links_in(value: &Value, path: &mut Vec<String>, base: &Url, out: &mut Vec<(Vec<String>, Url)>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                path.push(key.clone());
                if is_next_key(key) {
                    let text = match child {
                        Value::String(s) => Some(s.as_str()),
                        Value::Object(link) => {
                            ["href", "url"].iter().find_map(|k| link.get(*k).and_then(Value::as_str))
                        }
                        _ => None,
                    };
                    if let Some(url) = text.and_then(|t| on_site(base, t)) {
                        out.push((path.clone(), url));
                    }
                }
                links_in(child, path, base, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().take(sources::MAX_WALKED).enumerate() {
                path.push(i.to_string());
                links_in(item, path, base, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn is_next_key(key: &str) -> bool {
    let key: String = key.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
    NEXT_KEYS.contains(&key.as_str())
}

/// A link's address on the page's own host, not the page itself: a text written as an address (a path, a query, or a whole
/// `http(s)` address), resolved against the page. A word ("Next"), a cursor or a page number is no address: a sibling path
/// made of one would be a page of the site that does not exist. The scheme stays the page's, or goes from `http` to `https`.
fn on_site(base: &Url, text: &str) -> Option<Url> {
    let text = text.trim();
    let written = ["/", "?", "http://", "https://"].iter().any(|start| text.starts_with(start));
    if !written {
        return None;
    }
    let mut url = base.join(text).ok()?;
    url.set_fragment(None);
    let scheme = url.scheme() == base.scheme() || (base.scheme() == "http" && url.scheme() == "https");
    let same =
        scheme && url.host_str() == base.host_str() && url.port_or_known_default() == base.port_or_known_default();
    let mut here = base.clone();
    here.set_fragment(None);
    (same && url != here).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://shop.test/oils/c/7";

    fn base() -> Url {
        Url::parse(BASE).unwrap()
    }

    fn product(n: usize) -> Value {
        serde_json::json!({ "display_name": format!("Oil {n}"), "prices": { "price": n }, "url": format!("/p/{n}"), "sku": n })
    }

    // An app's store: the list in one section, the link to its next batch in another, and an empty next link elsewhere.
    fn store() -> Value {
        serde_json::json!({ "state": {
            "menu": { "nextPage": "" },
            "section": { "items": (1..=4).map(product).collect::<Vec<_>>(), "total": 9 },
            "pagination": { "pagination": { "page_number": 2, "next_url": "/api/list?page=2" } },
        }})
    }

    fn data(value: Value) -> Data {
        Data::from_entries(vec![("json:app".to_string(), value)])
    }

    #[test]
    fn the_next_page_is_the_link_the_data_of_the_largest_list_names_on_the_site() {
        let next = next_of(&data(store()), &base(), false).unwrap();
        assert_eq!(next.urls[0].as_str(), "https://shop.test/api/list?page=2");
        assert_eq!(next.keys, ["display_name", "prices", "sku", "url"]);
        // Linked as an object, as many APIs do.
        let linked = serde_json::json!({ "data": (1..=3).map(product).collect::<Vec<_>>(), "links": { "next": { "href": "https://shop.test/api/list?cursor=x" } } });
        assert_eq!(
            next_of(&data(linked), &base(), false).unwrap().urls[0].as_str(),
            "https://shop.test/api/list?cursor=x"
        );
    }

    #[test]
    fn no_next_page_off_the_site_nor_the_page_itself_nor_where_the_data_holds_no_list() {
        let off =
            serde_json::json!({ "items": (1..=3).map(product).collect::<Vec<_>>(), "next": "https://ads.test/next" });
        assert_eq!(next_of(&data(off), &base(), false), None);
        let itself = serde_json::json!({ "items": (1..=3).map(product).collect::<Vec<_>>(), "next": BASE });
        assert_eq!(next_of(&data(itself), &base(), false), None);
        let no_list = serde_json::json!({ "page": { "next_url": "/api/list?page=2" } });
        assert_eq!(next_of(&data(no_list), &base(), false), None);
        // A label, a cursor or a page number under a next key is no address.
        for text in ["Next", "eyJvZmZzZXQiOjIwfQ==", "3"] {
            let labelled =
                serde_json::json!({ "items": (1..=3).map(product).collect::<Vec<_>>(), "i18n": { "next": text } });
            assert_eq!(next_of(&data(labelled), &base(), false), None, "{text}");
        }
        // Two things are no list on a page, but a batch may hold two.
        let two = serde_json::json!({ "items": [product(1), product(2)], "next": "/api/list?page=2" });
        assert_eq!(next_of(&data(two.clone()), &base(), false), None);
        assert!(next_of(&data(two), &base(), true).is_some());
        // Not to another port, nor from https down to http.
        let other = serde_json::json!({ "items": (1..=3).map(product).collect::<Vec<_>>(), "next": "http://shop.test/api/list?page=2" });
        assert_eq!(next_of(&data(other), &base(), false), None);
    }

    #[test]
    fn a_batch_counts_when_its_data_holds_things_like_the_lists_and_one_thing_is_enough() {
        let keys = next_of(&data(store()), &base(), false).unwrap().keys;
        let last = serde_json::json!({ "items": [product(9)] });
        assert!(holds_like(&shapes_of(&data(last)), &keys));
        let feed = serde_json::json!({ "events": [{ "event": "view", "ts": 1 }, { "event": "click", "ts": 2 }], "pagination": { "next_url": "/api/feed?page=2" } });
        assert!(!holds_like(&shapes_of(&data(feed)), &keys));
    }

    #[test]
    fn a_json_page_is_its_own_data_and_a_recipe_finds_its_list_by_its_fields() {
        let url = Url::parse("https://shop.test/api/list?page=2").unwrap();
        let body =
            serde_json::json!({ "items": [product(5), product(6)], "pagination": { "next_url": "/api/list?page=3" } })
                .to_string();
        let page = document(&body, &url).unwrap();
        assert_eq!(page.entries().next().unwrap().0, "json:fetch:/api/list");
        assert_eq!(next_of(&page, &url, true).unwrap().urls[0].as_str(), "https://shop.test/api/list?page=3");
        let recipe = Recipe {
            source: "json:app".to_string(),
            path: ["state", "section", "items"].iter().map(|s| s.to_string()).collect(),
            fields: [("name", vec!["display_name"]), ("price", vec!["prices", "price"])]
                .into_iter()
                .map(|(n, p)| (n.to_string(), p.into_iter().map(String::from).collect()))
                .collect(),
        };
        let Applied::Rows { rows, .. } = apply_by_shape(&page, &recipe) else { panic!("no rows") };
        assert_eq!(rows.iter().map(|r| r["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Oil 5", "Oil 6"]);
        assert!(document("<html></html>", &url).is_none());
        // As a browser shows it, and not a page with a code sample in it.
        let shown = format!(
            "<html><head></head><body><pre>{}</pre></body></html>",
            body.replace('&', "&amp;").replace('<', "&lt;")
        );
        assert!(document(&shown, &url).is_some());
        assert!(document(&format!("<html><body><h1>API</h1><pre>{body}</pre></body></html>"), &url).is_none());
    }
}

//! A list's things from the public API of the platform its site runs on, when it runs on one that has one: WordPress's REST
//! API (each kind of post: shows, dogs, courses), a WooCommerce shop's Store API (its products), and The Events Calendar's
//! events API. These are the site's own data, served to anyone as JSON, so a list read over pages can take every thing from
//! them, with its name, date, price, photo and link as the site keeps them, instead of reading them off the page's text.
//!
//! Nothing here knows a site. A platform is told by the page's own head (WordPress links its API root with
//! `rel="https://api.w.org/"`) and by the plugins' names in its markup. Which of its lists is the page's is told by the links:
//! the endpoint whose things link to the pages the listing links to (`MIN_SHOWN` of them, or more) is the listing's own, and
//! the one that links to the most of them wins. A list that holds more than the page shows (every show the site ever had, a
//! shop's every product) is narrowed with the site's own filters: a category or other term that every shown thing has and
//! others lack, else, for posts, the date of the oldest one shown.

use std::collections::HashSet;

use scraper::{Html, Selector};
use serde_json::{Map, Value};
use url::Url;

use crate::extract::sources::Data;
use crate::extract::{Block, Kind};

/// The fewest shown things an endpoint's things must link to for it to be the listing's own: one or two can be a menu's.
const MIN_SHOWN: usize = 3;
/// How many things an endpoint gives at once: the most WordPress and WooCommerce give.
const PER_PAGE: usize = 100;
/// How many events The Events Calendar gives at once: its own most.
const EVENTS_PER_PAGE: usize = 50;
/// What a post is read with (see `tidy`): its photo comes embedded, which WordPress gives only with `_links` and `_embedded`
/// asked for. A plugin's fields for SEO, the post's whole text and the API's bookkeeping are left out.
const READ_FIELDS: &str = "id,date,link,title,excerpt,acf,_links,_embedded";
/// The most kinds of post that are tried: each one tried is a request to the site.
const MAX_KINDS: usize = 8;

/// Kinds of post that are no list a page shows: the site's own building blocks, and the kinds the shop and events APIs give
/// more fully.
const NOT_LISTS: [&str; 12] = [
    "attachment",
    "page",
    "nav_menu_item",
    "wp_block",
    "wp_template",
    "wp_template_part",
    "wp_navigation",
    "wp_global_styles",
    "wp_font_family",
    "wp_font_face",
    "product",
    "tribe_events",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// WordPress's REST API: `wp/v2/<kind>`.
    Posts,
    /// WooCommerce's Store API: `wc/store/v1/products`.
    Products,
    /// The Events Calendar: `tribe/events/v1/events`.
    Events,
}

impl Api {
    /// The API a URL is an endpoint of, by its route.
    fn of(url: &Url) -> Option<Api> {
        let route = route_of(url);
        if route.starts_with("/wp/v2/") {
            Some(Api::Posts)
        } else if route.starts_with("/wc/store/") {
            Some(Api::Products)
        } else if route.starts_with("/tribe/events/v1/events") {
            Some(Api::Events)
        } else {
            None
        }
    }

    /// The key that holds a thing's own page.
    fn link_key(self) -> &'static str {
        match self {
            Api::Posts => "link",
            Api::Products => "permalink",
            Api::Events => "url",
        }
    }
}

/// The route of an endpoint: the path after `/wp-json`, or the `rest_route` of a site without pretty addresses.
fn route_of(url: &Url) -> String {
    if let Some((_, route)) = url.query_pairs().find(|(k, _)| k == "rest_route") {
        return route.into_owned();
    }
    let path = url.path();
    path.find("/wp-json/").map(|at| path[at + "/wp-json".len()..].to_string()).unwrap_or_default()
}

/// The root of the site's WordPress API as its page links it (`<link rel="https://api.w.org/">`), when it is on the page's
/// own site; else `/wp-json/` on a page that WordPress serves.
pub fn api_root(html: &str, page: &Url) -> Option<Url> {
    let doc = Html::parse_document(html);
    let link = Selector::parse(r#"link[rel="https://api.w.org/"]"#).expect("static selector");
    let root = match doc.select(&link).find_map(|l| l.value().attr("href")) {
        Some(href) => page.join(href.trim()).ok()?,
        // Many sites take the link out of their head, and keep the API: a WordPress page still loads its theme and plugins
        // from `/wp-content/`, and its API is at the site's root.
        None if html.contains("/wp-content/") || html.contains("/wp-includes/") => page.join("/wp-json/").ok()?,
        None => return None,
    };
    (site_of(&root) == site_of(page) && matches!(root.scheme(), "http" | "https")).then_some(root)
}

/// An endpoint of the API at `root`: its route, and its query.
fn endpoint(root: &Url, route: &str, query: &[(String, String)]) -> Url {
    let mut url = root.clone();
    if root.query_pairs().any(|(k, _)| k == "rest_route") {
        url.query_pairs_mut().clear().append_pair("rest_route", &format!("/{route}"));
    } else {
        let path = format!("{}/{route}", root.path().trim_end_matches('/'));
        url.set_path(&path);
        url.set_query(None);
    }
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in query {
            pairs.append_pair(k, v);
        }
    }
    url
}

/// A host without its `www.`.
fn site_of(url: &Url) -> String {
    url.host_str().unwrap_or_default().trim_start_matches("www.").to_ascii_lowercase()
}

/// A page's address as a link to it is told by: its site and its path, without the trailing slash, query or fragment.
fn page_key(url: &Url) -> String {
    format!("{}{}", site_of(url), url.path().trim_end_matches('/'))
}

/// A link as a thing's own page is told by (`page_key`): a page of its own on the site, not the site's front page, nor a
/// page told by another query (`?elementor_snippet=12`, a site's building blocks, which every one of them shares the path of).
fn item_key(url: &Url) -> Option<String> {
    if url.query().is_none() {
        return url.path().trim_matches('/').contains(|c: char| c != '/').then(|| page_key(url));
    }
    // A site without pretty addresses names a post by its query (`/?p=123`, `/?post_type=show&p=5`).
    let mut post: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| matches!(k.as_ref(), "p" | "page_id" | "post_type"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    post.sort();
    let named = post.iter().any(|(k, _)| k != "post_type");
    (named && url.path().trim_matches('/').is_empty()).then(|| {
        format!("{}?{}", page_key(url), post.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"))
    })
}

/// One list an API may give: the endpoint that tells what it holds, the things it gave, and how its taxonomies are asked for.
struct Candidate {
    api: Api,
    /// The route and query of the endpoint the list is read from, before narrowing.
    route: String,
    query: Vec<(String, String)>,
    /// The keys of its things that hold their terms (`categories`, a kind's own taxonomies), with the query parameter each is
    /// asked by.
    terms: Vec<(String, String)>,
}

/// The endpoint a list read over pages takes the listing's things from, page 1, when the page's site runs on a platform with
/// a public API and one of its lists is the page's (see the module's doc). `get` gives an address's body, or none when it
/// can't be read (or its robots.txt does not let jurl read it); the lists are asked for together. `shown` are the links on
/// the page, `listed` how many things the page's own JSON-LD lists, and `question` what the read asks. Under `timing`, what each list
/// held of the page is said on stderr.
pub async fn listing_endpoint(
    html: &str,
    page: &Url,
    shown: &[Url],
    listed: usize,
    question: &str,
    timing: bool,
    get: impl AsyncFn(&Url) -> Option<String>,
) -> Option<Url> {
    let root = api_root(html, page)?;
    let here = page_key(page);
    let shown: HashSet<String> = shown.iter().filter_map(item_key).filter(|k| *k != here).collect();
    let (kinds_at, taxonomies_at) = (endpoint(&root, "wp/v2/types", &[]), endpoint(&root, "wp/v2/taxonomies", &[]));
    let (kinds, taxonomies) = futures::join!(get(&kinds_at), get(&taxonomies_at));
    let kind_list: Option<Value> = kinds.as_deref().and_then(|k| serde_json::from_str(k).ok());
    let has_kind = |slug: &str| kind_list.as_ref().is_some_and(|k| k.get(slug).is_some());
    let mut candidates = Vec::new();
    if html.contains("tribe-events") || html.contains("the-events-calendar") || has_kind("tribe_events") {
        let per = EVENTS_PER_PAGE.to_string();
        candidates.push(Candidate {
            api: Api::Events,
            route: "tribe/events/v1/events".into(),
            query: vec![q("per_page", per)],
            terms: vec![],
        });
    }
    if html.contains("woocommerce") || has_kind("product") {
        candidates.push(Candidate {
            api: Api::Products,
            route: "wc/store/v1/products".into(),
            query: vec![q("per_page", PER_PAGE.to_string())],
            terms: vec![("categories".into(), "category".into())],
        });
    }
    candidates.extend(post_kinds(kinds.as_deref(), taxonomies.as_deref()));

    let probes: Vec<Url> = candidates.iter().map(|c| endpoint(&root, &c.route, &c.query)).collect();
    let bodies = futures::future::join_all(probes.iter().map(|probe| get(probe))).await;
    let mut best: Option<(usize, Candidate, Vec<Value>)> = None;
    for (candidate, body) in candidates.into_iter().zip(bodies) {
        let Some(things) = body.and_then(|body| things_of(candidate.api, &body)) else {
            if timing {
                eprintln!("jurl: {} gives no list", candidate.route);
            }
            continue;
        };
        let count = shown_in(candidate.api, &things, &shown);
        if timing {
            eprintln!("jurl: {} gives {} things, {count} of them linked from the page", candidate.route, things.len());
        }
        if count >= MIN_SHOWN && best.as_ref().is_none_or(|(c, _, _)| count > *c) {
            best = Some((count, candidate, things));
        }
    }
    let (count, candidate, things) = best?;
    // The page's own JSON-LD gives its things with their own fields (an event's date, where a post has only when it was
    // published): when it lists as many of the shown things, it is read, not the API.
    if listed >= count {
        if timing {
            eprintln!("jurl: {} is not read: the page's JSON-LD lists {listed} of its things", candidate.route);
        }
        return None;
    }
    let is_shown = |t: &Value| link_of(candidate.api, t).is_some_and(|k| shown.contains(&k));
    let (on_page, off_page): (Vec<&Value>, Vec<&Value>) = things.iter().partition(|t| is_shown(t));
    // The probe asked only for what tells the list (`_fields`); the read asks for what a reader needs.
    let mut query: Vec<(String, String)> = candidate.query.iter().filter(|(k, _)| k != "_fields").cloned().collect();
    let mut narrowed = false;
    for (key, param) in &candidate.terms {
        if let Some(term) = narrowing_term(&on_page, &off_page, key) {
            if timing {
                eprintln!("jurl: {} narrowed to {param}={term}, which every thing the page links has", candidate.route);
            }
            query.push(q(param, term.to_string()));
            narrowed = true;
        }
    }
    // Else, the term the question names ("Perros en adopción" for "the dogs for adoption at this shelter"), when the page shows
    // a few things of the site's whole catalogue (a home page's latest products), which share none.
    let shared = narrowed;
    if !narrowed && words(question).any(|w| w.chars().count() > 3) {
        for (_, param) in &candidate.terms {
            let at = match candidate.api {
                Api::Products => {
                    endpoint(&root, "wc/store/v1/products/categories", &[q("per_page", PER_PAGE.to_string())])
                }
                _ => endpoint(
                    &root,
                    &format!("wp/v2/{param}"),
                    &[q("per_page", PER_PAGE.to_string()), q("_fields", "id,name,count".into())],
                ),
            };
            let Some(term) = get(&at).await.and_then(|body| named_term(&body, question)) else { continue };
            if timing {
                eprintln!("jurl: {} narrowed to {param}={term}, which the question names", candidate.route);
            }
            query.push(q(param, term.to_string()));
            narrowed = true;
            break;
        }
    }
    // Else, the terms that none of the shown things has and that hold others (a shelter's adopted dogs, a past season) are
    // left out.
    if !narrowed {
        for (key, param) in &candidate.terms {
            let out = unshown_terms(&on_page, &off_page, key);
            if out.is_empty() {
                continue;
            }
            let ids = out.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
            if timing {
                eprintln!(
                    "jurl: {} leaves out {param} {ids}, which none of the things the page links has",
                    candidate.route
                );
            }
            match candidate.api {
                Api::Products => query.extend([q(param, ids), q("category_operator", "not_in".into())]),
                _ => query.push(q(&format!("{param}_exclude"), ids)),
            }
        }
    }
    if candidate.api == Api::Posts {
        // A site's posts are its blog, news and notices: they are a page's list only as one of their categories.
        if candidate.route == "wp/v2/posts" && !narrowed {
            if timing {
                eprintln!("jurl: wp/v2/posts is not read: no category holds just the posts the page links");
            }
            return None;
        }
        query.push(q("_fields", READ_FIELDS.into()));
        query.push(q("_embed", "wp:featuredmedia".into()));
        if !shared
            && !off_page.is_empty()
            && let Some(after) = day_before_oldest(&on_page)
        {
            query.push(q("after", after));
        }
    }
    Some(endpoint(&root, &candidate.route, &query))
}

/// Whether a site's name for a list is a plain name (`shows`, `event-types`), which may go in a path.
fn plain(base: &str) -> bool {
    !base.is_empty() && base.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// How many different pages the page links to that `things` link to.
fn shown_in(api: Api, things: &[Value], shown: &HashSet<String>) -> usize {
    things.iter().filter_map(|t| link_of(api, t)).filter(|k| shown.contains(k)).collect::<HashSet<_>>().len()
}

fn q(name: &str, value: String) -> (String, String) {
    (name.to_string(), value)
}

/// The kinds of post a site's `wp/v2/types` lists that may be a page's list, its own kinds first, then its posts, with the
/// taxonomies of each as `wp/v2/taxonomies` names their parameters.
fn post_kinds(kinds: Option<&str>, taxonomies: Option<&str>) -> Vec<Candidate> {
    let Some(Value::Object(kinds)) = kinds.and_then(|k| serde_json::from_str::<Value>(k).ok()) else { return vec![] };
    let taxonomies: Map<String, Value> = taxonomies
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .and_then(|t| t.as_object().cloned())
        .unwrap_or_default();
    let mut out: Vec<(bool, Candidate)> = Vec::new();
    for (slug, kind) in &kinds {
        let Some(base) = kind.get("rest_base").and_then(Value::as_str).filter(|b| plain(b)) else {
            continue;
        };
        if NOT_LISTS.contains(&slug.as_str()) || kind.get("viewable").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let terms: Vec<(String, String)> = kind
            .get("taxonomies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|tax| taxonomies.get(tax).and_then(|t| t.get("rest_base")).and_then(Value::as_str))
            .filter(|b| plain(b))
            .map(|base| (base.to_string(), base.to_string()))
            .collect();
        let mut fields = vec!["link".to_string(), "date".to_string()];
        fields.extend(terms.iter().map(|(k, _)| k.clone()));
        out.push((
            slug == "post",
            Candidate {
                api: Api::Posts,
                route: format!("wp/v2/{base}"),
                query: vec![q("per_page", PER_PAGE.to_string()), q("_fields", fields.join(","))],
                terms,
            },
        ));
    }
    out.sort_by_key(|(post, _)| *post);
    out.into_iter().map(|(_, c)| c).take(MAX_KINDS).collect()
}

/// The things an endpoint's body holds: its list, or the events of The Events Calendar's answer.
fn things_of(api: Api, body: &str) -> Option<Vec<Value>> {
    let value: Value = serde_json::from_str(body.trim_start()).ok()?;
    things_in(api, &value).map(|things| things.into_iter().cloned().collect())
}

/// The things an endpoint's answer holds, as `things_of`.
fn things_in(api: Api, value: &Value) -> Option<Vec<&Value>> {
    let list = match api {
        Api::Events => value.get("events")?,
        _ => value,
    };
    Some(list.as_array()?.iter().filter(|t| t.is_object()).collect())
}

/// The page a thing links to, as links are told by (`page_key`).
fn link_of(api: Api, thing: &Value) -> Option<String> {
    thing.get(api.link_key()).and_then(Value::as_str).and_then(|l| Url::parse(l).ok()).and_then(|u| item_key(&u))
}

/// The ids of the terms a thing has under `key`: a list of ids, or of terms with their `id`.
fn term_ids(thing: &Value, key: &str) -> Vec<u64> {
    thing
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_u64().or_else(|| t.get("id").and_then(Value::as_u64)))
        .collect()
}

/// The term under `key` that every shown thing has and that leaves out the most of the others, when one leaves out any.
fn narrowing_term(on_page: &[&Value], off_page: &[&Value], key: &str) -> Option<u64> {
    let (first, rest) = on_page.split_first()?;
    let mut common: Vec<u64> = term_ids(first, key);
    for thing in rest {
        let ids = term_ids(thing, key);
        common.retain(|id| ids.contains(id));
    }
    common
        .into_iter()
        .map(|id| (off_page.iter().filter(|t| !term_ids(t, key).contains(&id)).count(), id))
        .filter(|(left_out, _)| *left_out > 0)
        .max_by_key(|(left_out, id)| (*left_out, std::cmp::Reverse(*id)))
        .map(|(_, id)| id)
}

/// Of a taxonomy's terms (`[{"id", "name", "count"}]`), the one whose name's words are all words of the question, with the most
/// words and then the most things, when it holds `MIN_SHOWN` things or more and its name is more than one word and more than small
/// words: "Perros en adopción" for the dogs for adoption at a shelter, not "Gatos en adopción", nor a one-word tag the question
/// happens to hold ("Madrid"), which would read only the things tagged with it.
fn named_term(body: &str, question: &str) -> Option<u64> {
    let asked: HashSet<String> = words(question).collect();
    let terms: Vec<Value> = serde_json::from_str(body).ok()?;
    let mut best: Option<(usize, u64, u64)> = None;
    for term in &terms {
        let (Some(id), Some(name)) = (term.get("id").and_then(Value::as_u64), term.get("name").and_then(Value::as_str))
        else {
            continue;
        };
        let count = term.get("count").and_then(Value::as_u64).unwrap_or(0);
        let named: Vec<String> = words(&shown_text(name)).collect();
        let named_by_question =
            named.len() > 1 && named.iter().any(|w| w.chars().count() > 3) && named.iter().all(|w| asked.contains(w));
        if count >= MIN_SHOWN as u64 && named_by_question && best.is_none_or(|(n, c, _)| (named.len(), count) > (n, c))
        {
            best = Some((named.len(), count, id));
        }
    }
    best.map(|(_, _, id)| id)
}

/// A text's words, in lower case.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase)
}

/// The terms under `key` that no shown thing has and that `MIN_SHOWN` of the others or more have, in order.
fn unshown_terms(on_page: &[&Value], off_page: &[&Value], key: &str) -> Vec<u64> {
    let shown: HashSet<u64> = on_page.iter().flat_map(|t| term_ids(t, key)).collect();
    let mut counts: std::collections::BTreeMap<u64, usize> = std::collections::BTreeMap::new();
    for id in off_page.iter().flat_map(|t| term_ids(t, key)).filter(|id| !shown.contains(id)) {
        *counts.entry(id).or_default() += 1;
    }
    counts.into_iter().filter(|(_, n)| *n >= MIN_SHOWN).map(|(id, _)| id).collect()
}

/// The day before the oldest shown post was published, at its start (`2026-02-28T00:00:00`): WordPress's `after` gives the
/// posts published after it, so the oldest shown one is kept, and the site's older posts (its past seasons) are not read.
fn day_before_oldest(on_page: &[&Value]) -> Option<String> {
    let oldest = on_page.iter().filter_map(|t| t.get("date").and_then(Value::as_str)).filter_map(ymd).min()?;
    let (y, m, d) = civil(days(oldest) - 1);
    Some(format!("{y:04}-{m:02}-{d:02}T00:00:00"))
}

/// A date's year, month and day, from its first ten characters (`2026-03-01`).
fn ymd(date: &str) -> Option<(i64, u32, u32)> {
    let mut parts = date.get(..10)?.split('-');
    let y = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok().filter(|m| (1..=12).contains(m))?;
    let d = parts.next()?.parse().ok().filter(|d| (1..=31).contains(d))?;
    Some((y, m, d))
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar.
fn days((y, m, d): (i64, u32, u32)) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date a count of days since 1970-01-01 is.
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A page of a list read over pages that is a batch of one of these APIs (its data, as `data_next::document` reads a JSON
/// page), as the things a reader of the list needs (see `tidy`), named as the page's data is, and the batch after it: The
/// Events Calendar names it (`next_rest_url`); WordPress and WooCommerce have one when this one is full. None for any other
/// page.
pub fn batch(data: &Data, url: &Url) -> Option<(Data, Option<Url>)> {
    let api = Api::of(url)?;
    let (name, value) = data.entries().next()?;
    let next = match api {
        Api::Events => value
            .get("next_rest_url")
            .and_then(Value::as_str)
            .and_then(|n| Url::parse(n).ok())
            .filter(|n| matches!(n.scheme(), "http" | "https") && site_of(n) == site_of(url) && n != url),
        _ => {
            let per = url.query_pairs().find(|(k, _)| k == "per_page").and_then(|(_, v)| v.parse::<usize>().ok());
            let full = per.is_some_and(|per| value.as_array().is_some_and(|things| things.len() >= per));
            full.then(|| next_numbered(url))
        }
    };
    let tidied: Vec<Value> = things_in(api, value)?.iter().filter_map(|t| tidy(api, t)).collect();
    Some((Data::from_entries(vec![(name.to_string(), Value::Array(tidied))]), next))
}

/// A batch's things as the page's text: one record per thing (a list's item, see `Block::record`), with its photo, its name and then its other values one per line under their key, linked
/// to its page. Read as text, the batch would be its JSON: on a shop's hundred products half a megabyte that a reader can make
/// nothing of, and that leaves a page's pages no room for its things.
pub fn blocks(things: &Data) -> Vec<Block> {
    let Some((_, Value::Array(things))) = things.entries().next() else { return Vec::new() };
    things
        .iter()
        .filter_map(Value::as_object)
        .enumerate()
        .map(|(i, thing)| {
            let url = thing.get("url").and_then(Value::as_str).map(str::to_string);
            let mut lines: Vec<String> =
                thing.get("name").and_then(Value::as_str).map(str::to_string).into_iter().collect();
            for (key, value) in thing.iter().filter(|(k, _)| !matches!(k.as_str(), "name" | "url" | "image")) {
                match value {
                    Value::Object(fields) => lines.extend(fields.iter().filter_map(|(k, v)| line(k, v))),
                    _ => lines.extend(line(key, value)),
                }
            }
            Block {
                links: url.clone().into_iter().collect(),
                href: url,
                record: Some(i + 1),
                image: thing.get("image").and_then(Value::as_str).map(str::to_string),
                ..Block::new(i, Kind::Para, lines.join("\n"))
            }
        })
        .collect()
}

fn line(key: &str, value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(format!("{key}: {text}")),
        Value::Number(n) => Some(format!("{key}: {n}")),
        Value::Bool(b) => Some(format!("{key}: {b}")),
        _ => None,
    }
}

/// The same endpoint, one page on.
fn next_numbered(url: &Url) -> Url {
    let page = url.query_pairs().find(|(k, _)| k == "page").and_then(|(_, v)| v.parse::<u32>().ok()).unwrap_or(1);
    let pairs: Vec<(String, String)> =
        url.query_pairs().filter(|(k, _)| k != "page").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let mut next = url.clone();
    {
        let mut query = next.query_pairs_mut();
        query.clear();
        for (k, v) in &pairs {
            query.append_pair(k, v);
        }
        query.append_pair("page", &(page + 1).to_string());
    }
    next
}

/// One thing as a reader of the list needs it: its name, its page, its dates, price and photo where the API has them (a
/// post's own date is when it was `published`, not the date of what it tells of), and
/// a short text, each value as the site keeps it. Texts the site keeps as HTML (`title.rendered`) are given as the page shows
/// them; a WooCommerce price is kept in the currency's minor unit (`1350` with 2 decimals) and is given as the page shows it
/// (`13.50`). The API's own bookkeeping (links to itself, ids, every size of each photo) is left out.
fn tidy(api: Api, thing: &Value) -> Option<Value> {
    let mut out = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(v) = value.filter(|v| !v.is_null() && v.as_str() != Some("")) {
            out.insert(key.to_string(), v);
        }
    };
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(shown_text).map(Value::String);
    let at = |path: &[&str]| {
        path.iter().try_fold(thing, |v, k| v.get(k).or_else(|| k.parse::<usize>().ok().and_then(|i| v.get(i))))
    };
    match api {
        Api::Posts => {
            put("name", text(at(&["title", "rendered"]).or_else(|| at(&["title"]))));
            put("url", at(&["link"]).cloned());
            put("published", at(&["date"]).cloned());
            put("summary", text(at(&["excerpt", "rendered"])));
            put("image", at(&["_embedded", "wp:featuredmedia", "0", "source_url"]).cloned());
            // A post's own fields, the ones that are a value: a gallery or a list of rows in them is no field of the list.
            let fields: Map<String, Value> = at(&["acf"])
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter(|(_, v)| matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_)))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            put("fields", (!fields.is_empty()).then_some(Value::Object(fields)));
        }
        Api::Products => {
            let minor = at(&["prices", "currency_minor_unit"]).and_then(Value::as_u64).unwrap_or(0);
            let price = |key: &str| {
                at(&["prices", key]).and_then(Value::as_str).and_then(|p| decimal(p, minor)).map(Value::String)
            };
            put("name", text(at(&["name"])));
            put("url", at(&["permalink"]).cloned());
            put("price", price("price"));
            if at(&["on_sale"]).and_then(Value::as_bool) == Some(true) {
                put("regular_price", price("regular_price"));
            }
            put("currency", at(&["prices", "currency_code"]).cloned());
            put("image", at(&["images", "0", "src"]).cloned());
            put("categories", names(at(&["categories"])));
            put("summary", text(at(&["short_description"])));
            put("in_stock", at(&["is_in_stock"]).cloned());
        }
        Api::Events => {
            put("name", text(at(&["title"])));
            put("url", at(&["url"]).cloned());
            put("start_date", at(&["start_date"]).cloned());
            put("end_date", at(&["end_date"]).cloned());
            put("cost", text(at(&["cost"])));
            put("image", at(&["image", "url"]).cloned());
            put("venue", text(at(&["venue", "venue"])));
            put("city", text(at(&["venue", "city"])));
            put("categories", names(at(&["categories"])));
            put("summary", text(at(&["excerpt"])));
        }
    }
    out.contains_key("name").then_some(Value::Object(out))
}

/// The names of a list of terms, as one text.
fn names(terms: Option<&Value>) -> Option<Value> {
    let names: Vec<&str> = terms?.as_array()?.iter().filter_map(|t| t.get("name").and_then(Value::as_str)).collect();
    (!names.is_empty()).then(|| Value::String(names.iter().map(|n| shown_text(n)).collect::<Vec<_>>().join(", ")))
}

/// A text the site keeps as HTML, as its page shows it: its entities read, its tags dropped, its spaces collapsed.
fn shown_text(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    let text: String = fragment.root_element().text().collect::<Vec<_>>().join(" ");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A whole number of a currency's minor unit as its amount (`1350`, 2 → `13.50`).
fn decimal(minor_units: &str, places: u64) -> Option<String> {
    let digits = minor_units.trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let places = places as usize;
    if places == 0 {
        return Some(digits.to_string());
    }
    let padded = format!("{digits:0>width$}", width = places + 1);
    let (whole, part) = padded.split_at(padded.len() - places);
    Some(format!("{whole}.{part}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    const HEAD: &str =
        r#"<html><head><link rel="https://api.w.org/" href="https://www.site.test/wp-json/"></head><body>"#;

    fn read(body: &str, url: &Url) -> Option<(Data, Option<Url>)> {
        batch(&crate::extract::data_next::document(body, url)?, url)
    }

    #[test]
    fn a_batch_reads_as_one_block_per_thing_with_its_values_by_key() {
        let url = u("https://site.test/wp-json/wc/store/v1/products?per_page=100");
        let body = json!([{ "name": "Luna", "permalink": "https://site.test/animal/luna/", "short_description": "<p>Mastina</p>",
            "prices": { "price": "0", "currency_minor_unit": 2, "currency_code": "EUR" }, "images": [{ "src": "https://site.test/l.jpg" }] }])
        .to_string();
        let (things, _) = read(&body, &url).unwrap();
        let blocks = blocks(&things);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "Luna\ncurrency: EUR\nprice: 0.00\nsummary: Mastina");
        assert_eq!(blocks[0].href.as_deref(), Some("https://site.test/animal/luna/"));
        assert_eq!((blocks[0].record, blocks[0].image.as_deref()), (Some(1), Some("https://site.test/l.jpg")));
    }

    #[test]
    fn a_batch_keeps_plain_fields_drops_lists_follows_only_its_own_site_and_reads_a_wrapped_body() {
        let url = u("https://site.test/wp-json/wp/v2/shows?per_page=100");
        let body = json!([{ "link": "https://site.test/shows/1/", "title": "Hamlet",
            "acf": { "venue": "Sala", "gallery": [1, 2], "seats": 90 } }])
        .to_string();
        let wrapped = format!("<html><body><pre>{body}</pre></body></html>");
        let (data, _) = read(&wrapped, &url).unwrap();
        assert_eq!(
            data.entries().next().unwrap().1,
            &json!([{ "name": "Hamlet", "url": "https://site.test/shows/1/", "fields": { "venue": "Sala", "seats": 90 } }])
        );
        let url = u("https://site.test/wp-json/tribe/events/v1/events?per_page=50");
        let body = json!({ "events": [{ "title": "A", "url": "https://site.test/e/1/" }],
            "next_rest_url": "https://elsewhere.test/wp-json/tribe/events/v1/events?page=2" })
        .to_string();
        assert_eq!(read(&body, &url).unwrap().1, None, "a next batch on another site is not followed");
    }

    #[test]
    fn the_api_root_is_the_one_the_head_links_on_the_page_s_own_site() {
        let page = u("https://site.test/agenda/");
        assert_eq!(api_root(HEAD, &page).unwrap().as_str(), "https://www.site.test/wp-json/");
        let off = r#"<link rel="https://api.w.org/" href="https://other.test/wp-json/">"#;
        assert_eq!(api_root(off, &page), None);
        let plain = r#"<link rel="https://api.w.org/" href="https://site.test/?rest_route=/">"#;
        let root = api_root(plain, &page).unwrap();
        assert_eq!(
            endpoint(&root, "wp/v2/types", &[q("per_page", "100".into())]).as_str(),
            "https://site.test/?rest_route=%2Fwp%2Fv2%2Ftypes&per_page=100"
        );
        assert_eq!(Api::of(&endpoint(&root, "wp/v2/dogs", &[])), Some(Api::Posts));
        assert_eq!(
            endpoint(&u("https://site.test/wp-json/"), "wc/store/v1/products", &[]).as_str(),
            "https://site.test/wp-json/wc/store/v1/products"
        );
    }

    fn post(n: u32, date: &str, cats: &[u64]) -> Value {
        json!({ "link": format!("https://www.site.test/shows/{n}/"), "date": date, "categories": cats })
    }

    fn types() -> String {
        json!({
            "post": { "slug": "post", "rest_base": "posts", "taxonomies": ["category"] },
            "page": { "slug": "page", "rest_base": "pages", "taxonomies": [] },
            "attachment": { "slug": "attachment", "rest_base": "media", "taxonomies": [] },
            "show": { "slug": "show", "rest_base": "shows", "taxonomies": ["season"] },
        })
        .to_string()
    }

    fn taxonomies() -> String {
        json!({ "category": { "rest_base": "categories" }, "season": { "rest_base": "seasons" } }).to_string()
    }

    /// A site's answers by route, recording what is asked.
    async fn discover(html: &str, shown: &[&str], answers: Vec<(&str, String)>) -> (Option<Url>, Vec<String>) {
        discover_listed(html, shown, 0, "", answers).await
    }

    async fn discover_listed(
        html: &str,
        shown: &[&str],
        listed: usize,
        question: &str,
        answers: Vec<(&str, String)>,
    ) -> (Option<Url>, Vec<String>) {
        let shown: Vec<Url> = shown.iter().map(|s| u(s)).collect();
        let asked = std::sync::Mutex::new(Vec::new());
        let found = listing_endpoint(
            html,
            &u("https://site.test/agenda/"),
            &shown,
            listed,
            question,
            false,
            async |url: &Url| {
                asked.lock().unwrap().push(url.to_string());
                let route = route_of(url);
                answers.iter().find(|(r, _)| *r == route).map(|(_, body)| body.clone())
            },
        )
        .await;
        (found, asked.into_inner().unwrap())
    }

    #[tokio::test]
    async fn the_kind_whose_posts_the_page_links_to_is_read_narrowed_to_the_term_they_share() {
        let shows = json!([
            { "link": "https://site.test/shows/1/", "date": "2026-09-01T10:00:00", "seasons": [7] },
            { "link": "https://site.test/shows/2/", "date": "2026-08-01T10:00:00", "seasons": [7, 3] },
            { "link": "https://site.test/shows/3/", "date": "2026-07-01T10:00:00", "seasons": [7] },
            { "link": "https://site.test/shows/4/", "date": "2019-07-01T10:00:00", "seasons": [3] },
        ])
        .to_string();
        let posts = json!([post(9, "2026-01-01T00:00:00", &[1])]).to_string();
        let shown = [
            "https://site.test/shows/1",
            "https://site.test/shows/2/",
            "https://site.test/shows/3/",
            "https://site.test/agenda/",
        ];
        let answers = vec![
            ("/wp/v2/types", types()),
            ("/wp/v2/taxonomies", taxonomies()),
            ("/wp/v2/shows", shows),
            ("/wp/v2/posts", posts),
        ];
        let (found, asked) = discover(HEAD, &shown, answers).await;
        assert_eq!(
            found.unwrap().as_str(),
            "https://www.site.test/wp-json/wp/v2/shows?per_page=100&seasons=7&_fields=id%2Cdate%2Clink%2Ctitle%2Cexcerpt%2Cacf%2C_links%2C_embedded&_embed=wp%3Afeaturedmedia"
        );
        assert!(asked.iter().all(|a| !a.contains("/pages") && !a.contains("/media")), "{asked:?}");
        assert!(asked[2].contains("/shows"), "a site's own kinds are tried before its posts");
    }

    #[tokio::test]
    async fn without_a_term_to_narrow_by_posts_are_read_from_the_day_before_the_oldest_shown() {
        let shows = json!([
            post(1, "2026-03-01T10:00:00", &[]),
            post(2, "2026-02-10T10:00:00", &[]),
            post(3, "2026-01-01T09:00:00", &[]),
            post(4, "2018-01-01T09:00:00", &[]),
        ])
        .to_string();
        let shown = ["https://site.test/shows/1/", "https://site.test/shows/2/", "https://site.test/shows/3/"];
        let answers = vec![("/wp/v2/types", types()), ("/wp/v2/taxonomies", taxonomies()), ("/wp/v2/shows", shows)];
        let found = discover(HEAD, &shown, answers).await.0.unwrap();
        assert!(found.as_str().ends_with("&after=2025-12-31T00%3A00%3A00"), "{found}");
    }

    #[tokio::test]
    async fn no_endpoint_when_its_things_link_to_fewer_than_three_shown_pages_or_the_site_has_no_api() {
        let shows = json!([post(1, "2026-03-01T10:00:00", &[]), post(2, "2026-02-10T10:00:00", &[])]).to_string();
        let shown = ["https://site.test/shows/1/", "https://site.test/shows/2/"];
        let answers = vec![("/wp/v2/types", types()), ("/wp/v2/taxonomies", taxonomies()), ("/wp/v2/shows", shows)];
        assert_eq!(discover(HEAD, &shown, answers.clone()).await.0, None);
        let (found, asked) = discover("<html><head></head></html>", &shown, answers).await;
        assert_eq!(found, None);
        assert!(asked.is_empty(), "a page that links no API asks nothing");
    }

    #[tokio::test]
    async fn building_blocks_a_blog_and_one_page_linked_again_and_again_are_no_listing() {
        // A theme's snippets live at the front page's path, told by their query.
        let snippets =
            json!((1..=4).map(|n| json!({ "link": format!("https://site.test/?snippet={n}") })).collect::<Vec<_>>())
                .to_string();
        let kinds = json!({ "snippet": { "rest_base": "snippets", "taxonomies": [] }, "post": { "rest_base": "posts", "taxonomies": ["category"] } }).to_string();
        let shown = [
            "https://site.test/",
            "https://site.test/?snippet=1",
            "https://site.test/p/1/",
            "https://site.test/p/2/",
            "https://site.test/p/3/",
        ];
        let answers =
            vec![("/wp/v2/types", kinds.clone()), ("/wp/v2/taxonomies", taxonomies()), ("/wp/v2/snippets", snippets)];
        assert_eq!(discover(HEAD, &shown, answers).await.0, None);
        // Posts the page links, with no category of their own among the others: a blog, not the listing.
        let posts = json!([
            { "link": "https://site.test/p/1/", "date": "2026-01-03T00:00:00", "categories": [1] },
            { "link": "https://site.test/p/2/", "date": "2026-01-02T00:00:00", "categories": [1] },
            { "link": "https://site.test/p/3/", "date": "2026-01-01T00:00:00", "categories": [1] },
            { "link": "https://site.test/p/4/", "date": "2025-01-01T00:00:00", "categories": [1] },
        ])
        .to_string();
        let answers =
            vec![("/wp/v2/types", kinds.clone()), ("/wp/v2/taxonomies", taxonomies()), ("/wp/v2/posts", posts)];
        assert_eq!(discover(HEAD, &shown, answers).await.0, None);
        // The same page linked by every thing counts once.
        let same = json!(
            (1..=4)
                .map(|_| json!({ "link": "https://site.test/p/1/", "date": "2026-01-01T00:00:00" }))
                .collect::<Vec<_>>()
        )
        .to_string();
        let kinds = json!({ "show": { "rest_base": "shows", "taxonomies": [] } }).to_string();
        let answers = vec![("/wp/v2/types", kinds), ("/wp/v2/taxonomies", taxonomies()), ("/wp/v2/shows", same)];
        assert_eq!(discover(HEAD, &shown, answers).await.0, None);
    }

    #[tokio::test]
    async fn the_terms_none_of_the_shown_things_have_are_left_out_and_a_page_s_own_json_ld_is_read_when_it_lists_as_many()
     {
        let product = |n: u32, cats: &[u64]| json!({ "permalink": format!("https://site.test/p/{n}/"), "categories": cats.iter().map(|c| json!({ "id": c })).collect::<Vec<_>>() });
        // Shown dogs in no category or in several; adopted ones only in theirs.
        let products = json!([
            product(1, &[]),
            product(2, &[64]),
            product(3, &[]),
            product(4, &[45]),
            product(5, &[45]),
            product(6, &[45]),
            product(7, &[])
        ])
        .to_string();
        let kinds = json!({ "product": { "rest_base": "product", "taxonomies": [] } }).to_string();
        let shown = ["https://site.test/p/1/", "https://site.test/p/2/", "https://site.test/p/3/"];
        let answers = vec![("/wp/v2/types", kinds), ("/wc/store/v1/products", products)];
        assert_eq!(
            discover(HEAD, &shown, answers.clone()).await.0.unwrap().as_str(),
            "https://www.site.test/wp-json/wc/store/v1/products?per_page=100&category=45&category_operator=not_in",
            "a shop told by its kinds, with no plugin name in the page"
        );
        let (found, _) = discover_listed(HEAD, &shown, 3, "", answers.clone()).await;
        assert_eq!(found, None);
        let (found, asked) = discover_listed(HEAD, &shown, 2, "", answers.clone()).await;
        assert!(found.is_some(), "a JSON-LD that lists fewer than the API");
        assert!(!asked.iter().any(|a| a.contains("categories")), "no question, no term to look for");
        let categories = json!([
            { "id": 45, "name": "Todos los ADOPTADOS", "count": 1395 },
            { "id": 29, "name": "Gatos en adopción", "count": 25 },
            { "id": 28, "name": "Perros en adopción", "count": 68 },
        ])
        .to_string();
        let answers = [answers, vec![("/wc/store/v1/products/categories", categories)]].concat();
        let (found, _) = discover_listed(HEAD, &shown, 0, "Los perros en adopción de esta protectora", answers).await;
        assert_eq!(
            found.unwrap().as_str(),
            "https://www.site.test/wp-json/wc/store/v1/products?per_page=100&category=28",
            "the category the question names, before leaving out the ones the page shows none of"
        );
    }

    #[test]
    fn the_term_a_question_names_is_its_longest_then_largest_one_of_more_than_small_words() {
        let terms = |list: Value| list.to_string();
        let asked = "Los perros en adopción de esta protectora";
        let both = terms(
            json!([{ "id": 1, "name": "Perros", "count": 90 }, { "id": 2, "name": "Perros en adopción", "count": 60 }]),
        );
        assert_eq!(named_term(&both, asked), Some(2));
        assert_eq!(named_term(&terms(json!([{ "id": 3, "name": "En", "count": 9 }])), asked), None);
        assert_eq!(named_term(&terms(json!([{ "id": 4, "name": "Perros", "count": 2 }])), asked), None);
        assert_eq!(named_term(&terms(json!([{ "id": 5, "name": "Gatos en adopción", "count": 30 }])), asked), None);
    }

    #[test]
    fn a_wordpress_page_that_links_no_api_root_has_it_at_its_site_s_root() {
        let page = u("https://site.test/agenda/");
        let theme = r#"<html><head><link rel="stylesheet" href="/wp-content/themes/t/style.css"></head></html>"#;
        assert_eq!(api_root(theme, &page).unwrap().as_str(), "https://site.test/wp-json/");
        assert_eq!(api_root("<html><head></head></html>", &page), None);
    }

    #[tokio::test]
    async fn a_shop_s_products_are_narrowed_to_the_category_the_shown_ones_share_and_events_are_read_as_given() {
        let product = |n: u32, cats: &[u64]| json!({ "permalink": format!("https://site.test/p/{n}/"), "categories": cats.iter().map(|c| json!({ "id": c, "name": "x" })).collect::<Vec<_>>() });
        let products = json!([product(1, &[5, 9]), product(2, &[5]), product(3, &[5]), product(4, &[8])]).to_string();
        let shop = format!("{HEAD}<body class=\"woocommerce\">");
        let shown = ["https://site.test/p/1/", "https://site.test/p/2/", "https://site.test/p/3/"];
        let answers = vec![("/wc/store/v1/products", products)];
        assert_eq!(
            discover(&shop, &shown, answers).await.0.unwrap().as_str(),
            "https://www.site.test/wp-json/wc/store/v1/products?per_page=100&category=5"
        );
        let events = json!({ "events": (1..=3).map(|n| json!({ "url": format!("https://site.test/event/{n}/") })).collect::<Vec<_>>() }).to_string();
        let calendar = format!("{HEAD}<div class=\"tribe-events\">");
        let shown = ["https://site.test/event/1/", "https://site.test/event/2/", "https://site.test/event/3/"];
        assert_eq!(
            discover(&calendar, &shown, vec![("/tribe/events/v1/events", events)]).await.0.unwrap().as_str(),
            "https://www.site.test/wp-json/tribe/events/v1/events?per_page=50"
        );
    }

    #[test]
    fn a_batch_gives_each_thing_as_the_page_shows_it_and_the_batch_after_a_full_one() {
        let url = u("https://site.test/wp-json/wp/v2/shows?per_page=2&_embed=wp%3Afeaturedmedia");
        let body = json!([
            { "id": 1, "link": "https://site.test/shows/1/", "date": "2026-09-01T10:00:00",
              "title": { "rendered": "Hamlet &#8211; <em>new</em>" }, "excerpt": { "rendered": "<p>A  play.</p>\n" },
              "_links": { "self": [{ "href": "x" }] },
              "_embedded": { "wp:featuredmedia": [{ "source_url": "https://site.test/h.jpg", "media_details": { "sizes": {} } }] } },
            { "id": 2, "link": "https://site.test/shows/2/", "title": { "rendered": "" } },
        ])
        .to_string();
        let (data, next) = read(&body, &url).unwrap();
        let (name, value) = data.entries().next().unwrap();
        assert_eq!(name, "json:fetch:/wp-json/wp/v2/shows");
        assert_eq!(
            value,
            &json!([{ "name": "Hamlet – new", "url": "https://site.test/shows/1/", "published": "2026-09-01T10:00:00",
                      "summary": "A play.", "image": "https://site.test/h.jpg" }])
        );
        assert_eq!(
            next.unwrap().as_str(),
            "https://site.test/wp-json/wp/v2/shows?per_page=2&_embed=wp%3Afeaturedmedia&page=2"
        );
        let last = read(&json!([{ "link": "a", "title": { "rendered": "A" } }]).to_string(), &url).unwrap();
        assert_eq!(last.1, None, "a batch that is not full is the last");
        assert!(read("[]", &u("https://site.test/api/list")).is_none());
    }

    #[test]
    fn a_product_s_price_is_given_in_its_currency_and_an_event_s_next_batch_is_the_one_it_names() {
        let url = u("https://site.test/wp-json/wc/store/v1/products?per_page=100");
        let body = json!([{ "name": "Curso de sushi", "permalink": "https://site.test/p/1/",
            "prices": { "price": "1350", "regular_price": "1500", "currency_minor_unit": 2, "currency_code": "EUR" },
            "on_sale": true, "images": [{ "src": "https://site.test/s.jpg" }], "categories": [{ "id": 5, "name": "Cursos &amp; talleres" }],
            "is_in_stock": true }])
        .to_string();
        let (data, _) = read(&body, &url).unwrap();
        assert_eq!(
            data.entries().next().unwrap().1,
            &json!([{ "name": "Curso de sushi", "url": "https://site.test/p/1/", "price": "13.50", "regular_price": "15.00",
                      "currency": "EUR", "image": "https://site.test/s.jpg", "categories": "Cursos & talleres", "in_stock": true }])
        );
        assert_eq!(decimal("5", 2).as_deref(), Some("0.05"));
        let url = u("https://site.test/wp-json/tribe/events/v1/events?per_page=50");
        let body = json!({ "events": [{ "title": "Concierto", "url": "https://site.test/e/1/", "start_date": "2026-11-05 21:00:00",
            "cost": "15 €", "image": { "url": "https://site.test/e.jpg", "sizes": {} }, "venue": { "venue": "Sala" } }],
            "next_rest_url": "https://site.test/wp-json/tribe/events/v1/events?per_page=50&page=2" })
        .to_string();
        let (data, next) = read(&body, &url).unwrap();
        assert_eq!(
            data.entries().next().unwrap().1,
            &json!([{ "name": "Concierto", "url": "https://site.test/e/1/", "start_date": "2026-11-05 21:00:00",
                      "cost": "15 €", "image": "https://site.test/e.jpg", "venue": "Sala" }])
        );
        assert_eq!(next.unwrap().as_str(), "https://site.test/wp-json/tribe/events/v1/events?per_page=50&page=2");
    }

    #[test]
    fn a_post_named_by_its_query_is_an_item_but_a_query_on_a_listing_is_not() {
        assert!(item_key(&u("https://site.test/?p=12")).is_some());
        assert_eq!(
            item_key(&u("https://site.test/?post_type=show&p=5")),
            item_key(&u("https://site.test/?p=5&post_type=show"))
        );
        assert!(item_key(&u("https://site.test/?s=hamlet")).is_none());
        assert!(item_key(&u("https://site.test/shows/?page=2")).is_none());
        assert!(item_key(&u("https://site.test/")).is_none());
    }

    #[test]
    fn days_and_dates_go_both_ways() {
        for date in [(1970, 1, 1), (2024, 2, 29), (2026, 3, 1), (2000, 12, 31)] {
            assert_eq!(civil(days(date)), date);
        }
        assert_eq!(civil(days((2026, 3, 1)) - 1), (2026, 2, 28));
    }
}

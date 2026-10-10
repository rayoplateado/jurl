//! `--follow`: when the page doesn't answer, look for the answer elsewhere on the same site, the way people play the
//! Wikipedia game. It is `--precise` (or `-q`) and `--links -q` in a loop: each page is asked for the answer and its
//! links are scored by how likely they lead to it, jurl opens the best few at once, and keeps going from whichever
//! page looks closest, backtracking when a trail goes cold (hot and cold). The site's own map (`llms.txt`,
//! `sitemap.xml`) is read alongside the first page: it often names the right page outright.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use futures::future::join_all;
use reqwest::Client;
use serde_json::Map;
use url::Url;

use crate::{
    answer::{PRECISE_BLOCK_FLOOR, PRECISE_BLOCKS, Pick, precise_pick, render_precise},
    blocks::{render_blocks, score_blocks, top},
    cli::Args,
    config::Config,
    decide::is_api_error,
    extract::{self, Extracted, Kind, Link},
    fetch::{Retry, Served},
    judge::{Ctx, Item, is_block_page},
    links::{self, FieldScores},
    load,
    output::{Rendered, missed, not_found},
    timing::Timer,
};

/// Pages opened at once on each step. Two, so the default 5 pages are two full steps (1 + 2 + 2): answers two links
/// away are within reach, and when Jev's first pick is right (most of the time) the second page costs little.
const PARALLEL: usize = 2;
/// A long search (`--follow 10` and up) goes wider: a long trail needs more than one or two guesses per step.
const PARALLEL_LONG: usize = 3;
/// URLs from the site map scored, like a page's links: at most this many besides the start page. Not fewer, and picked
/// by shared words then depth rather than ranked closer: an answer can sit behind a URL with none of the question's
/// words (Grafana's default port, in `/tutorials/grafana-fundamentals`), and only Jev reading the list finds it.
const MAX_HINTS: usize = 300;
/// A link's score is discounted per hop, so a good lead near the start beats a slightly better one deep down.
const HOP_DECAY: f64 = 0.85;
/// Hot or cold: a link is worth as much as the page it's on is close to the answer. A page with nothing on the
/// question still passes on this share of its links' scores, so a home page's menu stays usable.
const COLD_PAGE: f64 = 0.3;
/// When the best lead left is weaker than this, the trail has gone cold: stop instead of opening pages blindly.
const COLD_TRAIL: f64 = 0.02;
/// Each step, Jev compares this many of the best leads side by side to pick the next pages.
const SHORTLIST: usize = 10;
/// A page found to answer is kept unless a lead left could beat it by this much: a near tie isn't worth more pages.
const BETTER_BY: f64 = 0.15;
/// On a long search the answer is pages away, through the pages' text. A site's menus (its home page, contents,
/// search, a random page) lead everywhere, so on a page far from the question they look like the best way: they count
/// for this share of their score.
const LONG_MENU: f64 = 0.3;
const ASSETS: &[&str] = &[
    ".pdf", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".zip", ".gz", ".xml", ".json", ".css", ".js", ".mp4",
    ".mp3", ".dmg", ".exe", ".tar", ".ico",
];

/// The site: its host without `www.`, subdomains included (docs.example.com belongs to example.com).
struct Site {
    root: String,
    /// The search started at the site's front door, so the site itself is what the question is about.
    front_door: bool,
}

impl Site {
    fn new(u: &Url) -> Self {
        let root = u.host_str().unwrap_or_default().trim_start_matches("www.").to_string();
        Site { root, front_door: u.path() == "/" && u.query().is_none() }
    }

    /// Whose site it is, for [`Ctx::ask`]: only from the front door. Started from a page (an article, a repo),
    /// "it" is what the page is about, not who runs the site.
    fn owner(&self) -> Option<String> {
        self.front_door.then(|| self.root.clone())
    }

    /// The context to ask Jev about one of the site's pages: the owner rides along (see [`Site::owner`]).
    fn ctx<'a>(
        &self,
        args: &'a Args,
        client: &'a Client,
        api_key: &'a str,
        url: &'a Url,
        ex: &'a Extracted,
    ) -> Ctx<'a> {
        Ctx { owner: self.owner(), ..Ctx::new(args, client, api_key, url, ex) }
    }

    /// Whether `u` is a web page a search may open, on any host: http or https, and not a file (an image, a PDF, an
    /// archive). A page's links are candidates whatever their host, because the page gives them (see [`bare_links`]).
    fn reachable(u: &Url) -> bool {
        let lower = u.path().to_lowercase();
        matches!(u.scheme(), "http" | "https") && !ASSETS.iter().any(|ext| lower.ends_with(ext))
    }

    /// Whether `u` is on the site: [`Site::reachable`], and on the site's host or a subdomain of it.
    fn contains(&self, u: &Url) -> bool {
        Site::reachable(u)
            && u.host_str().is_some_and(|h| {
                let h = h.trim_start_matches("www.");
                h == self.root || h.ends_with(&format!(".{}", self.root))
            })
    }
}

/// `robots.txt` for every crawler (`User-agent: *`) on one host: the paths jurl won't open, decided as RFC 9309 §2.2.2
/// decides them (see [`Robots::allows`]).
#[derive(Default)]
struct Robots {
    rules: Vec<Rule>,
}

impl Robots {
    /// The rules in the `robots.txt` of `u`'s host. No file, or none within 4 s: no rules.
    async fn load(u: &Url, retry: Retry<'_>) -> Self {
        let Ok(url) = u.join("/robots.txt") else { return Self::default() };
        let Some(body) = small_text(&url, retry).await else { return Self::default() };
        Self::parse(&body)
    }

    fn parse(body: &str) -> Self {
        let mut rules = Vec::new();
        let (mut ours, mut in_rules) = (false, false);
        for line in body.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((field, value)) = line.split_once(':') else { continue };
            let (field, value) = (field.trim().to_lowercase(), value.trim());
            match field.as_str() {
                "user-agent" => {
                    // A new group starts at the first user-agent after some rules.
                    if in_rules {
                        ours = false;
                        in_rules = false;
                    }
                    ours |= value == "*";
                }
                "disallow" | "allow" => {
                    in_rules = true;
                    // An empty pattern says nothing; a rule in no group is not one of ours.
                    if ours && !value.is_empty() {
                        rules.push(Rule::new(field == "allow", value));
                    }
                }
                _ => {}
            }
        }
        Robots { rules }
    }

    /// Whether the rules allow `u`, as RFC 9309 §2.2.2 says: the rule that matches with the most octets decides, and
    /// an allow wins a tie. No rule matching allows it, and so does `/robots.txt` itself.
    fn allows(&self, u: &Url) -> bool {
        if u.path() == "/robots.txt" {
            return true;
        }
        let path = match u.query() {
            Some(query) => normalize(&format!("{}?{query}", u.path())),
            None => normalize(u.path()),
        };
        self.rules.iter().filter(|r| r.matches(&path)).max_by_key(|r| (r.len, r.allow)).is_none_or(|r| r.allow)
    }
}

/// One `Allow` or `Disallow` line of the `*` group.
struct Rule {
    allow: bool,
    /// The line's pattern as written, `*` and `$` included: its length is the octets a match is judged by.
    len: usize,
    /// The pattern between its `*`s, each piece in the form [`normalize`] gives it.
    pieces: Vec<String>,
    /// The pattern ends in `$`: the match must reach the end of the path and query.
    anchored: bool,
}

impl Rule {
    fn new(allow: bool, pattern: &str) -> Self {
        let (body, anchored) = match pattern.strip_suffix('$') {
            Some(body) => (body, true),
            None => (pattern, false),
        };
        Rule { allow, len: pattern.len(), pieces: body.split('*').map(normalize).collect(), anchored }
    }

    /// Whether the pattern matches the start of `path` (normalized, with its query): the first piece is a prefix, each
    /// middle piece is found after the one before, and the last piece is at the end when the pattern is anchored, or
    /// anywhere after the rest when it is not.
    fn matches(&self, path: &str) -> bool {
        let Some((first, rest)) = self.pieces.split_first() else { return false };
        let Some(mut at) = path.strip_prefix(first.as_str()) else { return false };
        let Some((last, middle)) = rest.split_last() else { return !self.anchored || at.is_empty() };
        for piece in middle {
            let Some(i) = at.find(piece.as_str()) else { return false };
            at = &at[i + piece.len()..];
        }
        if self.anchored { at.ends_with(last.as_str()) } else { at.contains(last.as_str()) }
    }
}

/// `s` as RFC 9309 §2.2.2 compares a path: an unreserved octet (a letter, a digit, `-`, `.`, `_` or `~`) is itself,
/// raw or percent-encoded, and every other octet is percent-encoded in upper-case hex. So `/foo/bar/ツ` and
/// `/foo/bar/%E3%83%84` are the same path, and `%62` is `b`. Both the rules and the URL are compared in this form.
fn normalize(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let (octet, next) = match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&h), Some(&l)) if h.is_ascii_hexdigit() && l.is_ascii_hexdigit() => {
                (hex_value(h) * 16 + hex_value(l), i + 3)
            }
            (octet, _, _) => (octet, i + 1),
        };
        if octet.is_ascii_alphanumeric() || matches!(octet, b'-' | b'.' | b'_' | b'~') {
            out.push(octet as char);
        } else {
            out.push_str(&format!("%{octet:02X}"));
        }
        i = next;
    }
    out
}

/// The value of a hex digit (checked by the caller).
fn hex_value(digit: u8) -> u8 {
    (digit as char).to_digit(16).unwrap_or_default() as u8
}

/// The key a host's `robots.txt` is kept under: the host without `www.`, as [`Site`] counts hosts.
fn host_key(u: &Url) -> Option<&str> {
    u.host_str().map(|h| h.trim_start_matches("www."))
}

/// Each host's `robots.txt`, by [`host_key`]. A host's file is read the first time one of its leads is among the best
/// few, so a host the search never gets to costs no request, and a subdomain's pages are checked against its own file.
/// A host with no usable file is kept with no rules, so it isn't asked again.
#[derive(Default)]
struct RobotsByHost {
    by_host: HashMap<String, Robots>,
}

impl RobotsByHost {
    /// Whether `u`'s host's rules allow it. A host not read yet allows it: its leads stay in the list until its file is
    /// read, and only then can a rule drop them.
    fn allows(&self, u: &Url) -> bool {
        host_key(u).and_then(|h| self.by_host.get(h)).is_none_or(|r| r.allows(u))
    }

    fn insert(&mut self, u: &Url, robots: Robots) {
        if let Some(host) = host_key(u) {
            self.by_host.insert(host.to_string(), robots);
        }
    }

    /// Reads the `robots.txt` of each host of `urls` that has none yet, all at once. Returns how many it read.
    async fn load_for(&mut self, urls: &[Url], retry: Retry<'_>) -> usize {
        let mut new: Vec<&Url> = Vec::new();
        for u in urls {
            let Some(host) = host_key(u) else { continue };
            if !self.by_host.contains_key(host) && !new.iter().any(|v| host_key(v) == Some(host)) {
                new.push(u);
            }
        }
        let read = join_all(new.iter().map(|u| Robots::load(u, retry))).await;
        for (u, robots) in new.iter().zip(read) {
            self.insert(u, robots);
        }
        new.len()
    }
}

/// The most a small text file (robots.txt, llms.txt, a sitemap) is read: a body past this is refused as it arrives, not
/// read whole first.
const SMALL_TEXT_MAX: usize = 5_000_000;
/// How long a small text file may take, the body included.
const SMALL_TEXT_TIMEOUT: Duration = Duration::from_secs(4);

/// A small text file from the site (robots.txt, llms.txt, a sitemap), or nothing. A host that a retry showed needs the
/// browser client gets its files from there, as its pages do (see [`Retry::sticky`]).
async fn small_text(url: &Url, retry: Retry<'_>) -> Option<String> {
    small_text_within(url, SMALL_TEXT_MAX, retry).await
}

/// [`small_text`] with its cap given. The body is read and decoded as fetch.rs reads and decodes a page.
async fn small_text_within(url: &Url, max: usize, retry: Retry<'_>) -> Option<String> {
    // The same retry rule as a page's, with the same memo: the file is read through it (see `fetch::fetch_small`).
    let body = crate::fetch::fetch_small(url, max, SMALL_TEXT_TIMEOUT, retry, Instant::now()).await?;
    (!body.trim_start().starts_with('<') || body.contains("<urlset") || body.contains("<sitemapindex")).then_some(body)
}

/// The pages a child sitemap of a sitemap index lists, or none if it can't be read.
async fn child_map(sitemap: Url, retry: Retry<'_>) -> Vec<String> {
    small_text(&sitemap, retry).await.map(|b| locs(&b)).unwrap_or_default()
}

/// The pages one sitemap lists: its `<loc>`s, or for a sitemap index, the pages of its newest few sitemaps (see
/// [`crate::sitemaps::index_children`]).
async fn sitemap_pages(sitemap: &Url, retry: Retry<'_>) -> Vec<String> {
    let Some(body) = small_text(sitemap, retry).await else { return Vec::new() };
    if !body.contains("<sitemapindex") {
        return locs(&body);
    }
    let children: Vec<Url> = crate::sitemaps::index_children(&body).iter().filter_map(|c| Url::parse(c).ok()).collect();
    join_all(children.into_iter().map(|c| child_map(c, retry))).await.into_iter().flatten().collect()
}

/// The sitemaps robots.txt names that are read: [`crate::sitemaps::ROBOTS_SITEMAPS`] of them, in file order, each once,
/// on the start page's own site (a sitemap of another host is read too: its URLs are kept only if they are the site's).
/// `skip` is /sitemap.xml, which is read already.
fn robots_sitemap_urls(body: &str, start: &Url, skip: Option<&Url>) -> Vec<Url> {
    let mut urls: Vec<Url> = Vec::new();
    for line in crate::sitemaps::robots_sitemaps(body) {
        let Ok(u) = start.join(&line) else { continue };
        if !matches!(u.scheme(), "http" | "https") || Some(&u) == skip || urls.contains(&u) {
            continue;
        }
        urls.push(u);
        if urls.len() == crate::sitemaps::ROBOTS_SITEMAPS {
            break;
        }
    }
    urls
}

/// What the site lists about itself, read beside the first page (see [`site_files`]).
struct SiteFiles {
    /// The start host's robots.txt, if it has a usable one. Its rules are the search's (see [`Search::start`]).
    robots: Option<String>,
    /// The pages the site lists: llms.txt, then its sitemaps, shallowest first. A page listed in several languages keeps
    /// one copy (see [`without_language_copies`]).
    links: Vec<Link>,
}

/// The site's own files, read at once: `robots.txt`, `llms.txt` (written for exactly this) and `/sitemap.xml`, and the
/// sitemaps `robots.txt` names (its `Sitemap:` lines, see [`robots_sitemap_urls`]), which wait for the robots file. A
/// sitemap index is read through its newest sitemaps (see [`sitemap_pages`]).
async fn site_files(start: &Url, site: &Site, retry: Retry<'_>) -> SiteFiles {
    let robots = async {
        let url = start.join("/robots.txt").ok()?;
        small_text(&url, retry).await
    };
    let llms = async {
        let url = start.join("/llms.txt").ok()?;
        let body = small_text(&url, retry).await?;
        Some(extract::markdown(&body, &url).links)
    };
    let own_url = start.join("/sitemap.xml").ok();
    let own = async {
        match &own_url {
            Some(url) => sitemap_pages(url, retry).await,
            None => Vec::new(),
        }
    };
    let (robots, llms, own) = tokio::join!(robots, llms, own);
    let listed = robots_sitemap_urls(robots.as_deref().unwrap_or_default(), start, own_url.as_ref());
    let listed_pages = join_all(listed.iter().map(|u| sitemap_pages(u, retry))).await;

    let mut out: Vec<Link> = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |url: Url, text: String| {
        if site.contains(&url) && seen.insert(links::key(&url)) {
            out.push(Link { i: out.len(), url, text, context: String::new(), marginal: false });
        }
    };
    for l in llms.unwrap_or_default() {
        add(l.url, l.text);
    }
    let mut pages: Vec<Url> =
        own.into_iter().chain(listed_pages.into_iter().flatten()).filter_map(|u| Url::parse(&u).ok()).collect();
    pages.sort_by_key(|u| (u.path_segments().map_or(0, |s| s.filter(|p| !p.is_empty()).count()), u.as_str().len()));
    for u in pages {
        let text = u.path().to_string();
        add(u, text);
    }
    SiteFiles { robots, links: without_language_copies(start, out) }
}

/// The pages the sitemaps of each of `hosts` list (see [`crate::hosts::linked_hosts`]): the host's /sitemap.xml and the
/// sitemaps its robots.txt names, read as the start site's own are (see [`site_files`]). Kept when they share the start
/// page's registrable domain.
async fn linked_sitemaps(start: &Url, hosts: &[Url], retry: Retry<'_>) -> Vec<Link> {
    let per_host = join_all(hosts.iter().map(|origin| async move {
        let own = origin.join("/sitemap.xml").ok();
        let robots = match origin.join("/robots.txt") {
            Ok(url) => small_text(&url, retry).await,
            Err(_) => None,
        };
        let mut urls: Vec<String> = Vec::new();
        if let Some(own) = &own {
            urls.extend(sitemap_pages(own, retry).await);
        }
        for listed in robots_sitemap_urls(robots.as_deref().unwrap_or_default(), origin, own.as_ref()) {
            urls.extend(sitemap_pages(&listed, retry).await);
        }
        urls
    }))
    .await;
    per_host
        .into_iter()
        .flatten()
        .filter_map(|u| Url::parse(&u).ok())
        .filter(|u| crate::hosts::same_registrable(start, u))
        .map(|url| Link { i: 0, text: url.path().to_string(), url, context: String::new(), marginal: false })
        .collect()
}

/// The llms.txt of each of `hosts` (see [`crate::hosts::linked_hosts`]): the pages it lists, kept when they share the start
/// page's registrable domain. Read as the start site's own llms.txt is (see [`site_files`]).
async fn linked_llms(start: &Url, hosts: &[Url], retry: Retry<'_>) -> Vec<Link> {
    let files = join_all(hosts.iter().filter_map(|h| h.join("/llms.txt").ok()).map(|url| async move {
        let body = small_text(&url, retry).await?;
        Some(extract::markdown(&body, &url).links)
    }))
    .await;
    files.into_iter().flatten().flatten().filter(|l| crate::hosts::same_registrable(start, &l.url)).collect()
}

/// Languages a site puts in its paths (`/de/pricing`, `/pt-br/pricing`): ISO 639-1 codes, without the ones that name
/// another country's market (`ca`, `uk`, `be`, `se`, `br`, `za`, `ee`, `et`, `my` and `ar`: `/ca/` is Canada, not
/// Catalan).
const LOCALES: &[&str] = &[
    "bg", "cs", "da", "de", "el", "en", "es", "fa", "fi", "fr", "he", "hi", "hr", "hu", "id", "it", "ja", "ko", "lt",
    "lv", "ms", "nb", "nl", "nn", "no", "pl", "pt", "ro", "ru", "sk", "sl", "sr", "sv", "th", "tr", "ur", "vi", "zh",
];

/// Whether a path segment is a locale: a language from [`LOCALES`], optionally with a two-letter region (`es-es`,
/// `pt-br`, `en-US`). A country code that isn't a language (`us`, as in `/us/en/`) isn't one.
fn is_locale(segment: &str) -> bool {
    let s = segment.to_ascii_lowercase();
    let (lang, region) = match s.split_once('-') {
        Some((lang, region)) => (lang, Some(region)),
        None => (s.as_str(), None),
    };
    LOCALES.contains(&lang) && region.is_none_or(|r| r.len() == 2 && r.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The locale a URL's first path segment names (lowercased), if it names one, and the URL's key without that segment:
/// the same page in every language has the same key.
fn locale_key(u: &Url) -> (Option<String>, String) {
    let segments: Vec<&str> = u.path().split('/').filter(|s| !s.is_empty()).collect();
    let (locale, rest) = match segments.split_first() {
        Some((first, rest)) if is_locale(first) => (Some(first.to_ascii_lowercase()), rest),
        _ => (None, segments.as_slice()),
    };
    let mut base = u.clone();
    base.set_path(&format!("/{}", rest.join("/")));
    (locale, links::key(&base))
}

/// A page the site lists in several languages keeps one copy: the one in the start page's language if the site lists
/// it, else the one with no language prefix. A page listed only in other languages keeps all of them. Renumbers the
/// links that stay.
fn without_language_copies(start: &Url, links: Vec<Link>) -> Vec<Link> {
    let own = locale_key(start).0;
    let keys: Vec<(Option<String>, String)> = links.iter().map(|l| locale_key(&l.url)).collect();
    let own_pages: HashSet<&str> =
        keys.iter().filter(|(locale, _)| *locale == own).map(|(_, key)| key.as_str()).collect();
    let plain_pages: HashSet<&str> =
        keys.iter().filter(|(locale, _)| locale.is_none()).map(|(_, key)| key.as_str()).collect();
    let keep: Vec<bool> = keys
        .iter()
        .map(|(locale, key)| {
            if own_pages.contains(key.as_str()) {
                *locale == own
            } else if plain_pages.contains(key.as_str()) {
                locale.is_none()
            } else {
                true
            }
        })
        .collect();
    links.into_iter().zip(keep).filter(|(_, keep)| *keep).enumerate().map(|(i, (l, _))| Link { i, ..l }).collect()
}

/// Every `<loc>…</loc>` in a sitemap.
fn locs(xml: &str) -> Vec<String> {
    xml.split("<loc>")
        .skip(1)
        .filter_map(|s| s.split("</loc>").next())
        .map(|s| s.trim().replace("&amp;", "&"))
        .collect()
}

/// What a page said about the question.
enum Found {
    Precise(Pick),
    Blocks { scores: Vec<Option<f64>>, keep: Vec<(usize, f64)>, kind: Option<(String, f64)> },
}

/// A link on a page, and how likely following it leads to the answer (`p`, before hops and heat).
struct ScoredLink {
    url: Url,
    text: String,
    p: f64,
}

struct Visit {
    url: Url,
    ex: Extracted,
    found: Option<Found>,
    /// How sure jurl is that this page answers: the --precise answer's p, or the best block's.
    score: f64,
    /// How close the page is to the question at all: its best block's probability of helping answer it.
    warmth: f64,
    links: Vec<ScoredLink>,
    /// Its menus and footers, see [`menus`].
    menus: HashSet<String>,
    /// The field scores this page asked Jev for, by link (see [`FieldScores`]): the search keeps them for later pages.
    new_field_scores: FieldScores,
    /// How the page was served, for the usage's `route`.
    served: Served,
}

/// What `-t` prints for each page read: how warm the page was, and how sure it is of an answer.
struct Reading {
    url: Url,
    warmth: f64,
    score: f64,
}

/// A page that answers: `rank` is how sure jurl is of the answer and of the page, times the score of the lead that led
/// to it. `path` is how jurl got there.
struct Ranked {
    rank: f64,
    visit: Visit,
    path: Vec<Url>,
}

/// A page's links outside its text: menus and footers, which a site repeats on every page.
fn menus(ex: &Extracted) -> HashSet<String> {
    let text: HashSet<String> = ex.links.iter().map(|l| links::key(&l.url)).collect();
    ex.site_links.iter().map(|l| links::key(&l.url)).filter(|k| !text.contains(k)).collect()
}

/// The URLs a page writes out in full in its text (see [`crate::hosts::bare_urls`]): links the page gives as much as its
/// `<a>`s do, so `--follow` may open them too, on any host. A code block's URLs are not what the page says to a reader,
/// so they are left out. The link's context is the text around its URL.
fn bare_links(ex: &Extracted) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    for block in ex.blocks.iter().filter(|b| b.kind != Kind::Code) {
        for url in crate::hosts::bare_urls(&block.text) {
            let context = around(&block.text, url.as_str());
            out.push(Link { i: out.len(), url, text: String::new(), context, marginal: false });
        }
    }
    out
}

/// Up to 100 characters on each side of `needle` in `text` (the whole text if it is shorter, or `needle` is not in it).
fn around(text: &str, needle: &str) -> String {
    let at = text.find(needle).unwrap_or(0);
    let start = text[..at].char_indices().rev().nth(99).map_or(0, |(i, _)| i);
    let end = text[at..].char_indices().nth(100).map_or(text.len(), |(i, _)| at + i);
    text[start..end].to_string()
}

/// Read one page: load it, then judge it (see [`judge`]).
#[allow(clippy::too_many_arguments)]
async fn visit(
    args: &Args,
    cfg: &Config,
    client: &Client,
    api_key: &str,
    url: &Url,
    site: &Site,
    known: &HashSet<String>,
    field_scores: &FieldScores,
    read: &HashSet<String>,
) -> Result<Visit> {
    let mut t = Timer::new();
    let (url, ex, served) = load(args, cfg, url, &mut t).await?;
    judge(args, client, api_key, url, ex, site, known, field_scores, read, served).await
}

/// Is the answer on a page that is loaded, and which of its links lead on? Both questions go to Jev at once. `known` holds
/// the menu links of the pages read before, and `field_scores` the field scores the search has so far.
#[allow(clippy::too_many_arguments)]
async fn judge(
    args: &Args,
    client: &Client,
    api_key: &str,
    url: Url,
    mut ex: Extracted,
    site: &Site,
    known: &HashSet<String>,
    field_scores: &FieldScores,
    read: &HashSet<String>,
    served: Served,
) -> Result<Visit> {
    // A lead can read as a page read already (a markdown lead whose HTML page the search read): it is not judged again.
    if read.contains(&links::key(&url)) {
        bail!("already read: {url}");
    }
    let menus = menus(&ex);
    let bare = bare_links(&ex);
    let (found, score, warmth, links, new_field_scores, json_block) = {
        let ctx = site.ctx(args, client, api_key, &url, &ex);
        // The links `--links -q` would score, menus and footers included, and the URLs the text writes out. Any host:
        // the page gives them, so a link to another domain is a candidate like any other, and is scored like one. A menu
        // link is scored on the first page it's on, not again on every page: on a page far from the question, the
        // site's "Main page" and "Search" would outscore everything in its text.
        let candidates = links::candidates(&ctx, &ex, &bare, |u| {
            let k = links::key(u);
            Site::reachable(u) && !(menus.contains(&k) && known.contains(&k))
        });
        // The answer the way `-q` finds it (score_blocks), and with --precise, the way --precise picks it.
        let answer = async {
            let mut t = Timer::new();
            // A page with nothing to read is a page without the answer; Jev failing, or the page being a block page,
            // fails the read.
            let (scores, kind) = match score_blocks(&ctx, &ex, &mut t).await {
                Ok(s) => s,
                Err(e) if is_api_error(&e) || is_block_page(&e) => return Err(e),
                Err(_) => return Ok((None, 0.0)),
            };
            let warmth = scores.iter().flatten().copied().fold(0.0, f64::max);
            if args.precise {
                let keep = top(&scores, PRECISE_BLOCK_FLOOR, PRECISE_BLOCKS);
                if keep.is_empty() {
                    return Ok((None, warmth));
                }
                match precise_pick(&ctx, &ex, &keep, &mut t).await {
                    Ok(pick) => {
                        let p = pick.p;
                        Ok((Some((Found::Precise(pick), p)), warmth))
                    }
                    Err(e) if is_api_error(&e) || is_block_page(&e) => Err(e),
                    Err(_) => Ok((None, warmth)),
                }
            } else {
                let keep = top(&scores, args.threshold(), args.limit(5));
                let best = keep.iter().map(|&(_, p)| p).fold(0.0, f64::max);
                Ok(((!keep.is_empty()).then_some((Found::Blocks { scores, keep, kind }, best)), warmth))
            }
        };
        // `--links -q` on the same page, at the same time; a long search also asks for the answer's field, except for
        // the links whose field the search already has.
        let long = args.follow.unwrap_or(5) >= 10;
        let leads = links::score(&ctx, &candidates, "Following the link in `links`", long.then_some(field_scores));
        let (answer, scores) = tokio::join!(answer, leads);
        let (answer, warmth) = answer?;
        // --precise, and the page's text gives no answer above the threshold: its JSON-LD values are judged on their own
        // (see `blocks::structured_pick`); the one that answers is appended to the page, so its pick is rendered from it.
        let (answer, json_block) = match answer {
            Some((Found::Precise(pick), p)) if p >= args.threshold_for(true) => (Some((Found::Precise(pick), p)), None),
            text => {
                let mut t = Timer::new();
                match crate::blocks::structured_pick(&ctx, &ex, &mut t).await? {
                    Some((block, pick)) => {
                        let p = pick.p;
                        (Some((Found::Precise(pick), p)), Some(block))
                    }
                    None => (text, None),
                }
            }
        };
        let (scores, new_field_scores) = match scores {
            Ok(s) => s,
            Err(e) if is_api_error(&e) || is_block_page(&e) => return Err(e),
            Err(_) => (vec![0.0; candidates.len()], FieldScores::new()),
        };
        let links = candidates
            .into_iter()
            .zip(scores)
            .map(|(l, p)| {
                let menu = long && menus.contains(&links::key(&l.url));
                ScoredLink { url: l.url, text: l.text, p: if menu { p * LONG_MENU } else { p } }
            })
            .collect();
        match answer {
            Some((found, score)) => (Some(found), score, warmth, links, new_field_scores, json_block),
            None => (None, 0.0, warmth, links, new_field_scores, json_block),
        }
    };
    if let Some(block) = json_block {
        ex.blocks.push(block);
    }
    Ok(Visit { url, ex, found, score, warmth, links, menus, new_field_scores, served })
}

/// A page waiting to be opened, and how jurl would get there.
struct Lead {
    url: Url,
    text: String,
    score: f64,
    /// The link's own score, before hops and heat.
    p: f64,
    path: Vec<Url>,
}

/// Jev compares the best leads side by side ("which of these is the next step?"): scores given to links on different
/// pages one at a time aren't on the same scale. Returns the shortlist's indices, reordered by Jev's choice, or `None`
/// when Jev gives no choice: the leads keep their own order. "None of these" is asked too (an option so the others
/// aren't forced to look good), but its share isn't used: the rest are ordered by their own shares. An API error ends
/// the search.
async fn rank_next_step(ctx: &Ctx<'_>, leads: &[&Lead]) -> Result<Option<Vec<usize>>> {
    let q = ctx.ask();
    let items: Vec<Item> = leads
        .iter()
        .enumerate()
        .map(|(i, l)| Item {
            state: serde_json::json!({ "i": i, "text": l.text, "path": l.url.path(), "found_on": l.path.last().map(|u| u.path()) }),
            questions: Vec::new(),
        })
        .collect();
    let mut criteria = Map::new();
    for (i, l) in leads.iter().enumerate() {
        let name = if l.text.is_empty() { l.url.path().to_string() } else { format!("{} ({})", l.text, l.url.path()) };
        criteria.insert(format!("o{i}"), serde_json::json!(name));
    }
    criteria.insert("none".into(), serde_json::json!("None of these pages has the answer or leads to it"));
    let pick = crate::decide::choice(
        &format!(
            "Looking for the answer to this question on this site: which of these pages is the best next step, the page \
             that has the answer or the one that leads to it? {q}"
        ),
        serde_json::Value::Object(criteria),
    );
    let a = match ctx.judge("links", items, Map::from_iter([("next".to_string(), pick)])).await {
        Ok(a) => a,
        Err(e) if is_api_error(&e) => return Err(e),
        Err(_) => return Ok(None),
    };
    let Some(probs) = a.probabilities("next") else { return Ok(None) };
    let mut order: Vec<usize> = (0..leads.len()).collect();
    order.sort_by(|&x, &y| {
        let p = |i: usize| probs.get(&format!("o{i}")).copied().unwrap_or(0.0);
        p(y).total_cmp(&p(x))
    });
    Ok(Some(order))
}

/// How a search stops before its budget runs out: a page that answers is good enough (`Found`), or the trail has gone
/// cold (`Cold`).
#[derive(Debug, PartialEq)]
enum Stop {
    Found,
    Cold,
}

/// A search of one site, carried from page to page: the leads not opened yet, the pages read, the pages that answer.
struct Search {
    site: Site,
    /// `--follow N`: the pages the search may open.
    max: usize,
    /// How sure a page must be of its answer to be found: `--threshold`, or the default of `--precise`.
    threshold: f64,
    /// `pages` is the pages read, which is what the search reports. `opened` is the pages opened, read or not: the
    /// budget `--follow N` stops at, so a page that fails to load still takes its step.
    pages: usize,
    opened: usize,
    leads: Vec<Lead>,
    /// The pages opened or read, by [`links::key`]: a search doesn't open the same page twice.
    visited: HashSet<String>,
    /// The pages read so far, by where each one was read from. A markdown lead that reads as an HTML page already read
    /// is not judged again. Kept apart from `visited`, which holds the leads picked as well.
    read: HashSet<String>,
    /// The menu links of the pages read so far (see [`menus`]).
    known: HashSet<String>,
    field_scores: FieldScores,
    robots: RobotsByHost,
    /// A page that answers is ranked by how sure Jev is of the answer AND of the page: a blog post from two years ago
    /// can answer "how much is it?" with full confidence and the old price, while the pricing page was the lead.
    found: Vec<Ranked>,
    /// The best page that answers but not sure enough (see [`Visit::score`]): what a miss reports as closest.
    closest: Option<Ranked>,
    /// What `-t` prints for each page read: how warm, how sure of an answer.
    log: Vec<Reading>,
    /// The trail went cold before the budget ran out (see [`Stop::Cold`]).
    cold: bool,
    /// How the start page was served: the usage's `route` when the search misses (see `conclude`).
    start_served: Served,
}

impl Search {
    /// The start of a search: the first page and the site's own files (robots.txt, llms.txt, sitemaps) are read at once,
    /// then the first page is judged and the site's pages become the first leads. The start host's robots.txt is the
    /// one the file read gave, so it is not read twice.
    async fn start(
        args: &Args,
        cfg: &Config,
        client: &Client,
        api_key: &str,
        start: &Url,
        t: &mut Timer,
    ) -> Result<Search> {
        let max = args.follow.unwrap_or(5).max(1);
        let threshold = args.threshold_for(args.precise);
        let site = Site::new(start);
        let retry =
            Retry::for_run(args.no_browser_retry, args.timing, &args.reach, &args.cookies, args.stealth.as_ref());

        let (loaded, files) = tokio::join!(load(args, cfg, start, t), site_files(start, &site, retry));
        let (url, ex, served) = loaded?;
        let SiteFiles { robots, links: mut map } = files;
        // The hosts the page links to on its own registrable domain (docs.stripe.com from stripe.com): their llms.txt is
        // read too, and their pages are leads like the site's own.
        let linked = crate::hosts::linked_hosts(
            start,
            ex.links.iter().chain(&ex.site_links).map(|l| &l.url),
            crate::hosts::LINKED_HOSTS,
        );
        let read_before = HashSet::new();
        let (known0, field0) = (HashSet::new(), FieldScores::new());
        let (first, hints) =
            tokio::join!(judge(args, client, api_key, url, ex, &site, &known0, &field0, &read_before, served), async {
                let (llms, sitemaps) =
                    tokio::join!(linked_llms(start, &linked, retry), linked_sitemaps(start, &linked, retry),);
                map.extend(llms);
                map.extend(sitemaps);
                site_hints(args, client, api_key, start, &site, map).await
            },);
        let mut first = first?;
        let mut hints = hints?;
        let mut known = HashSet::new();
        let mut field_scores = FieldScores::new();
        known.extend(first.menus.iter().cloned());
        field_scores.extend(std::mem::take(&mut first.new_field_scores));
        t.lap(format!("page 1 + site files ({} pages listed)", hints.len()));

        // How well the start page fits the question as a page, from the same scoring as the site's pages. A home page
        // answering in passing (a FAQ line) counts for less than a pricing page that the site lists. The start page is
        // always the first hint (see [`site_hints`]), so it's taken out here.
        let start_fit = hints.remove(0).p.max(COLD_PAGE);
        let mut robots_by_host = RobotsByHost::default();
        robots_by_host.insert(start, Robots::parse(robots.as_deref().unwrap_or_default()));
        let mut search = Search {
            visited: HashSet::from([links::key(start), links::key(&first.url)]),
            read: HashSet::from([links::key(&first.url)]),
            site,
            max,
            threshold,
            pages: 1,
            opened: 1,
            leads: Vec::new(),
            known,
            field_scores,
            robots: robots_by_host,
            found: Vec::new(),
            closest: None,
            log: Vec::new(),
            cold: false,
            start_served: served,
        };
        let first_path = vec![first.url.clone()];
        search.absorb(first, start_fit, 1.0, first_path);
        for l in hints {
            search.leads.push(Lead { url: l.url, text: l.text, score: l.p, p: l.p, path: vec![start.clone()] });
        }
        Ok(search)
    }

    /// Takes a page that was read: its links become leads, and it is found, or closest, if it answers. `lead_score` and
    /// `lead_p` are the lead it was reached by (its score, and the link's own score). `path` is how jurl got there,
    /// this page last.
    fn absorb(&mut self, v: Visit, lead_score: f64, lead_p: f64, path: Vec<Url>) {
        self.log.push(Reading { url: v.url.clone(), warmth: v.warmth, score: v.score });
        // Hot or cold: the links of a page far from the question count for less, so a wrong turn is dropped and
        // the search goes back to the leads of a page that was getting warmer.
        // The page you started from is never cold: its links are all there is to go on.
        // A hub page (a docs index, a category) says nothing itself but links straight to the answer: it's as warm
        // as its best link. And warmth is relative: far from the answer (Paris → … → Aspirin) every page is cold,
        // but one whose best link looks better than the link that led here is getting warmer.
        let best_link = v.links.iter().map(|l| l.p).fold(0.0, f64::max);
        let warmer = (best_link / lead_p.max(0.05)).min(1.0);
        let heat =
            if path.len() == 1 { 1.0 } else { COLD_PAGE + (1.0 - COLD_PAGE) * v.warmth.max(best_link).max(warmer) };
        let decay = HOP_DECAY.powi(path.len() as i32 - 1);
        for l in &v.links {
            self.leads.push(Lead {
                url: l.url.clone(),
                text: l.text.clone(),
                score: l.p * decay * heat,
                p: l.p,
                path: path.clone(),
            });
        }
        let rank = v.score * lead_score;
        if v.found.is_some() && v.score >= self.threshold {
            self.found.push(Ranked { rank, visit: v, path });
        } else if v.found.is_some() && self.closest.as_ref().is_none_or(|c| rank > c.rank) {
            self.closest = Some(Ranked { rank, visit: v, path });
        }
    }

    /// The stop rules, checked before each step (after the leads are sorted and the robots rules applied). `Found`: a
    /// page answers and no lead left could beat it by [`BETTER_BY`]. `Cold`: the best lead left is weaker than
    /// [`COLD_TRAIL`], or there is none.
    fn done(&self) -> Option<Stop> {
        // Done when no page left could beat what's been found: each lead's score is the most it could rank.
        let best_found = self.found.iter().map(|f| f.rank).fold(0.0, f64::max);
        if !self.found.is_empty() && self.leads.first().is_none_or(|l| l.score <= best_found + BETTER_BY) {
            return Some(Stop::Found);
        }
        if self.leads.first().is_none_or(|l| l.score < COLD_TRAIL) {
            return Some(Stop::Cold);
        }
        None
    }

    /// The next pages to open, or `None` when the search is over: the budget is spent, or [`Search::done`] says so.
    /// Each step sorts the leads, drops the visited and disallowed ones, and Jev picks among the best few.
    async fn next_batch(&mut self, ctx: &Ctx<'_>, t: &mut Timer) -> Result<Option<Vec<Lead>>> {
        if self.opened >= self.max {
            return Ok(None);
        }
        self.leads.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut seen = HashSet::new();
        self.leads.retain(|l| {
            let k = links::key(&l.url);
            !self.visited.contains(&k) && seen.insert(k)
        });
        // The leads that can be picked this step are the best few, each checked against its own host's robots.txt
        // first. A lead the rules drop brings the next one up, which may be on a host not read yet: so this goes round
        // until the best few are all read and checked. Leads further down wait in the list.
        let mut read = 0;
        loop {
            self.leads.retain(|l| self.robots.allows(&l.url));
            let best: Vec<Url> = self.leads.iter().take(SHORTLIST).map(|l| l.url.clone()).collect();
            let n = self
                .robots
                .load_for(
                    &best,
                    Retry::for_run(
                        ctx.args.no_browser_retry,
                        ctx.args.timing,
                        &ctx.args.reach,
                        &ctx.args.cookies,
                        ctx.args.stealth.as_ref(),
                    ),
                )
                .await;
            if n == 0 {
                break;
            }
            read += n;
        }
        if read > 0 {
            t.lap(format!("robots {read}"));
        }
        match self.done() {
            None => {}
            Some(Stop::Found) => return Ok(None),
            Some(Stop::Cold) => {
                self.cold = true;
                return Ok(None);
            }
        }
        // Jev picks the next pages out of the best few, side by side, or says none of them leads anywhere.
        let n = (if self.max >= 10 { PARALLEL_LONG } else { PARALLEL }).min(self.max - self.opened);
        let short: Vec<&Lead> = self.leads.iter().take(SHORTLIST).collect();
        // On a long trail (`--follow 10` and up) no page "is the next step" to something far away, so Jev's side-by-side
        // pick only adds noise there: the leads' own scores decide.
        let order = if short.len() > n && self.max < 10 { rank_next_step(ctx, &short).await? } else { None };
        t.lap("next");
        // "None of these leads anywhere" isn't a reason to stop: on a long trail (Paris → … → Aspirin) no single step
        // looks like it leads to the answer. It only orders the shortlist.
        let picks: Vec<usize> = match order {
            Some(order) => order.into_iter().take(n).collect(),
            None => (0..n.min(self.leads.len())).collect(),
        };
        let mut batch: Vec<Lead> = Vec::new();
        for i in picks.into_iter().rev().collect::<std::collections::BTreeSet<_>>().into_iter().rev() {
            batch.push(self.leads.remove(i));
        }
        for l in &batch {
            self.visited.insert(links::key(&l.url));
        }
        self.opened += batch.len();
        Ok(Some(batch))
    }

    /// Takes the results of a batch, in batch order. A page read is absorbed; one that failed to load, or was a block
    /// page, is skipped (and said so under -t), its links never become leads; an API error ends the search.
    fn absorb_batch(&mut self, batch: Vec<Lead>, results: Vec<Result<Visit>>, timing: bool) -> Result<()> {
        for (lead, r) in batch.into_iter().zip(results) {
            match r {
                Ok(mut v) => {
                    self.pages += 1;
                    self.visited.insert(links::key(&v.url));
                    self.read.insert(links::key(&v.url));
                    self.known.extend(v.menus.iter().cloned());
                    self.field_scores.extend(std::mem::take(&mut v.new_field_scores));
                    let mut path = lead.path.clone();
                    path.push(v.url.clone());
                    self.absorb(v, lead.score, lead.p, path);
                }
                Err(e) if is_api_error(&e) => return Err(e),
                Err(e) if timing => eprintln!("jurl: skipped {}: {e:#}", lead.url),
                Err(_) => {}
            }
        }
        Ok(())
    }

    /// The result of the search: the best page that answers or, when none does, the closest one (which a `--precise
    /// --json` miss still prints), or else the miss.
    fn conclude(mut self, args: &Args, client: &Client, api_key: &str) -> Result<Rendered> {
        let pages = self.pages;
        if args.timing {
            for Reading { url, warmth, score } in &self.log {
                eprintln!("   warmth {warmth:.2} · answer {score:.2} · {url}");
            }
            let tokens = crate::decide::USAGE.jev_tokens.load(std::sync::atomic::Ordering::Relaxed);
            eprintln!("   {pages} pages · {tokens} tokens");
        }
        self.found.sort_by(|a, b| b.rank.total_cmp(&a.rank));
        // The usage's route is the page that answers, or the start page when the search misses (see `Usage::record_route`).
        crate::decide::USAGE.record_route(self.found.first().map_or(self.start_served, |best| best.visit.served));
        let (ranked, missed_by) = match self.found.into_iter().next() {
            Some(best) => (best, None),
            None => match self.closest {
                Some(closest) => {
                    let v = &closest.visit;
                    let what = match &v.found {
                        Some(Found::Precise(pick)) if pick.p >= PRECISE_BLOCK_FLOOR => {
                            format!(
                                " (closest: \"{}\" on {}, p={:.2})",
                                &v.ex.blocks[pick.block].text[pick.range.clone()],
                                v.url,
                                pick.p
                            )
                        }
                        _ => String::new(),
                    };
                    let message = format!("read {pages} pages of {} and none answers that{what}", self.site.root);
                    // JSON says what came closest, as on a single page, and fails like it; text fails with it in the
                    // message.
                    if !(args.json && args.precise) {
                        return Err(not_found(message));
                    }
                    (closest, Some(message))
                }
                None if self.cold => {
                    return Err(not_found(format!(
                        "read {pages} pages of {} and none answers that; no link left looks promising",
                        self.site.root
                    )));
                }
                None => {
                    return Err(not_found(format!("read {pages} pages of {} and none answers that", self.site.root)));
                }
            },
        };
        let Ranked { visit: v, path, .. } = ranked;
        let ctx = self.site.ctx(args, client, api_key, &v.url, &v.ex);
        let rendered = match &v.found {
            Some(Found::Precise(pick)) => render_precise(&ctx, &v.ex, pick, Some(&path)),
            Some(Found::Blocks { scores, keep, kind }) => {
                render_blocks(&ctx, &v.ex, scores, keep, kind.clone(), Some(&path))?
            }
            None => unreachable!("found pages always have an answer"),
        };
        if let Some(message) = missed_by {
            return Err(missed(message, rendered));
        }
        if !args.json {
            eprintln!("jurl: found after reading {pages} page{}: {}", if pages == 1 { "" } else { "s" }, trail(&path));
        }
        Ok(rendered)
    }
}

/// The start page's hints: the pages the site lists (see [`site_files`] and [`linked_llms`]), the pages that share words with
/// the question first, each scored as a link. The start page is first, scored like a candidate too.
async fn site_hints(
    args: &Args,
    client: &Client,
    api_key: &str,
    start: &Url,
    site: &Site,
    map: Vec<Link>,
) -> Result<Vec<ScoredLink>> {
    let q = args.ask.as_deref().unwrap_or_default();
    // A page listed twice (by the site's files and by a linked host's) is kept once.
    let mut seen = HashSet::new();
    let map: Vec<Link> = map.into_iter().filter(|l| seen.insert(links::key(&l.url))).collect();
    let mut links = links::most_relevant(q, map, MAX_HINTS);
    // The start page is scored as a candidate too: how much its own answer counts against the site's other pages.
    links.retain(|l| links::key(&l.url) != links::key(start));
    links.insert(0, Link { i: 0, url: start.clone(), text: String::new(), context: String::new(), marginal: false });
    for (i, l) in links.iter_mut().enumerate() {
        l.i = i;
    }
    let empty = Extracted::default();
    let ctx = site.ctx(args, client, api_key, start, &empty);
    let scores = match links::score(&ctx, &links, "The page at the URL in `links`", None).await {
        Ok((scores, _)) => scores,
        Err(e) if is_api_error(&e) => return Err(e),
        Err(_) => vec![0.0; links.len()],
    };
    Ok(links.into_iter().zip(scores).map(|(l, p)| ScoredLink { url: l.url, text: l.text, p }).collect())
}

/// The path as the "found after reading" line prints it: each URL without its http:// or https:// scheme, joined by
/// arrows.
pub(crate) fn trail(path: &[Url]) -> String {
    path.iter()
        .map(|u| {
            let s = u.as_str();
            s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s).to_string()
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

pub async fn run(
    args: &Args,
    cfg: &Config,
    client: &Client,
    api_key: &str,
    start: Url,
    t: &mut Timer,
) -> Result<Rendered> {
    let mut search = Search::start(args, cfg, client, api_key, &start, t).await?;
    let empty = Extracted::default();
    let site_ctx = search.site.ctx(args, client, api_key, &start, &empty);
    read_batches(args, cfg, client, api_key, &mut search, &site_ctx, t).await?;
    // A precise search that found no answer gets a second pass: the site's own search (see `site_search`) adds its results
    // as leads, and the budget grows. A search that found one never gets here, so answered cells are not changed.
    if args.precise && search.found.is_empty() && second_pass(args, &start, &mut search, &site_ctx).await? {
        read_batches(args, cfg, client, api_key, &mut search, &site_ctx, t).await?;
    }
    search.conclude(args, client, api_key)
}

/// The pages a search opens, a batch at a time, until its budget or its trail runs out (or it finds an answer).
async fn read_batches(
    args: &Args,
    cfg: &Config,
    client: &Client,
    api_key: &str,
    search: &mut Search,
    site_ctx: &Ctx<'_>,
    t: &mut Timer,
) -> Result<()> {
    while let Some(batch) = search.next_batch(site_ctx, t).await? {
        // The pages of a batch are read side by side: each reads the field scores from before the batch, and what it
        // asked is kept once the batch is in.
        let results = join_all(batch.iter().map(|l| {
            visit(args, cfg, client, api_key, &l.url, &search.site, &search.known, &search.field_scores, &search.read)
        }))
        .await;
        t.lap(format!("{} more", batch.len()));
        search.absorb_batch(batch, results, args.timing)?;
    }
    Ok(())
}

/// How many pages the second pass may open on top of the first pass's budget.
const SECOND_PASS: usize = 3;

/// The second pass (see [`run`]): the site's own search results that are not read yet become leads, scored by Jev like the
/// others, and the search may open [`SECOND_PASS`] more pages. Returns whether there were any such leads.
async fn second_pass(args: &Args, start: &Url, search: &mut Search, site_ctx: &Ctx<'_>) -> Result<bool> {
    let retry = crate::fetch::Retry::for_run(
        args.no_browser_retry,
        args.timing,
        &args.reach,
        &args.cookies,
        args.stealth.as_ref(),
    );
    let question = args.ask.as_deref().unwrap_or_default();
    let mut results = crate::site_search::candidates(start, question, true, retry).await;
    results.retain(|l| !search.visited.contains(&links::key(&l.url)));
    if results.is_empty() {
        return Ok(false);
    }
    let (scores, _) = links::score(site_ctx, &results, "The page at the URL in `links`", None).await?;
    for (l, p) in results.into_iter().zip(scores) {
        search.leads.push(Lead { url: l.url, text: l.text, score: p, p, path: vec![start.clone()] });
    }
    search.max += SECOND_PASS;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        answer::PRECISE_THRESHOLD,
        fetch::{Memo, test_server},
        judge::BlockPage,
    };
    use test_server::{Kind, Served, reply, retry_off, retry_on, serve_replies, serve_routed};

    #[test]
    fn menus_are_the_links_outside_the_text() {
        let page = "<body><nav><a href='/pricing'>Pricing</a><a href='/about'>About</a></nav><main><p>Read \
            <a href='/docs'>the docs</a> and <a href='/about'>about us</a>, a long enough paragraph.</p></main></body>";
        let ex = extract::html(page, &Url::parse("https://x.com/").unwrap());
        assert_eq!(menus(&ex), HashSet::from(["https://x.com/pricing".to_string()]));
    }

    #[test]
    fn same_site_includes_subdomains_not_assets() {
        let site = Site::new(&Url::parse("https://www.linear.app/").unwrap());
        assert!(site.contains(&Url::parse("https://linear.app/pricing").unwrap()));
        assert!(site.contains(&Url::parse("https://docs.linear.app/start").unwrap()));
        assert!(!site.contains(&Url::parse("https://notlinear.app/").unwrap()));
        assert!(!site.contains(&Url::parse("https://linear.app/brand.pdf").unwrap()));
    }

    #[test]
    fn only_a_search_from_the_front_door_is_about_the_owner() {
        assert_eq!(Site::new(&Url::parse("https://www.figma.com/").unwrap()).owner().as_deref(), Some("figma.com"));
        assert_eq!(Site::new(&Url::parse("https://linear.app").unwrap()).owner().as_deref(), Some("linear.app"));
        assert_eq!(Site::new(&Url::parse("https://en.wikipedia.org/wiki/Paris").unwrap()).owner(), None);
        assert_eq!(Site::new(&Url::parse("https://github.com/BurntSushi/ripgrep").unwrap()).owner(), None);
    }

    #[test]
    fn robots_for_everyone_only() {
        let r = Robots::parse(
            "User-agent: Googlebot\nDisallow: /g\n\nUser-agent: *\nDisallow: /admin # no\nDisallow: /*?q=\nAllow: /\n",
        );
        assert!(!r.allows(&Url::parse("https://x.com/admin/users").unwrap()));
        assert!(r.allows(&Url::parse("https://x.com/g/page").unwrap()));
        assert!(r.allows(&Url::parse("https://x.com/pricing").unwrap()));
    }

    /// RFC 9309 §5.1's example file, rules verbatim: its `*` group is jurl's, and the named groups after it are not.
    const RFC_5_1: &str = "User-Agent: *\nDisallow: *.gif$\nDisallow: /example/\nAllow: /publications/\n\n\
        User-Agent: foobot\nDisallow:/\nAllow:/example/page.html\nAllow:/example/allowed.gif\n\n\
        User-Agent: barbot\nUser-Agent: bazbot\nDisallow: /example/page.html\n\nUser-Agent: quxbot\n";

    /// Whether the `*` group of `robots` allows `path` on x.com.
    fn allowed(robots: &str, path: &str) -> bool {
        Robots::parse(robots).allows(&Url::parse(&format!("https://x.com{path}")).unwrap())
    }

    #[test]
    fn the_simple_example_of_rfc_9309_is_jurls_rules() {
        assert!(allowed(RFC_5_1, "/publications/paper.html"));
        assert!(!allowed(RFC_5_1, "/example/page.html"));
        assert!(!allowed(RFC_5_1, "/images/logo.gif"));
        // `$` ends the match at the end of the path and query: a `.gif` with a query isn't one.
        assert!(allowed(RFC_5_1, "/images/logo.gif?v=2"));
        // The longer Allow beats `*.gif$` for a file under /publications/.
        assert!(allowed(RFC_5_1, "/publications/chart.gif"));
        // foobot's `Disallow:/` is not jurl's rule.
        assert!(allowed(RFC_5_1, "/index.html"));
    }

    #[test]
    fn the_longest_match_wins_whatever_the_order_as_in_rfc_9309_5_2() {
        let rules = "User-Agent: *\nAllow: /example/page/\nDisallow: /example/page/disallowed.gif\n";
        assert!(!allowed(rules, "/example/page/disallowed.gif"));
        assert!(allowed(rules, "/example/page/disallow.gif"));
        let reversed = "User-Agent: *\nDisallow: /example/page/disallowed.gif\nAllow: /example/page/\n";
        assert!(!allowed(reversed, "/example/page/disallowed.gif"));
    }

    #[test]
    fn an_allow_and_a_disallow_of_equal_length_allow() {
        // RFC 9309 §2.2.2: "If an 'allow' rule and a 'disallow' rule are equivalent, then the 'allow' rule SHOULD be
        // used."
        let rules = "User-Agent: *\nDisallow: /folder\nAllow: /folder\n";
        assert!(allowed(rules, "/folder/page.html"));
    }

    #[test]
    fn a_rule_matches_from_the_first_octet_and_case_counts() {
        // RFC 9309 §2.2.2: the match starts with the first octet of the path, and it is case sensitive.
        let rules = "User-Agent: *\nDisallow: /fish\n";
        assert!(!allowed(rules, "/fish"));
        assert!(!allowed(rules, "/fish.html"));
        assert!(!allowed(rules, "/fishheads/yummy.html"));
        assert!(allowed(rules, "/Fish.asp"));
        assert!(allowed(rules, "/catfish"));
    }

    #[test]
    fn a_star_is_any_sequence_and_a_dollar_ends_the_pattern() {
        // RFC 9309 §2.2.3, Figure 5.
        let exact = "User-Agent: *\nDisallow: /this/path/exactly$\n";
        assert!(!allowed(exact, "/this/path/exactly"));
        assert!(allowed(exact, "/this/path/exactly/"));
        assert!(allowed(exact, "/this/path/exactly?x=1"));
        let star = "User-Agent: *\nDisallow: /this/*/exactly\n";
        assert!(!allowed(star, "/this/a/b/exactly/more"));
        assert!(allowed(star, "/this/exactly"));
    }

    #[test]
    fn the_query_is_part_of_what_a_rule_matches() {
        assert!(!allowed("User-Agent: *\nDisallow: /*?q=\n", "/search?q=shoes"));
        assert!(allowed("User-Agent: *\nDisallow: /*?q=\n", "/search"));
        assert!(allowed("User-Agent: *\nDisallow: /*.php$\n", "/index.php?x=1"));
        assert!(!allowed("User-Agent: *\nDisallow: /*.php$\n", "/index.php"));
    }

    #[test]
    fn percent_encoding_is_compared_as_rfc_9309_figure_4_has_it() {
        assert!(!allowed("User-Agent: *\nDisallow: /foo/bar/ツ\n", "/foo/bar/%E3%83%84"));
        assert!(!allowed("User-Agent: *\nDisallow: /foo/bar/%E3%83%84\n", "/foo/bar/ツ"));
        assert!(!allowed("User-Agent: *\nDisallow: /foo/bar/%62%61%7A\n", "/foo/bar/baz"));
        assert!(!allowed(
            "User-Agent: *\nDisallow: /foo/bar?baz=https://foo.bar\n",
            "/foo/bar?baz=https%3A%2F%2Ffoo.bar"
        ));
    }

    #[test]
    fn an_escaped_star_or_dollar_matches_the_one_it_escapes() {
        // RFC 9309 §2.2.3, Figure 6: `%2A` is a `*` in the URL and `%24` a `$`.
        let rules = "User-Agent: *\nDisallow: /path/file-with-a-%2A.html\nDisallow: /path/foo-%24\n";
        assert!(!allowed(rules, "/path/file-with-a-*.html"));
        assert!(!allowed(rules, "/path/foo-$"));
    }

    #[test]
    fn robots_txt_is_always_allowed_and_a_rule_in_no_group_is_ignored() {
        // RFC 9309 §2.2.2: /robots.txt is implicitly allowed.
        assert!(allowed("User-Agent: *\nDisallow: /\n", "/robots.txt"));
        assert!(!allowed("User-Agent: *\nDisallow: /\n", "/pricing"));
        // §2.2.2: a rule before any user-agent line is in no group, and is ignored.
        assert!(allowed("Disallow: /\nUser-Agent: *\nAllow: /\n", "/pricing"));
    }

    #[test]
    fn each_host_is_checked_against_its_own_robots_txt() {
        let url = |s: &str| Url::parse(s).unwrap();
        let mut robots = RobotsByHost::default();
        // Not read yet: allowed, so its leads wait in the list until its file is read.
        assert!(robots.allows(&url("https://docs.x.com/admin")));
        robots.insert(&url("https://docs.x.com/"), Robots::parse("User-agent: *\nDisallow: /admin\n"));
        assert!(!robots.allows(&url("https://docs.x.com/admin")));
        // A host's rules are not another host's: x.com's file doesn't speak for docs, nor docs' for x.com.
        assert!(robots.allows(&url("https://x.com/admin")));
        robots.insert(&url("https://x.com/"), Robots::parse("User-agent: *\nDisallow: /private\n"));
        assert!(!robots.allows(&url("https://x.com/private/plans")));
        assert!(robots.allows(&url("https://docs.x.com/private/plans")));
        assert!(!robots.allows(&url("https://docs.x.com/admin")));
    }

    #[test]
    fn www_is_the_same_host_and_no_file_allows_everything() {
        let url = |s: &str| Url::parse(s).unwrap();
        let mut robots = RobotsByHost::default();
        robots.insert(&url("https://www.x.com/"), Robots::parse("User-agent: *\nDisallow: /admin\n"));
        assert!(!robots.allows(&url("https://x.com/admin")));
        assert!(!robots.allows(&url("https://www.x.com/admin")));
        // A host with no file (or none within 4 s) is kept with no rules: allowed, and it stays that way.
        robots.insert(&url("https://docs.x.com/"), Robots::default());
        assert!(robots.allows(&url("https://docs.x.com/admin")));
    }

    #[test]
    fn sitemap_locs() {
        let xml = "<urlset><url><loc>https://x.com/a?b=1&amp;c=2</loc></url><url><loc> https://x.com/pricing </loc></url></urlset>";
        assert_eq!(locs(xml), ["https://x.com/a?b=1&c=2", "https://x.com/pricing"]);
    }

    #[tokio::test]
    async fn a_small_file_is_decoded_as_a_page_is() {
        // 0xE9 is é and 0x80 is € in windows-1252: read as fetch.rs reads a page, and as reqwest's text() reads it.
        let head =
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=windows-1252\r\nTransfer-Encoding: chunked\r\n\r\n";
        let body = b"Caf\xE9 costs \x80 5".to_vec();
        let url = |s: String| Url::parse(&s).unwrap();
        let ours = small_text(&url(test_server::serve(head.into(), body.clone(), true)), retry_off())
            .await
            .expect("a small file");
        let page = crate::fetch::fetch(&test_server::serve(head.into(), body.clone(), true), retry_off())
            .await
            .expect("a page");
        let res = test_server::client().get(test_server::serve(head.into(), body, true)).send().await.expect("a reply");
        assert_eq!(ours, "Café costs € 5");
        assert_eq!(ours, page.body);
        assert_eq!(ours, res.text().await.expect("text"));
    }

    #[tokio::test]
    async fn a_small_file_past_its_cap_is_refused_and_one_at_it_is_read() {
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let url = |body: Vec<u8>| Url::parse(&test_server::serve(head.into(), body, true)).unwrap();
        assert_eq!(small_text_within(&url(vec![b'a'; 4096]), 1024, retry_off()).await, None);
        assert!(small_text_within(&url(vec![b'a'; 1024]), 1024, retry_off()).await.is_some());
    }

    /// A site's robots.txt, served from `base` (the server a test made).
    fn robots_of(base: &str) -> Url {
        Url::parse(base).unwrap().join("/robots.txt").unwrap()
    }

    #[tokio::test]
    async fn robots_and_sitemaps_go_straight_to_the_browser_client_for_a_host_that_needs_it() {
        let file = "User-agent: *\nDisallow: /private\n";
        let (base, served) =
            serve_replies(vec![Some(reply("200 OK", "Content-Type: text/plain\r\n", file.as_bytes()))]);
        let memo = Memo::default();
        // A retry showed a moment ago that this host needs the browser client.
        assert!(memo.learn_url(&Url::parse(&base).unwrap(), std::time::Instant::now()));
        let body = small_text(&robots_of(&base), retry_on(&memo)).await;
        assert_eq!(body.as_deref(), Some(file));
        assert_eq!(served.kinds(), [Kind::Browser]);
    }

    #[tokio::test]
    async fn robots_and_sitemaps_stay_plain_for_a_host_that_has_not_needed_the_browser_client() {
        let (base, served) =
            serve_replies(vec![Some(reply("200 OK", "Content-Type: text/plain\r\n", b"User-agent: *\n"))]);
        let memo = Memo::default();
        assert!(small_text(&robots_of(&base), retry_on(&memo)).await.is_some());
        assert_eq!(served.kinds(), [Kind::Plain]);
    }

    #[tokio::test]
    async fn robots_and_sitemaps_stay_plain_when_the_run_opts_out_even_for_a_host_on_the_browser_client() {
        let (base, served) =
            serve_replies(vec![Some(reply("200 OK", "Content-Type: text/plain\r\n", b"User-agent: *\n"))]);
        let memo = Memo::default();
        assert!(memo.learn_url(&Url::parse(&base).unwrap(), std::time::Instant::now()));
        let off = crate::fetch::Retry {
            on: false,
            memo: &memo,
            timing: false,
            reach: &crate::fetch::test_server::PRIVATE,
            cookies: &crate::fetch::test_server::NO_COOKIES,
            stealth: None,
        };
        assert!(small_text(&robots_of(&base), off).await.is_some());
        assert_eq!(served.kinds(), [Kind::Plain]);
    }

    /// The request's path, from its request line.
    fn path_of(head: &str) -> &str {
        head.split(' ').nth(1).unwrap_or("/")
    }

    /// A sitemap of one page on the server the request came to, named by the request's Host. Header names are matched
    /// without regard to case: the client may write `host:`.
    fn sitemap_for(head: &str) -> String {
        let host = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim().eq_ignore_ascii_case("host").then(|| value.trim())
            })
            .unwrap_or_default();
        format!("<urlset><url><loc>http://{host}/news/a.html</loc></url></urlset>")
    }

    #[tokio::test]
    async fn a_sitemap_refused_on_the_plain_client_is_read_from_the_browser_client_and_its_pages_are_hints() {
        // The plain client is refused the sitemap; the browser client has it. Nothing else is there.
        let (base, served) = serve_routed(|head| match (path_of(head), head.contains("Chrome/")) {
            ("/sitemap.xml", true) => Some(reply("200 OK", "Content-Type: text/xml\r\n", sitemap_for(head).as_bytes())),
            ("/sitemap.xml", false) => Some(reply("403 Forbidden", "", b"")),
            _ => Some(reply("404 Not Found", "", b"")),
        });
        let start = Url::parse(&base).unwrap();
        let memo = Memo::default();
        let links = site_files(&start, &Site::new(&start), retry_on(&memo)).await.links;
        assert!(links.iter().any(|l| l.url.path() == "/news/a.html"), "the sitemap's page is a hint");
        let kinds = served.kinds();
        assert_eq!(kinds.iter().filter(|k| **k == Kind::Browser).count(), 1, "one browser retry, for the sitemap");
        assert!(memo.on_browser(&format!("127.0.0.1:{}", start.port().unwrap()), std::time::Instant::now()));
    }

    #[tokio::test]
    async fn a_robots_file_refused_on_the_plain_client_is_read_from_the_browser_client_and_its_rules_apply() {
        let (base, served) = serve_routed(|head| match (path_of(head), head.contains("Chrome/")) {
            ("/robots.txt", true) => {
                Some(reply("200 OK", "Content-Type: text/plain\r\n", b"User-agent: *\nDisallow: /private\n"))
            }
            ("/robots.txt", false) => Some(reply("403 Forbidden", "", b"")),
            _ => Some(reply("404 Not Found", "", b"")),
        });
        let start = Url::parse(&base).unwrap();
        let memo = Memo::default();
        let mut rules = RobotsByHost::default();
        assert_eq!(rules.load_for(std::slice::from_ref(&start), retry_on(&memo)).await, 1);
        assert!(!rules.allows(&start.join("/private/page").unwrap()), "the browser client's rules apply");
        assert!(rules.allows(&start.join("/news").unwrap()));
        assert_eq!(served.kinds().iter().filter(|k| **k == Kind::Browser).count(), 1);
    }

    #[tokio::test]
    async fn a_sitemap_answering_503_with_a_retry_after_is_not_retried() {
        let (base, served) = serve_routed(|head| match (path_of(head), head.contains("Chrome/")) {
            ("/sitemap.xml", false) => Some(reply("503 Service Unavailable", "Retry-After: 120\r\n", b"")),
            ("/sitemap.xml", true) => Some(reply("200 OK", "Content-Type: text/xml\r\n", sitemap_for(head).as_bytes())),
            _ => Some(reply("404 Not Found", "", b"")),
        });
        let start = Url::parse(&base).unwrap();
        let memo = Memo::default();
        let links = site_files(&start, &Site::new(&start), retry_on(&memo)).await.links;
        assert!(links.is_empty(), "the sitemap is not read");
        assert!(served.kinds().iter().all(|k| *k == Kind::Plain), "a 503 that says when to come back is not retried");
    }

    #[tokio::test]
    async fn with_the_retry_off_a_refused_sitemap_and_robots_file_are_not_retried() {
        let (base, served) = serve_routed(|head| match (path_of(head), head.contains("Chrome/")) {
            ("/sitemap.xml", true) => Some(reply("200 OK", "Content-Type: text/xml\r\n", sitemap_for(head).as_bytes())),
            ("/robots.txt", true) => {
                Some(reply("200 OK", "Content-Type: text/plain\r\n", b"User-agent: *\nDisallow: /\n"))
            }
            _ => Some(reply("403 Forbidden", "", b"")),
        });
        let start = Url::parse(&base).unwrap();
        let links = site_files(&start, &Site::new(&start), retry_off()).await.links;
        assert!(links.is_empty());
        let mut rules = RobotsByHost::default();
        assert_eq!(rules.load_for(std::slice::from_ref(&start), retry_off()).await, 1);
        assert!(rules.allows(&start.join("/private/page").unwrap()), "no rules were read");
        assert!(served.kinds().iter().all(|k| *k == Kind::Plain), "the browser client was never asked");
    }

    #[test]
    fn the_trail_drops_the_scheme_of_http_and_https_pages_alike() {
        let path = [Url::parse("https://x.com/").unwrap(), Url::parse("http://docs.x.com/pricing").unwrap()];
        assert_eq!(trail(&path), "x.com/ → docs.x.com/pricing");
    }

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    /// A page as `visit` returns it, built without the network: `answers` says whether the page has an answer at all,
    /// and `links` are its links with their own scores.
    fn visit_of(page: &str, warmth: f64, score: f64, answers: bool, links: &[(&str, f64)]) -> Visit {
        let url = u(page);
        Visit {
            ex: extract::html("", &url),
            url,
            found: answers.then(|| Found::Blocks { scores: Vec::new(), keep: Vec::new(), kind: None }),
            score,
            warmth,
            links: links.iter().map(|&(l, p)| ScoredLink { url: u(l), text: String::new(), p }).collect(),
            menus: HashSet::new(),
            new_field_scores: FieldScores::new(),
            served: crate::fetch::Served::default(),
        }
    }

    /// A search with nothing read yet.
    fn search() -> Search {
        Search {
            site: Site::new(&u("https://x.com/")),
            max: 5,
            threshold: PRECISE_THRESHOLD,
            pages: 1,
            opened: 1,
            leads: Vec::new(),
            visited: HashSet::new(),
            read: HashSet::new(),
            known: HashSet::new(),
            field_scores: FieldScores::new(),
            robots: RobotsByHost::default(),
            found: Vec::new(),
            closest: None,
            log: Vec::new(),
            cold: false,
            start_served: crate::fetch::Served::default(),
        }
    }

    fn lead(score: f64) -> Lead {
        Lead { url: u("https://x.com/next"), text: String::new(), score, p: score, path: Vec::new() }
    }

    #[test]
    fn the_start_page_is_never_cold() {
        // Its links are all there is to go on: no heat and no hop, so each keeps its own score, however little the
        // start page says about the question.
        let start = u("https://x.com/");
        let mut s = search();
        let page = visit_of("https://x.com/", 0.0, 0.0, false, &[("https://x.com/a", 0.6)]);
        s.absorb(page, COLD_PAGE, 1.0, vec![start]);
        assert_eq!(s.leads.len(), 1);
        assert_eq!(s.leads[0].score, 0.6);
    }

    #[test]
    fn each_hop_discounts_the_links_of_the_page_it_reaches() {
        // Two hops out, on a page that is hot (warmth 1): its links keep HOP_DECAY twice over.
        let path = vec![u("https://x.com/"), u("https://x.com/a"), u("https://x.com/b")];
        let mut s = search();
        s.absorb(visit_of("https://x.com/b", 1.0, 0.0, false, &[("https://x.com/c", 0.5)]), 0.5, 0.5, path);
        assert!((s.leads[0].score - 0.5 * HOP_DECAY * HOP_DECAY).abs() < 1e-12, "{}", s.leads[0].score);
    }

    #[test]
    fn a_cold_page_is_warm_relative_to_the_lead_that_reached_it() {
        // Nothing on the question here (warmth 0), but its best link scores 0.5. Reached by a lead of 0.4, the page is
        // getting warmer, so its links count in full; reached by a lead of 0.9, it is colder, so they count for less.
        let path = || vec![u("https://x.com/"), u("https://x.com/a")];
        let page = || visit_of("https://x.com/a", 0.0, 0.0, false, &[("https://x.com/c", 0.5)]);
        let mut warmer = search();
        warmer.absorb(page(), 0.4, 0.4, path());
        let mut colder = search();
        colder.absorb(page(), 0.9, 0.9, path());
        assert!((warmer.leads[0].score - 0.5 * HOP_DECAY).abs() < 1e-12);
        let heat = COLD_PAGE + (1.0 - COLD_PAGE) * (0.5 / 0.9);
        assert!((colder.leads[0].score - 0.5 * HOP_DECAY * heat).abs() < 1e-12);
        assert!(warmer.leads[0].score > colder.leads[0].score);
    }

    #[test]
    fn a_page_that_answers_is_found_or_else_the_closest() {
        let start = u("https://x.com/");
        let mut s = search();
        // Sure enough (0.9, against the threshold of 0.4): found, ranked 0.9 * 0.5.
        let a = vec![start.clone(), u("https://x.com/a")];
        s.absorb(visit_of("https://x.com/a", 0.9, 0.9, true, &[]), 0.5, 0.5, a);
        // Not sure enough: closest, then a closer-ranked miss replaces it, and a worse one doesn't.
        let b = vec![start.clone(), u("https://x.com/b")];
        s.absorb(visit_of("https://x.com/b", 0.9, 0.2, true, &[]), 0.5, 0.5, b);
        let c = vec![start.clone(), u("https://x.com/c")];
        s.absorb(visit_of("https://x.com/c", 0.9, 0.2, true, &[]), 0.9, 0.9, c);
        let d = vec![start.clone(), u("https://x.com/d")];
        s.absorb(visit_of("https://x.com/d", 0.9, 0.2, true, &[]), 0.3, 0.3, d);
        // A page with no answer is neither found nor closest.
        let e = vec![start, u("https://x.com/e")];
        s.absorb(visit_of("https://x.com/e", 0.9, 0.9, false, &[]), 0.9, 0.9, e);
        assert_eq!(s.found.len(), 1);
        assert_eq!(s.found[0].visit.url.path(), "/a");
        assert_eq!(s.closest.as_ref().map(|c| c.visit.url.path()), Some("/c"));
    }

    #[test]
    fn a_found_page_stops_the_search_once_no_lead_could_beat_it_by_better_by() {
        let mut s = search();
        let visit = visit_of("https://x.com/a", 0.9, 0.9, true, &[]);
        s.found.push(Ranked { rank: 0.5, visit, path: Vec::new() });
        // Within BETTER_BY of the find (0.5 + 0.15): no lead could beat it by enough, so the search stops.
        s.leads.push(lead(0.6));
        assert_eq!(s.done(), Some(Stop::Found));
        // A lead that could beat it by more keeps the search going.
        s.leads[0].score = 0.7;
        assert_eq!(s.done(), None);
        // With no lead left, nothing could beat it.
        s.leads.clear();
        assert_eq!(s.done(), Some(Stop::Found));
    }

    #[test]
    fn a_trail_whose_best_lead_is_below_cold_trail_is_cold() {
        let mut s = search();
        s.leads.push(lead(0.01));
        assert_eq!(s.done(), Some(Stop::Cold));
        s.leads[0].score = 0.03;
        assert_eq!(s.done(), None);
        // No lead at all, and nothing found: cold too.
        assert_eq!(search().done(), Some(Stop::Cold));
    }

    #[test]
    fn a_block_page_is_told_apart_by_its_error_type_not_its_message() {
        let block = anyhow::Error::from(BlockPage("https://x.com/ served a block page".into()));
        assert!(is_block_page(&block.context("reading https://x.com/")));
        assert!(!is_block_page(&anyhow::anyhow!("https://x.com/ served a block page")));
    }

    #[test]
    fn a_block_page_read_later_is_skipped_and_leads_nothing() {
        // The interstitial's links are not the site's: nothing is absorbed, and the search goes on as it was.
        let mut s = search();
        let block = BlockPage("https://x.com/next served a block page".into()).into();
        s.absorb_batch(vec![lead(0.5)], vec![Err(block)], false).expect("the search goes on");
        assert_eq!(s.pages, 1);
        assert!(s.leads.is_empty());
        assert!(s.log.is_empty());
        assert!(s.found.is_empty() && s.closest.is_none());
    }

    #[test]
    fn a_captcha_on_a_later_page_is_skipped_and_the_search_goes_on() {
        // A page behind an interactive CAPTCHA is for human review, not a stop: it is skipped like a page that failed to load, and
        // its links never become leads. Only the start page's CAPTCHA fails a run (see `Search::start`).
        let mut s = search();
        let captcha: anyhow::Error = crate::stealth::Captcha("https://x.com/next".into()).into();
        assert!(!is_api_error(&captcha), "an API error would end the search");
        s.absorb_batch(vec![lead(0.5)], vec![Err(captcha)], true).expect("the search goes on");
        assert_eq!(s.pages, 1);
        assert!(s.leads.is_empty());
        assert!(s.log.is_empty());
        assert!(s.found.is_empty() && s.closest.is_none());
    }

    #[tokio::test]
    async fn a_captcha_on_the_start_page_fails_the_search() {
        // The start page is refused and the stealth sidecar answers with a CAPTCHA: the start cannot be read, so the search fails
        // with that error, before any judgement is asked of Jev.
        let (base, _) = serve_routed(|_| Some(reply("403 Forbidden", "", b"denied")));
        let captcha = r#"{"outcome":"captcha","status":403,"html":"","text":"","title":"","reason":null,"wall_s":1.0,"robots":"allowed"}"#;
        let (side, seen) = crate::mock::serve(vec![(200, "", captcha)]);
        let stealth = crate::stealth::Sidecar::configured(
            |key| match key {
                "JURL_STEALTH_URL" => Some(side.clone()),
                "JURL_STEALTH_TOKEN" => Some("token".to_string()),
                _ => None,
            },
            false,
            false,
            false,
        )
        .expect("a sidecar that is set up");
        use clap::Parser;
        let mut args = Args::parse_from(["jurl", "-q", "what does it cost?", "--follow", "2", "http://127.0.0.1:1/"]);
        args.stealth = Some(stealth);
        let start = Url::parse(&format!("{base}/tienda")).expect("a URL");
        let err = Search::start(&args, &Config::default(), &test_server::client(), "key", &start, &mut Timer::new())
            .await
            .err()
            .expect("the start page's CAPTCHA fails the search");
        assert!(err.downcast_ref::<crate::stealth::Captcha>().is_some(), "{err:#}");
        assert_eq!(format!("{err:#}"), format!("{start}: an interactive CAPTCHA, not solved; for human review"));
        assert_eq!(seen.lock().expect("the request log").len(), 1, "the sidecar was asked once");
    }

    /// The links a site lists for `urls`, in that order, as `site_map` lists them.
    fn listed(urls: &[&str]) -> Vec<Link> {
        urls.iter()
            .enumerate()
            .map(|(i, s)| Link { i, url: u(s), text: String::new(), context: String::new(), marginal: false })
            .collect()
    }

    /// The URLs that stay of `urls` once the site's language copies are dropped, as `site_map` keeps them.
    fn kept(start: &str, urls: &[&str]) -> Vec<String> {
        without_language_copies(&u(start), listed(urls)).iter().map(|l| l.url.as_str().to_string()).collect()
    }

    #[test]
    fn locales_are_a_language_with_an_optional_two_letter_region() {
        for s in ["de", "pt-br", "en-US", "zh", "es-es"] {
            assert!(is_locale(s), "{s}");
        }
        for s in ["us", "ca", "uk", "english", "de-deu", "en-", "pricing"] {
            assert!(!is_locale(s), "{s}");
        }
    }

    #[test]
    fn a_page_listed_without_a_language_keeps_one_copy_of_it() {
        let urls = [
            "https://x.com/pricing",
            "https://x.com/de/pricing",
            "https://x.com/es-es/pricing/",
            "https://x.com/pt-BR/pricing",
            "https://x.com/en-US/docs/Web",
            "https://x.com/docs/Web",
        ];
        assert_eq!(kept("https://x.com/", &urls), ["https://x.com/pricing", "https://x.com/docs/Web"]);
        // The links that stay are renumbered.
        let out = without_language_copies(
            &u("https://x.com/"),
            listed(&["https://x.com/de/a", "https://x.com/a", "https://x.com/b"]),
        );
        assert_eq!(out.iter().map(|l| (l.i, l.url.path())).collect::<Vec<_>>(), [(0, "/a"), (1, "/b")]);
    }

    #[test]
    fn the_start_pages_language_keeps_its_own_copy() {
        let urls = [
            "https://cabify.com/precios",
            "https://cabify.com/es/precios",
            "https://cabify.com/en/precios",
            "https://cabify.com/en/about-us",
            "https://cabify.com/es/sobre-nosotros",
        ];
        assert_eq!(
            kept("https://cabify.com/es", &urls),
            ["https://cabify.com/es/precios", "https://cabify.com/en/about-us", "https://cabify.com/es/sobre-nosotros"]
        );
    }

    #[test]
    fn without_the_start_pages_language_the_unprefixed_copy_wins() {
        let urls = ["https://docs.python.org/fr/3/library/os.html", "https://docs.python.org/3/library/os.html"];
        assert_eq!(kept("https://docs.python.org/es/3/", &urls), ["https://docs.python.org/3/library/os.html"]);
    }

    #[test]
    fn the_real_answers_that_are_in_a_language_survive() {
        // The answer pages of bench/follow-real.notes.md that carry a language prefix, each listed beside its other copies
        // (docs.python.org also lists the unprefixed one). Each answer survives.
        let cases: [(&str, &str, &[&str]); 5] = [
            (
                "https://www.santander.com/",
                "https://www.santander.com/en/about-us/our-history",
                &["https://www.santander.com/es/sobre-nosotros/nuestra-historia"],
            ),
            ("https://mullvad.net/", "https://mullvad.net/en/pricing", &["https://mullvad.net/de/pricing"]),
            ("https://cabify.com/es", "https://cabify.com/es/sobre-nosotros", &["https://cabify.com/en/about-us"]),
            (
                "https://docs.python.org/es/3/",
                "https://docs.python.org/es/3/library/sys.html",
                &["https://docs.python.org/fr/3/library/sys.html", "https://docs.python.org/3/library/sys.html"],
            ),
            (
                "https://docs.djangoproject.com/en/stable/",
                "https://docs.djangoproject.com/en/stable/ref/settings/",
                &["https://docs.djangoproject.com/fr/stable/ref/settings/"],
            ),
        ];
        for (start, answer, others) in cases {
            let mut urls = vec![answer];
            urls.extend_from_slice(others);
            assert!(kept(start, &urls).contains(&answer.to_string()), "{answer}");
        }
    }

    #[test]
    fn a_country_code_or_a_subdomain_is_not_a_language_copy() {
        // `us` is a country, not a language: the IKEA pages under /us/en/ are their own pages.
        let ikea = [
            "https://www.ikea.com/us/en/customer-service/returns-claims/",
            "https://www.ikea.com/customer-service/returns-claims/",
        ];
        assert_eq!(kept("https://www.ikea.com/us/en/", &ikea), ikea);
        // `ca` names Canada's market, not Catalan: /ca/pricing is not a copy of /pricing. A subdomain is not a path.
        let ca = ["https://x.com/ca/pricing", "https://x.com/pricing"];
        assert_eq!(kept("https://x.com/", &ca), ca);
        let sub = ["https://de.x.com/pricing", "https://x.com/pricing"];
        assert_eq!(kept("https://x.com/", &sub), sub);
    }

    #[test]
    fn a_language_alone_is_a_copy_of_the_home_page() {
        assert_eq!(
            kept("https://x.com/", &["https://x.com/", "https://x.com/de/", "https://x.com/de"]),
            ["https://x.com/"]
        );
    }

    /// A site at a loopback server: each path is given its file, with `{host}` in it replaced by the request's own host
    /// (the server's address), and any other path is a 404. Returns the site's URL and what the server was sent.
    fn site_at(files: Vec<(&'static str, &'static str)>) -> (Url, Served) {
        let (base, served) = serve_routed(move |head| {
            let host = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim().eq_ignore_ascii_case("host").then(|| value.trim().to_string())
                })
                .unwrap_or_default();
            Some(match files.iter().find(|(p, _)| *p == path_of(head)) {
                Some((_, body)) => {
                    reply("200 OK", "Content-Type: text/xml\r\n", body.replace("{host}", &host).as_bytes())
                }
                None => reply("404 Not Found", "", b""),
            })
        });
        (Url::parse(&base).unwrap(), served)
    }

    #[tokio::test]
    async fn a_sitemap_robots_txt_names_is_read_beside_the_sites_own() {
        let (start, _) = site_at(vec![
            ("/robots.txt", "User-agent: *\nDisallow: /private\nSitemap: http://{host}/feeds/pages.xml\n"),
            ("/feeds/pages.xml", "<urlset><url><loc>http://{host}/news/a.html</loc></url></urlset>"),
            ("/sitemap.xml", "<urlset><url><loc>http://{host}/news/b.html</loc></url></urlset>"),
        ]);
        let links = site_files(&start, &Site::new(&start), retry_off()).await.links;
        let mut paths: Vec<&str> = links.iter().map(|l| l.url.path()).collect();
        paths.sort();
        assert_eq!(paths, ["/news/a.html", "/news/b.html"]);
    }

    #[tokio::test]
    async fn a_public_run_reads_its_own_sitemap_and_no_file_at_a_private_address() {
        // The start is the loopback server, admitted as the run's own address; robots.txt names a sitemap on another
        // loopback address, which the public run never asks for.
        let (start, _served) = site_at(vec![
            ("/robots.txt", "User-agent: *\nSitemap: http://127.0.0.2:1/feeds/pages.xml\n"),
            ("/sitemap.xml", "<urlset><url><loc>http://{host}/news/b.html</loc></url></urlset>"),
        ]);
        let reach = crate::reach::Reach::Public {
            allowed: std::collections::HashSet::from([std::net::IpAddr::from([127, 0, 0, 1])]),
        };
        let memo = Memo::default();
        let retry = Retry {
            on: true,
            memo: &memo,
            timing: false,
            reach: &reach,
            cookies: &crate::fetch::test_server::NO_COOKIES,
            stealth: None,
        };
        let links = site_files(&start, &Site::new(&start), retry).await.links;
        let paths: Vec<&str> = links.iter().map(|l| l.url.path()).collect();
        assert_eq!(paths, ["/news/b.html"]);
        assert!(links.iter().all(|l| l.url.host_str() != Some("127.0.0.2")));
    }

    #[tokio::test]
    async fn a_sitemap_index_is_read_through_its_three_newest_sitemaps() {
        let (start, served) = site_at(vec![
            (
                "/sitemap.xml",
                "<sitemapindex>\
                 <sitemap><loc>http://{host}/s/2019.xml</loc><lastmod>2019-05-01</lastmod></sitemap>\
                 <sitemap><loc>http://{host}/s/2024.xml</loc><lastmod>2024-05-01</lastmod></sitemap>\
                 <sitemap><loc>http://{host}/s/2021.xml</loc><lastmod>2021-05-01</lastmod></sitemap>\
                 <sitemap><loc>http://{host}/s/2023.xml</loc><lastmod>2023-05-01</lastmod></sitemap>\
                 </sitemapindex>",
            ),
            ("/s/2019.xml", "<urlset><url><loc>http://{host}/news/2019.html</loc></url></urlset>"),
            ("/s/2024.xml", "<urlset><url><loc>http://{host}/news/2024.html</loc></url></urlset>"),
            ("/s/2021.xml", "<urlset><url><loc>http://{host}/news/2021.html</loc></url></urlset>"),
            ("/s/2023.xml", "<urlset><url><loc>http://{host}/news/2023.html</loc></url></urlset>"),
        ]);
        let links = site_files(&start, &Site::new(&start), retry_off()).await.links;
        let mut paths: Vec<&str> = links.iter().map(|l| l.url.path()).collect();
        paths.sort();
        assert_eq!(paths, ["/news/2021.html", "/news/2023.html", "/news/2024.html"]);
        // robots.txt, llms.txt, /sitemap.xml and the three children: the 2019 sitemap is never asked for.
        assert_eq!(served.count(), 6);
    }

    #[tokio::test]
    async fn the_llms_txt_of_a_linked_host_is_read_and_its_pages_are_kept_on_the_registrable_domain() {
        let (linked, _) = site_at(vec![(
            "/llms.txt",
            "# Docs\n\n- [Rate limits](http://{host}/rate-limits): the limits\n- [Off site](https://elsewhere.test/x)\n",
        )]);
        // The front door is on the same host (and so the same registrable domain) as the linked one.
        let start = Url::parse(&format!("http://127.0.0.1:{}/", linked.port().unwrap())).unwrap();
        let links = linked_llms(&start, std::slice::from_ref(&linked), retry_off()).await;
        let urls: Vec<String> = links.iter().map(|l| l.url.path().to_string()).collect();
        assert_eq!(urls, ["/rate-limits"]);
    }

    #[test]
    fn a_page_gives_the_urls_its_text_writes_out_but_not_those_of_its_code() {
        let ex = Extracted {
            blocks: vec![
                crate::extract::Block::new(
                    0,
                    crate::extract::Kind::Para,
                    "The limits are documented at https://shopify.dev/api/usage/rate-limits.".into(),
                ),
                crate::extract::Block::new(1, crate::extract::Kind::Code, "curl https://api.example.com/v1".into()),
            ],
            ..Default::default()
        };
        let bare = bare_links(&ex);
        let urls: Vec<String> = bare.iter().map(|l| l.url.to_string()).collect();
        assert_eq!(urls, ["https://shopify.dev/api/usage/rate-limits"]);
        assert!(bare[0].context.contains("documented at"), "the context is the text around the URL");
        assert!(!bare[0].marginal && bare[0].text.is_empty());
    }

    #[test]
    fn a_page_may_link_to_any_host_as_a_web_page_while_the_site_is_its_own_hosts() {
        let site = Site::new(&u("https://www.shopify.com/"));
        assert!(site.contains(&u("https://help.shopify.com/x")));
        assert!(!site.contains(&u("https://shopify.dev/docs")));
        assert!(Site::reachable(&u("https://shopify.dev/docs")));
        assert!(!Site::reachable(&u("https://shopify.dev/report.pdf")));
        assert!(!Site::reachable(&u("mailto:a@shopify.com")));
    }

    #[tokio::test]
    async fn a_linked_hosts_sitemap_is_read_through_its_robots_txt_and_its_pages_are_kept() {
        let (linked, _) = site_at(vec![
            ("/robots.txt", "User-agent: *\nSitemap: http://{host}/feeds/docs.xml\n"),
            ("/feeds/docs.xml", "<urlset><url><loc>http://{host}/rate-limits</loc></url></urlset>"),
        ]);
        let start = Url::parse(&format!("http://127.0.0.1:{}/", linked.port().unwrap())).unwrap();
        let links = linked_sitemaps(&start, std::slice::from_ref(&linked), retry_off()).await;
        let urls: Vec<String> = links.iter().map(|l| l.url.path().to_string()).collect();
        assert_eq!(urls, ["/rate-limits"]);
    }

    #[test]
    fn robots_names_three_sitemaps_once_each_and_not_the_sites_own() {
        let start = u("https://x.com/");
        let own = u("https://x.com/sitemap.xml");
        let body = "Sitemap: https://x.com/sitemap.xml\nSitemap: https://x.com/a.xml\nSitemap: /b.xml\n\
                    Sitemap: https://x.com/a.xml\nSitemap: https://x.com/c.xml\nSitemap: https://x.com/d.xml\n\
                    Sitemap: ftp://x.com/e.xml\n";
        let urls: Vec<String> = robots_sitemap_urls(body, &start, Some(&own)).iter().map(Url::to_string).collect();
        assert_eq!(urls, ["https://x.com/a.xml", "https://x.com/b.xml", "https://x.com/c.xml"]);
    }
}

//! `--follow`: when the page doesn't answer, look for the answer elsewhere on the same site, the way people play the
//! Wikipedia game. Jev scores every link by how likely it leads to the answer, jurl opens the best few at once, and
//! keeps going from whichever page looks closest, backtracking when a trail goes cold. The site's own map
//! (`llms.txt`, `sitemap.xml`) is read alongside the first page: it often names the right page outright.

use std::collections::HashSet;

use anyhow::{Result, bail};
use futures::future::join_all;
use reqwest::Client;
use serde_json::Map;
use url::Url;

use crate::{
    Args, Ctx, Item, PRECISE_BLOCK_FLOOR, PRECISE_THRESHOLD, Pick, Timer,
    config::Config,
    decide::noul,
    extract::{self, Extracted, Link},
    load, precise_pick, print_blocks, print_precise, score_blocks, top,
};

/// Pages opened at once on each step.
const PARALLEL: usize = 3;
/// Links scored per page, and URLs from the site map: cheap to score (~45 tokens each).
const MAX_LINKS: usize = 250;
const MAX_HINTS: usize = 300;
/// A link's score is discounted per hop, so a good lead near the start beats a slightly better one deep down.
const HOP_DECAY: f64 = 0.85;
/// Hot or cold: a link is worth as much as the page it's on is close to the answer. A page with nothing on the
/// question still passes on this share of its links' scores, so a home page's menu stays usable.
const COLD_PAGE: f64 = 0.3;
/// When the best lead left is weaker than this, the trail has gone cold: stop instead of opening pages blindly.
const COLD_TRAIL: f64 = 0.02;
const ASSETS: &[&str] = &[
    ".pdf", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".zip", ".gz", ".xml", ".json", ".css", ".js", ".mp4",
    ".mp3", ".dmg", ".exe", ".tar", ".ico",
];

/// The site: its host without `www.`, subdomains included (docs.example.com belongs to example.com).
struct Site {
    root: String,
}

impl Site {
    fn new(u: &Url) -> Self {
        Site { root: u.host_str().unwrap_or_default().trim_start_matches("www.").to_string() }
    }

    fn contains(&self, u: &Url) -> bool {
        let lower = u.path().to_lowercase();
        matches!(u.scheme(), "http" | "https")
            && !ASSETS.iter().any(|ext| lower.ends_with(ext))
            && u.host_str().is_some_and(|h| {
                let h = h.trim_start_matches("www.");
                h == self.root || h.ends_with(&format!(".{}", self.root))
            })
    }
}

/// The same page however it was linked: no fragment, no trailing slash.
fn key(u: &Url) -> String {
    let mut u = u.clone();
    u.set_fragment(None);
    u.as_str().trim_end_matches('/').to_string()
}

/// `robots.txt` for every crawler (`User-agent: *`): the paths jurl won't open. Patterns with wildcards are left out
/// rather than guessed at.
#[derive(Default)]
struct Robots {
    disallow: Vec<String>,
}

impl Robots {
    async fn load(client: &Client, start: &Url) -> Self {
        let Ok(url) = start.join("/robots.txt") else { return Self::default() };
        let Some(body) = small_text(client, &url).await else { return Self::default() };
        Self::parse(&body)
    }

    fn parse(body: &str) -> Self {
        let mut disallow = Vec::new();
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
                    if ours && field == "disallow" && !value.is_empty() && !value.contains(['*', '$']) {
                        disallow.push(value.to_string());
                    }
                }
                _ => {}
            }
        }
        Robots { disallow }
    }

    fn allows(&self, u: &Url) -> bool {
        !self.disallow.iter().any(|d| u.path().starts_with(d.as_str()))
    }
}

/// A small text file from the site (robots.txt, llms.txt, a sitemap), or nothing.
async fn small_text(client: &Client, url: &Url) -> Option<String> {
    let res = client.get(url.as_str()).timeout(std::time::Duration::from_secs(4)).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let body = res.text().await.ok()?;
    (!body.trim_start().starts_with('<') || body.contains("<urlset") || body.contains("<sitemapindex"))
        .then_some(body)
        .filter(|b| b.len() < 5_000_000)
}

/// The pages the site lists itself: `llms.txt` (written for exactly this) and `sitemap.xml`, shallowest first.
async fn site_map(client: &Client, start: &Url, site: &Site) -> Vec<Link> {
    let llms = async {
        let url = start.join("/llms.txt").ok()?;
        let body = small_text(client, &url).await?;
        Some(extract::markdown(&body, &url).links)
    };
    let sitemap = async {
        let url = start.join("/sitemap.xml").ok()?;
        let body = small_text(client, &url).await?;
        let mut urls = locs(&body);
        // A sitemap index points at sitemaps: read the first few.
        if body.contains("<sitemapindex") {
            let children = join_all(
                urls.iter()
                    .take(3)
                    .filter_map(|u| Url::parse(u).ok())
                    .map(|u| async move { small_text(client, &u).await.map(|b| locs(&b)).unwrap_or_default() }),
            )
            .await;
            urls = children.into_iter().flatten().collect();
        }
        Some(urls)
    };
    let (llms, sitemap) = tokio::join!(llms, sitemap);

    let mut out: Vec<Link> = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |url: Url, text: String| {
        if site.contains(&url) && seen.insert(key(&url)) {
            out.push(Link { i: out.len(), url, text, context: String::new() });
        }
    };
    for l in llms.unwrap_or_default() {
        add(l.url, l.text);
    }
    let mut pages: Vec<Url> = sitemap.unwrap_or_default().iter().filter_map(|u| Url::parse(u).ok()).collect();
    pages.sort_by_key(|u| (u.path_segments().map_or(0, |s| s.filter(|p| !p.is_empty()).count()), u.as_str().len()));
    for u in pages {
        let text = u.path().to_string();
        add(u, text);
    }
    out.truncate(MAX_HINTS);
    out
}

/// Every `<loc>…</loc>` in a sitemap.
fn locs(xml: &str) -> Vec<String> {
    xml.split("<loc>")
        .skip(1)
        .filter_map(|s| s.split("</loc>").next())
        .map(|s| s.trim().replace("&amp;", "&"))
        .collect()
}

/// How likely each link is to lead to the answer, in `links` order.
async fn score_links(ctx: &Ctx<'_>, links: &[Link], what: &str) -> Vec<f64> {
    if links.is_empty() {
        return Vec::new();
    }
    let q = ctx.args.ask.as_deref().unwrap_or_default();
    let items = links
        .iter()
        .map(|l| Item {
            id: format!("l{}", l.i),
            state: serde_json::json!({ "i": l.i, "text": l.text, "path": l.url.path() }),
            question: Some(noul(format!(
                "{what} with i={} is the page that answers this question, or leads to it: {q}",
                l.i
            ))),
        })
        .collect();
    match ctx.judge("links", items, Map::new()).await {
        Ok(a) => links.iter().map(|l| a.noul(&format!("l{}", l.i)).unwrap_or(0.0)).collect(),
        Err(_) => vec![0.0; links.len()],
    }
}

/// What a page said about the question.
enum Found {
    Precise(Pick),
    Blocks { scores: Vec<Option<f64>>, keep: std::collections::HashMap<usize, f64>, kind: Option<(String, f64)> },
}

struct Visit {
    url: Url,
    ex: Extracted,
    found: Option<Found>,
    /// How sure jurl is that this page answers: the --precise answer's p, or the best block's.
    score: f64,
    /// How close the page is to the question at all: its best block's probability of helping answer it.
    warmth: f64,
    links: Vec<(Url, f64)>,
}

/// Read one page: is the answer here, and which of its links lead on? Both questions go to Jev at once.
async fn visit(args: &Args, cfg: &Config, client: &Client, key: &str, url: &Url, site: &Site) -> Result<Visit> {
    let mut t = Timer::new();
    let (url, ex) = load(args, cfg, client, url, &mut t).await?;
    let (found, score, warmth, links) = {
        let ctx = Ctx::new(args, client, key, &url, &ex);
        let mut seen = HashSet::new();
        // The article's own links first (on Wikipedia, "France" and "Medicine"), then menus and footers ("Pricing").
        let candidates: Vec<Link> = ex
            .links
            .iter()
            .chain(&ex.site_links)
            .filter(|l| site.contains(&l.url) && key_differs(&l.url, &url) && seen.insert(self::key(&l.url)))
            .take(MAX_LINKS)
            .enumerate()
            .map(|(i, l)| Link { i, url: l.url.clone(), text: l.text.clone(), context: String::new() })
            .collect();
        let answer = async {
            let mut t = Timer::new();
            let Ok((scores, kind)) = score_blocks(&ctx, &ex, &mut t).await else { return (None, 0.0) };
            let warmth = scores.iter().flatten().copied().fold(0.0, f64::max);
            if args.precise {
                let keep = top(&scores, PRECISE_BLOCK_FLOOR, 3);
                if keep.is_empty() {
                    return (None, warmth);
                }
                match precise_pick(&ctx, &ex, &keep, &mut t).await {
                    Ok(pick) => {
                        let p = pick.p;
                        (Some((Found::Precise(pick), p)), warmth)
                    }
                    Err(_) => (None, warmth),
                }
            } else {
                let keep = top(&scores, args.threshold(), args.limit(5));
                let best = keep.values().copied().fold(0.0, f64::max);
                ((!keep.is_empty()).then_some((Found::Blocks { scores, keep, kind }, best)), warmth)
            }
        };
        let ((answer, warmth), scores) =
            tokio::join!(answer, score_links(&ctx, &candidates, "Following the link in `links`"));
        let links = candidates.into_iter().zip(scores).map(|(l, p)| (l.url, p)).collect();
        match answer {
            Some((found, score)) => (Some(found), score, warmth, links),
            None => (None, 0.0, warmth, links),
        }
    };
    Ok(Visit { url, ex, found, score, warmth, links })
}

fn key_differs(a: &Url, b: &Url) -> bool {
    key(a) != key(b)
}

/// A page waiting to be opened, and how jurl would get there.
struct Lead {
    url: Url,
    score: f64,
    path: Vec<Url>,
}

pub async fn run(args: &Args, cfg: &Config, client: &Client, key: &str, start: Url, t: &mut Timer) -> Result<()> {
    let max = args.follow.unwrap_or(5).max(1);
    let threshold = if args.precise { args.threshold.unwrap_or(PRECISE_THRESHOLD) } else { args.threshold() };
    let site = Site::new(&start);

    // The first page, the site's own map and robots.txt, all at once.
    let hints = async {
        let links = site_map(client, &start, &site).await;
        let empty = Extracted {
            title: String::new(),
            blocks: Vec::new(),
            images: Vec::new(),
            links: Vec::new(),
            site_links: Vec::new(),
            app_shell: false,
        };
        let ctx = Ctx::new(args, client, key, &start, &empty);
        let scores = score_links(&ctx, &links, "The page at the URL in `links`").await;
        links.into_iter().zip(scores).map(|(l, p)| (l.url, p)).collect::<Vec<_>>()
    };
    let (first, hints, robots) =
        tokio::join!(visit(args, cfg, client, key, &start, &site), hints, Robots::load(client, &start));
    let first = first?;
    t.lap(format!("page 1 + site map ({} pages listed)", hints.len()));

    let mut visited: HashSet<String> = [self::key(&start), self::key(&first.url)].into_iter().collect();
    let mut pages = 1;
    let mut closest: Option<(f64, Visit, Vec<Url>)> = None;
    let mut leads: Vec<Lead> = Vec::new();
    let mut found: Vec<(f64, Visit, Vec<Url>)> = Vec::new();

    // A page that answers is ranked by how sure Jev is of the answer AND of the page: a blog post from two years ago
    // can answer "how much is it?" with full confidence and the old price, while the pricing page was the lead.
    // What each page was like, for -t: how warm, how sure of an answer.
    let mut log: Vec<(Url, f64, f64)> = Vec::new();
    let mut take =
        |v: Visit, lead: f64, path: Vec<Url>, leads: &mut Vec<Lead>, found: &mut Vec<(f64, Visit, Vec<Url>)>| {
            log.push((v.url.clone(), v.warmth, v.score));
            // Hot or cold: the links of a page far from the question count for less, so a wrong turn is dropped and
            // the search goes back to the leads of a page that was getting warmer.
            // The page you started from is never cold: its links are all there is to go on.
            let heat = if path.len() == 1 { 1.0 } else { COLD_PAGE + (1.0 - COLD_PAGE) * v.warmth };
            let decay = HOP_DECAY.powi(path.len() as i32 - 1);
            for (url, p) in &v.links {
                leads.push(Lead { url: url.clone(), score: p * decay * heat, path: path.clone() });
            }
            let rank = v.score * lead;
            if v.found.is_some() && v.score >= threshold {
                found.push((rank, v, path));
            } else if v.found.is_some() && closest.as_ref().is_none_or(|(r, _, _)| rank > *r) {
                closest = Some((rank, v, path));
            }
        };
    let first_path = vec![first.url.clone()];
    take(first, 1.0, first_path, &mut leads, &mut found);
    for (url, p) in hints {
        leads.push(Lead { url, score: p, path: vec![start.clone()] });
    }

    let mut cold = false;
    while found.is_empty() && pages < max {
        leads.sort_by(|a, b| b.score.total_cmp(&a.score));
        leads.retain(|l| !visited.contains(&self::key(&l.url)));
        if leads.first().is_none_or(|l| l.score < COLD_TRAIL) {
            cold = true;
            break;
        }
        let mut batch = Vec::new();
        let mut rest = Vec::new();
        for lead in leads.drain(..) {
            let k = self::key(&lead.url);
            if visited.contains(&k) || !robots.allows(&lead.url) {
                continue;
            }
            if batch.len() < PARALLEL.min(max - pages) {
                visited.insert(k);
                batch.push(lead);
            } else {
                rest.push(lead);
            }
        }
        leads = rest;
        if batch.is_empty() {
            break;
        }
        let results = join_all(batch.iter().map(|l| visit(args, cfg, client, key, &l.url, &site))).await;
        pages += batch.len();
        t.lap(format!("{} more", batch.len()));
        for (lead, r) in batch.into_iter().zip(results) {
            match r {
                Ok(v) => {
                    visited.insert(self::key(&v.url));
                    let mut path = lead.path.clone();
                    path.push(v.url.clone());
                    take(v, lead.score, path, &mut leads, &mut found);
                }
                Err(e) if args.timing => eprintln!("jurl: skipped {}: {e:#}", lead.url),
                Err(_) => {}
            }
        }
    }

    if args.timing {
        for (url, warmth, score) in &log {
            eprintln!("   warmth {warmth:.2} · answer {score:.2} · {url}");
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let trail = |path: &[Url]| {
        path.iter().map(|u| u.as_str().trim_start_matches("https://").to_string()).collect::<Vec<_>>().join(" → ")
    };
    let (v, path, answered) = match found.into_iter().next() {
        Some((_, v, path)) => (v, path, true),
        None => match closest {
            // JSON says what came closest, as on a single page; text fails with it.
            Some((_, v, path)) if args.json && args.precise => (v, path, false),
            Some((_, v, path)) => {
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
                let _ = path;
                bail!("read {pages} pages of {} and none answers that{what}", site.root);
            }
            None if cold => {
                bail!("read {pages} pages of {} and none answers that; no link left looks promising", site.root)
            }
            None => bail!("read {pages} pages of {} and none answers that", site.root),
        },
    };
    let ctx = Ctx::new(args, client, key, &v.url, &v.ex);
    match &v.found {
        Some(Found::Precise(pick)) => print_precise(&ctx, &v.ex, pick, Some(&path))?,
        Some(Found::Blocks { scores, keep, kind }) => {
            print_blocks(&ctx, &v.ex, scores, keep, kind.clone(), Some(&path))?
        }
        None => unreachable!("found pages always have an answer"),
    }
    if !args.json {
        let state = if answered { "found" } else { "closest" };
        eprintln!("jurl: {state} after reading {pages} page{}: {}", if pages == 1 { "" } else { "s" }, trail(&path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_site_includes_subdomains_not_assets() {
        let site = Site::new(&Url::parse("https://www.linear.app/").unwrap());
        assert!(site.contains(&Url::parse("https://linear.app/pricing").unwrap()));
        assert!(site.contains(&Url::parse("https://docs.linear.app/start").unwrap()));
        assert!(!site.contains(&Url::parse("https://notlinear.app/").unwrap()));
        assert!(!site.contains(&Url::parse("https://linear.app/brand.pdf").unwrap()));
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

    #[test]
    fn sitemap_locs() {
        let xml = "<urlset><url><loc>https://x.com/a?b=1&amp;c=2</loc></url><url><loc> https://x.com/pricing </loc></url></urlset>";
        assert_eq!(locs(xml), ["https://x.com/a?b=1&c=2", "https://x.com/pricing"]);
        assert_eq!(key(&Url::parse("https://x.com/pricing/#plans").unwrap()), "https://x.com/pricing");
    }
}

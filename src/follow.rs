//! `--follow`: when the page doesn't answer, look for the answer elsewhere on the same site, the way people play the
//! Wikipedia game. It is `--precise` (or `-q`) and `--links -q` in a loop: each page is asked for the answer and its
//! links are scored by how likely they lead to it, jurl opens the best few at once, and keeps going from whichever
//! page looks closest, backtracking when a trail goes cold (hot and cold). The site's own map (`llms.txt`,
//! `sitemap.xml`) is read alongside the first page: it often names the right page outright.

use std::collections::HashSet;

use anyhow::{Result, bail};
use futures::future::join_all;
use reqwest::Client;
use serde_json::Map;
use url::Url;

use crate::{
    Args, Ctx, Item, PRECISE_BLOCK_FLOOR, PRECISE_THRESHOLD, Pick, Timer,
    config::Config,
    extract::{self, Extracted, Link},
    links::{self, key, overlap},
    load, precise_pick, print_blocks, print_precise, score_blocks, top,
};

/// Pages opened at once on each step. Two, so the default 5 pages are two full steps (1 + 2 + 2): answers two links
/// away are within reach, and when Jev's first pick is right (most of the time) the second page costs little.
const PARALLEL: usize = 2;
/// A long search (`--follow 10` and up) goes wider: a long trail needs more than one or two guesses per step.
const PARALLEL_LONG: usize = 3;
/// URLs from the site map scored, like a page's links (at most [`links::MAX_LINKS`]).
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
            out.push(Link { i: out.len(), url, text, context: String::new(), marginal: false });
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
    links: Vec<(Url, String, f64)>,
    /// Its menus and footers, see [`menus`].
    menus: HashSet<String>,
}

/// A page's links outside its text: menus and footers, which a site repeats on every page.
fn menus(ex: &Extracted) -> HashSet<String> {
    let text: HashSet<String> = ex.links.iter().map(|l| key(&l.url)).collect();
    ex.site_links.iter().map(|l| key(&l.url)).filter(|k| !text.contains(k)).collect()
}

/// Read one page: is the answer here, and which of its links lead on? Both questions go to Jev at once. `known` holds
/// the menu links of the pages read before.
async fn visit(
    args: &Args,
    cfg: &Config,
    client: &Client,
    key: &str,
    url: &Url,
    site: &Site,
    known: &HashSet<String>,
) -> Result<Visit> {
    let mut t = Timer::new();
    let (url, ex) = load(args, cfg, client, url, &mut t).await?;
    let menus = menus(&ex);
    let (found, score, warmth, links) = {
        let ctx = Ctx::new(args, client, key, &url, &ex);
        // The links `--links -q` would score, menus and footers included, as long as they stay on the site. A menu
        // link is scored on the first page it's on, not again on every page: on a page far from the question, the
        // site's "Main page" and "Search" would outscore everything in its text.
        let candidates = links::candidates(&ctx, &ex, |u| {
            let k = links::key(u);
            site.contains(u) && !(menus.contains(&k) && known.contains(&k))
        });
        // The answer the way `-q` finds it (score_blocks), and with --precise, the way --precise picks it.
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
        // `--links -q` on the same page, at the same time; a long search also asks for the answer's field.
        let long = args.follow.unwrap_or(5) >= 10;
        let leads = links::score(&ctx, &candidates, "Following the link in `links`", long);
        let ((answer, warmth), scores) = tokio::join!(answer, leads);
        let scores = scores.unwrap_or_else(|_| vec![0.0; candidates.len()]);
        let links = candidates
            .into_iter()
            .zip(scores)
            .map(|(l, p)| {
                let menu = long && menus.contains(&links::key(&l.url));
                (l.url, l.text, if menu { p * LONG_MENU } else { p })
            })
            .collect();
        match answer {
            Some((found, score)) => (Some(found), score, warmth, links),
            None => (None, 0.0, warmth, links),
        }
    };
    Ok(Visit { url, ex, found, score, warmth, links, menus })
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
/// pages one at a time aren't on the same scale. Returns the shortlist reordered by Jev's choice, and the share that
/// went to "none of these" (an option so the others aren't forced to look good).
async fn shortlist(ctx: &Ctx<'_>, leads: &[&Lead]) -> Option<(Vec<usize>, f64)> {
    let q = ctx.args.ask.as_deref().unwrap_or_default();
    let items: Vec<Item> = leads
        .iter()
        .enumerate()
        .map(|(i, l)| Item {
            id: format!("o{i}"),
            state: serde_json::json!({ "i": i, "text": l.text, "path": l.url.path(), "found_on": l.path.last().map(|u| u.path()) }),
            question: None,
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
    let a = ctx.judge("links", items, Map::from_iter([("next".to_string(), pick)])).await.ok()?;
    let probs = a.probabilities("next")?;
    let mut order: Vec<usize> = (0..leads.len()).collect();
    order.sort_by(|&x, &y| {
        let p = |i: usize| probs.get(&format!("o{i}")).copied().unwrap_or(0.0);
        p(y).total_cmp(&p(x))
    });
    Some((order, probs.get("none").copied().unwrap_or(0.0)))
}

pub async fn run(args: &Args, cfg: &Config, client: &Client, key: &str, start: Url, t: &mut Timer) -> Result<()> {
    let max = args.follow.unwrap_or(5).max(1);
    let threshold = if args.precise { args.threshold.unwrap_or(PRECISE_THRESHOLD) } else { args.threshold() };
    let site = Site::new(&start);

    // The first page, the site's own map and robots.txt, all at once.
    let hints = async {
        let q = args.ask.as_deref().unwrap_or_default();
        let mut links = site_map(client, &start, &site).await;
        // The site's pages that share words with the question first, then the shallowest.
        if links.len() > MAX_HINTS {
            let mut ranked: Vec<(usize, Link)> = links.into_iter().map(|l| (overlap(q, &l), l)).collect();
            ranked.sort_by_key(|(o, l)| (std::cmp::Reverse(*o), l.i));
            links = ranked.into_iter().take(MAX_HINTS).map(|(_, l)| l).collect();
        }
        // The start page is scored as a candidate too: how much its own answer counts against the site's other pages.
        links.retain(|l| self::key(&l.url) != self::key(&start));
        links
            .insert(0, Link { i: 0, url: start.clone(), text: String::new(), context: String::new(), marginal: false });
        for (i, l) in links.iter_mut().enumerate() {
            l.i = i;
        }
        let empty = Extracted {
            title: String::new(),
            blocks: Vec::new(),
            images: Vec::new(),
            links: Vec::new(),
            site_links: Vec::new(),
            app_shell: false,
        };
        let ctx = Ctx::new(args, client, key, &start, &empty);
        let scores = links::score(&ctx, &links, "The page at the URL in `links`", false)
            .await
            .unwrap_or_else(|_| vec![0.0; links.len()]);
        links.into_iter().zip(scores).map(|(l, p)| (l.url, l.text, p)).collect::<Vec<_>>()
    };
    let mut known = HashSet::new();
    let (first, hints, robots) =
        tokio::join!(visit(args, cfg, client, key, &start, &site, &known), hints, Robots::load(client, &start));
    let first = first?;
    known.extend(first.menus.iter().cloned());
    t.lap(format!("page 1 + site map ({} pages listed)", hints.len()));

    let mut visited: HashSet<String> = [self::key(&start), self::key(&first.url)].into_iter().collect();
    let mut pages = 1;
    let mut closest: Option<(f64, Visit, Vec<Url>)> = None;
    let mut leads: Vec<Lead> = Vec::new();
    let mut found: Vec<(f64, Visit, Vec<Url>)> = Vec::new();
    // How well the start page fits the question as a page, from the same scoring as the site map's pages. A home page
    // answering in passing (a FAQ line) counts for less than a pricing page that the site lists.
    let mut hints = hints;
    let start_fit = if hints.first().is_some_and(|h| self::key(&h.0) == self::key(&start)) {
        hints.remove(0).2.max(COLD_PAGE)
    } else {
        1.0
    };

    // A page that answers is ranked by how sure Jev is of the answer AND of the page: a blog post from two years ago
    // can answer "how much is it?" with full confidence and the old price, while the pricing page was the lead.
    // What each page was like, for -t: how warm, how sure of an answer.
    let mut log: Vec<(Url, f64, f64)> = Vec::new();
    let mut take = |v: Visit,
                    lead: f64,
                    lead_p: f64,
                    path: Vec<Url>,
                    leads: &mut Vec<Lead>,
                    found: &mut Vec<(f64, Visit, Vec<Url>)>| {
        log.push((v.url.clone(), v.warmth, v.score));
        // Hot or cold: the links of a page far from the question count for less, so a wrong turn is dropped and
        // the search goes back to the leads of a page that was getting warmer.
        // The page you started from is never cold: its links are all there is to go on.
        // A hub page (a docs index, a category) says nothing itself but links straight to the answer: it's as warm
        // as its best link. And warmth is relative: far from the answer (Paris → … → Aspirin) every page is cold,
        // but one whose best link looks better than the link that led here is getting warmer.
        let best_link = v.links.iter().map(|l| l.2).fold(0.0, f64::max);
        let warmer = (best_link / lead_p.max(0.05)).min(1.0);
        let heat =
            if path.len() == 1 { 1.0 } else { COLD_PAGE + (1.0 - COLD_PAGE) * v.warmth.max(best_link).max(warmer) };
        let decay = HOP_DECAY.powi(path.len() as i32 - 1);
        for (url, text, p) in &v.links {
            leads.push(Lead {
                url: url.clone(),
                text: text.clone(),
                score: p * decay * heat,
                p: *p,
                path: path.clone(),
            });
        }
        let rank = v.score * lead;
        if v.found.is_some() && v.score >= threshold {
            found.push((rank, v, path));
        } else if v.found.is_some() && closest.as_ref().is_none_or(|(r, _, _)| rank > *r) {
            closest = Some((rank, v, path));
        }
    };
    let first_path = vec![first.url.clone()];
    take(first, start_fit, 1.0, first_path, &mut leads, &mut found);
    for (url, text, p) in hints {
        leads.push(Lead { url, text, score: p, p, path: vec![start.clone()] });
    }

    let empty = Extracted {
        title: String::new(),
        blocks: Vec::new(),
        images: Vec::new(),
        links: Vec::new(),
        site_links: Vec::new(),
        app_shell: false,
    };
    let site_ctx = Ctx::new(args, client, key, &start, &empty);
    let mut cold = false;
    while pages < max {
        leads.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut seen = HashSet::new();
        leads.retain(|l| {
            let k = self::key(&l.url);
            !visited.contains(&k) && robots.allows(&l.url) && seen.insert(k)
        });
        // Done when no page left could beat what's been found: each lead's score is the most it could rank.
        let best_found = found.iter().map(|f| f.0).fold(0.0, f64::max);
        if !found.is_empty() && leads.first().is_none_or(|l| l.score <= best_found + BETTER_BY) {
            break;
        }
        if leads.first().is_none_or(|l| l.score < COLD_TRAIL) {
            cold = true;
            break;
        }
        // Jev picks the next pages out of the best few, side by side, or says none of them leads anywhere.
        let n = (if max >= 10 { PARALLEL_LONG } else { PARALLEL }).min(max - pages);
        let short: Vec<&Lead> = leads.iter().take(SHORTLIST).collect();
        // On a long trail (`--follow 10` and up) no page "is the next step" to something far away, so Jev's side-by-side
        // pick only adds noise there: the leads' own scores decide.
        let order = if short.len() > n && max < 10 { shortlist(&site_ctx, &short).await } else { None };
        t.lap("next");
        // "None of these leads anywhere" isn't a reason to stop: on a long trail (Paris → … → Aspirin) no single step
        // looks like it leads to the answer. It only orders the shortlist.
        let picks: Vec<usize> = match order {
            Some((order, _)) => order.into_iter().take(n).collect(),
            None => (0..n.min(leads.len())).collect(),
        };
        let mut batch: Vec<Lead> = Vec::new();
        for i in picks.into_iter().rev().collect::<std::collections::BTreeSet<_>>().into_iter().rev() {
            batch.push(leads.remove(i));
        }
        for l in &batch {
            visited.insert(self::key(&l.url));
        }
        let results = join_all(batch.iter().map(|l| visit(args, cfg, client, key, &l.url, &site, &known))).await;
        pages += batch.len();
        t.lap(format!("{} more", batch.len()));
        for (lead, r) in batch.into_iter().zip(results) {
            match r {
                Ok(v) => {
                    visited.insert(self::key(&v.url));
                    known.extend(v.menus.iter().cloned());
                    let mut path = lead.path.clone();
                    path.push(v.url.clone());
                    take(v, lead.score, lead.p, path, &mut leads, &mut found);
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
        let tokens = crate::decide::JEV_TOKENS.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!("   {pages} pages · {tokens} tokens");
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
    }
}

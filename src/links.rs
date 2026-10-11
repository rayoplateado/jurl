//! Links scored against a question: `--links -q` prints them, and `--follow` opens the best ones, page after page.
//! The question is whether following a link leads to the answer, so a menu's "Pricing" counts as much as a link in
//! the text: it is often the way there.

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicU64,
};

use anyhow::Result;
use serde_json::{Map, json};
use url::Url;

use crate::{
    blocks::top,
    decide::{Answers, noul},
    extract::{Extracted, Link},
    judge::{Ctx, Item, tag},
    output::{Rendered, not_found},
    timing::Timer,
};

/// Links scored per page: cheap to score (~45 tokens each).
pub const MAX_LINKS: usize = 250;
/// Links printed when `--max` doesn't say.
pub(crate) const DEFAULT_LINKS: usize = 20;
/// On a long search, how much a link's field of knowledge counts next to whether it leads to the answer.
const FIELD_WEIGHT: f64 = 0.3;
/// A link's field score (see [`score`]), by [`key`]. The question doesn't depend on the page the link is on, so a
/// search asks it once per link and keeps the answers for the pages after.
pub type FieldScores = HashMap<String, f64>;

/// The same page however it was linked: no fragment, no trailing slash.
pub fn key(u: &Url) -> String {
    let mut u = u.clone();
    u.set_fragment(None);
    u.as_str().trim_end_matches('/').to_string()
}

/// The links worth asking about, at most [`MAX_LINKS`]: the page's own links first (on Wikipedia, "France" and
/// "Medicine"), then menus and footers ("Pricing"), each page once and not the page itself. Footnote marks and
/// image-only links are left out ([`Link::marginal`]): they open "[clarification needed]" or a
/// photo's own page, never the topic. `keep` narrows them (`--follow` stays on the site).
pub fn candidates(ctx: &Ctx<'_>, ex: &Extracted, extra: &[Link], keep: impl Fn(&Url) -> bool) -> Vec<Link> {
    let all = page_links(ctx.url, ex, extra, keep);
    most_relevant(ctx.args.ask.as_deref().unwrap_or_default(), all, MAX_LINKS)
}

/// The links a page gives that may be candidates, in page order: its own links, then its menus and footers, then the URLs
/// its text writes out (`extra`). Each page once; the page itself, a footnote mark and what `keep` drops are left out. A
/// link the page doesn't give is never here: nothing is made up.
fn page_links(page: &Url, ex: &Extracted, extra: &[Link], keep: impl Fn(&Url) -> bool) -> Vec<Link> {
    let mut seen = HashSet::from([key(page)]);
    ex.links
        .iter()
        .chain(&ex.site_links)
        .chain(extra)
        .filter(|l| !l.marginal && keep(&l.url) && seen.insert(key(&l.url)))
        .enumerate()
        .map(|(i, l)| Link { i, ..l.clone() })
        .collect()
}

/// How many of the question's words a link shares (by their first five letters: "limit" finds `/limits/`). A word of
/// under four bytes is left out: most short words are articles and prepositions ("the", "for", "how", "and"), which
/// many links share by chance and which say nothing about a link's topic. Measured on 25 answers on 19 sites: at three
/// bytes "the" matched 370 of Vercel's 8,628 links, more than the cap, so its pricing page was pushed out; four drops
/// "the" and keeps "plan" and "paid" (with five, they stop counting). Only used to choose which links get scored when a
/// page or a site has more than the cap; Jev does the scoring.
pub fn overlap(question: &str, link: &Link) -> usize {
    let stem = |w: &str| w.chars().take(5).collect::<String>();
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 4).map(|w| w.to_lowercase()).collect()
    };
    let link_words: HashSet<String> =
        words(&format!("{} {}", link.text, link.url.path())).iter().map(|w| stem(w)).collect();
    words(question).iter().filter(|w| link_words.contains(&stem(w))).count()
}

/// At most `max`: those that share words with the question first, then in page order; a list that fits is kept as it
/// is. A page's own links are cut this way (see [`candidates`]): where a link sits on the page is its order. Not spread
/// over directories, as [`spread`] spreads a site's listing: on Wikipedia's pages the references are in other hosts'
/// directories, and spreading them over those took the in-host links out of the cap (Pizza kept 99 of its in-host
/// links, 22 when spread).
pub fn most_relevant(question: &str, links: Vec<Link>, max: usize) -> Vec<Link> {
    if links.len() <= max {
        return in_order(&links, 0..links.len());
    }
    let shared: Vec<usize> = links.iter().map(|l| overlap(question, l)).collect();
    let mut order: Vec<usize> = (0..links.len()).collect();
    order.sort_by_key(|&p| (std::cmp::Reverse(shared[p]), p));
    in_order(&links, order.into_iter().take(max))
}

/// The links of a site's own listing (its sitemaps and llms.txt), at most `max`, in the order a search reads them.
/// Those that share words with the question come first; then the shallowest pages, since a site's sections and their
/// answers sit near its root (Vercel's `/pricing`); then, within one depth, the directories take turns, one link from
/// each per turn. So a directory of hundreds of similar pages (Air Europa's `/es-es/ofertas-…`) gives one link per turn
/// instead of the whole cap, and a small section at the same depth is in the cap. In a directory, page order holds. A
/// listing that fits is kept as it is.
///
/// Measured on 25 answers on 19 sites, against word-then-page order: no answer is lost; the cap spans more directories
/// (Cloudflare's docs: 108 to 149, the top directory's share 10% to 9%); the top directory's share is lower in 17
/// listings and higher in 3. Spreading across depths as well, not only within one, lost root-level answers.
pub fn spread(question: &str, links: Vec<Link>, max: usize) -> Vec<Link> {
    if links.len() <= max {
        return in_order(&links, 0..links.len());
    }
    let shared: Vec<usize> = links.iter().map(|l| overlap(question, l)).collect();
    let depths: Vec<usize> = links.iter().map(|l| segments(&l.url).len()).collect();
    let mut order: Vec<usize> = (0..links.len()).collect();
    order.sort_by_key(|&p| (std::cmp::Reverse(shared[p]), depths[p], p));
    let mut picked: Vec<usize> = Vec::with_capacity(links.len());
    for bucket in order.chunk_by(|&a, &b| (shared[a], depths[a]) == (shared[b], depths[b])) {
        // The bucket's directories in the order each one's first link comes, each with its links in page order.
        let mut dirs: Vec<Vec<usize>> = Vec::new();
        let mut at: HashMap<String, usize> = HashMap::new();
        for &p in bucket {
            let d = *at.entry(directory(&links[p].url)).or_insert_with(|| {
                dirs.push(Vec::new());
                dirs.len() - 1
            });
            dirs[d].push(p);
        }
        let turns = dirs.iter().map(Vec::len).max().unwrap_or(0);
        for turn in 0..turns {
            picked.extend(dirs.iter().filter_map(|d| d.get(turn).copied()));
        }
    }
    in_order(&links, picked.into_iter().take(max))
}

/// The links at `order`, in that order, numbered from 0.
fn in_order(links: &[Link], order: impl Iterator<Item = usize>) -> Vec<Link> {
    order.enumerate().map(|(i, p)| Link { i, ..links[p].clone() }).collect()
}

/// A URL's path segments, empty ones left out.
fn segments(url: &Url) -> Vec<&str> {
    url.path_segments().map(|s| s.filter(|p| !p.is_empty()).collect()).unwrap_or_default()
}

/// The directory a page is in: its host and its path without the last segment (`/es-es/ofertas-x` is in `host/es-es`).
fn directory(url: &Url) -> String {
    let parent = segments(url);
    format!("{}/{}", url.host_str().unwrap_or_default(), parent[..parent.len().saturating_sub(1)].join("/"))
}

/// How likely following each link leads to the answer to `-q`, in `links` order. `what` names the link in the
/// question ("Following the link in `links`"; for the site map's URLs, "The page at the URL in `links`").
///
/// Far from the answer (Tennis → … → the boiling point of mercury) no link "leads to the answer" and those scores are
/// noise (Birmingham 0.08, Philadelphia 0.07). With `field`, each link is also asked whether its page is in the
/// answer's field of knowledge: vulcanized rubber and polyester (0.9) are chemistry, the way to the elements. That
/// counts for [`FIELD_WEIGHT`] of what the first score leaves. Only long searches ask it (`--follow 10` and up), and
/// `field` is `Some` for them: the field scores the search has so far. A link in it isn't asked again, its known score
/// counts, and the field scores asked here come back with the scores, for the search to keep.
pub async fn score(
    ctx: &Ctx<'_>,
    links: &[Link],
    what: &str,
    field: Option<&FieldScores>,
) -> Result<(Vec<f64>, FieldScores)> {
    Ok(score_shared(std::slice::from_ref(ctx), &[links], what, &[field]).await?.swap_remove(0))
}

/// [`score`] for the questions of one page, or of a site's listing (see `follow::run_many`): one request, in which a link that
/// two questions score is one item carrying both questions (see [`tag`]). `lists[k]` and `fields[k]` are question `k`'s links and
/// field scores. Each question gets its own scores, in its list's order, and the field scores it asked for.
pub async fn score_shared(
    ctxs: &[Ctx<'_>],
    lists: &[&[Link]],
    what: &str,
    fields: &[Option<&FieldScores>],
) -> Result<Vec<(Vec<f64>, FieldScores)>> {
    let n = ctxs.len();
    let per_question: Vec<Vec<Item>> =
        (0..n).map(|k| items_tagged(&ctxs[k], lists[k], what, fields[k], &tag(k, n))).collect();
    let (merged, weights) = merge_items(per_question);
    let answers = if merged.is_empty() {
        Answers::default()
    } else {
        let shares: Vec<(&AtomicU64, u64)> =
            ctxs.iter().zip(&weights).map(|(c, &w)| (c.args.spent.as_ref(), w)).collect();
        ctxs[0].judge_shared("links", merged, Map::new(), &shares).await?
    };
    Ok((0..n).map(|k| read_tagged(lists[k], &answers, fields[k], &tag(k, n))).collect())
}

/// Several questions' items as one request's items: an item whose state another question's item has too is the same item, carrying
/// both questions. Also each question's number of entries, its share of the request's tokens (see `Ctx::judge_shared`).
fn merge_items(per_question: Vec<Vec<Item>>) -> (Vec<Item>, Vec<u64>) {
    let mut merged: Vec<Item> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut weights = Vec::with_capacity(per_question.len());
    for items in per_question {
        weights.push(items.iter().map(|item| item.questions.len() as u64).sum());
        for item in items {
            let state = item.state.to_string();
            match at.get(&state) {
                Some(&i) => merged[i].questions.extend(item.questions),
                None => {
                    at.insert(state, merged.len());
                    merged.push(item);
                }
            }
        }
    }
    (merged, weights)
}

/// What `score` asks Jev about, one item per link: its entry in the state and the question whether following it leads
/// to the answer. A link whose field isn't known yet also gets its field question on the same item, so the two share a
/// request and the link's state is sent once. Each question's id carries `tag` (see [`tag`]): several questions of the same
/// links share a request (see [`score_shared`]).
fn items_tagged(ctx: &Ctx<'_>, links: &[Link], what: &str, field: Option<&FieldScores>, tag: &str) -> Vec<Item> {
    let q = ctx.ask_per_link();
    // A link off the page's host says where it goes (`--links` keeps them; `--follow` only leaves for a subdomain).
    let host = |l: &Link| l.url.host_str().filter(|h| Some(*h) != ctx.url.host_str()).map(str::to_string);
    links
        .iter()
        .map(|l| {
            let mut state = json!({ "i": l.i, "text": l.text, "context": l.context, "path": l.url.path() });
            if let Some(h) = host(l) {
                state["host"] = json!(h);
            }
            let mut questions = vec![(
                format!("l{}{tag}", l.i),
                noul(format!("{what} with i={} is the page that answers this question, or leads to it: {q}", l.i)),
            )];
            if field.is_some_and(|known| !known.contains_key(&key(&l.url))) {
                questions.push((
                    format!("f{}{tag}", l.i),
                    noul(format!(
                        "{what} with i={} is about the same field of knowledge as the answer to this question \
                         (chemistry, astronomy, literature, medicine…): {q}",
                        l.i
                    )),
                ));
            }
            Item { state, questions }
        })
        .collect()
}

/// Each link's score from Jev's answers, in `links` order, and the field scores asked for here (not the known ones).
/// A field Jev left out counts for nothing on this page and isn't kept, so the next page asks it again.
/// The scores of `links` from Jev's answers to the ids [`items_tagged`] made with `tag`, and the field scores asked for here.
fn read_tagged(links: &[Link], a: &Answers, field: Option<&FieldScores>, tag: &str) -> (Vec<f64>, FieldScores) {
    let mut asked = FieldScores::new();
    let mut scores = Vec::with_capacity(links.len());
    for l in links {
        let p = a.noul(&format!("l{}{tag}", l.i)).unwrap_or(0.0);
        let k = key(&l.url);
        let f = match field {
            None => 0.0,
            Some(known) => match known.get(&k) {
                Some(&f) => f,
                None => match a.noul(&format!("f{}{tag}", l.i)) {
                    Some(f) => {
                        asked.insert(k, f);
                        f
                    }
                    None => 0.0,
                },
            },
        };
        scores.push(p + (1.0 - p) * FIELD_WEIGHT * f);
    }
    (scores, asked)
}

/// --links: the links worth following, best first. With -q, the links most likely to lead to the answer: what
/// --follow opens, menus and footers included (a nav bar's "Pricing" is often the way to a price).
pub(crate) async fn links(ctx: &Ctx<'_>, ex: &Extracted, t: &mut Timer) -> Result<Rendered> {
    let (candidates, scores): (Vec<Link>, Vec<Option<f64>>) = if ctx.args.ask.is_some() {
        let candidates = candidates(ctx, ex, &[], |_| true);
        if candidates.is_empty() {
            return Err(not_found(format!("no links found in {}", ctx.url)));
        }
        let (scores, _) = score(ctx, &candidates, "Following the link in `links`", None).await?;
        t.lap(format!("{} links", candidates.len()));
        (candidates, scores.into_iter().map(Some).collect())
    } else {
        // Footnote marks and image-only links are left out, as with -q: they open a note or a photo, not the topic.
        let candidates: Vec<Link> = ex.links.iter().filter(|l| !l.marginal).cloned().collect();
        if candidates.is_empty() {
            return Err(not_found(format!("no links found in {}", ctx.url)));
        }
        let items = candidates
            .iter()
            .map(|l| Item {
                state: json!({ "i": l.i, "text": l.text, "context": l.context, "host": l.url.host_str() }),
                questions: vec![(
                    format!("l{}", l.i),
                    ctx.question(
                        &format!("The link in `links` with i={}", l.i),
                        "points to something a reader of this page would want to follow — referenced articles, \
                         sources, docs, downloads or related content — not site navigation, login, social sharing, \
                         legal pages or ads.",
                    ),
                )],
            })
            .collect();
        let a = ctx.judge("links", items, Map::new()).await?;
        t.lap(a.label());
        let scores = candidates.iter().map(|l| a.noul(&format!("l{}", l.i))).collect();
        (candidates, scores)
    };
    let kept = top(&scores, ctx.args.threshold(), ctx.args.limit(DEFAULT_LINKS));
    if kept.is_empty() {
        return Err(not_found(format!("no links worth following in {} (try a lower --threshold)", ctx.url)));
    }

    let v: Vec<_> = kept
        .iter()
        .map(|(i, p)| {
            let l = &candidates[*i];
            json!({ "url": l.url.as_str(), "text": l.text, "p": p })
        })
        .collect();
    let text = kept.iter().map(|(i, _)| format!("{}\n", candidates[*i].url)).collect();
    Ok(Rendered { text, json: json!({ "url": ctx.url.as_str(), "title": ex.title, "links": v }) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cli::Args, extract};
    use clap::Parser;
    use reqwest::Client;

    #[test]
    fn question_words_pick_which_links_get_scored() {
        let link = |i: usize, path: &str| Link {
            i,
            url: Url::parse(&format!("https://x.com{path}")).unwrap(),
            text: String::new(),
            context: String::new(),
            marginal: false,
        };
        let mut links: Vec<Link> = (0..MAX_LINKS + 50).map(|i| link(i, &format!("/page/{i}"))).collect();
        links.push(link(MAX_LINKS + 50, "/workers/platform/limits/"));
        let kept = most_relevant("What is the CPU time limit for Workers?", links, MAX_LINKS);
        assert_eq!(kept.len(), MAX_LINKS);
        assert_eq!(kept[0].url.path(), "/workers/platform/limits/");
        assert_eq!(kept[0].i, 0);
    }

    #[test]
    fn a_flooded_directory_does_not_take_the_whole_cap() {
        // Three directories at one depth: 300 similar pages in one, two pages in each of the others. The cap is 100,
        // and the two small directories are in it beside the flood, whose page order holds.
        let mut links: Vec<Link> = (0..300).map(|i| link(&format!("https://x.com/ofertas/o{i}"))).collect();
        for path in ["/sitemap/a", "/sitemap/b", "/rutas/c", "/rutas/d"] {
            links.push(link(&format!("https://x.com{path}")));
        }
        let kept = spread("What is the baggage allowance?", links, 100);
        let paths: Vec<&str> = kept.iter().map(|l| l.url.path()).collect();
        assert_eq!(paths.len(), 100);
        for path in ["/sitemap/a", "/sitemap/b", "/rutas/c", "/rutas/d"] {
            assert!(paths.contains(&path), "{path}");
        }
        assert_eq!(paths[0], "/ofertas/o0");
    }

    #[test]
    fn a_shallower_page_comes_before_a_deeper_one_whatever_its_directory() {
        // The site's `/pricing` is one level down, the other pages two levels further. The cap of two keeps `/pricing`
        // and the first deep page, not the first pages of two directories one level further down.
        let links = vec![link("https://x.com/docs/a/b"), link("https://x.com/docs/c/d"), link("https://x.com/pricing")];
        let paths: Vec<String> =
            spread("Which plan is cheapest?", links, 2).iter().map(|l| l.url.path().to_string()).collect();
        assert_eq!(paths, ["/pricing", "/docs/a/b"]);
    }

    #[test]
    fn a_shared_word_comes_first_within_a_depth_and_the_directories_take_turns_after() {
        // "Workers" and "limits" are shared by one link each, in two directories, two levels down. The 50 pages of
        // another directory at that depth share nothing. The cap of three keeps the two shared links, then the first
        // page.
        let mut links: Vec<Link> = (0..50).map(|i| link(&format!("https://x.com/a/p{i}"))).collect();
        links.push(link("https://x.com/b/limits"));
        links.push(link("https://x.com/c/workers"));
        let paths: Vec<String> =
            spread("What are the Workers limits?", links, 3).iter().map(|l| l.url.path().to_string()).collect();
        assert_eq!(paths, ["/b/limits", "/c/workers", "/a/p0"]);
    }

    #[test]
    fn a_word_under_four_bytes_shares_nothing_and_one_of_four_does() {
        // "the" and "for" are articles and prepositions; "plan" and "paid" are the words of a pricing question.
        let question = "What is the plan for paid members?";
        assert_eq!(overlap(question, &link("https://x.com/the-team/for-you")), 0);
        assert_eq!(overlap(question, &link("https://x.com/plan/paid")), 2);
    }

    #[test]
    fn the_same_listing_gives_the_same_cut_in_the_same_order_every_time() {
        let links: Vec<Link> = (0..400).map(|i| link(&format!("https://x.com/d{}/p{i}", i % 7))).collect();
        let cut = |links: Vec<Link>| -> Vec<(usize, String)> {
            spread("Which page is it?", links, 100).into_iter().map(|l| (l.i, l.url.to_string())).collect()
        };
        let first = cut(links.clone());
        assert_eq!(first.len(), 100);
        assert_eq!(first, cut(links.clone()));
        assert!(first.iter().enumerate().all(|(k, (i, _))| k == *i), "numbered in order");
    }

    #[test]
    fn a_pages_own_links_keep_their_page_order_and_are_not_spread() {
        // 300 links in one directory, then one in another, at the same depth: a page's links are read in the order they
        // sit on it, so the first five are the page's first five.
        let mut links: Vec<Link> = (0..300).map(|i| link(&format!("https://x.com/a/p{i}"))).collect();
        links.push(link("https://x.com/b/c"));
        let paths: Vec<String> =
            most_relevant("Which page is it?", links, 5).iter().map(|l| l.url.path().to_string()).collect();
        assert_eq!(paths, ["/a/p0", "/a/p1", "/a/p2", "/a/p3", "/a/p4"]);
    }

    #[test]
    fn a_link_two_questions_score_is_one_item_with_both_questions() {
        let state = |i: usize| json!({ "i": i, "text": "", "context": "", "path": "/pricing" });
        let item = |i: usize, ids: &[&str]| Item {
            state: state(i),
            questions: ids.iter().map(|id| (id.to_string(), noul("?"))).collect(),
        };
        // Question 0 scores links 0 and 1; question 1 scores link 1 (the same state) and link 2.
        let per_question =
            vec![vec![item(0, &["l0_0"]), item(1, &["l1_0"])], vec![item(1, &["l1_1"]), item(2, &["l2_1"])]];
        let (merged, weights) = merge_items(per_question);
        assert_eq!(merged.len(), 3, "one item per distinct state");
        assert_eq!(ids(&merged[1]), ["l1_0", "l1_1"]);
        assert_eq!(weights, vec![2, 2], "each question's entries");
    }

    #[test]
    fn a_single_question_keeps_its_ids_and_items() {
        // One question: the request is the one `items` makes, with no suffix on its ids.
        let (merged, weights) =
            merge_items(vec![vec![Item { state: json!({ "i": 0 }), questions: vec![("l0".to_string(), noul("?"))] }]]);
        assert_eq!(ids(&merged[0]), ["l0"]);
        assert_eq!(weights, vec![1]);
    }

    #[test]
    fn same_page_key() {
        assert_eq!(key(&Url::parse("https://x.com/pricing/#plans").unwrap()), "https://x.com/pricing");
    }

    /// The question ids of an item, in order.
    fn ids(item: &Item) -> Vec<&str> {
        item.questions.iter().map(|(id, _)| id.as_str()).collect()
    }

    #[test]
    fn a_links_field_question_rides_on_its_item() {
        let args = Args::parse_from(["jurl", "-q", "Which element is in vulcanized rubber?", "x.com"]);
        let url = Url::parse("https://x.com/").unwrap();
        let ex = extract::html("", &url);
        let client = Client::new();
        let ctx = Ctx::new(&args, &client, "", &url, &ex);
        let link = |i: usize, path: &str| Link {
            i,
            url: Url::parse(&format!("https://x.com{path}")).unwrap(),
            text: String::new(),
            context: String::new(),
            marginal: false,
        };
        let links = [link(0, "/rubber"), link(1, "/polyester")];
        let leads = "Following the link in `links`";

        let plain = items_tagged(&ctx, &links, leads, None, "");
        assert_eq!(ids(&plain[0]), ["l0"]);
        assert_eq!(plain[0].state, json!({ "i": 0, "text": "", "context": "", "path": "/rubber" }));
        assert_eq!(
            plain[0].questions[0].1,
            json!({
                "type": "noul",
                "instructions": "Following the link in `links` with i=0 is the page that answers this question, or leads \
                                 to it: Which element is in vulcanized rubber?",
            })
        );

        // With the field, the link's field question is on its item beside its leads question: the two share a request,
        // and the link's state is sent once.
        let field = items_tagged(&ctx, &links, leads, Some(&FieldScores::new()), "");
        assert_eq!(field.len(), 2);
        assert_eq!(ids(&field[1]), ["l1", "f1"]);
        assert_eq!(field[1].state, plain[1].state);
        assert_eq!(
            field[1].questions[1].1,
            json!({
                "type": "noul",
                "instructions": "Following the link in `links` with i=1 is about the same field of knowledge as the answer \
                                 to this question (chemistry, astronomy, literature, medicine…): Which element is in \
                                 vulcanized rubber?",
            })
        );
    }

    #[test]
    fn a_field_score_is_asked_once_per_search() {
        let args = Args::parse_from(["jurl", "-q", "Which element is in vulcanized rubber?", "x.com"]);
        let url = Url::parse("https://x.com/").unwrap();
        let ex = extract::html("", &url);
        let client = Client::new();
        let ctx = Ctx::new(&args, &client, "", &url, &ex);
        let link = |i: usize, path: &str| Link {
            i,
            url: Url::parse(&format!("https://x.com{path}")).unwrap(),
            text: String::new(),
            context: String::new(),
            marginal: false,
        };
        let links = [link(0, "/rubber"), link(1, "/polyester")];
        let leads = "Following the link in `links`";

        // An earlier page asked the field of the rubber page: this page doesn't ask it again.
        let known = FieldScores::from([(key(&links[0].url), 0.9)]);
        let again = items_tagged(&ctx, &links, leads, Some(&known), "");
        assert_eq!(ids(&again[0]), ["l0"]);
        assert_eq!(ids(&again[1]), ["l1", "f1"]);

        let answers = Answers {
            answers: HashMap::from([
                ("l0".to_string(), json!({ "noul": 0.2 })),
                ("l1".to_string(), json!({ "noul": 0.1 })),
                ("f1".to_string(), json!({ "noul": 0.5 })),
            ]),
            ..Answers::default()
        };
        // The known field counts as if it were asked; the one asked here comes back, for the search to keep.
        let (scores, asked) = read_tagged(&links, &answers, Some(&known), "");
        assert!((scores[0] - (0.2 + 0.8 * FIELD_WEIGHT * 0.9)).abs() < 1e-9, "{scores:?}");
        assert!((scores[1] - (0.1 + 0.9 * FIELD_WEIGHT * 0.5)).abs() < 1e-9, "{scores:?}");
        assert_eq!(asked, FieldScores::from([(key(&links[1].url), 0.5)]));

        // Without a field, a score is the leads alone, and nothing is asked.
        let (scores, asked) = read_tagged(&links, &answers, None, "");
        assert_eq!(scores, vec![0.2, 0.1]);
        assert!(asked.is_empty());

        // A field Jev left out counts for nothing on this page and isn't kept: the next page asks it again.
        let (scores, asked) = read_tagged(&links, &Answers::default(), Some(&FieldScores::new()), "");
        assert_eq!(scores, vec![0.0, 0.0]);
        assert!(asked.is_empty());
    }

    fn link(url: &str) -> Link {
        Link {
            i: 0,
            url: Url::parse(url).expect("a URL"),
            text: String::new(),
            context: String::new(),
            marginal: false,
        }
    }

    #[test]
    fn a_page_offers_its_own_links_on_any_host_and_nothing_it_does_not_give() {
        let page = Url::parse("https://www.shopify.com/legal/api-terms").expect("a URL");
        let ex = Extracted { links: vec![link("https://shopify.dev/docs")], ..Default::default() };
        let bare = [link("https://shopify.dev/api/usage/rate-limits")];
        let urls: Vec<String> = page_links(&page, &ex, &bare, |_| true).iter().map(|l| l.url.to_string()).collect();
        assert_eq!(urls, ["https://shopify.dev/docs", "https://shopify.dev/api/usage/rate-limits"]);
        // The keep rule narrows what is offered, and the page itself is never its own candidate.
        let only_shopify = page_links(&page, &ex, &bare, |u| u.host_str() == Some("shopify.dev")).len();
        assert_eq!(only_shopify, 2);
        let own = Extracted { links: vec![link("https://www.shopify.com/legal/api-terms#top")], ..Default::default() };
        assert!(page_links(&page, &own, &[], |_| true).is_empty());
    }

    #[test]
    fn a_footnote_mark_is_never_a_candidate() {
        let page = Url::parse("https://x.test/").expect("a URL");
        let mark = Link { marginal: true, ..link("https://x.test/cite") };
        let ex = Extracted { links: vec![mark, link("https://x.test/pricing")], ..Default::default() };
        let urls: Vec<String> =
            page_links(&page, &ex, &[], |_| true).iter().map(|l| l.url.path().to_string()).collect();
        assert_eq!(urls, ["/pricing"]);
    }
}

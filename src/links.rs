//! Links scored against a question: `--links -q` prints them, and `--follow` opens the best ones, page after page.
//! The question is whether following a link leads to the answer, so a menu's "Pricing" counts as much as a link in
//! the text: it is often the way there.

use std::collections::HashSet;

use anyhow::Result;
use serde_json::{Map, json};
use url::Url;

use crate::{
    Ctx, Item,
    decide::noul,
    extract::{Extracted, Link},
};

/// Links scored per page: cheap to score (~45 tokens each).
pub const MAX_LINKS: usize = 250;
/// On a long search, how much a link's field of knowledge counts next to whether it leads to the answer.
const FIELD_WEIGHT: f64 = 0.3;

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
pub fn candidates(ctx: &Ctx<'_>, ex: &Extracted, keep: impl Fn(&Url) -> bool) -> Vec<Link> {
    let mut seen = HashSet::from([key(ctx.url)]);
    let all: Vec<Link> = ex
        .links
        .iter()
        .chain(&ex.site_links)
        .filter(|l| !l.marginal && keep(&l.url) && seen.insert(key(&l.url)))
        .enumerate()
        .map(|(i, l)| Link { i, ..l.clone() })
        .collect();
    shortlist(ctx.args.ask.as_deref().unwrap_or_default(), all)
}

/// How many of the question's words a link shares (by their first five letters: "limit" finds `/limits/`). Only used to
/// choose which links get scored when a page has more than [`MAX_LINKS`]; Jev does the scoring.
pub fn overlap(question: &str, link: &Link) -> usize {
    const STOP: &[&str] =
        &["what", "which", "where", "when", "does", "with", "that", "this", "from", "have", "the", "for", "and", "how"];
    let stem = |w: &str| w.chars().take(5).collect::<String>();
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3).map(|w| w.to_lowercase()).collect()
    };
    let link_words: HashSet<String> =
        words(&format!("{} {}", link.text, link.url.path())).iter().map(|w| stem(w)).collect();
    words(question).iter().filter(|w| !STOP.contains(&w.as_str())).filter(|w| link_words.contains(&stem(w))).count()
}

/// At most [`MAX_LINKS`]: those that share words with the question first, then in page order.
fn shortlist(question: &str, links: Vec<Link>) -> Vec<Link> {
    let mut links = links;
    if links.len() > MAX_LINKS {
        let mut ranked: Vec<(usize, Link)> = links.into_iter().map(|l| (overlap(question, &l), l)).collect();
        ranked.sort_by_key(|(o, l)| (std::cmp::Reverse(*o), l.i));
        links = ranked.into_iter().take(MAX_LINKS).map(|(_, l)| l).collect();
    }
    links.into_iter().enumerate().map(|(i, l)| Link { i, ..l }).collect()
}

/// How likely following each link leads to the answer to `-q`, in `links` order. `what` names the link in the
/// question ("Following the link in `links`"; for the site map's URLs, "The page at the URL in `links`").
///
/// Far from the answer (Tennis → … → the boiling point of mercury) no link "leads to the answer" and those scores are
/// noise (Birmingham 0.08, Philadelphia 0.07). With `field`, each link is also asked whether its page is in the
/// answer's field of knowledge: vulcanized rubber and polyester (0.9) are chemistry, the way to the elements. That
/// counts for [`FIELD_WEIGHT`] of what the first score leaves. Only long searches ask it (`--follow 10` and up).
pub async fn score(ctx: &Ctx<'_>, links: &[Link], what: &str, field: bool) -> Result<Vec<f64>> {
    if links.is_empty() {
        return Ok(Vec::new());
    }
    let q = ctx.ask_per_link();
    // A link off the page's host says where it goes (`--links` keeps them; `--follow` only leaves for a subdomain).
    let host = |l: &Link| l.url.host_str().filter(|h| Some(*h) != ctx.url.host_str()).map(str::to_string);
    let mut items: Vec<Item> = links
        .iter()
        .map(|l| {
            let mut state = json!({ "i": l.i, "text": l.text, "context": l.context, "path": l.url.path() });
            if let Some(h) = host(l) {
                state["host"] = json!(h);
            }
            Item {
                id: format!("l{}", l.i),
                state,
                question: Some(noul(format!(
                    "{what} with i={} is the page that answers this question, or leads to it: {q}",
                    l.i
                ))),
            }
        })
        .collect();
    if field {
        let mut more = Vec::new();
        for l in links {
            more.push(Item {
                id: format!("f{}", l.i),
                state: json!({ "i": l.i, "text": l.text, "path": l.url.path() }),
                question: Some(noul(format!(
                    "{what} with i={} is about the same field of knowledge as the answer to this question \
                     (chemistry, astronomy, literature, medicine…): {q}",
                    l.i
                ))),
            });
        }
        items.extend(more);
    }
    let a = ctx.judge("links", items, Map::new()).await?;
    Ok(links
        .iter()
        .map(|l| {
            let p = a.noul(&format!("l{}", l.i)).unwrap_or(0.0);
            let f = if field { a.noul(&format!("f{}", l.i)).unwrap_or(0.0) } else { 0.0 };
            p + (1.0 - p) * FIELD_WEIGHT * f
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let kept = shortlist("What is the CPU time limit for Workers?", links);
        assert_eq!(kept.len(), MAX_LINKS);
        assert_eq!(kept[0].url.path(), "/workers/platform/limits/");
        assert_eq!(kept[0].i, 0);
    }

    #[test]
    fn same_page_key() {
        assert_eq!(key(&Url::parse("https://x.com/pricing/#plans").unwrap()), "https://x.com/pricing");
    }
}

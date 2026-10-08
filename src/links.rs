//! Links scored against a question: `--links -q` prints them, and `--follow` opens the best ones, page after page.
//! The question is whether following a link leads to the answer, so a menu's "Pricing" counts as much as a link in
//! the text: it is often the way there.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde_json::{Map, json};
use url::Url;

use crate::{
    Ctx, Item,
    decide::{Answers, noul},
    extract::{Extracted, Link},
};

/// Links scored per page: cheap to score (~45 tokens each).
pub const MAX_LINKS: usize = 250;
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
/// counts for [`FIELD_WEIGHT`] of what the first score leaves. Only long searches ask it (`--follow 10` and up), and
/// `field` is `Some` for them: the field scores the search has so far. A link in it isn't asked again, its known score
/// counts, and the field scores asked here come back with the scores, for the search to keep.
pub async fn score(
    ctx: &Ctx<'_>,
    links: &[Link],
    what: &str,
    field: Option<&FieldScores>,
) -> Result<(Vec<f64>, FieldScores)> {
    if links.is_empty() {
        return Ok((Vec::new(), FieldScores::new()));
    }
    let a = ctx.judge("links", items(ctx, links, what, field), Map::new()).await?;
    Ok(read(links, &a, field))
}

/// What `score` asks Jev about, one item per link: its entry in the state and the question whether following it leads
/// to the answer. A link whose field isn't known yet also gets its field question on the same item, so the two share a
/// request and the link's state is sent once.
fn items(ctx: &Ctx<'_>, links: &[Link], what: &str, field: Option<&FieldScores>) -> Vec<Item> {
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
                format!("l{}", l.i),
                noul(format!("{what} with i={} is the page that answers this question, or leads to it: {q}", l.i)),
            )];
            if field.is_some_and(|known| !known.contains_key(&key(&l.url))) {
                questions.push((
                    format!("f{}", l.i),
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
fn read(links: &[Link], a: &Answers, field: Option<&FieldScores>) -> (Vec<f64>, FieldScores) {
    let mut asked = FieldScores::new();
    let mut scores = Vec::with_capacity(links.len());
    for l in links {
        let p = a.noul(&format!("l{}", l.i)).unwrap_or(0.0);
        let k = key(&l.url);
        let f = match field {
            None => 0.0,
            Some(known) => match known.get(&k) {
                Some(&f) => f,
                None => match a.noul(&format!("f{}", l.i)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Args, extract};
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
        let kept = shortlist("What is the CPU time limit for Workers?", links);
        assert_eq!(kept.len(), MAX_LINKS);
        assert_eq!(kept[0].url.path(), "/workers/platform/limits/");
        assert_eq!(kept[0].i, 0);
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

        let plain = items(&ctx, &links, leads, None);
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
        let field = items(&ctx, &links, leads, Some(&FieldScores::new()));
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
        let again = items(&ctx, &links, leads, Some(&known));
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
        let (scores, asked) = read(&links, &answers, Some(&known));
        assert!((scores[0] - (0.2 + 0.8 * FIELD_WEIGHT * 0.9)).abs() < 1e-9, "{scores:?}");
        assert!((scores[1] - (0.1 + 0.9 * FIELD_WEIGHT * 0.5)).abs() < 1e-9, "{scores:?}");
        assert_eq!(asked, FieldScores::from([(key(&links[1].url), 0.5)]));

        // Without a field, a score is the leads alone, and nothing is asked.
        let (scores, asked) = read(&links, &answers, None);
        assert_eq!(scores, vec![0.2, 0.1]);
        assert!(asked.is_empty());

        // A field Jev left out counts for nothing on this page and isn't kept: the next page asks it again.
        let (scores, asked) = read(&links, &Answers::default(), Some(&FieldScores::new()));
        assert_eq!(scores, vec![0.0, 0.0]);
        assert!(asked.is_empty());
    }
}

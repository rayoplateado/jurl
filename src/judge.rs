//! Asking Jev: each candidate is an item with its questions, chunked into requests that fit the budget.
//! A block page fails here.

use anyhow::Result;
use futures::future::join_all;
use reqwest::Client;
use serde_json::{Map, Value, json};

use crate::{
    cli::Args,
    decide::{self, Answers, noul},
    extract::{Block, Extracted},
};

/// One Jev request stays well under the ~64k token budget.
const MAX_STATE_CHARS: usize = 60_000;
const MAX_QUESTIONS: usize = 120;
pub(crate) const STATE_TEXT_CHARS: usize = 1_200;
/// --links and --image don't send the blocks, so Jev sees this much page text to spot a block page.
const EXCERPT_CHARS: usize = 800;
/// Past this, the page is a block page (rate limit, bot check…) standing in for the real one.
const BLOCKED_P: f64 = 0.8;

/// The page is a block page (a rate limit, bot check or access denied) standing in for the real one: a typed error, so
/// `--follow` can skip such a page without reading the message.
#[derive(Debug)]
pub(crate) struct BlockPage(pub(crate) String);

impl std::fmt::Display for BlockPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BlockPage {}

pub(crate) fn is_block_page(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<BlockPage>())
}

/// Everything a mode needs to talk to Jev about one page.
pub(crate) struct Ctx<'a> {
    pub(crate) args: &'a Args,
    pub(crate) client: &'a Client,
    pub(crate) key: &'a str,
    pub(crate) url: &'a url::Url,
    pub(crate) title: &'a str,
    pub(crate) excerpt: String,
    /// With --follow from a site's front door: the site whose owner the question is about (see [`Ctx::ask`]).
    pub(crate) owner: Option<String>,
}

/// A candidate for Jev: its entry in the state, plus the questions asked about it, each under its id (a heading has
/// none and rides along as context). The questions of an item always share a request.
pub(crate) struct Item {
    pub(crate) state: Value,
    pub(crate) questions: Vec<(String, Value)>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(args: &'a Args, client: &'a Client, key: &'a str, url: &'a url::Url, ex: &'a Extracted) -> Self {
        Ctx { args, client, key, url, title: &ex.title, excerpt: excerpt_of(&ex.blocks), owner: None }
    }

    /// The question as Jev is asked it. On a search of a site, "the company" is the site's owner: on a customer story
    /// it would otherwise read as the customer ("Founded: San Francisco" for OpenAI, on linear.app).
    pub(crate) fn ask(&self) -> String {
        let q = self.args.ask.clone().unwrap_or_default();
        match self.owner_note() {
            Some(note) => format!("{q} ({note})"),
            None => q,
        }
    }

    fn owner_note(&self) -> Option<String> {
        self.owner.as_ref().map(|site| {
            format!(
                "Asked about {site}: unless the question names someone, \"the company\", \"they\", \"we\" or \"it\" is the \
                 organisation behind {site}, not a customer or partner it writes about."
            )
        })
    }

    /// The question for each of a page's links: the owner's note is in the state once (`asked_about`, see
    /// [`Ctx::judge`]), not repeated for 250 links.
    pub(crate) fn ask_per_link(&self) -> String {
        let q = self.args.ask.clone().unwrap_or_default();
        if self.owner.is_some() { format!("{q} (read it as `asked_about` says)") } else { q }
    }

    /// The question asked about each candidate: the user's, or the mode's default.
    pub(crate) fn question(&self, target: &str, default: &str) -> Value {
        match &self.args.ask {
            Some(_) => noul(format!("{target} helps answer this question: {}", self.ask())),
            None => noul(format!("{target} {default}")),
        }
    }

    /// Chunk items so each request fits the budget, ask all chunks in parallel, merge.
    /// A page that is really a rate-limit or bot-check interstitial fails instead of printing nothing.
    pub(crate) async fn judge(&self, field: &str, items: Vec<Item>, mut extra: Map<String, Value>) -> Result<Answers> {
        extra.insert(
            "blocked".to_string(),
            noul(
                "This page is an access-denied, rate-limit, CAPTCHA, bot-check or 'enable JavaScript/cookies' \
                 interstitial standing in for the real page, not a page whose content merely discusses those topics.",
            ),
        );
        let requests = chunks(items, extra);
        let n = requests.len();
        let results = join_all(requests.into_iter().map(|(entries, qs)| {
            let mut state = json!({ "title": self.title, "url": self.url.as_str(), field: entries });
            if field != "blocks" {
                state["page_text"] = json!(self.excerpt);
            }
            // The note for each link's question (`ask_per_link`). Jev's side-by-side pick of leads is also asked about
            // `links` and has it in its one question too; twice there costs nothing.
            if field == "links"
                && let Some(note) = self.owner_note()
            {
                state["asked_about"] = json!(note);
            }
            decide::jev(self.client, self.key, state, qs)
        }))
        .await;
        let mut merged = Answers { requests: n, ..Answers::default() };
        for r in results {
            let a = r?;
            merged.input_tokens += a.input_tokens;
            merged.answers.extend(a.answers);
        }
        if let Some(p) = merged.noul("blocked").filter(|&p| p >= BLOCKED_P) {
            let title = if self.title.is_empty() { String::new() } else { format!(": \"{}\"", self.title) };
            return Err(BlockPage(format!(
                "{} served a block page (rate limit, bot check or access denied), not its content{title} (p={p:.2})",
                self.url
            ))
            .into());
        }
        Ok(merged)
    }
}

/// The start of the page's text, its blocks joined by newlines, cut to `EXCERPT_CHARS` characters while it is built.
fn excerpt_of(blocks: &[Block]) -> String {
    blocks
        .iter()
        .enumerate()
        .flat_map(|(k, b)| (k > 0).then_some('\n').into_iter().chain(b.text.chars()))
        .take(EXCERPT_CHARS)
        .collect()
}

/// Splits items into the requests `judge` sends. An item goes with all its questions in one request: they open a new
/// one when they would take the open request past MAX_QUESTIONS (`extra` counts; it goes in the first) or past
/// MAX_STATE_CHARS. Items without questions (headings) never open one.
fn chunks(items: Vec<Item>, extra: Map<String, Value>) -> Vec<(Vec<Value>, Map<String, Value>)> {
    let mut requests: Vec<(Vec<Value>, Map<String, Value>)> = vec![(Vec::new(), extra)];
    let mut size = 0;
    for item in items {
        let len = item.state.to_string().len();
        let n = item.questions.len();
        let full = {
            let qs = &requests.last().unwrap().1;
            !qs.is_empty() && (qs.len() + n > MAX_QUESTIONS || size + len > MAX_STATE_CHARS)
        };
        if n > 0 && full {
            requests.push((Vec::new(), Map::new()));
            size = 0;
        }
        let (state, qs) = requests.last_mut().unwrap();
        size += len;
        state.push(item.state);
        qs.extend(item.questions);
    }
    requests.retain(|(_, qs)| !qs.is_empty());
    requests
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::extract::{self, Block, Kind};

    use super::*;

    /// Blocks holding `texts`, one each, as a page cut into blocks has them.
    fn page_blocks(texts: &[String]) -> Vec<Block> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| Block { i, kind: Kind::Para, level: None, lang: None, text: t.clone(), list: None })
            .collect()
    }

    #[test]
    fn the_excerpt_is_the_first_characters_of_the_blocks_joined_by_newlines() {
        let joined = |texts: &[String]| texts.join("\n").chars().take(EXCERPT_CHARS).collect::<String>();
        let cases: Vec<Vec<String>> = vec![
            vec![],
            vec!["one block".into()],
            vec!["a".into(), String::new(), "b".into()],
            // The cut falls on a separator, then inside the next block.
            vec!["x".repeat(EXCERPT_CHARS - 1), "yy".into(), "z".into()],
            // Characters, not bytes: each 'é' is two bytes.
            vec!["é".repeat(EXCERPT_CHARS + 100)],
            vec!["p".repeat(500), "q".repeat(500)],
            vec!["k".repeat(EXCERPT_CHARS), "after".into()],
        ];
        for texts in cases {
            assert_eq!(excerpt_of(&page_blocks(&texts)), joined(&texts));
        }
    }

    #[test]
    fn the_site_owner_rides_along_with_the_question() {
        let args = Args::parse_from(["jurl", "-q", "Where is the company headquartered?", "linear.app"]);
        let client = Client::new();
        let url = url::Url::parse("https://linear.app/customers/openai").unwrap();
        let ex = extract::html("<p>Founded: San Francisco</p>", &url);
        let ctx = Ctx::new(&args, &client, "", &url, &ex);
        assert_eq!(ctx.ask(), "Where is the company headquartered?");
        let ctx = Ctx { owner: Some("linear.app".into()), ..ctx };
        assert!(ctx.ask().starts_with("Where is the company headquartered? (Asked about linear.app:"), "{}", ctx.ask());
        // Each link only points at the note, which the links' state holds once.
        assert_eq!(ctx.ask_per_link(), "Where is the company headquartered? (read it as `asked_about` says)");
    }

    /// A block as `judge` gets it: its number, and a question unless it is a heading.
    fn block(i: usize, question: bool) -> Item {
        Item {
            state: json!({ "i": i }),
            questions: question.then(|| (format!("b{i}"), noul("?"))).into_iter().collect(),
        }
    }

    /// A state that is exactly `len` bytes of JSON, for the size budget.
    fn sized(i: usize, len: usize) -> Value {
        let mut state = json!({ "i": i, "pad": "" });
        let pad = len - state.to_string().len();
        state["pad"] = json!("x".repeat(pad));
        state
    }

    /// The block numbers a request sends, in order, headings included.
    fn numbers(request: &(Vec<Value>, Map<String, Value>)) -> Vec<u64> {
        request.0.iter().map(|s| s["i"].as_u64().unwrap()).collect()
    }

    /// The question ids a request asks, sorted.
    fn asked(request: &(Vec<Value>, Map<String, Value>)) -> Vec<&str> {
        request.1.keys().map(String::as_str).collect()
    }

    /// An extra question, like the `pick` that --precise adds.
    fn pick() -> Map<String, Value> {
        Map::from_iter([("pick".to_string(), json!("p"))])
    }

    #[test]
    fn a_few_blocks_are_one_request_with_the_extras() {
        let got = chunks(vec![block(0, true), block(1, false), block(2, true)], pick());
        assert_eq!(got.len(), 1);
        assert_eq!(numbers(&got[0]), vec![0, 1, 2]);
        assert_eq!(asked(&got[0]), vec!["b0", "b2", "pick"]);
    }

    #[test]
    fn a_request_holds_max_questions_extras_included() {
        let lens = |n: usize, extra: Map<String, Value>| -> Vec<usize> {
            chunks((0..n).map(|i| block(i, true)).collect(), extra).iter().map(|r| numbers(r).len()).collect()
        };
        // The extra is one of the MAX_QUESTIONS, so with it the first request holds one block less.
        assert_eq!(lens(300, pick()), vec![119, 120, 61]);
        assert_eq!(lens(119, pick()), vec![119]);
        assert_eq!(lens(120, pick()), vec![119, 1]);
        assert_eq!(lens(300, Map::new()), vec![120, 120, 60]);
        assert_eq!(lens(121, Map::new()), vec![120, 1]);

        let got = chunks((0..300).map(|i| block(i, true)).collect(), pick());
        assert_eq!(asked(&got[0]).len(), MAX_QUESTIONS);
        assert!(got[0].1.contains_key("pick"));
        assert!(got[1..].iter().all(|r| !r.1.contains_key("pick")));
    }

    #[test]
    fn big_states_split_at_max_state_chars() {
        // Three 20k states fill MAX_STATE_CHARS exactly; the fourth opens a request.
        let items = (0..4).map(|i| Item { state: sized(i, 20_000), ..block(i, true) }).collect();
        let got: Vec<Vec<u64>> = chunks(items, Map::new()).iter().map(numbers).collect();
        assert_eq!(got, vec![vec![0, 1, 2], vec![3]]);
    }

    #[test]
    fn headings_ride_along_and_never_open_a_request() {
        // The request is full by the headings: they stay in it, and the next question opens a new one.
        let mut items: Vec<Item> = (0..119).map(|i| block(i, true)).collect();
        items.extend((119..122).map(|i| block(i, false)));
        items.push(block(122, true));
        let got = chunks(items, pick());
        assert_eq!(got.len(), 2);
        assert_eq!(numbers(&got[0]), (0..122).collect::<Vec<u64>>());
        assert_eq!(asked(&got[1]), vec!["b122"]);

        // Headings count toward MAX_STATE_CHARS all the same.
        let items = vec![
            Item { state: sized(0, 30_000), ..block(0, true) },
            Item { state: sized(1, 40_000), ..block(1, false) },
            block(2, true),
        ];
        let got: Vec<Vec<u64>> = chunks(items, Map::new()).iter().map(numbers).collect();
        assert_eq!(got, vec![vec![0, 1], vec![2]]);

        // Headings alone ask nothing, so they make no request.
        assert!(chunks(vec![block(0, false)], Map::new()).is_empty());
    }

    #[test]
    fn a_pair_of_questions_shares_its_request() {
        // An item can carry two questions, as a link does (its leads and its field): 60 pairs fill a request, and the
        // 61st opens one.
        let pair = |i: usize| Item {
            state: json!({ "i": i }),
            questions: vec![(format!("l{i}"), noul("?")), (format!("f{i}"), noul("?"))],
        };
        let lens = |n: usize, extra: Map<String, Value>| -> Vec<usize> {
            chunks((0..n).map(pair).collect(), extra).iter().map(|r| numbers(r).len()).collect()
        };
        assert_eq!(lens(60, Map::new()), vec![60]);
        assert_eq!(lens(61, Map::new()), vec![60, 1]);
        // The extra is one of the 120 questions, so with it the first request holds one pair less.
        assert_eq!(lens(60, pick()), vec![59, 1]);

        // A pair that would overflow the request moves whole: both its questions open the next one.
        let items: Vec<Item> = (0..119).map(|i| block(i, true)).chain([pair(119)]).collect();
        let got = chunks(items, Map::new());
        assert_eq!(got.iter().map(|r| numbers(r).len()).collect::<Vec<_>>(), vec![119, 1]);
        assert_eq!(asked(&got[1]), vec!["f119", "l119"]);
    }
}

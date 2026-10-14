//! What a run prints: a result as text or JSON, and the exit code a miss or a failure gets.

use std::{
    collections::HashSet,
    io::{ErrorKind, Write},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    cli::Args,
    decide,
    extract::{
        Block,
        sources::{Applied, Source},
    },
    fetch::Served,
    links,
};

/// What a mode found, both ways: the text a person reads and the JSON a script reads. The CLI prints one of them
/// (`--json`); `jurl mcp` hands an agent both.
#[derive(Debug)]
pub(crate) struct Rendered {
    pub(crate) text: String,
    pub(crate) json: Value,
}

/// The page or site was read fine and has nothing to print: no answer, no image like that, nothing above the
/// threshold. Exits 1, the way grep does when nothing matches, so a script can tell it from a failure.
#[derive(Debug)]
struct NotFound {
    message: String,
    /// With --precise, what came closest: `--json` still prints it, with `"answer": null`.
    closest: Option<Rendered>,
}

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NotFound {}

pub(crate) fn not_found(message: String) -> anyhow::Error {
    NotFound { message, closest: None }.into()
}

/// A miss that still has something to show in JSON: the closest candidate.
pub(crate) fn missed(message: String, closest: Rendered) -> anyhow::Error {
    NotFound { message, closest: Some(closest) }.into()
}

/// The NotFound in `e`'s chain, if it has one (context added on top doesn't hide it).
fn not_found_in(e: &anyhow::Error) -> Option<&NotFound> {
    e.chain().find_map(|c| c.downcast_ref::<NotFound>())
}

/// Whether a run ended in a miss rather than a failure: the error is a NotFound, or has one in its chain.
pub(crate) fn is_not_found(e: &anyhow::Error) -> bool {
    not_found_in(e).is_some()
}

/// 1 when nothing was found, 2 for every failure (fetching, the API, arguments), as grep does.
pub(crate) fn exit_code(e: &anyhow::Error) -> u8 {
    if is_not_found(e) { 1 } else { 2 }
}

/// The pages a run has loaded, for `--with-pages` (see `Args::pages_log`). Pages load side by side under `--follow`, so each
/// is numbered as its load starts, and the list keeps that order, not the order the loads finish in.
#[derive(Default)]
pub(crate) struct PagesLog {
    started: AtomicUsize,
    loaded: Mutex<Vec<Loaded>>,
}

/// A page as it was loaded: its number (see [`PagesLog::start`]), its final URL, and what `pages` says of it.
struct Loaded {
    number: usize,
    url: url::Url,
    route: &'static str,
    rendered: bool,
    blocks: Vec<Block>,
    data: PageData,
}

/// What `pages` says of a page's own data, apart from its blocks: the things its JSON-LD lists (`items`), the lists its data
/// holds (`sources`, under --with-sources) and the rows a --recipe took from it.
#[derive(Default)]
pub(crate) struct PageData {
    pub(crate) items: Vec<Block>,
    pub(crate) sources: Vec<Source>,
    pub(crate) recipe: Option<Applied>,
}

impl PagesLog {
    /// The number of a page whose load is starting now.
    pub(crate) fn start(&self) -> usize {
        self.started.fetch_add(1, Ordering::Relaxed)
    }

    /// Notes a page that loaded, under the number `start` gave its load, with the blocks the run read it as and what its own
    /// data gave.
    pub(crate) fn record(&self, number: usize, url: &url::Url, served: Served, blocks: &[Block], data: PageData) {
        let page = Loaded {
            number,
            url: url.clone(),
            route: served.route.name(),
            rendered: served.rendered,
            blocks: blocks.to_vec(),
            data,
        };
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner).push(page);
    }

    /// The rows the --recipe took from the page last loaded at `url`, each as its JSON text: none when the recipe found no
    /// list there, or when no such page was recorded. What tells a list's next page from the one before it.
    pub(crate) fn rows_of(&self, url: &url::Url) -> Vec<String> {
        let loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        let page = loaded.iter().filter(|p| p.url == *url).max_by_key(|p| p.number);
        match page.and_then(|p| p.data.recipe.as_ref()) {
            Some(Applied::Rows { rows, .. }) => rows.iter().map(|r| Value::Object(r.clone()).to_string()).collect(),
            _ => Vec::new(),
        }
    }

    /// The `pages` of the JSON: in load order, once per final URL (its first load, by `links::key`), each with its route,
    /// whether it was rendered, its blocks, `items` when its JSON-LD lists things, `sources` when its data holds lists, and
    /// `rows` (or `recipe_error`) when a recipe was given, with `rows_truncated` when the list had more than it gives.
    pub(crate) fn json(&self) -> Result<Value> {
        let loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        let mut in_order: Vec<&Loaded> = loaded.iter().collect();
        in_order.sort_by_key(|p| p.number);
        let mut seen = HashSet::new();
        let mut pages = Vec::new();
        for p in in_order.into_iter().filter(|p| seen.insert(links::key(&p.url))) {
            let mut page = json!({
                "url": p.url.as_str(),
                "route": p.route,
                "rendered": p.rendered,
                "blocks": serde_json::to_value(&p.blocks)?,
            });
            if !p.data.items.is_empty() {
                page["items"] = serde_json::to_value(&p.data.items)?;
            }
            if !p.data.sources.is_empty() {
                page["sources"] = serde_json::to_value(&p.data.sources)?;
            }
            match &p.data.recipe {
                Some(Applied::Rows { rows, truncated }) => {
                    page["rows"] = serde_json::to_value(rows)?;
                    if *truncated {
                        page["rows_truncated"] = Value::Bool(true);
                    }
                }
                Some(Applied::RecipeError(why)) => page["recipe_error"] = Value::String(why.clone()),
                None => {}
            }
            pages.push(page);
        }
        Ok(Value::Array(pages))
    }
}

/// What goes to stdout. A miss prints nothing, except in JSON with --precise: what came closest, with
/// `"answer": null`, or with nothing close, `"closest": null` too, so a script reading stdout always gets an object.
/// The JSON carries the run's `usage` either way, so a miss can be costed too. With --with-pages the JSON is an object for
/// every outcome: a miss or a failure has its `error`, and the pages the run loaded.
pub(crate) fn stdout_for(args: &Args, done: &Result<Rendered>, usage: &decide::Usage) -> Result<Option<String>> {
    let json = |v: &Value| -> Result<String> {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.insert("usage".to_string(), usage.json(&args.fallback));
            // A `--pages` read gives the URLs of its list under `pages` (see `blocks.rs`): each entry here has its URL too,
            // so the entries take the key.
            if args.with_pages {
                o.insert("pages".to_string(), args.pages_log.json()?);
            }
            // A `--pages` read that read all its pages says where its list goes on (see `paging::ListNext`), when it was judged: a run
            // that failed after reading them is read again from the start.
            if (done.is_ok() || done.as_ref().is_err_and(is_not_found))
                && let Some(next) = args.list_next.get()
            {
                o.insert("next_page".to_string(), Value::String(next.into()));
            }
        }
        Ok(format!("{}\n", serde_json::to_string_pretty(&v)?))
    };
    match done {
        Ok(r) if args.json => json(&r.json).map(Some),
        Ok(r) => Ok(Some(r.text.clone())),
        Err(_) if !args.json => Ok(None),
        Err(e) => match not_found_in(e) {
            Some(NotFound { closest: Some(r), .. }) => json(&r.json).map(Some),
            Some(n) if args.precise => json(&no_answer(args, &n.message)).map(Some),
            _ if args.with_pages => json(&no_result(args, &format!("{e:#}"))).map(Some),
            _ => Ok(None),
        },
    }
}

/// A run with no result, under --with-pages: the URL, the question when there is one, and the error (a miss's message, or a
/// failure's), so the JSON is still an object for a script to read.
fn no_result(args: &Args, error: &str) -> Value {
    let mut v = json!({
        "url": url::Url::parse(&args.url).map_or_else(|_| args.url.clone(), String::from),
        "error": error,
    });
    if let Some(ask) = &args.ask {
        v["ask"] = json!(ask);
    }
    v
}

/// Prints the result. A reader that has stopped (`| head -0`) isn't a failure: the exit code is the run's own.
pub(crate) fn write_out(out: &mut impl Write, text: &str) -> std::io::Result<()> {
    match write!(out, "{text}") {
        Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(()),
        r => r,
    }
}

/// A --precise miss with nothing close enough to show (no block worth searching for the answer).
fn no_answer(args: &Args, message: &str) -> Value {
    json!({
        "url": url::Url::parse(&args.url).map_or_else(|_| args.url.clone(), String::from),
        "ask": args.ask.as_deref().unwrap_or_default(),
        "answer": null,
        "closest": null,
        "p": null,
        "error": message,
    })
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use clap::Parser;

    use futures::future::join_all;
    use url::Url;

    use crate::{
        config::Config,
        decide::Answers,
        extract::Kind,
        fetch::{Route, test_server},
        load, prepare,
        timing::Timer,
    };

    use super::*;

    #[test]
    fn exit_codes_tell_nothing_found_from_failures() {
        assert_eq!(exit_code(&not_found("nothing in x answers that".into())), 1);
        // Still nothing found when something along the way adds context.
        assert_eq!(exit_code(&not_found("no image".into()).context("reading x")), 1);
        assert_eq!(exit_code(&missed("no answer".into(), Rendered { text: String::new(), json: json!({}) })), 1);
        assert_eq!(exit_code(&anyhow!("https://x.com returned HTTP 404 Not Found")), 2);
        assert_eq!(exit_code(&anyhow!("api.typesafe.ai → HTTP 402: no credits")), 2);
        // The MCP reply uses the same test: a miss is a plain result, a failure a tool error.
        assert!(is_not_found(&not_found("no image".into()).context("reading x")));
        assert!(!is_not_found(&anyhow!("HTTP 404")));
    }

    #[test]
    fn a_precise_miss_still_prints_json() {
        let mut args = Args::parse_from(["jurl", "--precise", "--json", "-q", "books?", "jurl.dev"]);
        prepare(&mut args).unwrap();
        let usage = decide::Usage::new();
        // Nothing worth searching on the page: no closest, still an object with a null answer.
        let miss: Result<Rendered> = Err(not_found("nothing in https://jurl.dev/ answers that".into()));
        let out = stdout_for(&args, &miss, &usage).unwrap().expect("JSON on a miss");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["answer"], Value::Null);
        assert_eq!(v["closest"], Value::Null);
        assert_eq!(v["url"], "https://jurl.dev/");
        assert_eq!(v["ask"], "books?");
        // No block to name: the quote, block, context, link and the block's kind, level and language are left out, not null.
        for key in ["quote", "block", "context", "context_truncated", "link", "kind", "level", "lang"] {
            assert!(v.get(key).is_none(), "{key} in a miss: {v}");
        }
        // A miss with a closest candidate prints that.
        let close = Rendered { text: String::new(), json: json!({ "answer": null, "closest": "x" }) };
        let out = stdout_for(&args, &Err(missed("no answer".into(), close)), &usage).unwrap().unwrap();
        assert!(out.contains("\"closest\": \"x\""), "{out}");
        // Failures (exit 2) print nothing, and neither does a text-mode miss.
        assert_eq!(stdout_for(&args, &Err(anyhow!("HTTP 404")), &usage).unwrap(), None);
        args.json = false;
        assert_eq!(stdout_for(&args, &Err(not_found("nothing".into())), &usage).unwrap(), None);
    }

    #[test]
    fn json_says_what_the_run_cost() {
        let mut args = Args::parse_from(["jurl", "--precise", "--json", "-q", "price?", "jurl.dev"]);
        prepare(&mut args).unwrap();
        let usage = decide::Usage::new();
        usage.pages.fetch_add(3, std::sync::atomic::Ordering::Relaxed);
        usage.jev(&Answers { requests: 4, input_tokens: 5321, ..Answers::default() });
        let want = json!({
            "pages": 3,
            "browser_retry": false,
            "plain_refusals": 0,
            "browser_requests": 0,
            "route": null,
            "bytes": {
                "direct": 0,
                "proxy": 0,
                "proxy_wire": { "up": 0, "down": 0 },
                "proxy_render_wire": { "up": 0, "down": 0 },
            },
            "proxied_renders": 0,
            "proxied_hosts": [],
            "jev": { "requests": 4, "input_tokens": 5321, "capped": false },
            "clef": { "requests": 0, "input_tokens": 0, "images": 0 },
        });
        let usage_of = |done: Result<Rendered>| {
            let out = stdout_for(&args, &done, &usage).unwrap().expect("JSON");
            serde_json::from_str::<Value>(&out).unwrap()["usage"].clone()
        };
        // Found, a miss with something close and a miss with nothing close all carry it.
        let found = Rendered { text: String::new(), json: json!({ "answer": "$8" }) };
        assert_eq!(usage_of(Ok(found)), want);
        let close = Rendered { text: String::new(), json: json!({ "answer": null, "closest": "x" }) };
        assert_eq!(usage_of(Err(missed("no answer".into(), close))), want);
        assert_eq!(usage_of(Err(not_found("nothing".into()))), want);
        // The text a person reads doesn't change.
        args.json = false;
        let found = Rendered { text: "$8\n".into(), json: json!({ "answer": "$8" }) };
        assert_eq!(stdout_for(&args, &Ok(found), &usage).unwrap().as_deref(), Some("$8\n"));
    }

    /// A stdout whose reader has gone: every write fails with this kind of error.
    struct Gone(ErrorKind);

    impl Write for Gone {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(self.0.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_reader_that_stopped_is_not_an_error() {
        assert!(write_out(&mut Gone(ErrorKind::BrokenPipe), "$8\n").is_ok());
        // Any other write error still is one.
        assert!(write_out(&mut Gone(ErrorKind::PermissionDenied), "$8\n").is_err());
    }

    /// A page's blocks for a test: one paragraph.
    fn a_page_of(text: &str) -> Vec<Block> {
        vec![Block::new(0, Kind::Para, text.into())]
    }

    fn served() -> Served {
        Served { route: Route::Direct, rendered: false }
    }

    #[test]
    fn without_with_pages_the_json_is_the_same_whatever_was_loaded() {
        let mut args = Args::parse_from(["jurl", "--json", "-q", "price?", "jurl.dev"]);
        prepare(&mut args).unwrap();
        let usage = decide::Usage::new();
        let done = Ok(Rendered { text: String::new(), json: json!({ "answer": "$8" }) });
        let plain = stdout_for(&args, &done, &usage).unwrap().expect("JSON");
        // Pages loaded and kept, but not asked for: the output is the same, with no `pages` key.
        args.pages_log.record(
            0,
            &Url::parse("https://jurl.dev/").unwrap(),
            served(),
            &a_page_of("$8"),
            PageData::default(),
        );
        assert_eq!(stdout_for(&args, &done, &usage).unwrap().expect("JSON"), plain);
        let expected = json!({ "answer": "$8", "usage": usage.json(&args.fallback) });
        assert_eq!(plain, format!("{}\n", serde_json::to_string_pretty(&expected).unwrap()));
    }

    #[test]
    fn a_list_that_goes_on_after_its_pages_says_where() {
        let mut args = Args::parse_from([
            "jurl",
            "--json",
            "--with-pages",
            "--pages",
            "3",
            "-q",
            "the events",
            "https://x.com/agenda",
        ]);
        prepare(&mut args).unwrap();
        let usage = decide::Usage::new();
        let done = Ok(Rendered { text: String::new(), json: json!({ "answer": "Tosca" }) });
        let json_of = |args: &Args| -> Value {
            serde_json::from_str(&stdout_for(args, &done, &usage).unwrap().expect("JSON")).unwrap()
        };
        assert!(json_of(&args).get("next_page").is_none(), "a list that ended says nothing");
        args.list_next.set(Url::parse("https://x.com/agenda?page=4").unwrap());
        assert_eq!(json_of(&args)["next_page"], "https://x.com/agenda?page=4");
        // A miss says it too: the pages after it may still list the items.
        let v: Value =
            serde_json::from_str(&stdout_for(&args, &Err(not_found("none".into())), &usage).unwrap().expect("JSON"))
                .unwrap();
        assert_eq!(v["next_page"], "https://x.com/agenda?page=4");
    }

    #[test]
    fn with_pages_a_miss_and_a_failure_still_list_the_pages_they_loaded() {
        let mut args = Args::parse_from(["jurl", "--json", "--with-pages", "-q", "price?", "https://x.com/"]);
        prepare(&mut args).unwrap();
        let usage = decide::Usage::new();
        args.pages_log.record(
            0,
            &Url::parse("https://x.com/").unwrap(),
            served(),
            &a_page_of("Nothing on prices."),
            PageData::default(),
        );
        let json_of = |done: Result<Rendered>| -> Value {
            let out = stdout_for(&args, &done, &usage).unwrap().expect("JSON");
            serde_json::from_str(&out).unwrap()
        };
        let found = json_of(Ok(Rendered { text: String::new(), json: json!({ "answer": "$8" }) }));
        assert_eq!((found["answer"].as_str(), found["pages"][0]["url"].as_str()), (Some("$8"), Some("https://x.com/")));
        // A miss (exit 1): an object with its error, the question, and the page it was read from.
        let miss = not_found("read 1 page of https://x.com/ and none answers that".into());
        assert_eq!(exit_code(&miss), 1);
        let v = json_of(Err(miss));
        assert_eq!(v["error"], "read 1 page of https://x.com/ and none answers that");
        assert_eq!((v["ask"].as_str(), v["url"].as_str()), (Some("price?"), Some("https://x.com/")));
        assert_eq!(v["pages"][0]["blocks"][0]["text"], "Nothing on prices.");
        assert!(v.get("answer").is_none(), "not a precise run");
        assert!(v["usage"].is_object(), "a miss is costed too");
        // A failure after the page was read (exit 2): the same object, with the failure as its error.
        let failure = anyhow!("api.typesafe.ai → HTTP 402: no credits");
        assert_eq!(exit_code(&failure), 2);
        let v = json_of(Err(failure));
        assert_eq!(v["error"], "api.typesafe.ai → HTTP 402: no credits");
        assert_eq!(v["pages"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn with_pages_a_precise_miss_keeps_its_shape_and_lists_the_pages() {
        let mut args = Args::parse_from(["jurl", "--precise", "--json", "--with-pages", "-q", "books?", "jurl.dev"]);
        prepare(&mut args).unwrap();
        args.pages_log.record(
            0,
            &Url::parse("https://jurl.dev/").unwrap(),
            served(),
            &a_page_of("Docs."),
            PageData::default(),
        );
        let miss: Result<Rendered> = Err(not_found("nothing in https://jurl.dev/ answers that".into()));
        let out = stdout_for(&args, &miss, &decide::Usage::new()).unwrap().expect("JSON on a miss");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!((&v["answer"], &v["closest"]), (&Value::Null, &Value::Null));
        assert_eq!(v["error"], "nothing in https://jurl.dev/ answers that");
        assert_eq!(v["pages"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn with_pages_a_page_whose_json_ld_lists_things_gives_them_as_items() {
        let mut args = Args::parse_from(["jurl", "--json", "--with-pages", "-q", "x?", "https://x.com/"]);
        prepare(&mut args).unwrap();
        let item = Block::new(0, Kind::Structured, "Event Tosca (JSON-LD): startDate 2027-03-02.".into());
        args.pages_log.record(
            0,
            &Url::parse("https://x.com/").unwrap(),
            served(),
            &a_page_of("one"),
            PageData { items: vec![item], ..PageData::default() },
        );
        args.pages_log.record(
            1,
            &Url::parse("https://x.com/2").unwrap(),
            served(),
            &a_page_of("two"),
            PageData::default(),
        );
        let read = Ok(Rendered { text: String::new(), json: json!({ "answer": null }) });
        let v: Value =
            serde_json::from_str(&stdout_for(&args, &read, &decide::Usage::new()).unwrap().unwrap()).unwrap();
        assert_eq!(v["pages"][0]["items"][0]["kind"], "structured");
        assert_eq!(v["pages"][0]["blocks"][0]["text"], "one");
        // A page with none has no `items` key, so its entry is as it was.
        assert!(v["pages"][1].get("items").is_none());
    }

    #[test]
    fn with_pages_a_pages_data_lists_are_its_sources_and_a_recipe_gives_rows_or_says_why_not() {
        let mut args =
            Args::parse_from(["jurl", "--json", "--with-pages", "--with-sources", "-q", "x?", "https://x.com/"]);
        prepare(&mut args).unwrap();
        let list = Source { source: "ld:0".into(), path: vec!["itemListElement".into()], count: 3, sample: vec![] };
        let mut row = serde_json::Map::new();
        row.insert("title".into(), "Tosca".into());
        let first = PageData {
            sources: vec![list],
            recipe: Some(Applied::Rows { rows: vec![row], truncated: false }),
            ..PageData::default()
        };
        args.pages_log.record(0, &Url::parse("https://x.com/").unwrap(), served(), &a_page_of("one"), first);
        let broken = PageData { recipe: Some(Applied::RecipeError("no list at ld:0 a".into())), ..PageData::default() };
        args.pages_log.record(1, &Url::parse("https://x.com/2").unwrap(), served(), &a_page_of("two"), broken);
        // What --pages tells a next page by: the rows as JSON, none for a page the recipe found no list on or never loaded.
        assert_eq!(args.pages_log.rows_of(&Url::parse("https://x.com/").unwrap()), [r#"{"title":"Tosca"}"#]);
        assert!(args.pages_log.rows_of(&Url::parse("https://x.com/2").unwrap()).is_empty());
        assert!(args.pages_log.rows_of(&Url::parse("https://x.com/3").unwrap()).is_empty());
        let read = Ok(Rendered { text: String::new(), json: json!({ "answer": null }) });
        let v: Value =
            serde_json::from_str(&stdout_for(&args, &read, &decide::Usage::new()).unwrap().unwrap()).unwrap();
        assert_eq!(v["pages"][0]["sources"][0]["path"], json!(["itemListElement"]));
        assert_eq!(v["pages"][0]["rows"], json!([{ "title": "Tosca" }]));
        assert_eq!(v["pages"][1]["recipe_error"], "no list at ld:0 a");
        assert!(v["pages"][1].get("sources").is_none() && v["pages"][1].get("rows").is_none());
    }

    #[test]
    fn with_sources_and_recipe_need_with_pages_and_a_recipe_is_read_when_parsed() {
        assert!(Args::try_parse_from(["jurl", "--with-sources", "x.com"]).is_err());
        let recipe = r#"{"source":"ld:0","path":["a"],"fields":{"t":["name"]}}"#;
        assert!(Args::try_parse_from(["jurl", "--recipe", recipe, "x.com"]).is_err());
        assert!(Args::try_parse_from(["jurl", "--json", "--with-pages", "--recipe", "{}", "x.com"]).is_err());
        let args = Args::try_parse_from(["jurl", "--json", "--with-pages", "--recipe", recipe, "x.com"]).unwrap();
        assert_eq!(args.recipe.map(|r| r.source), Some("ld:0".to_string()));
    }

    #[test]
    fn with_pages_the_entries_take_the_key_a_pages_read_gives_its_urls_under() {
        let mut args = Args::parse_from(["jurl", "--json", "--with-pages", "-q", "x?", "https://x.com/"]);
        prepare(&mut args).unwrap();
        args.pages_log.record(
            0,
            &Url::parse("https://x.com/").unwrap(),
            served(),
            &a_page_of("one"),
            PageData::default(),
        );
        args.pages_log.record(
            1,
            &Url::parse("https://x.com/2").unwrap(),
            served(),
            &a_page_of("two"),
            PageData::default(),
        );
        let read = Ok(Rendered {
            text: String::new(),
            json: json!({ "answer": null, "pages": ["https://x.com/", "https://x.com/2"] }),
        });
        let usage = decide::Usage::new();
        let v: Value = serde_json::from_str(&stdout_for(&args, &read, &usage).unwrap().unwrap()).unwrap();
        let urls: Vec<&str> = v["pages"].as_array().unwrap().iter().map(|p| p["url"].as_str().unwrap()).collect();
        assert_eq!(urls, ["https://x.com/", "https://x.com/2"]);
        assert_eq!(v["pages"][1]["blocks"][0]["text"], "two");
        // Without the flag, the list of URLs is as the read gave it.
        args.with_pages = false;
        let v: Value = serde_json::from_str(&stdout_for(&args, &read, &usage).unwrap().unwrap()).unwrap();
        assert_eq!(v["pages"], json!(["https://x.com/", "https://x.com/2"]));
    }

    #[test]
    fn pages_are_listed_in_the_order_their_loads_started_once_per_page() {
        let log = PagesLog::default();
        let (first, second, third) = (log.start(), log.start(), log.start());
        // The loads finish out of order, and the third is the second page again, with a trailing slash.
        log.record(second, &Url::parse("https://x.com/b").unwrap(), served(), &a_page_of("b"), PageData::default());
        log.record(first, &Url::parse("https://x.com/a").unwrap(), served(), &a_page_of("a"), PageData::default());
        log.record(
            third,
            &Url::parse("https://x.com/b/").unwrap(),
            served(),
            &a_page_of("b again"),
            PageData::default(),
        );
        let pages = log.json().unwrap();
        let listed: Vec<(&str, &str)> = pages
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["url"].as_str().unwrap(), p["blocks"][0]["text"].as_str().unwrap()))
            .collect();
        assert_eq!(listed, [("https://x.com/a", "a"), ("https://x.com/b", "b")]);
        assert_eq!(pages[0]["route"], "direct");
        assert_eq!(pages[0]["rendered"], false);
    }

    /// A loopback server that answers each connection on a thread of its own, so a slow page does not hold up the others.
    /// `reply` gives the bytes for a request's path, and may take its time.
    fn serve_each_on_its_own_thread(reply: fn(&str) -> Vec<u8>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut conn in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        match conn.read(&mut byte) {
                            Ok(1) => head.push(byte[0]),
                            _ => return,
                        }
                    }
                    let head = String::from_utf8_lossy(&head);
                    let path = head.split_whitespace().nth(1).unwrap_or_default();
                    let _ = conn.write_all(&reply(path));
                });
            }
        });
        base
    }

    /// The loads a `--follow` batch starts side by side (see `follow::read_batches`) are listed in the order they started,
    /// though the first one finishes last. A page that redirects to one already listed is not listed twice.
    #[tokio::test]
    async fn pages_loaded_side_by_side_are_listed_in_the_order_they_started() {
        let base = serve_each_on_its_own_thread(|path| {
            let html = |text: &str| {
                let body = format!("<html><body><article><p>{text}</p></article></body></html>");
                test_server::reply("200 OK", "Content-Type: text/html; charset=utf-8\r\n", body.as_bytes())
            };
            match path {
                "/slow" => {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    html("The slow page.")
                }
                "/fast" => html("The fast page."),
                "/moved" => test_server::reply("301 Moved Permanently", "Location: /fast\r\n", b""),
                _ => test_server::reply("404 Not Found", "", b""),
            }
        });
        let args = Args::parse_from(["jurl", "--json", "--with-pages", "-q", "fast?", base.as_str()]);
        let config = Config::default();
        let urls: Vec<Url> =
            ["/slow", "/fast", "/moved"].iter().map(|p| Url::parse(&format!("{base}{p}")).unwrap()).collect();
        let loads = join_all(urls.iter().map(|u| {
            let (args, config) = (&args, &config);
            async move {
                let mut t = Timer::new();
                load(args, config, u, &mut t).await
            }
        }))
        .await;
        assert!(loads.iter().all(Result::is_ok), "every load reads its page");
        let pages = args.pages_log.json().unwrap();
        let listed: Vec<&str> = pages.as_array().unwrap().iter().map(|p| p["url"].as_str().unwrap()).collect();
        assert_eq!(listed, [format!("{base}/slow"), format!("{base}/fast")]);
        assert_eq!(pages[0]["blocks"][0]["text"], "The slow page.");
        assert_eq!((pages[0]["route"].as_str(), pages[0]["rendered"].as_bool()), (Some("direct"), Some(false)));
    }
}

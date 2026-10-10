//! What a run prints: a result as text or JSON, and the exit code a miss or a failure gets.

use std::io::{ErrorKind, Write};

use anyhow::Result;
use serde_json::{Value, json};

use crate::{cli::Args, decide};

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

/// What goes to stdout. A miss prints nothing, except in JSON with --precise: what came closest, with
/// `"answer": null`, or with nothing close, `"closest": null` too, so a script reading stdout always gets an object.
/// The JSON carries the run's `usage` either way, so a miss can be costed too.
pub(crate) fn stdout_for(args: &Args, done: &Result<Rendered>, usage: &decide::Usage) -> Result<Option<String>> {
    let json = |v: &Value| -> Result<String> {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.insert("usage".to_string(), usage.json());
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
            _ => Ok(None),
        },
    }
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

    use crate::{decide::Answers, prepare};

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
        // No block to name: the quote, block, link and the block's kind, level and language are left out, not null.
        for key in ["quote", "block", "link", "kind", "level", "lang"] {
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
            "jev": { "requests": 4, "input_tokens": 5321 },
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
}

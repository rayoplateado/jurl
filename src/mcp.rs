//! `jurl mcp`: jurl as a tool for AI agents, over the Model Context Protocol on stdio. The client starts `jurl mcp`
//! and sends one JSON-RPC message per line on stdin; the answers go back one per line on stdout, so nothing else may
//! write there (jurl's own notes go to stderr, as always).
//!
//! This is the small part of MCP a local tool server needs: `initialize`, `ping`, `tools/list`, `tools/call` and
//! `notifications/cancelled`.
//! Each tool is a jurl command line, parsed and run by the same code as the CLI, so a tool answers exactly what
//! `jurl` would print. A miss ("the page doesn't say") is a normal result, as exit code 1 is; a failure (the page
//! couldn't be read, a bad key, no credits) is a tool error, as exit code 2 is.

use std::{collections::VecDeque, future::Future, sync::LazyLock};

use anyhow::Result;
use clap::Parser;
use futures::{StreamExt, stream::FuturesUnordered};
use reqwest::Client;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::{
    cli::Args,
    config::Config,
    output::{Rendered, is_not_found},
    timing::Timer,
};

/// The protocol versions that start with `initialize`, newest first. 2026-07-28 replaced the handshake with
/// `server/discover`; clients on it fall back to `initialize` when that method isn't found, as it isn't here.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// `answer` with `follow: true` reads this many pages, like `--follow`.
const FOLLOW_PAGES: u64 = 5;
/// A cap on `follow`, so one call can't read a whole site: a long search (15 pages) costs about $0.035.
const FOLLOW_MAX: u64 = 20;
const MAX_RESULTS: u64 = 50;
/// Tool calls running at once. The rest wait their turn, in arrival order, so one agent's burst can't fetch every page
/// at once.
const MAX_CALLS: usize = 4;
/// Tool calls that may wait for one of the `MAX_CALLS` slots. Past this a call is refused at once (see `drive`), so a
/// client that sends faster than calls finish can't grow the queue, or its own wait, without bound.
const MAX_PENDING: usize = 64;
/// JSON-RPC's error code for a message that is not a valid request.
const INVALID_REQUEST: i64 = -32600;

const VERBATIM: &str = "Everything returned is copied from the page, verbatim, with its links: jurl never writes, \
                        summarizes or guesses. When the page doesn't have it, the result says so (\"Not found\").";

/// The tools, in the order `tools/list` shows them. A call names one by `name()`, and `command` matches on the enum, so a
/// tool without a command line is a compile error, and a misspelt name can't reach one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    ReadPage,
    Answer,
    FindLinks,
    FindCode,
    FindImage,
}

impl Tool {
    const ALL: [Tool; 5] = [Tool::ReadPage, Tool::Answer, Tool::FindLinks, Tool::FindCode, Tool::FindImage];

    /// The name a client calls the tool by, as `tools/list` shows it.
    fn name(self) -> &'static str {
        match self {
            Tool::ReadPage => "read_page",
            Tool::Answer => "answer",
            Tool::FindLinks => "find_links",
            Tool::FindCode => "find_code",
            Tool::FindImage => "find_image",
        }
    }

    fn from_name(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// The tools, as `tools/list` shows them. Their input schemas are also what each call's arguments are checked against.
fn tools() -> Value {
    let url = json!({
        "type": "string",
        "minLength": 1,
        "description": "The page: a URL, or a domain and path like docs.github.com/en/rest (https:// is added)."
    });
    let max = |default: &str| {
        json!({
            "type": "integer",
            "minimum": 1,
            "maximum": MAX_RESULTS,
            "description": format!("How many results at most (default: {default}).")
        })
    };
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": true });
    json!([
        {
            "name": Tool::ReadPage.name(),
            "title": "Read a page",
            "description": format!(
                "Read a web page: its title, what kind of page it is, and the blocks that carry it (paragraphs, list \
                 items, code, tables) under their headings, in reading order. Navigation, cookie banners, sign-up \
                 prompts and footers are left out. With `question`, only the blocks that help answer the question. \
                 Pages that need JavaScript are rendered. {VERBATIM}"
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url,
                    "question": { "type": "string", "minLength": 1, "description": "Keep only what helps answer this question." },
                    "max": max("12 blocks, 5 with ask"),
                },
                "required": ["url"],
                "additionalProperties": false,
            },
            "annotations": read_only,
        },
        {
            "name": Tool::Answer.name(),
            "title": "Answer a question from a page",
            "description": format!(
                "The exact answer to a question, in the page's own words: a short span copied from the page (a \
                 price, a number, a name, a date, a command), then the block it came from and a link that opens the \
                 page with the answer highlighted. It never computes or infers: if the page says \"$8 a month\", it \
                 won't give a yearly price. With `follow`, when the page doesn't answer, jurl searches the same site \
                 (subdomains included), opening the links most likely to lead to the answer, and says which pages it \
                 went through. Start from a site's front door (linear.app) to ask about the company behind it. \
                 {VERBATIM}"
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url,
                    "question": { "type": "string", "minLength": 1, "description": "The question." },
                    "follow": {
                        "anyOf": [
                            { "type": "boolean" },
                            { "type": "integer", "minimum": 1, "maximum": FOLLOW_MAX },
                        ],
                        "description": format!(
                            "Search the same site when the page doesn't answer: true reads up to {FOLLOW_PAGES} \
                             pages, a number sets how many (10 or more for answers several clicks away)."
                        ),
                    },
                },
                "required": ["url", "question"],
                "additionalProperties": false,
            },
            "annotations": read_only,
        },
        {
            "name": Tool::FindLinks.name(),
            "title": "Find links worth following",
            "description": format!(
                "The links on a page worth following, best first, one URL per line. Without `question`: content links \
                 (sources, docs, related articles), not navigation, login, sharing or legal pages. With `question`: the \
                 links most likely to lead to the answer, menus included (a nav bar's Pricing, for a price). \
                 {VERBATIM}"
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url,
                    "question": { "type": "string", "minLength": 1, "description": "Rank the links by how likely they lead to the answer to this." },
                    "max": max("20"),
                },
                "required": ["url"],
                "additionalProperties": false,
            },
            "annotations": read_only,
        },
        {
            "name": Tool::FindCode.name(),
            "title": "Find code on a page",
            "description": format!(
                "The code blocks on a page (examples, commands, snippets), each under its heading, exactly as \
                 written. With `question`, only the code that helps answer it (\"how do I install it on macOS?\"). \
                 {VERBATIM}"
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url,
                    "question": { "type": "string", "minLength": 1, "description": "Keep only the code that helps answer this." },
                    "max": max("8"),
                },
                "required": ["url"],
                "additionalProperties": false,
            },
            "annotations": read_only,
        },
        {
            "name": Tool::FindImage.name(),
            "title": "Find images on a page",
            "description": format!(
                "Image URLs from a page, best first. With `description`, the image that shows it (\"a cathedral\"): \
                 a vision model looks at the pixels of every candidate, and when none looks like that the result says \
                 so instead of returning the least bad one. Without it, the page's content images (photos, charts, \
                 diagrams, screenshots), not logos, icons or avatars, judged by file name, alt text and caption; \
                 `vision` has the pixels checked too. `description` and `vision` need a Cloudflare Workers AI token \
                 (`jurl init`). {VERBATIM}"
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": url,
                    "description": { "type": "string", "minLength": 1, "description": "What the image shows." },
                    "vision": { "type": "boolean", "description": "Without `description`: also look at the pixels (slower)." },
                    "max": max("1 with description, every content image without"),
                },
                "required": ["url"],
                "additionalProperties": false,
            },
            "annotations": read_only,
        },
    ])
}

/// `tools/list`'s answer, built once: it never changes while the server runs, and every call is checked against it.
static TOOLS: LazyLock<Value> = LazyLock::new(tools);

/// The schema `tools/list` shows for `tool`: what its arguments are checked against.
fn input_schema(tool: Tool) -> &'static Value {
    let entry = TOOLS.as_array().unwrap().iter().find(|t| t["name"] == tool.name());
    &entry.expect("every tool is listed")["inputSchema"]
}

/// The jurl command line a tool call stands for, after checking its arguments against the tool's schema.
fn command(tool: Tool, args: &Value) -> Result<Vec<String>, String> {
    let schema = input_schema(tool);
    let args = match args {
        Value::Null => &Map::new(),
        Value::Object(a) => a,
        _ => return Err("arguments must be an object".into()),
    };
    check(schema, args)?;

    let str = |k: &str| args.get(k).and_then(Value::as_str).map(String::from);
    let mut argv = vec!["jurl".to_string()];
    match tool {
        Tool::ReadPage => {}
        Tool::Answer => argv.push("--precise".into()),
        Tool::FindLinks => argv.push("--links".into()),
        Tool::FindCode => argv.push("--code".into()),
        Tool::FindImage => match str("description") {
            Some(what) => argv.push(format!("--find={what}")),
            None if args.get("vision") == Some(&Value::Bool(true)) => argv.push("--vision".into()),
            None => argv.push("--image".into()),
        },
    }
    // `--ask=…`, so a question that starts with "-" is still the question.
    if let Some(q) = str("question") {
        argv.push(format!("--ask={q}"));
    }
    if let Some(n) = args.get("max").and_then(Value::as_u64) {
        argv.extend(["-n".into(), n.to_string()]);
    }
    match args.get("follow") {
        Some(Value::Bool(true)) => argv.extend(["--follow".into(), FOLLOW_PAGES.to_string()]),
        Some(Value::Number(n)) => argv.extend(["--follow".into(), n.to_string()]),
        _ => {}
    }
    // After `--`, a url starting with "-" is still a url. Its ends are trimmed, as `fits` checked it, so the page
    // read is the one the client named.
    let url = str("url").unwrap_or_default();
    argv.extend(["--".to_string(), url.trim().to_string()]);
    Ok(argv)
}

/// The part of JSON Schema the tools use: an object's required and allowed keys, and each value's type and bounds.
fn check(schema: &Value, args: &Map<String, Value>) -> Result<(), String> {
    let props = schema["properties"].as_object().unwrap();
    // Unknown keys first: a model that wrote `query` for `question` learns the right name, not just that one is missing.
    for (key, value) in args {
        let prop = props.get(key).ok_or_else(|| {
            let known: Vec<_> = props.keys().map(|k| format!("`{k}`")).collect();
            format!("unknown argument `{key}` (this tool takes {})", known.join(", "))
        })?;
        if !fits(prop, value) {
            return Err(format!("`{key}`: {}", prop["description"].as_str().unwrap_or("wrong type")));
        }
    }
    for key in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !args.contains_key(key) {
            return Err(format!("missing `{key}`"));
        }
    }
    Ok(())
}

fn fits(schema: &Value, v: &Value) -> bool {
    if let Some(any) = schema["anyOf"].as_array() {
        return any.iter().any(|s| fits(s, v));
    }
    let within =
        |n: f64| schema["minimum"].as_f64().is_none_or(|m| n >= m) && schema["maximum"].as_f64().is_none_or(|m| n <= m);
    match schema["type"].as_str() {
        Some("string") => {
            v.as_str().is_some_and(|s| s.trim().chars().count() as u64 >= schema["minLength"].as_u64().unwrap_or(0))
        }
        Some("integer") => v.as_u64().is_some_and(|n| within(n as f64)),
        Some("boolean") => v.is_boolean(),
        _ => false,
    }
}

/// What to do with one line from the client.
enum Reply {
    /// Send this now (or nothing, for a notification).
    Now(Option<Value>),
    /// A tool call: run this jurl command line, then answer request `id`.
    Run { id: Value, argv: Vec<String> },
    /// `notifications/cancelled`: the client no longer wants request `request_id` answered (see `drive`).
    Cancel { request_id: Value },
}

fn handle(line: &str) -> Reply {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Reply::Now(Some(error(Value::Null, -32700, &format!("parse error: {e}")))),
    };
    // Batches (MCP 2025-03-26 only) aren't served: say so, rather than drop the line as junk below.
    if msg.is_array() {
        return Reply::Now(Some(error(
            Value::Null,
            INVALID_REQUEST,
            "Invalid Request: batches aren't supported, send one message per line",
        )));
    }
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // A reply to a request we never sent has a result or an error and no method: nothing to answer. Any other
        // message with an id is an invalid request, which JSON-RPC answers under that id. Without one, it is junk.
        let is_reply = msg.get("result").is_some() || msg.get("error").is_some();
        return match msg.get("id") {
            Some(id) if !is_reply => {
                Reply::Now(Some(error(id.clone(), INVALID_REQUEST, "Invalid Request: `method` must be a string")))
            }
            _ => Reply::Now(None),
        };
    };
    // Cancelling is a notification too, but `drive` has to act on it: it drops the call, or stops its reply.
    if method == "notifications/cancelled" {
        return match msg["params"]["requestId"].clone() {
            Value::Null => Reply::Now(None),
            request_id => Reply::Cancel { request_id },
        };
    }
    // Notifications (`notifications/initialized`…) have no id and get no reply.
    let Some(id) = msg.get("id").cloned() else { return Reply::Now(None) };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "jurl", "title": "jurl", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "jurl reads web pages for you and returns their own words, verbatim, with links. \
                    Use `answer` for one fact (add `follow` when the page might not have it), `read_page` for what a \
                    page says, `find_links` to know where to go next, `find_code` for commands and examples, \
                    `find_image` for pictures. A \"Not found\" result means the page doesn't say it: rather than ask \
                    the same page again, add `follow` or try another page.",
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": TOOLS.clone() }),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let Some(tool) = Tool::from_name(name) else {
                return Reply::Now(Some(error(id, -32602, &format!("unknown tool: {name}"))));
            };
            // Bad arguments are the model's to fix, so they come back as a tool error it can read.
            return match command(tool, &params["arguments"]) {
                Ok(argv) => Reply::Run { id, argv },
                Err(e) => Reply::Now(Some(response(id, tool_error(&format!("{}: {e}", tool.name()))))),
            };
        }
        _ => return Reply::Now(Some(error(id, -32601, &format!("method not found: {method}")))),
    };
    Reply::Now(Some(response(id, result)))
}

fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// Run one jurl command line and turn what it found into a tool result.
async fn run(client: &Client, argv: &[String]) -> Value {
    let found = async {
        let mut args = Args::try_parse_from(argv).map_err(|e| anyhow::anyhow!("{}", e.to_string().trim()))?;
        crate::prepare(&mut args)?;
        crate::reach::set_reach(&mut args).await?;
        // Read per call, so keys added with `jurl init` or `jurl login` while the agent runs are picked up. Never prompts:
        // stdin is the protocol.
        let cfg = Config::load();
        args.stealth = crate::stealth::Sidecar::configured(
            |key| cfg.get(key),
            args.public_only,
            args.render_sandboxed,
            args.timing,
        );
        let access = crate::setup::saved_access(&cfg)?;
        crate::page(&args, &cfg, client, &access, &mut Timer::new()).await
    }
    .await;
    match found {
        // Text only, no `structuredContent`: clients that get both (Claude Code) hand the model the JSON, with every
        // probability in it, instead of the text the CLI prints. The text is what the model needs, in fewer tokens.
        Ok(r) => json!({ "content": [{ "type": "text", "text": text(&r) }] }),
        // The page was read and doesn't say it: a plain result, so the model takes it as the answer.
        Err(e) if is_not_found(&e) => {
            json!({ "content": [{ "type": "text", "text": format!("Not found: {e:#}") }] })
        }
        Err(e) => tool_error(&format!("jurl: {e:#}")),
    }
}

/// What the CLI prints, plus the pages a `follow` search went through (the CLI says that on stderr).
fn text(r: &Rendered) -> String {
    let mut text = r.text.trim_end().to_string();
    if let Some(path) = r.json["path"].as_array().filter(|p| p.len() > 1) {
        let trail: Vec<_> = path.iter().filter_map(Value::as_str).collect();
        text.push_str(&format!("\n\nFound by following {}", trail.join(" → ")));
    }
    text
}

/// Serve until the client closes stdin, then answer the calls still running. Tool calls run side by side, up to
/// `MAX_CALLS`: an agent may ask about several pages at once.
pub(crate) async fn serve(client: Client) -> Result<()> {
    let client = &client;
    drive(BufReader::new(tokio::io::stdin()), tokio::io::stdout(), move |argv: Vec<String>| async move {
        run(client, &argv).await
    })
    .await
}

/// The loop behind `serve`, over any reader and writer, so a test can run it with a fake `call`. A cancelled request
/// is not answered: see `cancel` and `finished`.
async fn drive<R, W, F, Fut>(input: R, mut output: W, call: F) -> Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    F: Fn(Vec<String>) -> Fut,
    Fut: Future<Output = Value>,
{
    // Lines are read as bytes: one that isn't UTF-8 is answered, not allowed to end the loop with calls in flight.
    let mut segments = input.split(b'\n');
    let mut open = true;
    // A read error stops the input, not the answers: the calls already sent are answered, then the error is returned.
    let mut failed: Option<anyhow::Error> = None;
    let mut pending: VecDeque<(Value, Vec<String>)> = VecDeque::new();
    let mut calls = FuturesUnordered::new();
    // The calls running now, by request id, with whether the client has cancelled each one since it started.
    let mut running: Vec<(Value, bool)> = Vec::new();
    loop {
        // Calls past the cap wait here, in arrival order.
        while calls.len() < MAX_CALLS {
            let Some((id, argv)) = pending.pop_front() else { break };
            running.push((id.clone(), false));
            let fut = call(argv);
            calls.push(async move { response(id, fut.await) });
        }
        // Once stdin has closed, only the calls still running are left to answer.
        if !open && calls.is_empty() {
            return failed.map_or(Ok(()), Err);
        }
        let reply = tokio::select! {
            segment = segments.next_segment(), if open => match segment {
                Ok(Some(bytes)) => match std::str::from_utf8(&bytes) {
                    Ok(line) if line.trim().is_empty() => continue,
                    Ok(line) => match handle(line) {
                        Reply::Now(reply) => reply,
                        Reply::Cancel { request_id } => {
                            cancel(&mut pending, &mut running, &request_id);
                            continue;
                        }
                        Reply::Run { id, argv } if pending.len() < MAX_PENDING => {
                            pending.push_back((id, argv));
                            continue;
                        }
                        // -32000 is the first code JSON-RPC leaves to the server. The call itself is fine (so this isn't a
                        // tool error the model would read and try to fix), it's the server that has no room for it.
                        Reply::Run { id, .. } => Some(error(
                            id,
                            -32000,
                            &format!("too many tool calls waiting (at most {MAX_PENDING}); try again later"),
                        )),
                    },
                    // Not text, so not JSON: there's no id to answer, and serving goes on for the calls in flight.
                    Err(_) => Some(error(Value::Null, -32700, "parse error: not valid UTF-8")),
                },
                Ok(None) => {
                    open = false;
                    continue;
                }
                Err(e) => {
                    failed = Some(anyhow::Error::new(e).context("reading stdin"));
                    open = false;
                    continue;
                }
            },
            Some(reply) = calls.next(), if !calls.is_empty() => finished(&mut running, reply),
        };
        if let Some(reply) = reply {
            let mut line = serde_json::to_vec(&reply)?;
            line.push(b'\n');
            output.write_all(&line).await?;
            output.flush().await?;
        }
    }
}

/// Stops a request the client cancelled. A call still waiting for a slot is dropped, so it never runs. A call that is
/// running keeps running, but its reply isn't sent (see `finished`). A request that is neither, because it was
/// answered already or never came, is left alone.
fn cancel(pending: &mut VecDeque<(Value, Vec<String>)>, running: &mut [(Value, bool)], request_id: &Value) {
    if let Some(at) = pending.iter().position(|(id, _)| id == request_id) {
        pending.remove(at);
    } else if let Some((_, cancelled)) = running.iter_mut().find(|(id, _)| id == request_id) {
        *cancelled = true;
    }
}

/// A call that has finished: its reply, unless the client cancelled it while it ran. It leaves `running`.
fn finished(running: &mut Vec<(Value, bool)>, reply: Value) -> Option<Value> {
    let cancelled = match running.iter().position(|(id, _)| *id == reply["id"]) {
        Some(at) => running.swap_remove(at).1,
        None => false,
    };
    (!cancelled).then_some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn reply(msg: Value) -> Option<Value> {
        handle_raw(&msg.to_string())
    }

    fn handle_raw(line: &str) -> Option<Value> {
        match handle(line) {
            Reply::Now(r) => r,
            Reply::Run { argv, .. } => panic!("unexpected run: {argv:?}"),
            Reply::Cancel { request_id } => panic!("unexpected cancel of {request_id}"),
        }
    }

    fn call(name: &str, arguments: Value) -> Reply {
        let msg = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        handle(&msg.to_string())
    }

    #[test]
    fn initialize_agrees_on_a_version() {
        let init = |v: &str| {
            reply(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": v, "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } }))
                .unwrap()
        };
        let r = init("2025-06-18");
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "jurl");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        // A version we don't speak gets our newest; the client decides whether to go on.
        assert_eq!(init("2099-01-01")["result"]["protocolVersion"], VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_an_error() {
        assert!(reply(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
        // A cancellation with no request to cancel is a notification too: nothing to stop, nothing to say.
        assert!(reply(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": {} })).is_none());
        assert_eq!(reply(json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" })).unwrap()["result"], json!({}));
        // Newer clients try `server/discover` first and fall back to `initialize` on "method not found".
        let r = reply(json!({ "jsonrpc": "2.0", "id": "d", "method": "server/discover" })).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(r["id"], "d");
        assert!(reply(json!({ "jsonrpc": "2.0", "id": 4, "result": {} })).is_none());
        assert_eq!(handle_raw("{not json").unwrap()["error"]["code"], -32700);
    }

    #[test]
    fn a_request_without_a_string_method_is_an_invalid_request() {
        // JSON-RPC 2.0 answers an invalid request under its own id, so the client can match the error to it.
        let r = reply(json!({ "jsonrpc": "2.0", "id": 10, "method": 42 })).expect("an invalid request got no reply");
        assert_eq!(r["error"]["code"], -32600);
        assert_eq!(r["id"], 10);
        assert_eq!(reply(json!({ "jsonrpc": "2.0", "id": "q" })).unwrap()["error"]["code"], -32600);
        // A reply to a request we never sent (a result or an error, and no method) is not a request: it gets none.
        assert!(reply(json!({ "jsonrpc": "2.0", "id": 4, "result": {} })).is_none());
        assert!(reply(json!({ "jsonrpc": "2.0", "id": 4, "error": { "code": -1, "message": "x" } })).is_none());
    }

    #[test]
    fn a_batch_is_an_invalid_request_not_silence() {
        for batch in [json!([]), json!([{ "jsonrpc": "2.0", "id": 1, "method": "ping" }])] {
            let r = reply(batch).expect("a batch got no reply");
            assert_eq!(r["error"]["code"], -32600);
            assert_eq!(r["id"], Value::Null);
            assert!(r["error"]["message"].as_str().unwrap().starts_with("Invalid Request: "));
        }
    }

    #[test]
    fn tools_list_has_every_tool_with_an_object_schema() {
        let r = reply(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" })).unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["read_page", "answer", "find_links", "find_code", "find_image"]);
        for t in tools {
            let s = &t["inputSchema"];
            assert_eq!(s["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().unwrap().contains("verbatim"), "{}", t["name"]);
            let props = s["properties"].as_object().unwrap();
            for req in s["required"].as_array().unwrap() {
                assert!(props.contains_key(req.as_str().unwrap()), "{} requires an unknown {req}", t["name"]);
            }
            for (k, p) in props {
                assert!(p["type"].is_string() || p["anyOf"].is_array(), "{}.{k} has no type", t["name"]);
                assert!(p["description"].is_string(), "{}.{k} has no description", t["name"]);
            }
        }
    }

    #[test]
    fn tool_calls_become_jurl_command_lines() {
        let argv = |name: &str, a: Value| match call(name, a) {
            Reply::Run { id, argv } => {
                assert_eq!(id, 7);
                argv[1..].join(" ")
            }
            Reply::Now(r) => panic!("{r:?}"),
            Reply::Cancel { request_id } => panic!("cancelled {request_id}, not run"),
        };
        assert_eq!(argv("read_page", json!({ "url": "example.com" })), "-- example.com");
        assert_eq!(
            argv("read_page", json!({ "url": "x.com", "question": "why?", "max": 3 })),
            "--ask=why? -n 3 -- x.com"
        );
        assert_eq!(argv("answer", json!({ "url": "x.com", "question": "price?" })), "--precise --ask=price? -- x.com");
        assert_eq!(
            argv("answer", json!({ "url": "x.com", "question": "price?", "follow": true })),
            "--precise --ask=price? --follow 5 -- x.com"
        );
        assert_eq!(
            argv("answer", json!({ "url": "x.com", "question": "price?", "follow": 12 })),
            "--precise --ask=price? --follow 12 -- x.com"
        );
        assert_eq!(
            argv("answer", json!({ "url": "x.com", "question": "p?", "follow": false })),
            "--precise --ask=p? -- x.com"
        );
        assert_eq!(argv("find_links", json!({ "url": "x.com" })), "--links -- x.com");
        assert_eq!(
            argv("find_code", json!({ "url": "x.com", "question": "install" })),
            "--code --ask=install -- x.com"
        );
        assert_eq!(argv("find_image", json!({ "url": "x.com", "description": "a cat" })), "--find=a cat -- x.com");
        assert_eq!(argv("find_image", json!({ "url": "x.com", "vision": true })), "--vision -- x.com");
        assert_eq!(argv("find_image", json!({ "url": "x.com" })), "--image -- x.com");
        // The url's length is checked without its spaces, so jurl must get it without them too.
        assert_eq!(
            argv("answer", json!({ "url": "  x.com\t", "question": "price?" })),
            "--precise --ask=price? -- x.com"
        );
    }

    #[test]
    fn every_command_line_is_one_the_cli_accepts() {
        for (name, a) in [
            ("read_page", json!({ "url": "-x.com", "question": "--why", "max": 50 })),
            ("answer", json!({ "url": "x.com", "question": "price?", "follow": 20 })),
            ("find_links", json!({ "url": "x.com", "question": "a" })),
            ("find_code", json!({ "url": "x.com" })),
            ("find_image", json!({ "url": "x.com", "description": "a cat", "max": 2 })),
            ("find_image", json!({ "url": "x.com", "vision": true })),
        ] {
            let Reply::Run { argv, .. } = call(name, a) else { panic!("{name}") };
            let mut args = Args::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
            crate::prepare(&mut args).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        }
    }

    #[test]
    fn bad_arguments_are_tool_errors_and_unknown_tools_protocol_errors() {
        let error_text = |name: &str, a: Value| match call(name, a) {
            Reply::Now(Some(r)) => {
                assert_eq!(r["result"]["isError"], true, "{r}");
                r["result"]["content"][0]["text"].as_str().unwrap().to_string()
            }
            _ => panic!("{name} ran"),
        };
        assert!(error_text("answer", json!({ "url": "x.com" })).contains("missing `question`"));
        assert!(error_text("read_page", json!({})).contains("missing `url`"));
        assert!(error_text("read_page", json!({ "url": "" })).contains("`url`"));
        assert!(error_text("read_page", json!({ "url": "x.com", "max": 0 })).contains("`max`"));
        assert!(error_text("read_page", json!({ "url": "x.com", "max": "3" })).contains("`max`"));
        assert!(error_text("answer", json!({ "url": "x.com", "question": "q", "follow": 500 })).contains("`follow`"));
        assert!(error_text("find_links", json!({ "url": "x.com", "query": "q" })).contains("unknown argument `query`"));
        assert!(error_text("find_code", json!("x.com")).contains("must be an object"));

        let Reply::Now(Some(r)) = call("summarize", json!({ "url": "x.com" })) else { panic!() };
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(r["id"], 7);
    }

    #[test]
    fn a_follow_search_says_where_it_went() {
        let r = Rendered {
            text: "$10 per user/month\n\n…\n".into(),
            json: json!({ "path": ["https://linear.app/", "https://linear.app/pricing"] }),
        };
        assert_eq!(
            text(&r),
            "$10 per user/month\n\n…\n\nFound by following https://linear.app/ → https://linear.app/pricing"
        );
    }

    /// One JSON-RPC request, as a line.
    fn request(id: u64, method: &str, params: Value) -> String {
        format!("{}\n", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
    }

    /// The replies in `out`, one JSON value per line.
    fn lines_of(out: &[u8]) -> Vec<Value> {
        String::from_utf8(out.to_vec()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// A tool call that takes many polls, so it is still running when the input, which is all in memory, has ended.
    async fn slow_call(_argv: Vec<String>) -> Value {
        for _ in 0..1000 {
            tokio::task::yield_now().await;
        }
        json!({ "content": [{ "type": "text", "text": "ok" }] })
    }

    #[tokio::test]
    async fn replies_still_arrive_after_stdin_closes() {
        // `printf '<initialize>\n<tools/call>\n' | jurl mcp`: the call is still running when the input ends.
        let input = [
            request(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
            request(2, "tools/call", json!({ "name": "read_page", "arguments": { "url": "x.com" } })),
        ]
        .concat();
        let mut out = Vec::new();
        drive(input.as_bytes(), &mut out, slow_call).await.unwrap();
        let replies = lines_of(&out);
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[1]["id"], 2);
        assert_eq!(replies[1]["result"]["content"][0]["text"], "ok");
    }

    #[tokio::test]
    async fn tool_calls_run_four_at_a_time_in_arrival_order_and_a_ping_does_not_wait() {
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Mutex::new(Vec::new()));
        let call = |argv: Vec<String>| {
            started.lock().unwrap().push(argv.last().unwrap().clone());
            let (running, peak) = (running.clone(), peak.clone());
            async move {
                peak.fetch_max(running.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                for _ in 0..1000 {
                    tokio::task::yield_now().await;
                }
                running.fetch_sub(1, Ordering::SeqCst);
                json!({ "content": [{ "type": "text", "text": "ok" }] })
            }
        };
        let mut input: String = (1..=10)
            .map(|i| {
                request(i, "tools/call", json!({ "name": "read_page", "arguments": { "url": format!("x{i}.com") } }))
            })
            .collect();
        input += &request(100, "ping", json!({}));
        let mut out = Vec::new();
        drive(input.as_bytes(), &mut out, call).await.unwrap();

        let replies = lines_of(&out);
        assert_eq!(replies.len(), 11, "{replies:?}");
        // The ping is answered while the calls are still running, not after them.
        assert_eq!(replies[0]["id"], 100);
        assert_eq!(peak.load(Ordering::SeqCst), MAX_CALLS);
        let urls: Vec<String> = (1..=10).map(|i| format!("x{i}.com")).collect();
        assert_eq!(*started.lock().unwrap(), urls);
        let mut ids: Vec<u64> = replies[1..].iter().map(|r| r["id"].as_u64().unwrap()).collect();
        ids.sort();
        assert_eq!(ids, (1..=10).collect::<Vec<u64>>());
    }

    #[tokio::test]
    async fn a_line_that_is_not_utf8_is_a_parse_error_and_serving_goes_on() {
        // The tool call is still running when the bad line arrives: its reply comes last, after both pings.
        let mut input =
            request(1, "tools/call", json!({ "name": "read_page", "arguments": { "url": "x.com" } })).into_bytes();
        input.extend_from_slice(request(2, "ping", json!({})).as_bytes());
        input.extend_from_slice(b"\xff\xfe not text\n");
        input.extend_from_slice(request(3, "ping", json!({})).as_bytes());
        let mut out = Vec::new();
        drive(input.as_slice(), &mut out, slow_call).await.unwrap();
        let replies = lines_of(&out);
        assert_eq!(replies.len(), 4, "{replies:?}");
        assert_eq!(replies[0]["id"], 2);
        assert_eq!(replies[1]["error"]["code"], -32700);
        assert_eq!(replies[1]["id"], Value::Null);
        assert_eq!(replies[2]["id"], 3);
        assert_eq!(replies[3]["id"], 1);
    }

    #[test]
    fn tool_enum_matches_the_list_in_order_and_parses_back() {
        let listed: Vec<&str> = TOOLS.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        let names: Vec<&str> = Tool::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(listed, names);
        for tool in Tool::ALL {
            assert_eq!(Tool::from_name(tool.name()), Some(tool));
            assert_eq!(input_schema(tool)["type"], "object");
        }
        assert_eq!(Tool::from_name("summarize"), None);
    }

    #[tokio::test]
    async fn a_call_past_the_waiting_bound_is_refused_at_once_and_never_runs() {
        // MAX_CALLS run and MAX_PENDING wait; the next one is refused as soon as it is read, while none has finished.
        let started = Arc::new(Mutex::new(Vec::new()));
        let call = |argv: Vec<String>| {
            started.lock().unwrap().push(argv.last().unwrap().clone());
            async {
                // Far more polls than the input has lines, so no call finishes before the input has been read.
                for _ in 0..1_000 {
                    tokio::task::yield_now().await;
                }
                json!({ "content": [{ "type": "text", "text": "ok" }] })
            }
        };
        let accepted = MAX_CALLS + MAX_PENDING;
        let total = accepted + 1;
        let input: String = (1..=total)
            .map(|i| {
                request(
                    i as u64,
                    "tools/call",
                    json!({ "name": "read_page", "arguments": { "url": format!("x{i}.com") } }),
                )
            })
            .collect();
        let mut out = Vec::new();
        drive(input.as_bytes(), &mut out, call).await.unwrap();

        let replies = lines_of(&out);
        assert_eq!(replies.len(), total, "{replies:?}");
        // The refusal comes first, and it is a protocol error with the request's id, not a tool result.
        assert_eq!(replies[0]["id"], total as u64);
        assert_eq!(replies[0]["error"]["code"], -32000);
        assert!(replies[0].get("result").is_none(), "{}", replies[0]);
        let mut ids: Vec<u64> = replies[1..].iter().map(|r| r["id"].as_u64().unwrap()).collect();
        ids.sort();
        assert_eq!(ids, (1..=accepted as u64).collect::<Vec<u64>>());
        // The refused call never ran; the accepted ones ran in arrival order.
        let urls: Vec<String> = (1..=accepted).map(|i| format!("x{i}.com")).collect();
        assert_eq!(*started.lock().unwrap(), urls);
    }

    /// A stdin that delivers `bytes`, then fails, as a pipe does when its read errors.
    struct BrokenStdin(Vec<u8>);

    impl tokio::io::AsyncRead for BrokenStdin {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.0.is_empty() {
                return std::task::Poll::Ready(Err(std::io::Error::other("stdin broke")));
            }
            let n = self.0.len().min(buf.remaining());
            buf.put_slice(&self.0[..n]);
            self.0.drain(..n);
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_stdin_read_error_answers_the_call_already_sent_and_is_then_returned() {
        let line = request(1, "tools/call", json!({ "name": "read_page", "arguments": { "url": "x.com" } }));
        let mut out = Vec::new();
        let err = drive(BufReader::new(BrokenStdin(line.into_bytes())), &mut out, slow_call).await.unwrap_err();
        assert_eq!(format!("{err:#}"), "reading stdin: stdin broke");
        let replies = lines_of(&out);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[0]["result"]["content"][0]["text"], "ok");
    }

    #[tokio::test]
    async fn a_stdin_read_error_answers_the_calls_waiting_for_a_slot_too() {
        // More calls than slots, so some are still waiting when the read fails: all of them are answered.
        let calls = MAX_CALLS + 2;
        let input: String = (1..=calls as u64)
            .map(|i| request(i, "tools/call", json!({ "name": "read_page", "arguments": { "url": "x.com" } })))
            .collect();
        let mut out = Vec::new();
        assert!(drive(BufReader::new(BrokenStdin(input.into_bytes())), &mut out, slow_call).await.is_err());
        let mut ids: Vec<u64> = lines_of(&out).iter().map(|r| r["id"].as_u64().unwrap()).collect();
        ids.sort();
        assert_eq!(ids, (1..=calls as u64).collect::<Vec<u64>>());
    }

    /// `notifications/cancelled` for request `id`, as a line.
    fn cancelled(id: u64) -> String {
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": id } });
        format!("{note}\n")
    }

    #[tokio::test]
    async fn a_call_cancelled_while_it_waits_never_runs_and_gets_no_reply() {
        // MAX_CALLS run, so the next call waits for a slot, and the client cancels it before a slot frees.
        let started = Arc::new(Mutex::new(Vec::new()));
        let recorder = started.clone();
        let call = move |argv: Vec<String>| {
            recorder.lock().unwrap().push(argv.last().unwrap().clone());
            slow_call(argv)
        };
        let waiting = MAX_CALLS as u64 + 1;
        let mut input: String = (1..=waiting)
            .map(|i| {
                request(i, "tools/call", json!({ "name": "read_page", "arguments": { "url": format!("x{i}.com") } }))
            })
            .collect();
        input += &cancelled(waiting);
        let mut out = Vec::new();
        drive(input.as_bytes(), &mut out, call).await.unwrap();

        let mut ids: Vec<u64> = lines_of(&out).iter().map(|r| r["id"].as_u64().unwrap()).collect();
        ids.sort();
        assert_eq!(ids, (1..=MAX_CALLS as u64).collect::<Vec<u64>>());
        let urls: Vec<String> = (1..=MAX_CALLS).map(|i| format!("x{i}.com")).collect();
        assert_eq!(*started.lock().unwrap(), urls);
    }

    #[tokio::test]
    async fn a_call_cancelled_while_it_runs_finishes_but_its_reply_is_not_sent() {
        let started = Arc::new(Mutex::new(Vec::new()));
        let recorder = started.clone();
        let call = move |argv: Vec<String>| {
            recorder.lock().unwrap().push(argv.last().unwrap().clone());
            slow_call(argv)
        };
        let input = [
            request(1, "tools/call", json!({ "name": "read_page", "arguments": { "url": "x.com" } })),
            cancelled(1),
            request(2, "ping", json!({})),
        ]
        .concat();
        let mut out = Vec::new();
        drive(input.as_bytes(), &mut out, call).await.unwrap();

        // Only the ping is answered: the cancelled call ran to its end, but the client had stopped waiting for it.
        let replies = lines_of(&out);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0]["id"], 2);
        assert_eq!(*started.lock().unwrap(), vec!["x.com".to_string()]);
    }
}

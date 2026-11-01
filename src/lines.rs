//! `--with-line`: a second reading of the answer's page, one line at a time. The page's lines are numbered and Jev is asked,
//! in one request per window of lines, one choice ("which line contains the answer?") and one yes/no ("does any line
//! answer it?"). The line it points at comes back beside the answer, for a caller that checks the answer against it: on
//! sheet cells the two disagree where the answer is most often wrong.

use std::ops::Range;

use anyhow::Result;
use futures::future::try_join_all;
use serde_json::{Map, Value, json};

use crate::{
    decide::{Answers, choice},
    extract::{Block, Extracted},
    judge::{Ctx, Item, STATE_TEXT_CHARS},
    output::Rendered,
    timing::Timer,
};

/// The most options one Jev choice takes.
const MAX_LINES: usize = 255;
/// The most characters of lines in one request, well under Jev's token budget.
const MAX_WINDOW_CHARS: usize = 50_000;

/// Every non-empty line of the page's blocks, trimmed and cut to `STATE_TEXT_CHARS` characters.
fn lines_of(blocks: &[Block]) -> Vec<&str> {
    blocks
        .iter()
        .flat_map(|b| b.text.lines().map(str::trim).filter(|l| !l.is_empty()))
        .map(|l| cut(l, STATE_TEXT_CHARS))
        .collect()
}

fn cut(s: &str, max: usize) -> &str {
    s.char_indices().nth(max).map_or(s, |(i, _)| &s[..i])
}

/// Consecutive runs of lines, each at most `MAX_LINES` lines and `MAX_WINDOW_CHARS` characters.
fn windows(lines: &[&str]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let (mut start, mut chars) = (0, 0);
    for (i, l) in lines.iter().enumerate() {
        let len = l.len() + 6;
        if i > start && (i - start == MAX_LINES || chars + len > MAX_WINDOW_CHARS) {
            out.push(start..i);
            (start, chars) = (i, 0);
        }
        chars += len;
    }
    if start < lines.len() {
        out.push(start..lines.len());
    }
    out
}

fn line_id(k: usize) -> String {
    format!("L{k:03}")
}

/// The two questions about one window, asked together so its lines are sent once.
fn questions(q: &str, n: usize) -> Map<String, Value> {
    let criteria: Map<String, Value> = (0..n).map(|k| (line_id(k), Value::Null)).collect();
    Map::from_iter([
        (
            "where".to_string(),
            choice(&format!("Which line of the document contains the answer to: \"{q}\"?"), Value::Object(criteria)),
        ),
        (
            "exists".to_string(),
            json!({
                "type": "noul",
                "instructions": format!("Does any line of the document address or answer: \"{q}\"?"),
                "criteria": {
                    "true": "At least one line of the document states or directly implies the answer",
                    "false": "No line of the document addresses this",
                },
            }),
        ),
    ])
}

/// The line Jev points at: in the window it is surest answers, that window's most likely line. Its index, its share of
/// the choice, and the window's yes/no. None when no window has an answer.
fn best(wins: &[Range<usize>], answers: &[Answers]) -> Option<(usize, f64, f64)> {
    let (w, a) = wins
        .iter()
        .zip(answers)
        .max_by(|x, y| x.1.noul("exists").unwrap_or(0.0).total_cmp(&y.1.noul("exists").unwrap_or(0.0)))?;
    let probs = a.probabilities("where")?;
    let (k, p) = (0..w.len())
        .filter_map(|k| probs.get(&line_id(k)).map(|&p| (k, p)))
        .max_by(|x, y| x.1.total_cmp(&y.1).then(y.0.cmp(&x.0)))?;
    Some((w.start + k, p, a.noul("exists").unwrap_or(0.0)))
}

/// `--with-line`: the answer, with the line of its page Jev points at under `line` (`text`, `p`, `exists`), or null when
/// the page has no lines. A failed reading keeps the answer, with the reason under `line_error`, as --with-link does.
pub(crate) async fn with_line(ctx: &Ctx<'_>, ex: &Extracted, mut answer: Rendered, t: &mut Timer) -> Result<Rendered> {
    if !ctx.args.with_line {
        return Ok(answer);
    }
    let lines = lines_of(&ex.blocks);
    let wins = windows(&lines);
    let q = ctx.ask();
    let asks = wins.iter().map(|w| {
        let doc: Vec<String> =
            lines[w.clone()].iter().enumerate().map(|(k, l)| format!("{}| {l}", line_id(k))).collect();
        // The window's lines are the state's one entry, numbered as the choice names them.
        let item = Item { state: json!(doc.join("\n")), questions: Vec::new() };
        ctx.judge("blocks", vec![item], questions(&q, w.len()))
    });
    match try_join_all(asks).await {
        Ok(answers) => {
            t.lap(format!("{} lines for --with-line", lines.len()));
            answer.json["line"] = match best(&wins, &answers) {
                Some((i, p, exists)) => json!({ "text": lines[i], "p": p, "exists": exists }),
                None => Value::Null,
            };
        }
        Err(e) => {
            eprintln!("jurl: no line: {e:#}");
            answer.json["line"] = Value::Null;
            answer.json["line_error"] = json!(format!("{e:#}"));
        }
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::extract::Kind;

    fn block(i: usize, text: &str) -> Block {
        Block::new(i, Kind::Para, text.to_string())
    }

    #[test]
    fn a_block_gives_its_non_empty_lines_trimmed() {
        let blocks = [block(0, "Urgencias 24h\n\n  944 424 040 "), block(1, "Calle Mayor 3")];
        assert_eq!(lines_of(&blocks), ["Urgencias 24h", "944 424 040", "Calle Mayor 3"]);
    }

    #[test]
    fn windows_hold_at_most_the_choice_s_options_and_cover_every_line() {
        let lines = vec!["xxxxxxxxxx"; 600];
        assert_eq!(windows(&lines), [0..255, 255..510, 510..600]);
    }

    #[test]
    fn windows_stay_under_the_character_budget() {
        // Each line is cut to STATE_TEXT_CHARS (1,200), so 1,206 characters a line with its number: 41 fit in a window.
        let text = "y".repeat(20_000);
        let blocks: Vec<Block> = (0..100).map(|i| block(i, &text)).collect();
        assert_eq!(windows(&lines_of(&blocks)), [0..41, 41..82, 82..100]);
    }

    fn answers(exists: f64, probs: &[(&str, f64)]) -> Answers {
        let probs: HashMap<&str, f64> = probs.iter().copied().collect();
        Answers {
            answers: HashMap::from([
                ("exists".to_string(), json!({ "noul": exists })),
                ("where".to_string(), json!({ "probabilities": probs })),
            ]),
            input_tokens: 0,
            requests: 1,
        }
    }

    #[test]
    fn the_line_is_the_likeliest_of_the_window_surest_to_answer() {
        let wins = [0..2, 2..4];
        let a = [answers(0.2, &[("L000", 0.9), ("L001", 0.1)]), answers(0.8, &[("L000", 0.3), ("L001", 0.7)])];
        assert_eq!(best(&wins, &a), Some((3, 0.7, 0.8)));
        assert_eq!(best(&[], &[]), None);
    }
}

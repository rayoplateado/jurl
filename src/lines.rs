//! `--precise`, first step: which lines of the page hold the answer. The page's lines are numbered and Jev is asked, in
//! one request per window of lines, one choice ("which line contains the answer?") and one yes/no ("does any line answer
//! it?"). A block's score is its lines' share of the choice times the yes/no: the same scale as one yes/no per block (the
//! block holds the answer), at about half the tokens, and lines weighed against each other instead of each block alone.

use std::ops::Range;

use anyhow::Result;
use futures::future::try_join_all;
use serde_json::{Map, Value, json};

use crate::{
    decide::{Answers, choice},
    extract::{Block, Extracted, Kind},
    judge::{Ctx, Item, STATE_TEXT_CHARS},
    timing::Timer,
};

/// The most options one Jev choice takes.
const MAX_LINES: usize = 255;
/// The most characters of lines in one request, well under Jev's token budget.
const MAX_WINDOW_CHARS: usize = 50_000;

/// One line of a block's text, as Jev is shown it.
struct Line<'a> {
    block: usize,
    text: &'a str,
}

/// The lines Jev is asked about: every non-empty line of the blocks that may hold an answer (`keep`), each cut to
/// `STATE_TEXT_CHARS` characters.
fn lines_of<'a>(blocks: &'a [Block], keep: impl Fn(&Block) -> bool) -> Vec<Line<'a>> {
    blocks
        .iter()
        .filter(|b| keep(b))
        .flat_map(|b| b.text.lines().map(str::trim).filter(|l| !l.is_empty()).map(move |text| Line { block: b.i, text }))
        .map(|l| Line { text: cut(l.text, STATE_TEXT_CHARS), ..l })
        .collect()
}

fn cut(s: &str, max: usize) -> &str {
    s.char_indices().nth(max).map_or(s, |(i, _)| &s[..i])
}

/// Consecutive runs of lines, each at most `MAX_LINES` lines and `MAX_WINDOW_CHARS` characters (a longer line is a
/// window of its own).
fn windows(lines: &[Line]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let (mut start, mut chars) = (0, 0);
    for (i, l) in lines.iter().enumerate() {
        let len = l.text.len() + 6;
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

/// Each block's score from the windows' answers: the share of the choice its lines got, times that window's yes/no.
/// A block without lines asked about has none.
fn block_scores(blocks: usize, lines: &[Line], wins: &[Range<usize>], answers: &[Answers]) -> Vec<Option<f64>> {
    let mut scores = vec![None; blocks];
    for (w, a) in wins.iter().zip(answers) {
        let exists = a.noul("exists").unwrap_or(0.0);
        let probs = a.probabilities("where").unwrap_or_default();
        for (k, l) in lines[w.clone()].iter().enumerate() {
            let p = probs.get(&line_id(k)).copied().unwrap_or(0.0) * exists;
            if let Some(s) = scores.get_mut(l.block) {
                *s = Some(s.unwrap_or(0.0) + p);
            }
        }
    }
    scores.into_iter().map(|s| s.map(|p: f64| p.min(1.0))).collect()
}

/// Jev's score for each block as the place of the answer to `--ask`, from its lines (see the module's note).
pub(crate) async fn score(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    keep: impl Fn(&Block) -> bool,
    t: &mut Timer,
) -> Result<Vec<Option<f64>>> {
    let lines = lines_of(&ex.blocks, keep);
    let wins = windows(&lines);
    let q = ctx.ask();
    let asks = wins.iter().map(|w| {
        let doc: Vec<String> = lines[w.clone()].iter().enumerate().map(|(k, l)| format!("{}| {}", line_id(k), l.text)).collect();
        // The window's lines are the state's one entry, numbered as the choice names them.
        let item = Item { state: json!(doc.join("\n")), questions: Vec::new() };
        ctx.judge("blocks", vec![item], questions(&q, w.len()))
    });
    let answers = try_join_all(asks).await?;
    let label = format!(
        "jev({} req, {} tok)",
        answers.iter().map(|a| a.requests).sum::<usize>(),
        answers.iter().map(|a| a.input_tokens).sum::<u64>()
    );
    t.lap(label);
    Ok(block_scores(ex.blocks.len(), &lines, &wins, &answers))
}

/// Section titles hold no answer of their own, unless they are an item's linked headline.
pub(crate) fn may_answer(b: &Block, candidate: bool) -> bool {
    b.kind != Kind::Heading || candidate
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn block(i: usize, text: &str) -> Block {
        Block::new(i, Kind::Para, text.to_string())
    }

    #[test]
    fn a_block_gives_its_non_empty_lines_trimmed() {
        let blocks = [block(0, "Urgencias 24h\n\n  944 424 040 "), block(1, "Calle Mayor 3")];
        let lines = lines_of(&blocks, |_| true);
        let got: Vec<(usize, &str)> = lines.iter().map(|l| (l.block, l.text)).collect();
        assert_eq!(got, [(0, "Urgencias 24h"), (0, "944 424 040"), (1, "Calle Mayor 3")]);
    }

    #[test]
    fn windows_hold_at_most_the_choice_s_options_and_cover_every_line() {
        let text = "x".repeat(10);
        let blocks: Vec<Block> = (0..600).map(|i| block(i, &text)).collect();
        let lines = lines_of(&blocks, |_| true);
        let wins = windows(&lines);
        assert_eq!(wins, [0..255, 255..510, 510..600]);
    }

    #[test]
    fn windows_stay_under_the_character_budget() {
        // Each line is cut to STATE_TEXT_CHARS (1,200), so 1,206 characters a line with its number: 41 fit in a window.
        let text = "y".repeat(20_000);
        let blocks: Vec<Block> = (0..100).map(|i| block(i, &text)).collect();
        let wins = windows(&lines_of(&blocks, |_| true));
        assert_eq!(wins, [0..41, 41..82, 82..100]);
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
    fn a_block_scores_its_lines_share_times_whether_the_page_answers() {
        let blocks = [block(0, "a\nb"), block(1, "c"), Block::new(2, Kind::Heading, "Title".into())];
        let lines = lines_of(&blocks, |b| may_answer(b, false));
        let wins = windows(&lines);
        let a = answers(0.5, &[("L000", 0.2), ("L001", 0.6), ("L002", 0.2)]);
        let scores = block_scores(blocks.len(), &lines, &wins, &[a]);
        assert_eq!(scores.len(), 3);
        assert!((scores[0].unwrap() - 0.4).abs() < 1e-9);
        assert!((scores[1].unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(scores[2], None);
    }
}

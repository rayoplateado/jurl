//! `--precise`: Jev picks the answer as one span of the best blocks' text, and says how sure it is.

use std::{collections::HashMap, ops::Range};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::{
    decide::choice,
    extract::{Block, Extracted, Kind},
    judge::{Ctx, Item, STATE_TEXT_CHARS},
    output::{Rendered, not_found},
    precise,
    timing::Timer,
};

/// The answer's share of one choice over every span and "none". On 30 pricing pages every answer at or above
/// this was right; the wrong ones scored 0.37 or less.
pub(crate) const PRECISE_THRESHOLD: f64 = 0.4;
/// Blocks below this aren't searched for an answer at all.
pub(crate) const PRECISE_BLOCK_FLOOR: f64 = 0.1;
/// --precise looks for the answer in the best this many blocks.
pub(crate) const PRECISE_BLOCKS: usize = 3;

/// The --precise answer: a byte range of one block's text, and how sure Jev is that it's exactly the answer.
pub(crate) struct Pick {
    pub(crate) block: usize,
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) p: f64,
}

/// --precise: Jev scores spans of the best blocks as the exact answer.
pub(crate) async fn precise_pick(ctx: &Ctx<'_>, ex: &Extracted, keep: &[(usize, f64)], t: &mut Timer) -> Result<Pick> {
    let top: Vec<&Block> = keep.iter().take(PRECISE_BLOCKS).map(|&(i, _)| &ex.blocks[i]).collect();
    let spans = candidate_spans(ctx, &top)?;
    let probs = pick_span(ctx, ex, &top, &spans, t).await?;
    let scored = ranked(&spans, &probs);
    let (best, own) = scored[0];
    let p = share(best, own, &scored);
    let threshold = ctx.args.threshold_for(true);
    let inside = inside_ranges(&spans, best);
    let block = &ex.blocks[best.block];

    if p >= threshold && block.kind != Kind::Code && precise::refinable(&block.text, best) {
        let range = refine(ctx, block, &best.range, &inside, threshold, t).await?;
        return Ok(Pick { block: best.block, range, p });
    }
    let range = if p >= threshold && !inside.is_empty() {
        tighter(ctx, block, &best.range, &inside, t).await?
    } else {
        best.range.clone()
    };
    Ok(Pick { block: best.block, range, p })
}

/// The candidate spans of the top blocks. Nothing to pick from is a miss.
fn candidate_spans(ctx: &Ctx<'_>, top: &[&Block]) -> Result<Vec<precise::Span>> {
    let spans = precise::candidates(top);
    if spans.is_empty() {
        return Err(not_found(format!("nothing in {} answers that exactly", ctx.url)));
    }
    Ok(spans)
}

/// One choice over every span, plus "none": Jev weighs the spans against each other, so the one that is exactly the
/// answer beats the sentence around it, and "none" wins when the page doesn't say it.
async fn pick_span(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    top: &[&Block],
    spans: &[precise::Span],
    t: &mut Timer,
) -> Result<Vec<f64>> {
    let q = ctx.ask();
    let labels: Vec<&str> = spans.iter().map(|s| s.label.as_deref().unwrap_or_else(|| span_text(ex, s))).collect();
    let question = span_choice(
        &format!(
            "Which of these spans from the blocks is exactly the answer to this question, with nothing missing \
             and nothing extra? {q}"
        ),
        's',
        &labels,
        true,
    );
    let items = top.iter().map(|b| block_item(b)).collect();
    let a = ctx.judge("blocks", items, Map::from_iter([("pick".to_string(), question)])).await?;
    t.lap(a.label());
    let probs = a.probabilities("pick").context("jev returned no answer")?;
    Ok(option_scores(&probs, 's', spans.len()))
}

/// Most likely span wins; on a tie (to two decimals) the shorter one, which says the same with less.
fn ranked<'a>(spans: &'a [precise::Span], probs: &[f64]) -> Vec<(&'a precise::Span, f64)> {
    let mut scored: Vec<(&precise::Span, f64)> = spans.iter().zip(probs.iter().copied()).collect();
    let key = |(s, p): &(&precise::Span, f64)| ((-p * 100.0).round() as i64, s.range.len());
    scored.sort_by_key(key);
    scored
}

/// "$8", "$8 per user" and "$8 per user, per month" are one answer with more or less around it, and they split the
/// vote. How sure jurl is that the answer is there counts them together: the winner plus every span that holds it or
/// sits inside it.
fn share(best: &precise::Span, own: f64, scored: &[(&precise::Span, f64)]) -> f64 {
    let nested = |s: &precise::Span| {
        s.block == best.block
            && s.range != best.range
            && ((s.range.start <= best.range.start && s.range.end >= best.range.end)
                || (s.range.start >= best.range.start && s.range.end <= best.range.end))
    };
    let p = (own + scored.iter().filter(|(s, _)| nested(s)).map(|(_, p)| p).sum::<f64>()).min(1.0);
    (p * 100.0).round() / 100.0
}

/// The shorter candidates of the winner's block that sit inside the winner.
fn inside_ranges(spans: &[precise::Span], best: &precise::Span) -> Vec<Range<usize>> {
    spans
        .iter()
        .filter(|s| s.block == best.block && s.range != best.range)
        .filter(|s| s.range.start >= best.range.start && s.range.end <= best.range.end)
        .map(|s| s.range.clone())
        .collect()
}

/// A long winner (a line, clause or sentence) may hold the answer and more. Its pieces, the shorter
/// candidates inside it and the winner itself are scored once more with the same question; a piece wins
/// only when Jev likes it at least as much as the whole, and at the answer's own threshold.
async fn refine(
    ctx: &Ctx<'_>,
    block: &Block,
    winner: &Range<usize>,
    inside: &[Range<usize>],
    threshold: f64,
    t: &mut Timer,
) -> Result<Range<usize>> {
    let q = ctx.ask();
    let options = refine_options(winner, inside, precise::refinements(&block.text, winner));
    let labels: Vec<&str> = options.iter().map(|r| &block.text[r.clone()]).collect();
    let question = span_choice(
        &format!(
            "Which of these spans from the block is exactly the answer to this question, with nothing missing \
             and nothing extra? {q}"
        ),
        'o',
        &labels,
        true,
    );
    let b = ctx.judge("blocks", vec![block_item(block)], Map::from_iter([("refine".to_string(), question)])).await?;
    t.lap(b.label());
    let probs = b.probabilities("refine").unwrap_or_default();
    Ok(options[refine_winner(&option_scores(&probs, 'o', options.len()), threshold)].clone())
}

/// The options a refine call scores: the winner first, then the shorter candidates inside it and then its
/// pieces, each once, and at most MAX_REFINE pieces besides the winner.
fn refine_options(winner: &Range<usize>, inside: &[Range<usize>], pieces: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut options = vec![winner.clone()];
    for r in inside.iter().cloned().chain(pieces) {
        if !options.contains(&r) && options.len() <= precise::MAX_REFINE {
            options.push(r);
        }
    }
    options
}

/// The option a refine call keeps, by index, where 0 is the whole: a piece wins only when Jev likes it at
/// least as much as the whole and at least at the threshold; on a tie the earlier piece wins.
fn refine_winner(scores: &[f64], threshold: f64) -> usize {
    let whole = scores[0];
    let piece = (1..scores.len()).map(|k| (k, scores[k])).max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
    match piece {
        Some((k, pk)) if pk >= threshold && pk >= whole => k,
        _ => 0,
    }
}

/// The winner can carry more than the answer ("2009; 17 years ago"). When shorter candidates sit inside it,
/// ask once more, among just those and the winner, which one is the answer with nothing extra.
async fn tighter(
    ctx: &Ctx<'_>,
    block: &Block,
    winner: &Range<usize>,
    inside: &[Range<usize>],
    t: &mut Timer,
) -> Result<Range<usize>> {
    let q = ctx.ask();
    let options: Vec<Range<usize>> = std::iter::once(winner.clone()).chain(inside.iter().cloned()).collect();
    let labels: Vec<&str> = options.iter().map(|r| &block.text[r.clone()]).collect();
    let question = span_choice(
        &format!(
            "All of these say the answer to this question. Which one is exactly the answer, without any \
             extra words around it? {q}"
        ),
        'o',
        &labels,
        false,
    );
    let b = ctx.judge("blocks", vec![block_item(block)], Map::from_iter([("tighter".to_string(), question)])).await?;
    t.lap(b.label());
    let mut range = winner.clone();
    if let Some((k, c)) = b.choice("tighter")
        && c >= 0.5
        && let Some(i) = k.strip_prefix('o').and_then(|i| i.parse::<usize>().ok())
        && let Some(r) = options.get(i)
    {
        range = r.clone();
    }
    Ok(range)
}

/// One choice over spans (or over a block's ranges): `{prefix}{k}` for each label in order, and "none" when the
/// page may not hold the answer at all.
fn span_choice(instructions: &str, prefix: char, labels: &[&str], with_none: bool) -> Value {
    let mut criteria = Map::new();
    for (k, label) in labels.iter().enumerate() {
        criteria.insert(format!("{prefix}{k}"), json!(label));
    }
    if with_none {
        criteria.insert("none".to_string(), json!("None of these is exactly the answer"));
    }
    choice(instructions, Value::Object(criteria))
}

/// A block as Jev is shown it: its number and the start of its text.
fn block_item(b: &Block) -> Item {
    Item {
        state: json!({ "block": b.i, "text": b.text.chars().take(STATE_TEXT_CHARS).collect::<String>() }),
        questions: Vec::new(),
    }
}

/// A span's text in its block: the answer itself, without the label a table cell carries.
fn span_text<'a>(ex: &'a Extracted, s: &precise::Span) -> &'a str {
    &ex.blocks[s.block].text[s.range.clone()]
}

/// The probability Jev gave each `{prefix}{k}` of the first `n` options; one it left out counts as 0.
fn option_scores(probs: &HashMap<String, f64>, prefix: char, n: usize) -> Vec<f64> {
    (0..n).map(|k| probs.get(&format!("{prefix}{k}")).copied().unwrap_or(0.0)).collect()
}

/// The answer on its own line, then the block it's in and a link to it; JSON says how sure, and below the threshold
/// has it as `closest` instead of `answer`. `path` is how --follow got here.
pub(crate) fn render_precise(ctx: &Ctx<'_>, ex: &Extracted, pick: &Pick, path: Option<&[url::Url]>) -> Rendered {
    let q = ctx.args.ask.as_deref().unwrap_or_default();
    let threshold = ctx.args.threshold_for(true);
    let block = &ex.blocks[pick.block];
    let answer = &block.text[pick.range.clone()];
    let link = precise::link(ctx.url, &block.text, &pick.range);
    let p = pick.p;

    let mut doc = json!({
        "url": ctx.url.as_str(),
        "title": ex.title,
        "ask": q,
        "answer": (p >= threshold).then_some(answer),
        "closest": (p < threshold).then_some(answer),
        "p": p,
        "quote": block.text,
        "block": block.i,
        "link": link,
    });
    if let Some(path) = path {
        doc["path"] = json!(path.iter().map(url::Url::as_str).collect::<Vec<_>>());
    }
    Rendered { text: format!("{answer}\n\n{}\n\n<{link}>\n", block.markdown()), json: doc }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decide::Answers;

    /// A span of `block` covering `range`, with no label.
    fn span(block: usize, range: Range<usize>) -> precise::Span {
        precise::Span { block, range, label: None }
    }

    /// Jev's answer to one choice, as `judge` hands it back: the probability of each option key.
    fn answered(id: &str, probabilities: &[(&str, f64)]) -> Answers {
        let probs: Map<String, Value> = probabilities.iter().map(|&(k, p)| (k.to_string(), json!(p))).collect();
        Answers { answers: HashMap::from([(id.to_string(), json!({ "probabilities": probs }))]), ..Default::default() }
    }

    /// The probability of each span, read from Jev's answer to the pick the way `precise_pick` reads it.
    fn pick_scores(spans: &[precise::Span], probabilities: &[(&str, f64)]) -> Vec<f64> {
        let probs = answered("pick", probabilities).probabilities("pick").unwrap();
        option_scores(&probs, 's', spans.len())
    }

    #[test]
    fn the_share_is_the_winner_plus_the_spans_that_hold_it_or_sit_inside_it() {
        let spans = [
            span(0, 0..10),
            span(0, 0..20),
            span(0, 2..5),
            span(0, 12..15),
            span(1, 0..10),
            precise::Span { block: 0, range: 0..10, label: Some("ten".into()) },
        ];
        let probs =
            pick_scores(&spans, &[("s0", 0.30), ("s1", 0.20), ("s2", 0.10), ("s3", 0.25), ("s4", 0.15), ("s5", 0.05)]);
        let scored = ranked(&spans, &probs);
        let (best, own) = scored[0];
        assert_eq!(best.range, 0..10);
        // The holding span (0.20) and the inside one (0.10) count; the span beside the winner, the one in another
        // block and the one with the same range don't.
        assert_eq!(share(best, own, &scored), 0.6);
    }

    #[test]
    fn the_share_is_capped_at_one_and_rounded_to_two_decimals() {
        let spans = [span(0, 0..10), span(0, 0..20)];
        let scored = ranked(&spans, &pick_scores(&spans, &[("s0", 0.7), ("s1", 0.5)]));
        assert_eq!(share(scored[0].0, scored[0].1, &scored), 1.0);

        let spans = [span(0, 0..10), span(0, 2..5)];
        let scored = ranked(&spans, &pick_scores(&spans, &[("s0", 0.5), ("s1", 0.126)]));
        assert_eq!(share(scored[0].0, scored[0].1, &scored), 0.63);
    }

    #[test]
    fn the_likeliest_span_wins_and_a_tie_at_two_decimals_goes_to_the_shorter() {
        let spans = [span(0, 0..30), span(0, 0..5)];
        // 0.404 and 0.396 both read as 40 in whole percent: the shorter span wins.
        let scored = ranked(&spans, &[0.404, 0.396]);
        assert_eq!(scored[0].0.range, 0..5);
        // Past that, the likelier span wins whatever its length.
        let scored = ranked(&spans, &[0.52, 0.50]);
        assert_eq!(scored[0].0.range, 0..30);
        // Equal in both: the one that came first.
        let spans = [span(0, 0..5), span(0, 6..11)];
        let scored = ranked(&spans, &[0.3, 0.3]);
        assert_eq!(scored[0].0.range, 0..5);
    }

    #[test]
    fn the_inside_spans_are_the_shorter_candidates_of_the_winners_block_within_it() {
        let winner = span(0, 0..10);
        // Not the winner's own range, not one that runs past it, not one in another block: the two inside, in order.
        let spans = [winner.clone(), span(0, 2..5), span(0, 0..10), span(0, 8..12), span(1, 2..5), span(0, 4..6)];
        let want: Vec<Range<usize>> = vec![2..5, 4..6];
        assert_eq!(inside_ranges(&spans, &winner), want);
    }

    #[test]
    fn the_refine_options_are_the_winner_then_the_inside_spans_then_the_pieces_each_once() {
        let winner = 0..100;
        let inside = [10..20, 30..40];
        let pieces = vec![0..50, 10..20, 60..70, 0..100, 80..90];
        let want: Vec<Range<usize>> = vec![0..100, 10..20, 30..40, 0..50, 60..70, 80..90];
        assert_eq!(refine_options(&winner, &inside, pieces), want);
    }

    #[test]
    fn the_refine_options_cap_the_pieces_at_max_refine_with_inside_spans_first() {
        let winner = 0..1000;
        let inside: Vec<Range<usize>> = (500..560).map(|i| i..i + 1).collect();
        let pieces: Vec<Range<usize>> = (0..150).map(|i| i..i + 1).collect();
        let options = refine_options(&winner, &inside, pieces);
        // The winner, then all 60 inside spans, then the first 40 pieces: 100 pieces besides the winner.
        assert_eq!(options.len(), precise::MAX_REFINE + 1);
        assert_eq!(options[1..61], inside[..]);
        let first_pieces: Vec<Range<usize>> = (0..40).map(|i| i..i + 1).collect();
        assert_eq!(options[61..], first_pieces[..]);
    }

    #[test]
    fn a_piece_wins_only_if_at_least_as_likely_as_the_whole_and_at_the_threshold() {
        // The whole is option 0. The likeliest piece wins when it reaches the threshold and is at least the whole.
        assert_eq!(refine_winner(&[0.3, 0.5, 0.45], 0.4), 1);
        assert_eq!(refine_winner(&[0.6, 0.5], 0.4), 0);
        assert_eq!(refine_winner(&[0.9, 0.3], 0.4), 0);
        // On a tie the earlier piece wins.
        assert_eq!(refine_winner(&[0.2, 0.5, 0.5], 0.4), 1);
    }

    #[test]
    fn span_choice_names_each_span_and_maybe_none() {
        assert_eq!(
            span_choice("Which?", 's', &["$8", "$8 per user"], true),
            json!({
                "type": "choice",
                "instructions": "Which?",
                "criteria": { "s0": "$8", "s1": "$8 per user", "none": "None of these is exactly the answer" },
            }),
        );
        assert_eq!(
            span_choice("Which one?", 'o', &["a", "b"], false),
            json!({ "type": "choice", "instructions": "Which one?", "criteria": { "o0": "a", "o1": "b" } }),
        );
    }
}

//! `--precise`: Jev picks the answer as one span of the best blocks' text, and says how sure it is.

use std::{collections::HashMap, ops::Range};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::{
    decide::choice,
    extract::{Block, Extracted, Kind, collapse},
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
/// Another answer on the page is a choice when Jev gives it at least this share, in the same scores that pick the answer.
/// On the eight pages measured, a sentence that restates the answer (the single-answer MDN control) scored 0.12 and must not
/// be a choice, and the other prices of the price lists scored 0.09 at most, bar one free-tier price at 0.15: 0.15 is the first
/// bar above the restatement.
pub(crate) const CHOICE_THRESHOLD: f64 = 0.15;
/// Most answers one run lists as choices, the chosen one included.
pub(crate) const MAX_CHOICES: usize = 20;

/// The --precise answer: a byte range of one block's text, how sure Jev is that it's exactly the answer, and the answers
/// the run found on the page (see [`page_choices`]): none when there is no answer.
#[derive(Debug, Clone)]
pub(crate) struct Pick {
    pub(crate) block: usize,
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) p: f64,
    pub(crate) choices: Vec<Choice>,
}

/// One answer the run found, as `--json` lists it in `choices`: its text, what it is for, its quote and its link. `p` is how
/// sure Jev is of it, the share its spans have, which orders the others.
#[derive(Debug, Clone)]
pub(crate) struct Choice {
    pub(crate) label: String,
    pub(crate) value: String,
    pub(crate) quote: String,
    pub(crate) link: String,
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

    let range = if p >= threshold && block.kind != Kind::Code && precise::refinable(&block.text, best) {
        refine(ctx, block, &best.range, &inside, threshold, t).await?
    } else if p >= threshold && !inside.is_empty() {
        tighter(ctx, block, &best.range, &inside, t).await?
    } else {
        best.range.clone()
    };
    let choices = page_choices(ctx, ex, &scored, &range, p, threshold);
    Ok(Pick { block: best.block, range, p, choices })
}

/// The answers the run found on this page, for the question, as `--json` lists them: the chosen answer first (at `range`,
/// its share `p`), then every other answer Jev scored at least [`CHOICE_THRESHOLD`], most likely first. Nothing when the page
/// has no answer (`p` under the threshold). No request is made for them: they are the spans the pick was scored over.
fn page_choices(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    scored: &[(&precise::Span, f64)],
    range: &Range<usize>,
    p: f64,
    threshold: f64,
) -> Vec<Choice> {
    if p < threshold {
        return Vec::new();
    }
    let winner = scored[0].0;
    let mut out = vec![choice_of(ctx, ex, winner.block, range, p, winner.context.as_deref())];
    let mut others = answers_of(scored, winner);
    others.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.extend(
        others
            .into_iter()
            // A span inside a table cell that the cell itself is not among is some of the cell's words, not an answer.
            .filter(|(span, share_p)| *share_p >= CHOICE_THRESHOLD && !(span.label.is_none() && span.context.is_some()))
            .map(|(span, share_p)| choice_of(ctx, ex, span.block, &span.range, share_p, span.context.as_deref())),
    );
    finish(out)
}

/// The answers the spans make apart from `winner`'s: a span and the spans of its block that contain it or sit inside it are
/// one answer (the way [`share`] counts them), taken from the likeliest span down. Each answer is one of its spans, with the
/// share of all of them. A table's answer is its cell: when the cell is among the spans it is the one, not a part of it.
fn answers_of<'a>(scored: &[(&'a precise::Span, f64)], winner: &precise::Span) -> Vec<(&'a precise::Span, f64)> {
    let nested = |a: &precise::Span, b: &precise::Span| {
        a.block == b.block
            && ((a.range.start <= b.range.start && a.range.end >= b.range.end)
                || (a.range.start >= b.range.start && a.range.end <= b.range.end))
    };
    let mut taken: Vec<bool> = scored.iter().map(|(s, _)| nested(s, winner)).collect();
    let mut out = Vec::new();
    for k in 0..scored.len() {
        if taken[k] {
            continue;
        }
        let members: Vec<usize> =
            (k..scored.len()).filter(|&j| !taken[j] && nested(scored[j].0, scored[k].0)).collect();
        for &j in &members {
            taken[j] = true;
        }
        let total: f64 = members.iter().map(|&j| scored[j].1).sum();
        let rep = members.iter().map(|&j| scored[j].0).find(|s| s.label.is_some()).unwrap_or(scored[k].0);
        out.push((rep, (total.min(1.0) * 100.0).round() / 100.0));
    }
    out
}

/// The choice for `range` of block `block` of `ex`: the answer's text, what it is for, its quote and its link.
fn choice_of(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    block: usize,
    range: &Range<usize>,
    p: f64,
    context: Option<&str>,
) -> Choice {
    let block = &ex.blocks[block];
    Choice {
        label: label_of(ex, block, range, context),
        value: answer_text(block, range),
        quote: quoted(block, range).0,
        link: answer_link(ctx.url, block, range),
        p,
    }
}

/// The structured values of a page (its JSON-LD, in `alone`, scored one by one by `score_blocks`) as choices, once one of them
/// answers: the best is the answer, first, then every other value scored at the answer's threshold, most likely first. Their
/// scores are the ones that picked the answer, so no request is made for them.
pub(crate) fn structured_choices(
    ctx: &Ctx<'_>,
    alone: &Extracted,
    scores: &[Option<f64>],
    best: usize,
    p: f64,
) -> Vec<Choice> {
    let threshold = ctx.args.threshold_for(true);
    let whole = |i: usize| 0..alone.blocks[i].text.len();
    let mut out = vec![choice_of(ctx, alone, best, &whole(best), p, None)];
    let mut others: Vec<(usize, f64)> = scores
        .iter()
        .enumerate()
        .filter_map(|(j, s)| s.filter(|&s| j != best && s >= threshold).map(|s| (j, s)))
        .collect();
    others.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.extend(others.into_iter().map(|(j, s)| choice_of(ctx, alone, j, &whole(j), s, None)));
    finish(out)
}

/// What an answer is for, from the page's structure: a table cell's row and column (`context`); in a list item, the words
/// before the value's colon; otherwise the nearest heading above the block. Empty when nothing names it, and always for
/// structured data, whose quote names its own field.
fn label_of(ex: &Extracted, block: &Block, range: &Range<usize>, context: Option<&str>) -> String {
    if let Some(context) = context {
        return context.to_string();
    }
    if block.kind == Kind::Structured {
        return String::new();
    }
    if block.kind == Kind::Item {
        let before = &block.text[..range.start];
        if let Some(cut) = before.rfind(':') {
            let words = collapse(&before[..cut]);
            if !words.is_empty() {
                return words;
            }
        }
    }
    ex.blocks[..block.i].iter().rev().find(|b| b.kind == Kind::Heading).map(|h| collapse(&h.text)).unwrap_or_default()
}

/// The choices without repeats (the same value under the same label), the first of each kept, at most [`MAX_CHOICES`].
pub(crate) fn finish(choices: Vec<Choice>) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    for c in choices {
        if !out.iter().any(|o| o.value == c.value && o.label == c.label) {
            out.push(c);
        }
        if out.len() == MAX_CHOICES {
            break;
        }
    }
    out
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

/// The link of a precise answer: the answer's words as a text fragment on the page. A structured value's words are not on the
/// page (they are in its JSON-LD), so its link is the page itself, with no fragment.
fn answer_link(url: &url::Url, block: &Block, range: &Range<usize>) -> String {
    if block.kind == Kind::Structured {
        let mut page = url.clone();
        page.set_fragment(None);
        return page.to_string();
    }
    precise::link(url, &block.text, range)
}

/// The answer's text, as the answer line prints it. A span over a cell's paragraphs has a line break in it, and the answer is one
/// line, as the page reads; code keeps its lines.
pub(crate) fn answer_text(block: &Block, range: &Range<usize>) -> String {
    let text = &block.text[range.clone()];
    if block.kind == Kind::Code { text.to_string() } else { collapse(text) }
}

/// What a precise answer quotes from its block, as JSON and as text. A block with rows (a table, in HTML or in a markdown
/// page) is quoted by its header and the answer's row, so a cell says what it is; any other block is quoted whole. Code is
/// always whole: its pipes are its own.
fn quoted(block: &Block, range: &Range<usize>) -> (String, String) {
    let row = (block.kind != Kind::Code).then(|| precise::table_quote(&block.text, range)).flatten();
    match row {
        Some(row) => (row.clone(), row),
        None => (block.text.clone(), block.markdown()),
    }
}

/// The answer on its own line, then the block it's in and a link to it; JSON says how sure, and below the threshold
/// has it as `closest` instead of `answer`. It also says the quoted block's `kind`, and its `level` and `lang` when the
/// block has them, encoded as the block arrays encode them. A table's quote is its header and the answer's row. `path` is
/// how --follow got here.
pub(crate) fn render_precise(ctx: &Ctx<'_>, ex: &Extracted, pick: &Pick, path: Option<&[url::Url]>) -> Rendered {
    let q = ctx.args.ask.as_deref().unwrap_or_default();
    let threshold = ctx.args.threshold_for(true);
    let block = &ex.blocks[pick.block];
    let answer = answer_text(block, &pick.range);
    let link = answer_link(ctx.url, block, &pick.range);
    let source =
        if block.kind == Kind::Structured { " (JSON-LD: the page's structured data, not text it shows)" } else { "" };
    // A table's quote is its header and the answer's row, so a cell says what it is ("Clase Turista"), not the whole table.
    let (quote, shown) = quoted(block, &pick.range);
    let p = pick.p;

    let mut doc = json!({
        "url": ctx.url.as_str(),
        "title": ex.title,
        "ask": q,
        "answer": (p >= threshold).then_some(answer.as_str()),
        "closest": (p < threshold).then_some(answer.as_str()),
        "p": p,
        "quote": quote,
        "block": block.i,
        "kind": block.kind,
        "link": link,
    });
    // As in the block arrays (`Block` skips a missing level or language): left out when the block has none, not null.
    if let Some(level) = block.level {
        doc["level"] = json!(level);
    }
    if let Some(lang) = &block.lang {
        doc["lang"] = json!(lang);
    }
    // Several answers on the page, the chosen one first: one answer is not a choice, so there is no list for it.
    if pick.choices.len() >= 2 {
        doc["choices"] = json!(
            pick.choices
                .iter()
                .map(|c| json!({ "label": c.label, "value": c.value, "quote": c.quote, "link": c.link }))
                .collect::<Vec<_>>()
        );
    }
    if let Some(path) = path {
        doc["path"] = json!(path.iter().map(url::Url::as_str).collect::<Vec<_>>());
    }
    Rendered { text: format!("{answer}\n\n{shown}\n\n<{link}>{source}\n"), json: doc }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cli::Args, decide::Answers};
    use clap::Parser;

    /// A span of `block` covering `range`, with no label.
    fn span(block: usize, range: Range<usize>) -> precise::Span {
        precise::Span { block, range, label: None, context: None }
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
    fn a_multi_line_answer_prints_on_one_line_and_code_keeps_its_lines() {
        // A cell's two paragraphs are one line in the answer; a code block's lines are its own.
        let cell = "| Economy | 10 kg.\n56 x 40 cm. | Overhead |";
        let span = cell.find("10 kg").unwrap()..cell.find(" | Overhead").unwrap();
        let table = Block::new(0, Kind::Table, cell.into());
        assert_eq!(answer_text(&table, &span), "10 kg. 56 x 40 cm.");
        assert_eq!(precise_json(&table, span, 0.93)["answer"], "10 kg. 56 x 40 cm.");
        let code = Block::new(0, Kind::Code, "brew install ripgrep\nrg --version".into());
        let v = precise_json(&code, 0..code.text.len(), 0.93);
        assert_eq!(v["answer"], "brew install ripgrep\nrg --version");
    }

    #[test]
    fn a_paragraph_with_rows_is_quoted_by_its_row_and_other_text_whole() {
        let text = "| | Bag | Place |\n| Economy | 10 kg | Overhead |\n| Business | 14 kg | Under |";
        let at = text.find("14 kg").unwrap();
        let rows = Block::new(0, Kind::Para, text.into());
        assert_eq!(quoted(&rows, &(at..at + 5)).0, "| | Bag | Place |\n| Business | 14 kg | Under |");
        // Code keeps its pipes, and prose without rows is quoted as the block is.
        let code = Block::new(0, Kind::Code, text.into());
        assert_eq!(quoted(&code, &(at..at + 5)).0, text);
        let prose = Block::new(0, Kind::Para, "Business: 14 kg.".into());
        assert_eq!(quoted(&prose, &(10..14)), ("Business: 14 kg.".to_string(), "Business: 14 kg.".to_string()));
    }

    #[test]
    fn a_structured_value_links_to_the_page_with_no_fragment() {
        // Its words are in the page's JSON-LD, so a text fragment for them would point at nothing on the page.
        let url = url::Url::parse("https://mistral.ai/pricing/#plans").unwrap();
        let text = "Offer Free (JSON-LD, Mistral Vibe): price 0 USD.";
        let structured = Block::new(3, Kind::Structured, text.into());
        assert_eq!(answer_link(&url, &structured, &(0..text.len())), "https://mistral.ai/pricing/");
        let para = Block::new(3, Kind::Para, text.into());
        assert!(answer_link(&url, &para, &(0..5)).starts_with("https://mistral.ai/pricing/#:~:text="));
    }

    #[test]
    fn a_table_cells_fragment_is_the_cells_words_only() {
        // The words either side of a cell are its border (`|`), so the fragment is the cell and nothing of its neighbours.
        let url = url::Url::parse("https://www.iberia.com/es/equipajes/de-mano/").unwrap();
        let text = "|  | Bag | Place |\n| Economy | 30 cm | Under |";
        let at = text.find("30 cm").unwrap();
        let block = Block::new(0, Kind::Table, text.into());
        assert_eq!(
            answer_link(&url, &block, &(at..at + 5)),
            "https://www.iberia.com/es/equipajes/de-mano/#:~:text=30%20cm"
        );
    }

    #[test]
    fn the_share_is_the_winner_plus_the_spans_that_hold_it_or_sit_inside_it() {
        let spans = [
            span(0, 0..10),
            span(0, 0..20),
            span(0, 2..5),
            span(0, 12..15),
            span(1, 0..10),
            precise::Span { block: 0, range: 0..10, label: Some("ten".into()), context: None },
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

    /// What a precise answer prints for `block` with `range` of it as the answer: a run's own `Ctx`, no network.
    fn precise_json(block: &Block, range: Range<usize>, p: f64) -> Value {
        let args = Args::parse_from(["jurl", "--precise", "--json", "-q", "Which one?", "example.com"]);
        let client = reqwest::Client::new();
        let url = url::Url::parse("https://example.com/").unwrap();
        let ex = Extracted { title: "Pricing".into(), blocks: vec![block.clone()], ..Default::default() };
        let ctx = Ctx::new(&args, &client, "", &url, &ex);
        render_precise(&ctx, &ex, &Pick { block: 0, range, p, choices: Vec::new() }, None).json
    }

    /// The `kind`, `level` and `lang` keys of a JSON object, `None` where a key is absent. A precise answer's fields for
    /// its block must be the block arrays' fields for that block.
    fn block_fields(v: &Value) -> [Option<&Value>; 3] {
        ["kind", "level", "lang"].map(|k| v.get(k))
    }

    #[test]
    fn a_precise_heading_says_its_kind_and_level_and_no_language() {
        let heading = Block { level: Some(2), ..Block::new(0, Kind::Heading, "Pro plan".into()) };
        let v = precise_json(&heading, 0..8, 0.93);
        assert_eq!(v["answer"], "Pro plan");
        assert_eq!(v["kind"], "heading");
        assert_eq!(v["level"], 2);
        assert!(v.get("lang").is_none());
        assert_eq!(block_fields(&v), block_fields(&serde_json::to_value(&heading).unwrap()));
    }

    #[test]
    fn a_precise_code_block_says_its_kind_and_language_and_no_level() {
        let code = Block { lang: Some("sh".into()), ..Block::new(0, Kind::Code, "cargo install jurl".into()) };
        let v = precise_json(&code, 0..18, 0.93);
        assert_eq!(v["answer"], "cargo install jurl");
        assert_eq!(v["kind"], "code");
        assert_eq!(v["lang"], "sh");
        assert!(v.get("level").is_none());
        assert_eq!(block_fields(&v), block_fields(&serde_json::to_value(&code).unwrap()));
    }

    #[test]
    fn a_precise_paragraph_says_its_kind_and_neither_level_nor_language() {
        let para = Block::new(0, Kind::Para, "Pro plan: $8 a month".into());
        let v = precise_json(&para, 10..12, 0.93);
        assert_eq!(v["answer"], "$8");
        assert_eq!(v["kind"], "para");
        assert!(v.get("level").is_none() && v.get("lang").is_none());
        assert_eq!(block_fields(&v), block_fields(&serde_json::to_value(&para).unwrap()));
    }

    #[test]
    fn a_precise_closest_below_the_threshold_still_says_its_block() {
        // No answer: the span is only the closest, so `answer` is null, and the block it's in is still named.
        let heading = Block { level: Some(2), ..Block::new(0, Kind::Heading, "Pro plan".into()) };
        let v = precise_json(&heading, 0..8, 0.2);
        assert_eq!(v["answer"], Value::Null);
        assert_eq!(v["closest"], "Pro plan");
        assert_eq!(v["kind"], "heading");
        assert_eq!(v["level"], 2);
        assert!(v.get("lang").is_none());
    }

    /// A run of `-p -q` over `blocks` on `page`, with no network: what the choices of a pick are made from.
    struct Run {
        args: Args,
        client: reqwest::Client,
        url: url::Url,
        ex: Extracted,
    }

    impl Run {
        fn at(page: &str, blocks: Vec<Block>) -> Run {
            Run {
                args: Args::parse_from(["jurl", "--precise", "--json", "-q", "Which one?", "example.com"]),
                client: reqwest::Client::new(),
                url: url::Url::parse(page).unwrap(),
                ex: Extracted { title: "Pricing".into(), blocks, ..Default::default() },
            }
        }

        fn ctx(&self) -> Ctx<'_> {
            Ctx::new(&self.args, &self.client, "", &self.url, &self.ex)
        }
    }

    /// The probability of each span as a pick would have it: `give` decides it from the span's block and text.
    fn scores_of(spans: &[precise::Span], ex: &Extracted, give: impl Fn(usize, &str) -> f64) -> Vec<f64> {
        spans.iter().map(|s| give(s.block, &ex.blocks[s.block].text[s.range.clone()])).collect()
    }

    #[test]
    fn a_table_answers_are_labelled_by_their_row_and_column_and_quoted_by_their_row() {
        let text = "|  | Equipaje de mano | Ubicación |\n| Clase Turista | 1 equipaje de hasta 10 kg | Compartimentos superiores |\n\
            | Business | 2 equipajes de hasta 12 kg | Compartimentos superiores |";
        let run =
            Run::at("https://www.iberia.com/es/equipajes/de-mano/", vec![Block::new(0, Kind::Table, text.into())]);
        let spans = precise::candidates(&[&run.ex.blocks[0]]);
        let probs: Vec<f64> = spans
            .iter()
            .map(|s| match s.label.as_deref() {
                Some(l) if l.starts_with("1 equipaje") => 0.6,
                Some(l) if l.starts_with("2 equipajes") => 0.3,
                _ => 0.0,
            })
            .collect();
        let scored = ranked(&spans, &probs);
        let (best, own) = scored[0];
        let p = share(best, own, &scored);
        let choices = page_choices(&run.ctx(), &run.ex, &scored, &best.range, p, 0.4);
        let got: Vec<(&str, &str)> = choices.iter().map(|c| (c.value.as_str(), c.label.as_str())).collect();
        assert_eq!(
            got,
            [
                ("1 equipaje de hasta 10 kg", "Clase Turista · Equipaje de mano"),
                ("2 equipajes de hasta 12 kg", "Business · Equipaje de mano"),
            ]
        );
        assert_eq!(
            choices[1].quote,
            "|  | Equipaje de mano | Ubicación |\n| Business | 2 equipajes de hasta 12 kg | Compartimentos superiores |"
        );
        assert_eq!(
            choices[1].link,
            "https://www.iberia.com/es/equipajes/de-mano/#:~:text=2%20equipajes%20de%20hasta%2012%20kg"
        );
    }

    #[test]
    fn a_list_item_is_labelled_by_its_lead_words_and_a_paragraph_by_the_heading_above_it() {
        let blocks = vec![
            Block::new(0, Kind::Heading, "Plans".into()),
            Block::new(1, Kind::Item, "Pro: $8 per user/month".into()),
            Block::new(2, Kind::Item, "Business: $14 per user/month".into()),
            Block::new(3, Kind::Para, "Annual billing costs $80 a year.".into()),
        ];
        let run = Run::at("https://example.com/pricing", blocks);
        let all: Vec<&Block> = run.ex.blocks.iter().collect();
        let spans = precise::candidates(&all);
        let probs = scores_of(&spans, &run.ex, |block, text| match (block, text) {
            (1, "$8") => 0.6,
            (2, "$14") => 0.3,
            (3, "$80") => 0.2,
            _ => 0.0,
        });
        let scored = ranked(&spans, &probs);
        let (best, own) = scored[0];
        let p = share(best, own, &scored);
        let choices = page_choices(&run.ctx(), &run.ex, &scored, &best.range, p, 0.4);
        let got: Vec<(&str, &str)> = choices.iter().map(|c| (c.value.as_str(), c.label.as_str())).collect();
        assert_eq!(got, [("$8", "Pro"), ("$14", "Business"), ("$80", "Plans")]);
    }

    #[test]
    fn a_table_answer_is_its_cell_and_not_a_value_inside_it() {
        // "$0.015" is the cached price in the cell "$0.06 / $0.015 cached": the cell says what the price is, so the cell is the
        // second choice, and the value inside it is not one of its own.
        let text = "| Model | $ per 1M input tokens | $ per 1M output tokens |\n\
            | DeepSeek-V4-Flash-0731 | $0.06 / $0.015 cached | $0.18 |\n| Llama-3.1-8B | $0.02 | $0.04 |";
        let run = Run::at("https://deepinfra.com/pricing", vec![Block::new(0, Kind::Table, text.into())]);
        let spans = precise::candidates(&[&run.ex.blocks[0]]);
        let probs = scores_of(&spans, &run.ex, |_, text| match text {
            "$0.02" => 0.6,
            "$0.015" => 0.3,
            _ => 0.0,
        });
        let scored = ranked(&spans, &probs);
        let (best, own) = scored[0];
        let p = share(best, own, &scored);
        let choices = page_choices(&run.ctx(), &run.ex, &scored, &best.range, p, 0.4);
        let got: Vec<(&str, &str)> = choices.iter().map(|c| (c.value.as_str(), c.label.as_str())).collect();
        assert_eq!(
            got,
            [
                ("$0.02", "Llama-3.1-8B · $ per 1M input tokens"),
                ("$0.06 / $0.015 cached", "DeepSeek-V4-Flash-0731 · $ per 1M input tokens"),
            ]
        );
    }

    #[test]
    fn spans_that_hold_or_sit_inside_an_answer_are_one_answer_with_their_shares_added() {
        let span = |block: usize, range: Range<usize>| precise::Span { block, range, label: None, context: None };
        let spans = [
            span(0, 0..10),  // the winner
            span(0, 2..5),   // inside the winner: the same answer
            span(0, 12..20), // an answer of its own, the likeliest of its group
            span(0, 14..16), // inside it: the same answer
            span(1, 0..5),   // in another block: another answer
        ];
        let probs = [0.5, 0.1, 0.2, 0.1, 0.15];
        let scored = ranked(&spans, &probs);
        let winner = scored[0].0.clone();
        let got: Vec<(Range<usize>, usize, f64)> =
            answers_of(&scored, &winner).into_iter().map(|(s, p)| (s.range.clone(), s.block, p)).collect();
        assert_eq!(got, [(12..20, 0, 0.3), (0..5, 1, 0.15)]);
    }

    #[test]
    fn structured_values_that_answer_are_choices_with_the_bare_page_link() {
        let run = Run::at("https://mistral.ai/pricing/", Vec::new());
        let alone = Extracted {
            blocks: vec![
                Block::new(0, Kind::Structured, "Offer Free (JSON-LD, Mistral Vibe): price 0 USD.".into()),
                Block::new(1, Kind::Structured, "Offer Pro (JSON-LD, Mistral): price 8 EUR.".into()),
                Block::new(2, Kind::Structured, "Offer Team (JSON-LD, Mistral): price 15 EUR.".into()),
            ],
            ..Default::default()
        };
        // Free answers (the chosen one), Pro does not reach the threshold, Team answers too.
        let choices = structured_choices(&run.ctx(), &alone, &[Some(0.9), Some(0.2), Some(0.8)], 0, 0.9);
        let got: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(
            got,
            ["Offer Free (JSON-LD, Mistral Vibe): price 0 USD.", "Offer Team (JSON-LD, Mistral): price 15 EUR."]
        );
        assert!(choices.iter().all(|c| c.label.is_empty() && c.link == "https://mistral.ai/pricing/"));
        assert_eq!(choices[1].quote, "Offer Team (JSON-LD, Mistral): price 15 EUR.");
    }

    #[test]
    fn choices_are_deduped_by_value_and_label_and_capped_at_twenty() {
        let choice = |value: &str, label: &str| Choice {
            label: label.into(),
            value: value.into(),
            quote: value.into(),
            link: "https://example.com/".into(),
            p: 0.5,
        };
        let mut all = vec![choice("$8", "Pro"), choice("$8", "Pro"), choice("$8", "Team")];
        all.extend((0..30).map(|i| choice(&format!("{i}"), "row")));
        let out = finish(all);
        assert_eq!(out.len(), MAX_CHOICES);
        assert_eq!((out[0].value.as_str(), out[0].label.as_str()), ("$8", "Pro"));
        assert_eq!((out[1].value.as_str(), out[1].label.as_str()), ("$8", "Team"));
    }

    #[test]
    fn choices_are_listed_in_json_only_from_two_and_each_has_the_four_keys() {
        let run = Run::at("https://example.com/pricing", vec![Block::new(0, Kind::Para, "Pro: $8. Team: $14.".into())]);
        let choice = |value: &str, label: &str| Choice {
            label: label.into(),
            value: value.into(),
            quote: "Pro: $8. Team: $14.".into(),
            link: "https://example.com/pricing#:~:text=x".into(),
            p: 0.5,
        };
        let json_of = |choices: Vec<Choice>| {
            let pick = Pick { block: 0, range: 0..2, p: 0.9, choices };
            render_precise(&run.ctx(), &run.ex, &pick, None).json
        };
        assert!(json_of(vec![choice("$8", "Pro")]).get("choices").is_none());
        let v = json_of(vec![choice("$8", "Pro"), choice("$14", "Team")]);
        let list = v["choices"].as_array().expect("two choices");
        assert_eq!(list.len(), 2);
        for item in list {
            let obj = item.as_object().expect("a choice is an object");
            assert_eq!(obj.len(), 4, "{obj:?}");
            assert!(
                obj.contains_key("label")
                    && obj.contains_key("value")
                    && obj.contains_key("quote")
                    && obj.contains_key("link")
            );
        }
        assert_eq!(list[1]["value"], "$14");
        assert_eq!(list[1]["label"], "Team");
    }

    #[test]
    fn a_page_without_an_answer_lists_no_choices() {
        let run = Run::at("https://example.com/pricing", vec![Block::new(0, Kind::Para, "Pro: $8. Team: $14.".into())]);
        let spans = precise::candidates(&[&run.ex.blocks[0]]);
        let scored = ranked(&spans, &vec![0.3; spans.len()]);
        let (best, _) = scored[0];
        assert!(page_choices(&run.ctx(), &run.ex, &scored, &best.range, 0.3, 0.4).is_empty());
    }

    #[test]
    fn a_single_answer_that_a_sentence_restates_is_still_one_answer() {
        // The MDN control: the answer is said twice on the page, and Jev scores the restated sentence at 0.12, under the
        // choice bar: one answer, so no choices, and no `choices` key in the JSON.
        let blocks = vec![
            Block::new(0, Kind::Para, "Negative integers count back from the last item in the array.".into()),
            Block::new(1, Kind::Para, "Negative index counts back from the end of the array.".into()),
        ];
        let run = Run::at(
            "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Array/at",
            blocks,
        );
        let spans = precise::candidates(&run.ex.blocks.iter().collect::<Vec<_>>());
        let probs = scores_of(&spans, &run.ex, |block, text| match (block, text) {
            (1, "Negative index counts back from the end of the array") => 0.85,
            (0, "Negative integers count back from the last item in the array") => 0.12,
            _ => 0.0,
        });
        let scored = ranked(&spans, &probs);
        let (best, own) = scored[0];
        let p = share(best, own, &scored);
        let choices = page_choices(&run.ctx(), &run.ex, &scored, &best.range, p, 0.4);
        assert_eq!(choices.len(), 1, "{choices:?}");
        let pick = Pick { block: best.block, range: best.range.clone(), p, choices };
        assert!(render_precise(&run.ctx(), &run.ex, &pick, None).json.get("choices").is_none());
    }
}

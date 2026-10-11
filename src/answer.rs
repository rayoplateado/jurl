//! `--precise`: Jev picks the answer as one span of the best blocks' text, and says how sure it is.

use std::{
    collections::{BTreeSet, HashMap},
    ops::Range,
};

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
/// The most rows a precise answer's `context` carries: a table's header and rows, or a list's items. Past this, or past
/// `CONTEXT_MAX_CHARS`, the context keeps the rows nearest the answer that fit (see `context_of`).
pub(crate) const CONTEXT_MAX_ROWS: usize = 60;
/// The most characters a precise answer's `context` carries.
pub(crate) const CONTEXT_MAX_CHARS: usize = 6_000;

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

/// The whole table or list a precise answer came from, for the `context` of its JSON: the page's own words, as text.
pub(crate) struct BlockContext {
    pub(crate) text: String,
    /// Whether `text` is less than the whole: the header of a table and the rows nearest the answer that fit.
    pub(crate) truncated: bool,
}

/// The context of a precise answer: the table its block is (its header and rows, as the page has them) or the list its block is in
/// (its items, one per line, a joined run of short items included). A table is a block of two or more rows, whatever its kind: a
/// markdown page's table is a paragraph. Any other block has none, and so does a table whose answer is in no row, or whose header and
/// answer's row are over the caps on their own. Past `CONTEXT_MAX_ROWS` rows or `CONTEXT_MAX_CHARS` characters, the context keeps
/// the rows nearest the answer that fit, above and below in turn, and is marked truncated. Rows are kept whole, never cut.
pub(crate) fn context_of(ex: &Extracted, pick: &Pick) -> Option<BlockContext> {
    let block = &ex.blocks[pick.block];
    if let Some(list) = block.list {
        let items: Vec<(usize, &str)> = ex
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.list == Some(list))
            .map(|(i, b)| (i, b.text.as_str()))
            .collect();
        let hot = items.iter().position(|&(i, _)| i == pick.block)?;
        let texts: Vec<&str> = items.iter().map(|&(_, text)| text).collect();
        let kept = kept_rows(&texts, 0, hot)?;
        let text = kept.iter().map(|&k| texts[k]).collect::<Vec<_>>().join("\n");
        return Some(BlockContext { text, truncated: kept.len() < texts.len() });
    }
    if matches!(block.kind, Kind::Code | Kind::Structured) {
        return None;
    }
    let spans = precise::table_rows(&block.text);
    if spans.len() < 2 {
        return None;
    }
    let rows: Vec<&str> = spans.iter().map(|r| &block.text[r.clone()]).collect();
    let hot = spans.iter().position(|r| r.contains(&pick.range.start))?;
    let kept = kept_rows(&rows, 1, hot)?;
    // The table as the page has it, prose lines and all, when it is whole and within the cap.
    if kept.len() == rows.len() && block.text.chars().count() <= CONTEXT_MAX_CHARS {
        return Some(BlockContext { text: block.text.clone(), truncated: false });
    }
    let text = kept.iter().map(|&k| rows[k]).collect::<Vec<_>>().join("\n");
    Some(BlockContext { text, truncated: true })
}

/// The rows a context keeps, as indexes in order: all of them when they fit the caps. Otherwise the first `lead` rows (a table's
/// header) and the row at `hot`, with the rows next to it added above and below in turn for as long as they fit. None when the
/// first rows and the row at `hot` are over the caps on their own.
fn kept_rows(rows: &[&str], lead: usize, hot: usize) -> Option<Vec<usize>> {
    let (mut lo, mut hi) = (hot, hot);
    let mut kept = fitting(rows, lead, lo, hi)?;
    while kept.len() < rows.len() {
        let mut grew = false;
        if hi + 1 < rows.len()
            && let Some(next) = fitting(rows, lead, lo, hi + 1)
        {
            (hi, kept, grew) = (hi + 1, next, true);
        }
        if lo > lead
            && let Some(next) = fitting(rows, lead, lo - 1, hi)
        {
            (lo, kept, grew) = (lo - 1, next, true);
        }
        if !grew {
            break;
        }
    }
    Some(kept)
}

/// The rows `0..lead` and `lo..=hi`, in order, when they fit both caps; None when they don't.
fn fitting(rows: &[&str], lead: usize, lo: usize, hi: usize) -> Option<Vec<usize>> {
    let kept: BTreeSet<usize> = (0..lead).chain(lo..=hi).collect();
    let chars = kept.iter().map(|&k| rows[k].chars().count()).sum::<usize>() + kept.len().saturating_sub(1);
    (kept.len() <= CONTEXT_MAX_ROWS && chars <= CONTEXT_MAX_CHARS).then(|| kept.into_iter().collect())
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
    // An answer from a later page of a list (`--pages`) is a link on that page, not on the start page.
    let page_url = block.page.and_then(|n| ex.pages.get(n - 1)).unwrap_or(ctx.url);
    let link = answer_link(page_url, block, &pick.range);
    let source =
        if block.kind == Kind::Structured { " (JSON-LD: the page's structured data, not text it shows)" } else { "" };
    // A table's quote is its header and the answer's row, so a cell says what it is ("Clase Turista"), not the whole table.
    let (quote, shown) = quoted(block, &pick.range);
    let p = pick.p;

    let mut doc = json!({
        "url": page_url.as_str(),
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
    // The record of a repeated list the answer is in, numbered as the block arrays number it (see `cards`).
    if let Some(record) = block.record {
        doc["record"] = json!(record);
    }
    // The table or list the answer came from, as much of it as fits (`context_of`). A miss's closest candidate has it too.
    if let Some(context) = context_of(ex, pick) {
        doc["context"] = json!(context.text);
        if context.truncated {
            doc["context_truncated"] = json!(true);
        }
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

    /// What a precise answer prints for `block` with `range` of it as the answer: a run's own `Ctx`, no network.
    fn precise_json(block: &Block, range: Range<usize>, p: f64) -> Value {
        precise_json_in(vec![block.clone()], 0, range, p)
    }

    /// What a precise answer prints for a page of `blocks`, with `range` of block `at` as the answer.
    fn precise_json_in(blocks: Vec<Block>, at: usize, range: Range<usize>, p: f64) -> Value {
        let args = Args::parse_from(["jurl", "--precise", "--json", "-q", "Which one?", "example.com"]);
        let client = reqwest::Client::new();
        let url = url::Url::parse("https://example.com/").unwrap();
        let ex = Extracted { title: "Pricing".into(), blocks, ..Default::default() };
        let ctx = Ctx::new(&args, &client, "", &url, &ex);
        render_precise(&ctx, &ex, &Pick { block: at, range, p }, None).json
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

    #[test]
    fn a_table_answer_carries_its_whole_table_as_context() {
        let text = "| Plan | Price |\n| Free | $0 |\n| Pro | $20 |\n| Team | $30 |";
        let at = text.find("$20").unwrap();
        let table = Block::new(0, Kind::Table, text.into());
        let v = precise_json(&table, at..at + 3, 0.93);
        assert_eq!(v["quote"], "| Plan | Price |\n| Pro | $20 |");
        assert_eq!(v["context"], text);
        assert!(v.get("context_truncated").is_none());
    }

    #[test]
    fn a_markdown_table_in_a_paragraph_is_a_table_and_its_context_is_the_whole_block() {
        // A markdown page's table is a paragraph; the prose line above it is in the block, so it is in the context too.
        let text = "Prices in USD:\n| Plan | Price |\n| Pro | $20 |\n| Team | $30 |";
        let at = text.find("$30").unwrap();
        let para = Block::new(0, Kind::Para, text.into());
        let v = precise_json(&para, at..at + 3, 0.93);
        assert_eq!(v["quote"], "| Plan | Price |\n| Team | $30 |");
        assert_eq!(v["context"], text);
    }

    #[test]
    fn a_paragraph_a_code_block_a_structured_value_or_a_single_row_has_no_context() {
        let para = Block::new(0, Kind::Para, "Pro plan: $8 a month".into());
        assert!(precise_json(&para, 10..12, 0.93).get("context").is_none());
        let code = Block::new(0, Kind::Code, "| a | b |\n| c | d |".into());
        assert!(precise_json(&code, 0..5, 0.93).get("context").is_none());
        let structured = Block::new(0, Kind::Structured, "Offer Free (JSON-LD, Mistral Vibe): price 0 USD.".into());
        assert!(precise_json(&structured, 0..5, 0.93).get("context").is_none());
        let one_row = Block::new(0, Kind::Para, "| Pro | $20 |".into());
        assert!(precise_json(&one_row, 8..11, 0.93).get("context").is_none());
    }

    #[test]
    fn a_list_answer_carries_the_whole_list_one_item_per_line() {
        let items = ["Free: $0", "Pro: $20", "Team: $30"];
        let blocks: Vec<Block> = items
            .iter()
            .enumerate()
            .map(|(i, text)| Block { list: Some(0), ..Block::new(i, Kind::Item, text.to_string()) })
            .collect();
        let v = precise_json_in(blocks, 1, 0..items[1].len(), 0.93);
        assert_eq!(v["kind"], "item");
        assert_eq!(v["context"], "Free: $0\nPro: $20\nTeam: $30");
        assert!(v.get("context_truncated").is_none());
    }

    #[test]
    fn a_joined_run_of_short_items_is_part_of_its_list_and_another_list_is_not() {
        let blocks = vec![
            Block { list: Some(0), ..Block::new(0, Kind::Item, "Flour".into()) },
            Block { list: Some(0), ..Block::new(1, Kind::Para, "1 egg\nSalt".into()) },
            Block { list: Some(0), ..Block::new(2, Kind::Item, "Milk".into()) },
            Block::new(3, Kind::Para, "Serves four.".into()),
            Block { list: Some(1), ..Block::new(4, Kind::Item, "Another list".into()) },
        ];
        let v = precise_json_in(blocks.clone(), 2, 0..4, 0.93);
        assert_eq!(v["context"], "Flour\n1 egg\nSalt\nMilk");
        // The joined run is a paragraph, and an answer in it has the same list.
        let v = precise_json_in(blocks, 1, 0..6, 0.93);
        assert_eq!(v["kind"], "para");
        assert_eq!(v["context"], "Flour\n1 egg\nSalt\nMilk");
    }

    #[test]
    fn a_table_over_the_row_cap_keeps_its_header_and_the_rows_nearest_its_answer() {
        let rows: Vec<String> = (0..100).map(|i| format!("| Row {i} | {i} units |")).collect();
        let text = format!("| Name | Units |\n{}", rows.join("\n"));
        let at = text.find("| Row 50 |").unwrap() + 2;
        let table = Block::new(0, Kind::Table, text.clone());
        let v = precise_json(&table, at..at + 6, 0.93);
        assert_eq!(v["context_truncated"], true);
        let context = v["context"].as_str().unwrap();
        let lines: Vec<&str> = context.lines().collect();
        // The header, then the 59 rows from 21 to 79 (the answer's row, 50, in the middle): 60 rows, each one whole.
        assert_eq!(lines.len(), CONTEXT_MAX_ROWS);
        assert_eq!(lines[0], "| Name | Units |");
        let kept: Vec<usize> = lines[1..].iter().map(|l| l.split(' ').nth(2).unwrap().parse().unwrap()).collect();
        assert_eq!(kept, (21..=79).collect::<Vec<_>>());
        assert!(lines.iter().all(|l| text.lines().any(|t| t == *l)));
    }

    #[test]
    fn a_table_over_the_character_cap_is_cut_between_rows_never_inside_one() {
        // Rows of about 157 characters, accents included: the cap is in characters, and a cut falls between rows.
        let rows: Vec<String> = (0..50).map(|i| format!("| Fila {i} | {} | ñ |", "á".repeat(140))).collect();
        let text = format!("| Nombre | Notas |\n{}", rows.join("\n"));
        let at = text.find("| Fila 25 |").unwrap() + 2;
        let table = Block::new(0, Kind::Table, text.clone());
        let v = precise_json(&table, at..at + 6, 0.93);
        assert_eq!(v["context_truncated"], true);
        let context = v["context"].as_str().unwrap();
        assert!(context.chars().count() <= CONTEXT_MAX_CHARS, "{} characters", context.chars().count());
        assert!(context.starts_with("| Nombre | Notas |\n"));
        assert!(context.contains("| Fila 25 |"));
        assert!(context.lines().all(|l| text.lines().any(|t| t == l)), "{context}");
    }

    #[test]
    fn a_header_and_answer_row_over_the_cap_on_their_own_have_no_context() {
        let text = format!("| {} |\n| {} |", "h".repeat(3000), "v".repeat(3500));
        let at = text.find("vvv").unwrap();
        let table = Block::new(0, Kind::Table, text);
        let v = precise_json(&table, at..at + 3, 0.93);
        assert!(v.get("context").is_none());
        assert!(v["quote"].as_str().unwrap().starts_with("| hhh"));
    }

    #[test]
    fn a_list_over_the_row_cap_keeps_the_items_nearest_its_answer() {
        let items: Vec<Block> =
            (0..100).map(|i| Block { list: Some(0), ..Block::new(i, Kind::Item, format!("Item {i}")) }).collect();
        let v = precise_json_in(items, 40, 0..7, 0.93);
        assert_eq!(v["context_truncated"], true);
        let kept: Vec<usize> =
            v["context"].as_str().unwrap().lines().map(|l| l.trim_start_matches("Item ").parse().unwrap()).collect();
        assert_eq!(kept.len(), CONTEXT_MAX_ROWS);
        assert!(kept.contains(&40));
        assert!(kept.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[test]
    fn a_list_item_over_the_character_cap_on_its_own_has_no_context() {
        let long = Block { list: Some(0), ..Block::new(0, Kind::Item, "x".repeat(7000)) };
        assert!(precise_json(&long, 0..10, 0.93).get("context").is_none());
    }

    #[test]
    fn a_miss_with_a_closest_table_row_carries_its_table_too() {
        let text = "| Plan | Price |\n| Pro | $20 |\n| Team | $30 |";
        let at = text.find("$20").unwrap();
        let table = Block::new(0, Kind::Table, text.into());
        let v = precise_json(&table, at..at + 3, 0.2);
        assert_eq!(v["answer"], Value::Null);
        assert_eq!(v["closest"], "$20");
        assert_eq!(v["context"], text);
    }
}

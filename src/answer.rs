//! `--precise`: Jev picks the answer as one span of the best blocks' text, and says how sure it is.

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

/// The --precise answer: a byte range of one block's text, and how sure Jev is that it's exactly the answer.
pub(crate) struct Pick {
    pub(crate) block: usize,
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) p: f64,
}

/// --precise: Jev scores spans of the best blocks as the exact answer.
pub(crate) async fn precise_pick(ctx: &Ctx<'_>, ex: &Extracted, keep: &[(usize, f64)], t: &mut Timer) -> Result<Pick> {
    let q = ctx.ask();
    let top: Vec<&Block> = keep.iter().take(3).map(|&(i, _)| &ex.blocks[i]).collect();
    let spans = precise::candidates(&top);
    if spans.is_empty() {
        return Err(not_found(format!("nothing in {} answers that exactly", ctx.url)));
    }

    // One choice over every span, plus "none": Jev weighs the spans against each other, so the one that is
    // exactly the answer beats the sentence around it, and "none" wins when the page doesn't say it.
    let items: Vec<Item> = top
        .iter()
        .map(|b| Item {
            state: json!({ "block": b.i, "text": b.text.chars().take(STATE_TEXT_CHARS).collect::<String>() }),
            questions: Vec::new(),
        })
        .collect();
    let mut criteria = Map::new();
    for (k, s) in spans.iter().enumerate() {
        criteria
            .insert(format!("s{k}"), json!(s.label.as_deref().unwrap_or(&ex.blocks[s.block].text[s.range.clone()])));
    }
    criteria.insert("none".to_string(), json!("None of these is exactly the answer"));
    let pick = choice(
        &format!(
            "Which of these spans from the blocks is exactly the answer to this question, with nothing missing \
             and nothing extra? {q}"
        ),
        Value::Object(criteria),
    );
    let a = ctx.judge("blocks", items, Map::from_iter([("pick".to_string(), pick)])).await?;
    t.lap(a.label());

    // Most likely span wins; on a tie (to two decimals) the shorter one, which says the same with less.
    let probs = a.probabilities("pick").context("jev returned no answer")?;
    let mut scored: Vec<(&precise::Span, f64)> =
        spans.iter().enumerate().map(|(k, s)| (s, probs.get(&format!("s{k}")).copied().unwrap_or(0.0))).collect();
    let key = |(s, p): &(&precise::Span, f64)| ((-p * 100.0).round() as i64, s.range.len());
    scored.sort_by_key(key);
    let (mut best, own) = scored[0];
    // "$8", "$8 per user" and "$8 per user, per month" are one answer with more or less around it, and they split
    // the vote. How sure jurl is that the answer is there counts them together: the winner plus every span that
    // holds it or sits inside it.
    let nested = |s: &precise::Span| {
        s.block == best.block
            && s.range != best.range
            && ((s.range.start <= best.range.start && s.range.end >= best.range.end)
                || (s.range.start >= best.range.start && s.range.end <= best.range.end))
    };
    let p = (own + scored.iter().filter(|(s, _)| nested(s)).map(|(_, p)| p).sum::<f64>()).min(1.0);
    let p = (p * 100.0).round() / 100.0;
    let threshold = ctx.args.threshold.unwrap_or(PRECISE_THRESHOLD);

    // The winner can carry more than the answer ("2009; 17 years ago"). When shorter candidates sit inside it,
    // ask once more, among just those and the winner, which one is the answer with nothing extra.
    let inside: Vec<&precise::Span> = spans
        .iter()
        .filter(|s| s.block == best.block && s.range != best.range)
        .filter(|s| s.range.start >= best.range.start && s.range.end <= best.range.end)
        .collect();
    let text = ex.blocks[best.block].text.as_str();
    let context = || {
        vec![Item {
            state: json!({ "block": best.block, "text": text.chars().take(STATE_TEXT_CHARS).collect::<String>() }),
            questions: Vec::new(),
        }]
    };
    if p >= threshold && ex.blocks[best.block].kind != Kind::Code && precise::refinable(text, best) {
        // A long winner (a line, clause or sentence) may hold the answer and more. Its pieces, the shorter
        // candidates inside it and the winner itself are scored once more with the same question; a piece wins
        // only when Jev likes it at least as much as the whole, and at the answer's own threshold.
        let mut options: Vec<std::ops::Range<usize>> = vec![best.range.clone()];
        for r in inside.iter().map(|s| s.range.clone()).chain(precise::refinements(text, &best.range)) {
            if !options.contains(&r) && options.len() <= precise::MAX_REFINE {
                options.push(r);
            }
        }
        let mut criteria = Map::new();
        for (k, r) in options.iter().enumerate() {
            criteria.insert(format!("o{k}"), json!(&text[r.clone()]));
        }
        criteria.insert("none".to_string(), json!("None of these is exactly the answer"));
        let refine = choice(
            &format!(
                "Which of these spans from the block is exactly the answer to this question, with nothing missing \
                 and nothing extra? {q}"
            ),
            Value::Object(criteria),
        );
        let b = ctx.judge("blocks", context(), Map::from_iter([("refine".to_string(), refine)])).await?;
        t.lap(b.label());
        let probs = b.probabilities("refine").unwrap_or_default();
        let whole = probs.get("o0").copied().unwrap_or(0.0);
        let piece = (1..options.len())
            .map(|k| (k, probs.get(&format!("o{k}")).copied().unwrap_or(0.0)))
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
        if let Some((k, pk)) = piece
            && pk >= threshold
            && pk >= whole
        {
            return Ok(Pick { block: best.block, range: options[k].clone(), p });
        }
        return Ok(Pick { block: best.block, range: best.range.clone(), p });
    }
    if p >= threshold && !inside.is_empty() {
        let options: Vec<&precise::Span> = std::iter::once(best).chain(inside).collect();
        let mut criteria = Map::new();
        for (k, s) in options.iter().enumerate() {
            criteria.insert(format!("o{k}"), json!(&ex.blocks[s.block].text[s.range.clone()]));
        }
        let tighter = choice(
            &format!(
                "All of these say the answer to this question. Which one is exactly the answer, without any \
                 extra words around it? {q}"
            ),
            Value::Object(criteria),
        );
        let b = ctx.judge("blocks", context(), Map::from_iter([("tighter".to_string(), tighter)])).await?;
        t.lap(b.label());
        if let Some((k, c)) = b.choice("tighter")
            && c >= 0.5
            && let Some(i) = k.strip_prefix('o').and_then(|i| i.parse::<usize>().ok())
            && let Some(s) = options.get(i)
        {
            best = s;
        }
    }
    Ok(Pick { block: best.block, range: best.range.clone(), p })
}

/// The answer on its own line, then the block it's in and a link to it; JSON says how sure, and below the threshold
/// has it as `closest` instead of `answer`. `path` is how --follow got here.
pub(crate) fn render_precise(ctx: &Ctx<'_>, ex: &Extracted, pick: &Pick, path: Option<&[url::Url]>) -> Rendered {
    let q = ctx.args.ask.as_deref().unwrap_or_default();
    let threshold = ctx.args.threshold.unwrap_or(PRECISE_THRESHOLD);
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

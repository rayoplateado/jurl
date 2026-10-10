//! The default mode, `-q` and `--code`: Jev scores each block, and the best are printed in page order
//! under their headings.

use std::collections::HashSet;

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::{
    answer::{PRECISE_BLOCK_FLOOR, PRECISE_BLOCKS, Pick, precise_pick, render_precise},
    decide::choice,
    extract::{self, Block, Extracted, Kind},
    judge::{Ctx, Item, STATE_TEXT_CHARS},
    output::{Rendered, missed, not_found},
    timing::Timer,
};

/// Blocks printed when `--max` doesn't say: 12, or 5 with `-q`.
const DEFAULT_BLOCKS: usize = 12;
const DEFAULT_ASK_BLOCKS: usize = 5;
/// Code blocks printed by `--code` when `--max` doesn't say.
const DEFAULT_CODE_BLOCKS: usize = 8;

/// Default mode and --code: pick blocks, print them in page order.
pub(crate) async fn blocks(ctx: &Ctx<'_>, ex: &Extracted, t: &mut Timer) -> Result<Rendered> {
    let args = ctx.args;
    let (scores, kind) = score_blocks(ctx, ex, t).await?;

    // Top-N by probability, printed in page order. Headings survive when their section does.
    let default_max = if args.code {
        DEFAULT_CODE_BLOCKS
    } else if args.ask.is_some() {
        DEFAULT_ASK_BLOCKS
    } else {
        DEFAULT_BLOCKS
    };
    // --precise looks inside the best few blocks even when none of them answers on its own: whether a span of
    // them is the answer is decided next, at the span's own threshold.
    if args.precise {
        let keep = top(&scores, PRECISE_BLOCK_FLOOR, PRECISE_BLOCKS);
        if keep.is_empty() {
            return structured_or(ctx, ex, t, not_found(format!("nothing in {} answers that", ctx.url))).await;
        }
        let pick = precise_pick(ctx, ex, &keep, t).await?;
        let rendered = render_precise(ctx, ex, &pick, None);
        if pick.p < ctx.args.threshold_for(true) {
            let answer = &ex.blocks[pick.block].text[pick.range.clone()];
            let message =
                format!("no part of {} is exactly the answer (closest: \"{answer}\", p={:.2})", ctx.url, pick.p);
            return structured_or(ctx, ex, t, missed(message, rendered)).await;
        }
        return Ok(rendered);
    }
    let keep = top(&scores, args.threshold(), args.limit(default_max));
    render_blocks(ctx, ex, &scores, &keep, kind, None)
}

/// --precise, when the page's text gives no answer: the page's JSON-LD values (see `extract::json_ld`) are judged on their
/// own, and the best one answers if Jev is sure of it. It comes back as the block to append to the page, numbered as the
/// page's next block, with a pick over its whole text. A value is only looked at when the text has none, so the text's
/// own answer is never replaced by one. Jev failing is not an answer from the values: none is given.
pub(crate) async fn structured_pick(ctx: &Ctx<'_>, ex: &Extracted, t: &mut Timer) -> Result<Option<(Block, Pick)>> {
    if !ctx.args.precise || ex.structured.is_empty() {
        return Ok(None);
    }
    let alone = Extracted { blocks: ex.structured.clone(), ..Default::default() };
    let Ok((scores, _)) = score_blocks(ctx, &alone, t).await else { return Ok(None) };
    let best = scores.iter().enumerate().filter_map(|(i, s)| s.map(|p| (i, p))).max_by(|a, b| a.1.total_cmp(&b.1));
    let Some((i, p)) = best else { return Ok(None) };
    if p < ctx.args.threshold_for(true) {
        return Ok(None);
    }
    let mut block = alone.blocks[i].clone();
    block.i = ex.blocks.len();
    let pick = Pick { block: ex.blocks.len(), range: 0..block.text.len(), p };
    Ok(Some((block, pick)))
}

/// `structured_pick` for a single page: the answer is rendered from the values themselves, else `otherwise` stands.
async fn structured_or(ctx: &Ctx<'_>, ex: &Extracted, t: &mut Timer, otherwise: anyhow::Error) -> Result<Rendered> {
    match structured_pick(ctx, ex, t).await? {
        Some((block, pick)) => {
            let alone = Extracted { blocks: vec![block], ..Default::default() };
            let pick = Pick { block: 0, ..pick };
            Ok(render_precise(ctx, &alone, &pick, None))
        }
        None => Err(otherwise),
    }
}

/// Jev's probability for each block (None for headings and blocks too short to judge alone), and the page's kind.
pub(crate) async fn score_blocks(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    t: &mut Timer,
) -> Result<(Vec<Option<f64>>, Option<(String, f64)>)> {
    let args = ctx.args;
    let is_candidate = |b: &Block| match b.kind {
        Kind::Heading => false,
        Kind::Code => !b.text.trim().is_empty(),
        _ => !args.code && b.text.chars().count() >= extract::SHORT_BLOCK_CHARS,
    };
    if !ex.blocks.iter().any(is_candidate) {
        if args.code {
            return Err(not_found(format!("no code blocks in {}", ctx.url)));
        }
        if args.render {
            bail!("no readable content in {}", ctx.url);
        }
        bail!("no readable content in {} (if it needs JavaScript, try --render)", ctx.url);
    }

    let default = if args.code {
        "is a useful code example, command or snippet for this page's topic, not boilerplate."
    } else {
        "carries core information of this page — what a reader came here for — rather than navigation, \
         boilerplate, cookie or legal text, promos, sign-up prompts, author bios or comments."
    };
    let items = ex
        .blocks
        .iter()
        .map(|b| Item {
            state: json!({ "i": b.i, "kind": b.kind, "text": b.text.chars().take(STATE_TEXT_CHARS).collect::<String>() }),
            questions: if is_candidate(b) {
                vec![(format!("b{}", b.i), ctx.question(&format!("The block in `blocks` with i={}", b.i), default))]
            } else {
                Vec::new()
            },
        })
        .collect();
    let a = ctx.judge("blocks", items, page_questions(args.precise)).await?;
    t.lap(a.label());
    let kind = a.choice("page_kind");
    let scores: Vec<Option<f64>> = ex.blocks.iter().map(|b| a.noul(&format!("b{}", b.i))).collect();
    Ok((scores, kind))
}

/// The questions about the whole page, asked with its blocks. `page_kind` is what the plain output prints: --precise
/// prints only the answer, so it doesn't ask for the kind (one question fewer in each of its requests).
fn page_questions(precise: bool) -> Map<String, Value> {
    if precise {
        return Map::new();
    }
    Map::from_iter([(
        "page_kind".to_string(),
        choice(
            "What kind of page is this?",
            json!({
                "article": "News, blog post, essay or story",
                "docs": "Technical documentation, reference, tutorial or README",
                "product": "A product, service or pricing page",
                "repo": "A code repository page",
                "listing": "Index, search results, feed or category page",
                "forum": "Discussion thread, Q&A or comments",
                "other": null,
            }),
        ),
    )])
}

/// The kept blocks in page order, each with its heading, as markdown and JSON. `path` is how --follow got here.
pub(crate) fn render_blocks(
    ctx: &Ctx<'_>,
    ex: &Extracted,
    scores: &[Option<f64>],
    keep: &[(usize, f64)],
    kind: Option<(String, f64)>,
    path: Option<&[url::Url]>,
) -> Result<Rendered> {
    let args = ctx.args;
    let kept: HashSet<usize> = keep.iter().map(|&(i, _)| i).collect();
    let mut pending_heading = None;
    let mut selected = Vec::new();
    let items = extract::short_items_of_kept_lists(&ex.blocks, |i| kept.contains(&i));
    for b in &ex.blocks {
        if b.kind == Kind::Heading {
            pending_heading = Some(b);
        } else if kept.contains(&b.i) || items.contains(&b.i) {
            if let Some(h) = pending_heading.take() {
                selected.push(h);
            }
            selected.push(b);
        }
    }
    if selected.is_empty() {
        if args.ask.is_some() {
            return Err(not_found(format!("nothing in {} answers that (try a lower --threshold)", ctx.url)));
        }
        return Err(not_found(format!("nothing in {} looks like content (try a lower --threshold)", ctx.url)));
    }

    let blocks: Vec<_> = selected
        .iter()
        .map(|b| {
            let mut v = serde_json::to_value(b).unwrap();
            if let Some(p) = scores[b.i] {
                v["p"] = json!(p);
            }
            v
        })
        .collect();
    let mut doc = json!({
        "url": ctx.url.as_str(),
        "title": ex.title,
        "ask": args.ask,
        "kind": kind.as_ref().map(|k| json!({ "choice": k.0, "confidence": k.1 })),
        "blocks": blocks,
    });
    if let Some(path) = path {
        doc["path"] = json!(path.iter().map(url::Url::as_str).collect::<Vec<_>>());
    }
    let mut text = String::new();
    if !ex.title.is_empty() {
        text += &format!("# {}\n\n", ex.title);
    }
    let kind = kind.map(|(k, c)| format!(" · {k} ({c:.2})")).unwrap_or_default();
    text += &format!("<{}>{kind}\n\n", ctx.url);
    for b in selected {
        text += &format!("{}\n\n", b.markdown());
    }
    Ok(Rendered { text, json: doc })
}

/// The `max` best scores at or above `threshold` as (index, score), best first. Ties go to the lower index, so the
/// same page always gives the same order.
pub(crate) fn top(scores: &[Option<f64>], threshold: f64, max: usize) -> Vec<(usize, f64)> {
    let mut ranked: Vec<(usize, f64)> =
        scores.iter().enumerate().filter_map(|(i, p)| p.filter(|&p| p >= threshold).map(|p| (i, p))).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(max);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_is_best_first_and_ties_go_to_the_lower_index() {
        let scores = [Some(0.5), None, Some(0.9), Some(0.5), Some(0.2), Some(0.5), Some(0.4)];
        // A score at the threshold counts; one below it, or a None, does not.
        assert_eq!(top(&scores, 0.4, 10), vec![(2, 0.9), (0, 0.5), (3, 0.5), (5, 0.5), (6, 0.4)]);
        assert_eq!(top(&scores, 0.4, 2), vec![(2, 0.9), (0, 0.5)]);
        assert!(top(&scores, 0.95, 10).is_empty());
    }

    #[test]
    fn precise_runs_do_not_ask_for_the_page_kind() {
        // --precise prints the answer, not the kind of page, so Jev isn't asked for the kind.
        assert!(page_questions(false).contains_key("page_kind"));
        assert!(page_questions(true).is_empty());
    }
}

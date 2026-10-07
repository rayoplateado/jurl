//! `--precise`: the answer itself, not the block it sits in. Candidates are spans of the picked blocks' text,
//! kept as byte ranges, so whatever wins is still the page's own words; Jev only scores them.

use std::ops::Range;

use crate::extract::{Block, Kind};

/// A candidate answer: `blocks[block].text[range]`.
#[derive(Debug, Clone)]
pub struct Span {
    pub block: usize,
    pub range: Range<usize>,
}

/// More than this and the request stops being one cheap call.
const MAX_CANDIDATES: usize = 200;
const MAX_SPAN_CHARS: usize = 400;
/// Lowercase words a name or title can have inside it ("The Coal Question", "Bank of England").
const CONNECTORS: &[&str] = &["of", "the", "and", "for", "de", "del", "la", "von", "van", "&"];
const DANGLING: &[&str] =
    &["per", "of", "the", "and", "or", "to", "a", "an", "in", "for", "with", "at", "by", "from", "on"];
const CURRENCY: &[&str] = &["$", "€", "£", "¥", "US$", "USD", "EUR", "GBP"];

/// Candidate spans from `blocks`, taken from each block in turn so a long one (a big table) can't use up the
/// whole budget. Every span is a substring of its block's text.
pub fn candidates(blocks: &[&Block]) -> Vec<Span> {
    let mut queues: Vec<std::vec::IntoIter<Range<usize>>> = blocks
        .iter()
        .map(|b| if b.kind == Kind::Code { code_spans(&b.text) } else { prose_spans(&b.text) }.into_iter())
        .collect();
    let mut out: Vec<Span> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut live = true;
    while live && out.len() < MAX_CANDIDATES {
        live = false;
        for (b, queue) in queues.iter_mut().enumerate() {
            let text = blocks[b].text.as_str();
            // The next span of this block that is new and not too long.
            for r in queue.by_ref() {
                let Some(r) = trim(text, r) else { continue };
                let s = &text[r.clone()];
                if s.chars().count() > MAX_SPAN_CHARS || !seen.insert(s.to_string()) {
                    continue;
                }
                out.push(Span { block: b, range: r });
                live = true;
                break;
            }
            if out.len() >= MAX_CANDIDATES {
                break;
            }
        }
    }
    out
}

/// Code: each line, and the whole snippet when it's short.
fn code_spans(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        out.push(start..start + line.len());
        start += line.len();
    }
    if out.len() <= 3 {
        out.push(0..text.len());
    }
    out
}

/// Prose: things that look like a value (numbers with what follows them), names and titles, clauses and
/// sentences. Short ones first, so a value isn't crowded out by the sentence around it.
fn prose_spans(text: &str) -> Vec<Range<usize>> {
    let tokens = tokens(text);
    let mut out = Vec::new();

    // Runs of 1-4 tokens starting at a number, one token earlier when that one is a currency ("$ 8", "US$ 8").
    for (t, tok) in tokens.iter().enumerate() {
        // Digits in a footnote mark ("Spotify.[289]") aren't a value.
        if !strip_note(&text[tok.clone()]).chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        let start = if t > 0 && CURRENCY.contains(&&text[tokens[t - 1].clone()]) { t - 1 } else { t };
        // A value glued to a label by the markup ("Pro$10", "Plus*€9.50") also runs from where the value starts.
        let tok_text = &text[tok.clone()];
        let inner = tok_text
            .char_indices()
            .find(|&(_, c)| c.is_ascii_digit() || "$€£¥".contains(c))
            .map(|(i, _)| i)
            .filter(|&i| i > 0 && tok_text[..i].chars().any(char::is_alphabetic));
        let starts = [Some(tokens[start].start), inner.map(|i| tok.start + i)];
        // And a value glued to the label after it ("$8Per user") also ends where the value ends.
        let value_end = tok_text
            .char_indices()
            .skip_while(|&(_, c)| !c.is_ascii_digit())
            .find(|&(_, c)| c.is_uppercase())
            .map(|(i, _)| tok.start + i);
        if let Some(end) = value_end {
            for from in starts.into_iter().flatten().filter(|&f| f < end) {
                out.push(from..end);
            }
        }
        for from in starts.into_iter().flatten() {
            for len in 1..=4 {
                let Some(end) = tokens.get(t + len - 1) else { break };
                // "10 million included per" says less than "10 million": a run can't end on a little word.
                if len > 1 && DANGLING.contains(&text[end.clone()].to_lowercase().trim_end_matches([',', '.']).trim()) {
                    continue;
                }
                out.push(from..end.end);
            }
        }
    }

    // Names and titles: capitalised words, with connectors allowed inside.
    let capital = |r: &Range<usize>| {
        text[r.clone()].trim_start_matches(['(', '"', '\'', '“', '‘']).starts_with(char::is_uppercase)
    };
    let connector = |r: &Range<usize>| CONNECTORS.contains(&text[r.clone()].to_lowercase().as_str());
    // A name doesn't run across a line: "Dylan Field" and "Evan Wallace" listed one per line are two names.
    let same_line = |a: &Range<usize>, b: &Range<usize>| !text[a.end..b.start].contains('\n');
    let mut names: Vec<Range<usize>> = Vec::new();
    let mut t = 0;
    while t < tokens.len() {
        if !capital(&tokens[t]) {
            t += 1;
            continue;
        }
        let mut end = t;
        let mut k = t + 1;
        while k < tokens.len()
            && k - t < 8
            && same_line(&tokens[k - 1], &tokens[k])
            && !ends_sentence(&text[tokens[k - 1].clone()])
            && (capital(&tokens[k]) || connector(&tokens[k]))
        {
            if capital(&tokens[k]) {
                end = k;
            }
            k += 1;
        }
        names.push(tokens[t].start..tokens[end].end);
        t = end + 1;
    }
    // Names one per line, together: the answer to "who founded it?" can be all of them. Only names that are their
    // whole line (or table cell): "Matthew Prince (co-chair & CEO)" isn't a line of names.
    let whole = |r: &Range<usize>| {
        let edge = |c: char| c == '\n' || c == '|';
        let start = text[..r.start].rfind(edge).map_or(0, |i| i + 1);
        let end = text[r.end..].find(edge).map_or(text.len(), |i| r.end + i);
        text[start..r.start].trim().is_empty() && text[r.end..end].trim().is_empty()
    };
    let mut first = 0;
    for n in 1..=names.len() {
        let joined = n < names.len()
            && whole(&names[n - 1])
            && whole(&names[n])
            && text[names[n - 1].end..names[n].start].trim().is_empty()
            && !same_line(&names[n - 1], &names[n]);
        if !joined {
            if n - first > 1 {
                out.push(names[first].start..names[n - 1].end);
            }
            first = n;
        }
    }
    out.extend(names);

    // Lines, clauses, then sentences.
    out.extend(split(text, |c, _| c == '\n'));
    out.extend(split(text, |c, next| matches!(c, ',' | ';' | ':' | '—' | '–') && next.is_none_or(char::is_whitespace)));
    out.extend(split(text, |c, next| {
        matches!(c, '.' | '!' | '?') && next.is_none_or(|n| n.is_whitespace() || n == '[')
    }));
    out
}

/// "Spotify." or "Spotify.[289]": a name doesn't run past it.
fn ends_sentence(token: &str) -> bool {
    strip_note(token).ends_with(['.', '!', '?', ';', ':'])
}

/// A word without the footnote mark after it: "Holloway[1]" → "Holloway".
fn strip_note(s: &str) -> &str {
    let mut s = s;
    while let Some(open) = s.strip_suffix(']').and_then(|k| k.rfind('['))
        && open > 0
        && s.len() - open <= 6
        && s[open + 1..s.len() - 1].chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
    {
        s = &s[..open];
    }
    s
}

/// Whitespace-separated tokens as byte ranges.
fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                out.push(s..i);
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(s..text.len());
    }
    out
}

/// Pieces of `text` between characters where `cut(c, next)` holds, each keeping its cut character.
fn split(text: &str, cut: impl Fn(char, Option<char>) -> bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next = chars.peek().map(|&(_, n)| n);
        if cut(c, next) {
            let end = i + c.len_utf8();
            out.push(start..end);
            start = end;
        }
    }
    if start < text.len() {
        out.push(start..text.len());
    }
    out
}

/// Shrink a range past surrounding whitespace and punctuation that isn't part of an answer.
fn trim(text: &str, r: Range<usize>) -> Option<Range<usize>> {
    let s = &text[r.clone()];
    let lead = s.len() - s.trim_start_matches(|c: char| c.is_whitespace() || "(\"'“‘".contains(c)).len();
    let rest = &s[lead..];
    // A footnote mark isn't part of the answer: "Lee Holloway[1]", "[289] Daniel Ek".
    let mut lead = lead;
    let mut rest = rest;
    if let Some(close) =
        rest.find(']').filter(|&i| i <= 5 && rest[..i].trim_start_matches('[').chars().all(|c| c.is_ascii_digit()))
    {
        let after = rest[close + 1..].trim_start();
        lead += rest.len() - after.len();
        rest = after;
    }
    let trim_end = |s: &str| s.trim_end_matches(|c: char| c.is_whitespace() || ".,;:!?)|\"'”’—–".contains(c)).len();
    let mut kept = &rest[..trim_end(rest)];
    loop {
        let k = strip_note(kept);
        let k = &k[..trim_end(k)];
        if k.len() == kept.len() {
            break;
        }
        kept = k;
    }
    if kept.is_empty() {
        return None;
    }
    // A span that opens a parenthesis keeps the one that closes it: "330 metres (1,083 ft)".
    let mut len = kept.len();
    if kept.matches('(').count() > kept.matches(')').count() && rest[len..].starts_with(')') {
        len += 1;
    }
    Some(r.start + lead..r.start + lead + len)
}

/// A link that opens the page highlighting the answer, with a few words either side as context so the
/// browser finds the right occurrence.
pub fn link(url: &url::Url, text: &str, range: &Range<usize>) -> String {
    let words = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
    // Context is up to three words next to the answer, as the browser shows them: markdown emphasis is
    // dropped, and a word that is markup (a table's `|`, an HTML tag, a link) ends the context there.
    let context = |words: &mut dyn Iterator<Item = &str>| {
        words
            .map(|w| w.trim_matches(['*', '_', '`']))
            .take_while(|w| w.chars().any(char::is_alphanumeric) && !w.contains(['<', '[', ']', '|']))
            .take(3)
            .map(String::from)
            .collect::<Vec<_>>()
    };
    let mut before = context(&mut text[..range.start].split_whitespace().rev());
    before.reverse();
    let after = context(&mut text[range.end..].split_whitespace());
    let span = words(&text[range.clone()]);
    let prefix = before.join(" ");
    let suffix = after.join(" ");
    let lines: Vec<&str> = text[range.clone()].lines().filter(|l| !l.trim().is_empty()).collect();
    let body = if lines.len() > 1 {
        // Text fragments don't match across block boundaries in one piece: give the first and last line.
        let edge = |l: &str| enc(&words(l).join(" "));
        format!("{},{}", edge(lines[0]), edge(lines[lines.len() - 1]))
    } else if span.len() > 10 {
        format!("{},{}", enc(&span[..5].join(" ")), enc(&span[span.len() - 5..].join(" ")))
    } else {
        enc(&span.join(" "))
    };
    let mut page = url.clone();
    page.set_fragment(None);
    let mut out = format!("{page}#:~:text=");
    if !prefix.is_empty() {
        out.push_str(&format!("{}-,", enc(&prefix)));
    }
    out.push_str(&body);
    if !suffix.is_empty() {
        out.push_str(&format!(",-{}", enc(&suffix)));
    }
    out
}

/// Percent-encoding for a text directive: everything but letters, digits and `_.~`, so `-`, `,` and `&`
/// can't be read as the directive's own syntax.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: Kind, text: &str) -> Block {
        Block { i: 0, kind, level: None, lang: None, text: text.to_string() }
    }

    fn texts(b: &Block) -> Vec<String> {
        candidates(&[b]).iter().map(|s| b.text[s.range.clone()].to_string()).collect()
    }

    #[test]
    fn every_candidate_is_a_substring() {
        let b = block(Kind::Para, "Pro: $8 per user/month, billed yearly. Business costs US$ 14 (or €13).");
        for s in candidates(&[&b]) {
            assert!(b.text.get(s.range.clone()).is_some(), "{s:?}");
        }
    }

    #[test]
    fn values_names_and_sentences() {
        let b =
            block(Kind::Para, "Workers Paid includes 10 million requests per month, and $0.30 per additional million.");
        let t = texts(&b);
        assert!(t.contains(&"10 million".to_string()), "{t:?}");
        assert!(t.contains(&"$0.30".to_string()), "{t:?}");
        assert!(t.contains(&"Workers Paid".to_string()), "{t:?}");
        let b = block(Kind::Para, "He published The Coal Question in 1865.");
        let t = texts(&b);
        assert!(t.contains(&"The Coal Question".to_string()), "{t:?}");
        assert!(t.contains(&"1865".to_string()), "{t:?}");
    }

    #[test]
    fn runs_dont_end_on_little_words() {
        let t = texts(&block(Kind::Para, "Includes 10 million requests per month."));
        assert!(t.contains(&"10 million requests".to_string()), "{t:?}");
        assert!(!t.iter().any(|s| s.ends_with(" per")), "{t:?}");
    }

    #[test]
    fn currency_before_the_number() {
        let t = texts(&block(Kind::Para, "Business costs US$ 14 a month."));
        assert!(t.contains(&"US$ 14".to_string()), "{t:?}");
    }

    #[test]
    fn values_glued_to_a_label() {
        let t = texts(&block(Kind::Para, "Monthly Subscription Free Pro$10 / month Plus*€9.50"));
        assert!(t.contains(&"$10 / month".to_string()), "{t:?}");
        assert!(t.contains(&"€9.50".to_string()), "{t:?}");
    }

    #[test]
    fn names_one_per_line() {
        let b = block(Kind::Table, "| Founders | Dylan Field\nEvan Wallace |");
        let t = texts(&b);
        assert!(t.contains(&"Dylan Field".to_string()), "{t:?}");
        assert!(t.contains(&"Evan Wallace".to_string()), "{t:?}");
        assert!(t.contains(&"Dylan Field\nEvan Wallace".to_string()), "{t:?}");
        assert!(!t.contains(&"Dylan Field Evan Wallace".to_string()), "{t:?}");
        let t =
            texts(&block(Kind::Table, "| Key people | Matthew Prince (co-chair & CEO)\nMichelle Zatlyn (president) |"));
        assert!(t.contains(&"Matthew Prince".to_string()), "{t:?}");
        assert!(!t.iter().any(|s| s.starts_with("CEO)")), "{t:?}");
        let t = texts(&block(Kind::Para, "Founded by Lee Holloway[1] in 2009."));
        assert!(t.contains(&"Lee Holloway".to_string()), "{t:?}");
        let t = texts(&block(Kind::Para, "in streaming services like Spotify.[289] Daniel Ek, co-founder and CEO."));
        assert!(t.contains(&"Daniel Ek".to_string()), "{t:?}");
        assert!(!t.iter().any(|s| s.starts_with("289") || s.starts_with("Spotify.[289] Daniel")), "{t:?}");
        let url = url::Url::parse("https://example.com/").unwrap();
        let start = b.text.find("Dylan").unwrap();
        let l = link(&url, &b.text, &(start..b.text.rfind(" |").unwrap()));
        assert_eq!(l, "https://example.com/#:~:text=Dylan%20Field,Evan%20Wallace");
    }

    #[test]
    fn every_block_gets_a_turn() {
        let big =
            block(Kind::Table, &(0..200).map(|i| format!("| Row {i} | {i} units |")).collect::<Vec<_>>().join("\n"));
        let small = block(Kind::Para, "Daniel Ek is the CEO.");
        let spans = candidates(&[&big, &small]);
        assert!(spans.iter().any(|s| s.block == 1 && small.text[s.range.clone()] == *"Daniel Ek"), "{spans:?}");
    }

    #[test]
    fn a_table_cell_border_is_not_part_of_an_answer() {
        let t = texts(&block(Kind::Table, "| **Pro** | $20/mo. | Everything you need |"));
        assert!(t.contains(&"$20/mo".to_string()), "{t:?}");
        assert!(!t.iter().any(|s| s.ends_with('|')), "{t:?}");
    }

    #[test]
    fn value_glued_to_the_label_after_it() {
        let t = texts(&block(Kind::Para, "Standard\n$8Per user, per month"));
        assert!(t.contains(&"$8".to_string()), "{t:?}");
    }

    #[test]
    fn code_lines() {
        let t = texts(&block(Kind::Code, "$ brew install ripgrep\n$ cargo install ripgrep\n"));
        assert!(t.contains(&"$ brew install ripgrep".to_string()), "{t:?}");
    }

    #[test]
    fn parentheses_stay_balanced() {
        let t = texts(&block(Kind::Para, "The tower is 330 metres (1,083 ft) tall."));
        assert!(t.contains(&"330 metres (1,083 ft)".to_string()), "{t:?}");
    }

    #[test]
    fn link_context_stops_at_markup() {
        let url = url::Url::parse("https://example.com/pricing").unwrap();
        let text = "| **Standard** | 10 million included per month |";
        let start = text.find("10 million").unwrap();
        let l = link(&url, text, &(start..start + 10));
        assert_eq!(l, "https://example.com/pricing#:~:text=10%20million,-included%20per%20month");
    }

    #[test]
    fn link_has_context_and_escapes_directive_syntax() {
        let url = url::Url::parse("https://example.com/pricing#plans").unwrap();
        let text = "Pro: $8 per user-month, billed yearly";
        let start = text.find("$8").unwrap();
        let l = link(&url, text, &(start..start + 2));
        assert_eq!(l, "https://example.com/pricing#:~:text=Pro%3A-,%248,-per%20user%2Dmonth%2C%20billed");
    }
}

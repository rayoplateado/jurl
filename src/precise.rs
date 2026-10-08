//! `--precise`: the answer itself, not the block it sits in. Candidates are spans of the picked blocks' text,
//! kept as byte ranges, so whatever wins is still the page's own words; Jev only scores them.

use std::ops::Range;

use crate::extract::{Block, Kind};

/// A candidate answer: `blocks[block].text[range]`. A table cell carries its row and column as `label`, which is what
/// Jev is shown ("30 s (CPU time · Paid)"); the answer is still just the cell.
#[derive(Debug, Clone)]
pub struct Span {
    pub block: usize,
    pub range: Range<usize>,
    pub label: Option<String>,
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
    let mut queues: Vec<std::vec::IntoIter<(Range<usize>, Option<String>)>> = blocks
        .iter()
        .map(|b| {
            let mut spans = table_cells(&b.text);
            let plain = if b.kind == Kind::Code { code_spans(&b.text) } else { prose_spans(&b.text) };
            spans.extend(plain.into_iter().map(|r| (r, None)));
            spans.into_iter()
        })
        .collect();
    let mut out: Vec<Span> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut live = true;
    while live && out.len() < MAX_CANDIDATES {
        live = false;
        for (b, queue) in queues.iter_mut().enumerate() {
            let text = blocks[b].text.as_str();
            // The next span of this block that is new and not too long.
            for (r, label) in queue.by_ref() {
                let Some(r) = trim(text, r) else { continue };
                let s = &text[r.clone()];
                if s.chars().count() > MAX_SPAN_CHARS || !seen.insert(label.clone().unwrap_or_else(|| s.to_string())) {
                    continue;
                }
                out.push(Span { block: b, range: r, label });
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

/// The cells of a table written as `| a | b |` lines, each with its row's first cell and its column's header: in a
/// table comparing plans, "10 ms" and "30 s" only mean something next to "Free" and "Paid".
fn table_cells(text: &str) -> Vec<(Range<usize>, Option<String>)> {
    let mut rows: Vec<Vec<Range<usize>>> = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end();
        if body.starts_with('|') && body.ends_with('|') && body.len() > 1 {
            let inner = &body[1..body.len() - 1];
            // `|---|:--:|` is markdown's header separator, not a row.
            if !inner.chars().all(|c| matches!(c, '-' | ':' | '|' | ' ')) {
                let mut cells = Vec::new();
                let mut at = start + 1;
                for cell in inner.split('|') {
                    cells.push(at..at + cell.len());
                    at += cell.len() + 1;
                }
                rows.push(cells);
            }
        }
        start += line.len();
    }
    if rows.len() < 2 {
        return Vec::new();
    }
    let cell = |r: &Range<usize>| text[r.clone()].trim().trim_matches(['*', '_']).to_string();
    let header: Vec<String> = rows[0].iter().map(cell).collect();
    let mut out = Vec::new();
    for row in &rows[1..] {
        let name = row.first().map(cell).unwrap_or_default();
        // The first column names the row, but can be the answer too: in a glossary (`| C9H8O4 | aspirin |`) the
        // formula is the first cell. It's labelled by the cell next to it.
        let next = row.get(1).map(cell).unwrap_or_default();
        for (j, r) in row.iter().enumerate() {
            let value = cell(r);
            if value.is_empty() {
                continue;
            }
            let column = header.get(j).cloned().unwrap_or_default();
            let row_name = if j == 0 { next.as_str() } else { name.as_str() };
            let context = [row_name, column.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>();
            let label = (!context.is_empty()).then(|| format!("{value} ({})", context.join(" · ")));
            out.push((r.clone(), label));
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

    // Lines, clauses, then sentences; before them, the lists inside lines and sentences.
    let lines = split(text, |c, _| c == '\n');
    let sentences =
        split(text, |c, next| matches!(c, '.' | '!' | '?') && next.is_none_or(|n| n.is_whitespace() || n == '['));
    out.extend(lines.iter().chain(&sentences).filter_map(|r| enumeration(text, r.clone())));
    out.extend(link_runs(text));
    out.extend(lines);
    out.extend(split(text, |c, next| matches!(c, ',' | ';' | ':' | '—' | '–') && next.is_none_or(char::is_whitespace)));
    out.extend(sentences);
    out
}

/// The list a line or sentence ends with, after its last colon: "Available in three colours: red, green and blue"
/// → "red, green and blue". Only when what follows splits into two or more items (on `,` `;` `·`, "and", "or"),
/// counted outside brackets, so a link's URL or "(en español)" doesn't split an item.
fn enumeration(text: &str, r: Range<usize>) -> Option<Range<usize>> {
    let s = &text[r.clone()];
    let mut colon = None;
    let mut depth = 0i32;
    let mut cuts = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next = chars.peek().map(|&(_, n)| n);
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ':' if depth == 0 && next.is_some_and(char::is_whitespace) => {
                colon = Some(i + 1);
                cuts = 0;
            }
            ',' | ';' | '·' if depth == 0 && colon.is_some() => cuts += 1,
            _ if depth == 0 && colon.is_some() && c.is_whitespace() => {
                let word = s[i..].split_whitespace().next().unwrap_or("");
                if (word == "and" || word == "or") && s[i + c.len_utf8()..].starts_with(word) {
                    cuts += 1;
                }
            }
            _ => {}
        }
    }
    let colon = colon?;
    // Each cut sits between two items: something on both sides of the last one too.
    (cuts >= 1 && s[colon..].trim().len() > 1).then(|| r.start + colon..r.end)
}

/// Markdown links one after another, separated only by commas, `·`, "and"/"or" or a parenthesis about the one
/// before ("[La Única Verdad](…) (en español), [The Only Truth](…) (in English)"): a list of books or pages.
fn link_runs(text: &str) -> Vec<Range<usize>> {
    let links = md_links(text);
    let mut out = Vec::new();
    let mut first = 0;
    for n in 1..=links.len() {
        let joined = n < links.len() && only_separators(&text[links[n - 1].end..links[n].start]);
        if !joined {
            if n - first > 1 {
                // The run keeps a parenthesis right after its last link: "(in English)".
                let mut end = links[n - 1].end;
                let after = &text[end..];
                let lead = after.len() - after.trim_start_matches([' ', '\t']).len();
                if after[lead..].starts_with('(')
                    && let Some(close) = after[lead..].find(')')
                    && !after[lead..lead + close].contains(['\n', '['])
                {
                    end += lead + close + 1;
                }
                out.push(links[first].start..end);
            }
            first = n;
        }
    }
    out
}

/// `[text](url)` links in `text`, as byte ranges.
fn md_links(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(open) = text[at..].find('[').map(|i| at + i) {
        let Some(mid) = text[open..].find("](").map(|i| open + i) else { break };
        let label = &text[open + 1..mid];
        if label.contains(['[', '\n']) {
            at = open + 1;
            continue;
        }
        let Some(close) = text[mid + 2..].find([')', ' ', '\n']).map(|i| mid + 2 + i) else { break };
        if !text[close..].starts_with(')') {
            at = mid + 2;
            continue;
        }
        out.push(open..close + 1);
        at = close + 1;
    }
    out
}

/// What can sit between two items of one list: commas, `·`, "and"/"or", and a parenthesis without a link in it.
fn only_separators(gap: &str) -> bool {
    let mut rest = gap.trim();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix([',', ';', '·', '|', '/']) {
            rest = r.trim_start();
        } else if let Some(r) = rest.strip_prefix("and ").or_else(|| rest.strip_prefix("or ")) {
            rest = r.trim_start();
        } else if rest.starts_with('(')
            && let Some(close) = rest.find(')')
            && !rest[..close].contains(['[', '\n'])
        {
            rest = rest[close + 1..].trim_start();
        } else {
            return false;
        }
    }
    true
}

/// A winner longer than this many words may carry more than the answer, and gets its own pieces scored. Values,
/// names and short phrases stay whole: "5,000 requests per hour" (4 words), "629.88 K (356.73 °C, 674.11 °F)" (7).
const REFINE_WORDS: usize = 8;
/// At most this many pieces of a winner are scored, in one more call.
pub const MAX_REFINE: usize = 100;

/// Whether the winning span is long enough that a part of it could be the answer: a line, clause or sentence of
/// prose, not a value, a name or a table cell (those are labelled, or short).
pub fn refinable(text: &str, span: &Span) -> bool {
    span.label.is_none() && text[span.range.clone()].split_whitespace().count() > REFINE_WORDS
}

/// Contiguous pieces of `text[range]`, each trimmed like any candidate and never the whole span: first those that
/// start and end at punctuation or a link's edge, never cutting a link or parenthesis ("…: [A](…), [B](…)" → "[A](…)", "[A](…), [B](…)"), then any
/// run of words, longest first. At most `MAX_REFINE`.
pub fn refinements(text: &str, range: &Range<usize>) -> Vec<Range<usize>> {
    let words: Vec<Range<usize>> =
        tokens(&text[range.clone()]).into_iter().map(|t| range.start + t.start..range.start + t.end).collect();
    let n = words.len();
    if n < 2 {
        return Vec::new();
    }
    // Where an item can start: the first word, a word after one ending in punctuation, a link's `[`.
    let opens = |k: usize| {
        k == 0
            || text[words[k].clone()].starts_with('[')
            || text[words[k - 1].clone()].ends_with([',', ';', ':', '.', '·', ')'])
            || text[words[k - 1].clone()] == *"·"
    };
    // Where an item can end: the last word, a word ending in punctuation or closing a link.
    let closes = |k: usize| {
        k + 1 == n
            || text[words[k].clone()].ends_with([',', ';', ':', '.', ')'])
            || text[words[k + 1].clone()] == *"·"
            || text[words[k + 1].clone()].starts_with('(')
    };
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for a in (0..n).filter(|&a| opens(a)) {
        for b in (a..n).filter(|&b| closes(b)) {
            pairs.push((a, b));
        }
    }
    let mut runs: Vec<(usize, usize)> = (0..n).flat_map(|a| (a + 1..n).map(move |b| (a, b))).collect();
    runs.sort_by_key(|&(a, b)| (a as isize - b as isize, a));
    let mut out: Vec<Range<usize>> = Vec::new();
    let whole = trim(text, range.clone());
    for (a, b) in pairs.into_iter().chain(runs) {
        let Some(r) = trim(text, words[a].start..words[b].end) else { continue };
        if Some(&r) == whole.as_ref()
            || out.contains(&r)
            || text[r.clone()].chars().count() > MAX_SPAN_CHARS
            || !balanced(&text[r.clone()])
        {
            continue;
        }
        out.push(r);
        if out.len() >= MAX_REFINE {
            break;
        }
    }
    out
}

/// Every bracket closed, in order: a piece never cuts a link or a parenthesis in half.
fn balanced(s: &str) -> bool {
    let mut open = Vec::new();
    for c in s.chars() {
        match c {
            '(' | '[' => open.push(c),
            ')' if open.pop() != Some('(') => return false,
            ']' if open.pop() != Some('[') => return false,
            _ => {}
        }
    }
    open.is_empty()
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
        Block { i: 0, kind, level: None, lang: None, text: text.to_string(), list: None }
    }

    #[test]
    fn table_cells_know_their_row_and_column() {
        let b = block(Kind::Table, "| Limit | Free | Paid |\n| --- | --- | --- |\n| **CPU time** | 10 ms | 30 s |");
        let spans = candidates(&[&b]);
        let paid = spans.iter().find(|s| &b.text[s.range.clone()] == "30 s").expect("the cell is a candidate");
        assert_eq!(paid.label.as_deref(), Some("30 s (CPU time · Paid)"));
        let free = spans.iter().find(|s| s.label.as_deref() == Some("10 ms (CPU time · Free)"));
        assert!(free.is_some());
    }

    #[test]
    fn the_first_column_can_be_the_answer() {
        let b = block(Kind::Table, "| Formula | Synonyms |\n| --- | --- |\n| C9H8O4 | acetylsalicylic acid |");
        let spans = candidates(&[&b]);
        let formula = spans.iter().find(|s| s.label.as_deref() == Some("C9H8O4 (acetylsalicylic acid · Formula)"));
        assert!(formula.is_some(), "{spans:?}");
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

    const FOOTER: &str = "Made with love in Murcia by Ray García. jurl can't make things up. Ray can: he saves it for his \
        novels: [La Única Verdad](https://www.amazon.es/dp/B08LQYLVLK) (en español), [La Única Realidad](https://www.amazon.es/dp/B0BMJH7MF6) \
        (en español), [The Only Truth](https://www.amazon.com/dp/B0C8TSVW7F) (in English).";

    #[test]
    fn the_list_after_a_colon() {
        let t = texts(&block(Kind::Para, FOOTER));
        let books = "[La Única Verdad](https://www.amazon.es/dp/B08LQYLVLK) (en español), [La Única Realidad](https://www.amazon.es/dp/B0BMJH7MF6) \
            (en español), [The Only Truth](https://www.amazon.com/dp/B0C8TSVW7F) (in English)";
        assert!(t.contains(&books.to_string()), "{t:?}");
        // Only after the last colon: "he saves it for his novels: …" isn't the list.
        assert!(!t.iter().any(|s| s.starts_with("he saves it for his novels: [")), "{t:?}");
        let t = texts(&block(Kind::Para, "Available in three colours: red, green and blue. Ships in a week."));
        assert!(t.contains(&"red, green and blue".to_string()), "{t:?}");
        // One thing after a colon isn't a list.
        let t = texts(&block(Kind::Para, "Founded: San Francisco."));
        assert_eq!(t.iter().filter(|s| s.as_str() == "San Francisco").count(), 1, "{t:?}");
    }

    #[test]
    fn links_in_a_row() {
        let text =
            "[Blog](https://b.example) · [X](https://x.com/r) · [LinkedIn](https://linkedin.com/in/r) · r@example.com";
        let t = texts(&block(Kind::Para, text));
        assert!(
            t.contains(
                &"[Blog](https://b.example) · [X](https://x.com/r) · [LinkedIn](https://linkedin.com/in/r)".to_string()
            ),
            "{t:?}"
        );
        // A list ending on a bare link keeps the link's closing parenthesis.
        let t = texts(&block(Kind::Para, "Read more: [one](https://a.example/1), [two](https://a.example/2)."));
        assert!(t.contains(&"[one](https://a.example/1), [two](https://a.example/2)".to_string()), "{t:?}");
        // Links with words between them are not one list.
        let t =
            texts(&block(Kind::Para, "See [one](https://a.example/1) for setup and the [guide](https://a.example/2)."));
        assert!(!t.iter().any(|s| s.starts_with("[one]") && s.ends_with("2)")), "{t:?}");
    }

    #[test]
    fn short_winners_stay_whole() {
        for text in ["5,000 requests per hour", "629.88 K (356.73 °C, 674.11 °F)", "count back from the last item"] {
            let span = Span { block: 0, range: 0..text.len(), label: None };
            assert!(!refinable(text, &span), "{text}");
        }
        let text = "| Limit | Free | Paid |";
        let cell = Span { block: 0, range: 0..text.len(), label: Some("x".into()) };
        assert!(!refinable(text, &cell));
        assert!(refinable(FOOTER, &Span { block: 0, range: 0..FOOTER.len(), label: None }));
    }

    #[test]
    fn pieces_of_a_long_winner() {
        let start = FOOTER.find("Ray can").unwrap();
        let r = start..FOOTER.len();
        let pieces: Vec<&str> = refinements(FOOTER, &r).iter().map(|p| &FOOTER[p.clone()]).collect();
        assert!(pieces.len() <= MAX_REFINE);
        // The list, each book, and never the whole span; every piece is the page's own text.
        let books = &FOOTER[FOOTER.find("[La Única Verdad]").unwrap()..FOOTER.rfind('.').unwrap()];
        assert!(pieces.contains(&books), "{pieces:?}");
        assert!(pieces.contains(&"[The Only Truth](https://www.amazon.com/dp/B0C8TSVW7F) (in English)"), "{pieces:?}");
        assert!(pieces.contains(&"[La Única Verdad](https://www.amazon.es/dp/B08LQYLVLK)"), "{pieces:?}");
        assert!(!pieces.contains(&&FOOTER[trim(FOOTER, r.clone()).unwrap()]), "{pieces:?}");
        // Punctuation-aligned pieces come first.
        let first = pieces.iter().position(|p| *p == books).unwrap();
        assert!(first < 30, "{first}: {pieces:?}");
        for p in refinements(FOOTER, &r) {
            assert!(FOOTER.get(p.clone()).is_some() && p.start >= r.start && p.end <= r.end, "{p:?}");
            // A link's markdown is never cut in half.
            let s = &FOOTER[p];
            assert_eq!(s.matches('(').count(), s.matches(')').count(), "{s}");
        }
        assert!(refinements("one", &(0..3)).is_empty());
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

//! The markdown path: a page the server already sent as markdown, split into blocks on blank lines, with fenced code
//! kept whole. Links and images are read from the markdown syntax itself. Every other block's text is the words the
//! page shows (`words`), not its markdown, so a precise answer and its link are the page's own text.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use scraper::Html;
use url::Url;

use super::join::join_short;
use super::{BREAK, Block, Extracted, Kind, collapse, push_image, push_link};

/// What a page's markdown is read with: CommonMark, and the GitHub extensions docs use (tables, footnotes,
/// strikethrough, task lists, and the `> [!NOTE]` alerts that GFM adds to blockquotes).
const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_GFM);

/// Server already sent markdown (`Accept: text/markdown`). Split on blank lines,
/// keeping fenced code intact.
pub fn markdown(body: &str, base: &Url) -> Extracted {
    let (mut title, body) = frontmatter(body);
    let mut blocks = Vec::new();
    let mut images = Vec::new();
    let mut links = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut fence: Option<String> = None;
    let mut lists = 0usize;

    for line in body.lines() {
        let t = line.trim_start();
        if let Some(f) = &fence {
            cur.push(line);
            if closes_fence(t, f) {
                fence = None;
                push_chunk(&mut cur, &mut blocks, &mut lists);
            }
            continue;
        }
        if opens_fence(t) {
            push_chunk(&mut cur, &mut blocks, &mut lists);
            fence = Some(fence_run(t));
            cur.push(line);
        } else if t.is_empty() || t.starts_with('#') {
            push_chunk(&mut cur, &mut blocks, &mut lists);
            if !t.is_empty() {
                cur.push(line);
                push_chunk(&mut cur, &mut blocks, &mut lists);
            }
        } else {
            cur.push(line);
        }
        for (text, href) in md_links(line) {
            let context = if line.trim() == format!("[{text}]({href})") {
                String::new()
            } else {
                collapse(line).chars().take(200).collect()
            };
            push_link(&mut links, base, href, collapse(text), context, false);
        }
        for (alt, src) in md_images(line) {
            push_image(&mut images, base, src, None, collapse(alt), String::new(), None, None);
        }
    }
    push_chunk(&mut cur, &mut blocks, &mut lists);
    if title.is_empty()
        && let Some(h) = blocks.iter().find(|b| b.kind == Kind::Heading)
    {
        title = h.text.clone();
    }
    let site_links = links.clone();
    Extracted { title, blocks: join_short(blocks), structured: Vec::new(), images, links, site_links, app_shell: false }
}

/// YAML frontmatter: its `title:` line is the title, and the body after the closing `---` is the content. Only the
/// closing line goes: the body after it stays whole, a leading "- " bullet included.
fn frontmatter(body: &str) -> (String, &str) {
    let mut title = String::new();
    if let Some(rest) = body.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---")
    {
        for line in rest[..end].lines() {
            if let Some(v) = line.strip_prefix("title:") {
                title = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
            }
        }
        return (title, rest[end + 4..].split_once('\n').map_or("", |(_, after)| after));
    }
    (title, body)
}

/// Turns the lines in `cur` into blocks (a heading, a fenced code block, or the paragraphs, quotes and list items of
/// the rest), and empties `cur`. A block's text is its words; one with no words, like an image alone, is no block.
/// `lists` counts the page's lists, so that the items of one list share a `list`.
fn push_chunk(cur: &mut Vec<&str>, blocks: &mut Vec<Block>, lists: &mut usize) {
    let text = cur.join("\n").trim().to_string();
    cur.clear();
    if text.is_empty() {
        return;
    }
    if let Some(rest) = text.strip_prefix('#') {
        // Six levels, as in HTML: a longer run of #s is still a heading, at level 6.
        let level = (1 + rest.chars().take_while(|&c| c == '#').count()).min(6) as u8;
        let plain = words(rest.trim_start_matches('#').trim());
        if !plain.is_empty() {
            blocks.push(Block { level: Some(level), ..Block::new(blocks.len(), Kind::Heading, plain) });
        }
    } else if opens_fence(&text) {
        blocks.push(fenced_block(blocks.len(), &text));
    } else {
        // The chunk is read with its `>`s, so the parser sees a `> [!NOTE]` alert and takes its marker out.
        for part in parts(&text, lists) {
            blocks.push(Block { list: part.list, ..Block::new(blocks.len(), part.kind, part.text) });
        }
    }
}

/// One block read from a chunk of markdown: its kind, its words and, for a list item, the list it is in.
struct Part {
    kind: Kind,
    text: String,
    list: Option<usize>,
}

/// The blocks of a chunk of markdown, in page order: paragraphs, quotes and list items. An item's own words come before
/// the items of its nested list, and a list inside a quote is flattened into the quote, as the HTML path reads `<li>`
/// and `<blockquote>`. `lists` counts the page's lists; the lists read here take the next numbers.
fn parts(md: &str, lists: &mut usize) -> Vec<Part> {
    let mut out = Vec::new();
    let mut cur: Option<Part> = None;
    let mut open_lists: Vec<usize> = Vec::new();
    let (mut quotes, mut items, mut image) = (0usize, 0usize, 0usize);
    for event in Parser::new_ext(md, OPTIONS) {
        match &event {
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut cur, &mut out);
                quotes += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                quotes = quotes.saturating_sub(1);
                if quotes == 0 {
                    flush(&mut cur, &mut out);
                }
            }
            Event::Start(Tag::List(_)) if quotes == 0 => {
                // A nested list's items come after the words of the item it is in.
                flush(&mut cur, &mut out);
                *lists += 1;
                open_lists.push(*lists);
            }
            Event::End(TagEnd::List(_)) if quotes == 0 => {
                flush(&mut cur, &mut out);
                open_lists.pop();
            }
            Event::Start(Tag::Item) if quotes == 0 => {
                flush(&mut cur, &mut out);
                items += 1;
                cur = Some(Part { kind: Kind::Item, text: String::new(), list: open_lists.last().copied() });
            }
            Event::End(TagEnd::Item) if quotes == 0 => {
                flush(&mut cur, &mut out);
                items = items.saturating_sub(1);
            }
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_)) if quotes == 0 && items == 0 => {
                flush(&mut cur, &mut out);
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock) if quotes == 0 && items == 0 => {
                flush(&mut cur, &mut out);
            }
            _ => {}
        }
        // The event's words go to the part being read, which is started here when there is none.
        let (kind, list) = if quotes > 0 {
            (Kind::Quote, None)
        } else if items > 0 {
            (Kind::Item, open_lists.last().copied())
        } else {
            (Kind::Para, None)
        };
        let part = cur.get_or_insert(Part { kind, text: String::new(), list });
        push_event(event, &mut part.text, &mut image, true);
    }
    flush(&mut cur, &mut out);
    out
}

/// Ends the part being read: its words, collapsed, if it has any.
fn flush(cur: &mut Option<Part>, out: &mut Vec<Part>) {
    if let Some(mut part) = cur.take() {
        part.text = collapse(&part.text);
        if !part.text.is_empty() {
            out.push(part);
        }
    }
}

/// The words a chunk of markdown shows on the page, as a browser draws it. Emphasis, link, escape and code markers are
/// gone, and so is an image (its alt text is not on the page). Tags of inline HTML go, and their text stays; a `<br>`
/// is a line break. A block of raw HTML is its text, line by line, and that text is markdown still: a `<details>`
/// holds "**bold**" that the site shows bold. A paragraph, heading, list item, quote or code line is a line of its own,
/// with no bullet or `>` in front. A table row keeps its `|`s, which `precise` reads its cells by. A footnote mark
/// reads as "[1]".
fn words(md: &str) -> String {
    let mut out = String::new();
    push_events(md, &mut out, true);
    collapse(&out)
}

/// `words`, appended to `out`: the words of every event of `md`, a block boundary as a BREAK. Raw HTML is read as its
/// text, a line at a time, and that text is read as markdown too, once: with `html` false, the raw HTML inside it is
/// dropped, so a hostile page can't nest the reading without end.
fn push_events(md: &str, out: &mut String, html: bool) {
    let mut image = 0usize;
    for event in Parser::new_ext(md, OPTIONS) {
        push_event(event, out, &mut image, html);
    }
}

/// What one event shows on the page, appended to `out`; `image` counts the images it is inside, whose alt text is not
/// on the page.
fn push_event(event: Event, out: &mut String, image: &mut usize, html: bool) {
    match event {
        Event::Text(t) | Event::Code(t) if *image == 0 => out.push_str(&t),
        // Read as text, so a comment's words stay out; then line by line, as the words are laid out.
        Event::Html(raw) if html => {
            for line in Html::parse_fragment(&raw).root_element().text().collect::<String>().lines() {
                out.push(BREAK);
                push_events(line, out, false);
                out.push(BREAK);
            }
        }
        Event::SoftBreak => out.push(' '),
        Event::HardBreak => out.push(BREAK),
        Event::FootnoteReference(label) => {
            out.push('[');
            out.push_str(&label);
            out.push(']');
        }
        Event::InlineHtml(tag) if tag.get(..3).is_some_and(|t| t.eq_ignore_ascii_case("<br")) => out.push(BREAK),
        Event::Start(Tag::Image { .. }) => *image += 1,
        Event::End(TagEnd::Image) => *image = image.saturating_sub(1),
        Event::Start(Tag::TableCell) => out.push_str("| "),
        Event::End(TagEnd::TableCell) => out.push(' '),
        Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
            out.push('|');
            out.push(BREAK);
        }
        Event::End(
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::BlockQuote(_) | TagEnd::CodeBlock,
        ) => out.push(BREAK),
        _ => {}
    }
}

/// A fenced code block, from its opening line on: the info string after the fence is its language.
fn fenced_block(i: usize, text: &str) -> Block {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let fence = fence_run(first);
    let lang = first.trim_matches(|c| c == '`' || c == '~').trim().to_string();
    let mut body: Vec<_> = lines.collect();
    // Only a line that closes the fence goes: a fence still open at the end of input keeps its last line.
    if body.last().is_some_and(|l| closes_fence(l.trim_start(), &fence)) {
        body.pop();
    }
    Block { lang: Some(lang).filter(|l| !l.is_empty()), ..Block::new(i, Kind::Code, body.join("\n")) }
}

/// Whether a line opens a fenced code block.
fn opens_fence(line: &str) -> bool {
    line.starts_with("```") || line.starts_with("~~~")
}

/// The run of backticks or tildes that starts a line: the fence it opens, which a closing fence must match in length.
fn fence_run(line: &str) -> String {
    let mark = line.chars().next().unwrap_or('`');
    line.chars().take_while(|&x| x == mark).collect()
}

/// A fence closes on a line of the same character, at least as long, and nothing else: a ```` block can show
/// a ```js block inside it, and "```js" never closes anything.
fn closes_fence(line: &str, fence: &str) -> bool {
    let Some(c) = fence.chars().next() else { return false };
    let run = line.chars().take_while(|&x| x == c).count();
    run >= fence.chars().count() && line[run * c.len_utf8()..].trim().is_empty()
}

/// `[text](url)` that is not an image.
fn md_links(line: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(off) = line[pos..].find('[') {
        let start = pos + off;
        pos = start + 1;
        if start > 0 && line.as_bytes()[start - 1] == b'!' {
            continue;
        }
        let Some(close) = line[start..].find("](") else { break };
        let text = &line[start + 1..start + close];
        if text.contains('[') {
            continue;
        }
        let rest = &line[start + close + 2..];
        let Some(end) = rest.find(')') else { break };
        let href = rest[..end].split_whitespace().next().unwrap_or("");
        out.push((text, href));
        pos = start + close + 2 + end;
    }
    out
}

fn md_images(line: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("![") {
        rest = &rest[start + 2..];
        let Some(close) = rest.find("](") else { break };
        let alt = &rest[..close];
        rest = &rest[close + 2..];
        let Some(end) = rest.find(')') else { break };
        let src = rest[..end].split_whitespace().next().unwrap_or("");
        out.push((alt, src));
        rest = &rest[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/post/").unwrap()
    }

    #[test]
    fn links_skip_images_fragments_and_self() {
        let md = "See [the docs](/docs#intro), ![pic](a.png), [top](#top) and [here](https://example.com/post/).";
        let ex = markdown(md, &base());
        assert_eq!(ex.links.len(), 1);
        assert_eq!(ex.links[0].url.as_str(), "https://example.com/docs");
        assert_eq!(ex.links[0].text, "the docs");
    }

    #[test]
    fn markdown_code_keeps_its_lines() {
        let md = "Intro\n\n```js\nuseEffect(() => {\n  const c = connect();\n\n  return () => {\n    c.disconnect();\n  };\n}, []);\n```\n";
        let ex = markdown(md, &base());
        let code = ex.blocks.iter().find(|b| b.kind == Kind::Code).unwrap();
        assert_eq!(
            code.text,
            "useEffect(() => {\n  const c = connect();\n\n  return () => {\n    c.disconnect();\n  };\n}, []);"
        );
        assert_eq!(code.lang.as_deref(), Some("js"));
    }

    #[test]
    fn markdown_fence_closes_on_its_own_length() {
        let md = "Intro\n\n````\n$ jurl -q x\n```js\nlet a = 1;\n```\n````\n\nAfter: red, green and blue.\n\n```\n$ ls\n```\n\nThe end.\n";
        let ex = markdown(md, &base());
        let code: Vec<&Block> = ex.blocks.iter().filter(|b| b.kind == Kind::Code).collect();
        assert_eq!(code.len(), 2, "{:?}", ex.blocks);
        assert_eq!(code[0].text, "$ jurl -q x\n```js\nlet a = 1;\n```");
        assert_eq!(code[1].text, "$ ls");
        assert!(code[0].markdown().starts_with("````\n") && code[0].markdown().ends_with("\n````"));
        assert_eq!(code[1].markdown(), "```\n$ ls\n```");
        assert!(
            ex.blocks.iter().any(|b| b.kind == Kind::Para && b.text == "After: red, green and blue."),
            "{:?}",
            ex.blocks
        );
        assert!(ex.blocks.iter().any(|b| b.kind == Kind::Para && b.text == "The end."), "{:?}", ex.blocks);
    }

    #[test]
    fn an_unclosed_fence_keeps_its_last_line() {
        let code = |md: &str| -> Vec<String> {
            markdown(md, &base()).blocks.into_iter().filter(|b| b.kind == Kind::Code).map(|b| b.text).collect()
        };
        assert_eq!(code("```\nline1\nline2\n"), ["line1\nline2"]);
        // "```js" only starts like a closing fence, so it is code too.
        assert_eq!(code("```\nline1\n```js\n"), ["line1\n```js"]);
    }

    #[test]
    fn frontmatter_ends_at_its_own_delimiter() {
        let ex = markdown("---\ntitle: Pricing\n---\n- Free plan\n- Pro plan", &base());
        assert_eq!(ex.title, "Pricing");
        let texts: Vec<_> = ex.blocks.iter().map(|b| b.text.as_str()).collect();
        // Two short items join into one paragraph, a line each, as the HTML path joins short items: no bullets. The
        // joined block still belongs to their list.
        assert_eq!(texts, ["Free plan\nPro plan"]);
        assert_eq!((ex.blocks[0].kind, ex.blocks[0].list.is_some()), (Kind::Para, true));
    }

    #[test]
    fn list_items_are_items_and_print_their_bullets() {
        let md = "- Install the command-line tool from the downloads page\n- Then run the setup once to finish";
        let ex = markdown(md, &base());
        let got: Vec<_> = ex.blocks.iter().map(|b| (b.kind, b.list, b.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (Kind::Item, Some(1), "Install the command-line tool from the downloads page"),
                (Kind::Item, Some(1), "Then run the setup once to finish"),
            ]
        );
        assert_eq!(ex.blocks[0].markdown(), "- Install the command-line tool from the downloads page");
    }

    #[test]
    fn a_nested_list_comes_after_its_items_own_words() {
        let md = "- Install the command-line tool from the downloads page\n  - Use the flags below to pick a version\n- Then run the setup once to finish";
        let ex = markdown(md, &base());
        let got: Vec<_> = ex.blocks.iter().map(|b| (b.kind, b.list, b.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (Kind::Item, Some(1), "Install the command-line tool from the downloads page"),
                (Kind::Item, Some(2), "Use the flags below to pick a version"),
                (Kind::Item, Some(1), "Then run the setup once to finish"),
            ]
        );
    }

    #[test]
    fn a_paragraph_before_a_list_in_the_same_chunk_is_its_own_block() {
        let md = "Install with one of these:\n- the command-line tool from the downloads page\n- the desktop app from the website";
        let kinds: Vec<_> = markdown(md, &base()).blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, [Kind::Para, Kind::Item, Kind::Item]);
    }

    #[test]
    fn a_link_in_an_item_is_its_words() {
        let ex = markdown("- See [the setup guide](https://example.com/setup) for every flag", &base());
        assert_eq!((ex.blocks[0].kind, ex.blocks[0].text.as_str()), (Kind::Item, "See the setup guide for every flag"));
    }

    #[test]
    fn a_list_in_a_quote_is_flattened_into_the_quote() {
        let md = "> - the first note is long enough to stand alone\n> - the second note is long enough too";
        let ex = markdown(md, &base());
        assert_eq!((ex.blocks.len(), ex.blocks[0].kind), (1, Kind::Quote));
        assert_eq!(
            ex.blocks[0].text,
            "the first note is long enough to stand alone\nthe second note is long enough too"
        );
    }

    #[test]
    fn a_seventh_hash_is_a_level_six_heading() {
        let ex = markdown("####### Deep\n", &base());
        assert_eq!((ex.blocks[0].kind, ex.blocks[0].level), (Kind::Heading, Some(6)));
        assert_eq!(ex.blocks[0].text, "Deep");
    }

    #[test]
    fn a_run_of_hashes_longer_than_a_byte_is_not_an_overflow() {
        let ex = markdown(&format!("{} Deep\n", "#".repeat(256)), &base());
        assert_eq!((ex.blocks[0].kind, ex.blocks[0].level), (Kind::Heading, Some(6)));
        assert_eq!(ex.blocks[0].text, "Deep");
    }

    #[test]
    fn markdown_frontmatter_and_empty_images() {
        let md =
            "---\ntitle: Hello\n---\n\n# Heading\n\nSome text ![]() and ![pic](img.png)\n\n```rust\nfn x() {}\n```\n";
        let ex = markdown(md, &base());
        assert_eq!(ex.title, "Hello");
        assert_eq!(ex.images.len(), 1);
        assert!(ex.blocks.iter().any(|b| b.kind == Kind::Code && b.text == "fn x() {}"));
        assert!(ex.blocks.iter().all(|b| !b.text.contains("title:")));
    }

    #[test]
    fn a_paragraph_and_a_heading_are_the_words_the_page_shows() {
        // Emphasis, link, escape and entity markup goes; an image's alt text is not on the page.
        let md = "**Vercel has a SOC 2 Type 2 attestation**.\n\nAvailable to workspaces on our [Enterprise](https://linear.app/pricing) plan ![logo](logo.png) with `code` and \\*escaped\\* &amp; text.\n\n## **Pricing** [beta](/beta)";
        let texts: Vec<_> = markdown(md, &base()).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(
            texts,
            [
                "Vercel has a SOC 2 Type 2 attestation.",
                "Available to workspaces on our Enterprise plan with code and *escaped* & text.",
                "Pricing beta",
            ]
        );
    }

    #[test]
    fn a_quote_is_read_with_its_markers_so_an_alert_takes_its_own_out() {
        let md = "> [!NOTE]\n> Available to workspaces on our [Enterprise](https://linear.app/pricing) plan\n";
        let ex = markdown(md, &base());
        assert_eq!(ex.blocks.len(), 1, "{:?}", ex.blocks);
        assert_eq!(ex.blocks[0].kind, Kind::Quote);
        assert_eq!(ex.blocks[0].text, "Available to workspaces on our Enterprise plan");
    }

    #[test]
    fn a_table_keeps_its_pipes_so_its_cells_stay_cells() {
        let md = "| **Limit** | Free | Paid |\n| --- | --- | --- |\n| **CPU time** | 10 ms | 30 s |";
        let ex = markdown(md, &base());
        assert_eq!(ex.blocks[0].text, "| Limit | Free | Paid |\n| CPU time | 10 ms | 30 s |");
    }

    #[test]
    fn an_image_alone_is_no_block_and_a_br_breaks_the_line() {
        // A footnote definition reads as its text, without its "[^1]:" label.
        let md = "![logo](logo.png)\n\nLine one<br>Line two\n\n[^1]: The note is long enough to stand on its own.";
        let texts: Vec<_> = markdown(md, &base()).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["Line one\nLine two", "The note is long enough to stand on its own."]);
    }

    #[test]
    fn fenced_code_is_its_text_verbatim() {
        let md = "```sh\n$ brew install ripgrep\n**not bold** [x](y) &amp;\n```";
        let ex = markdown(md, &base());
        assert_eq!(ex.blocks.len(), 1, "{:?}", ex.blocks);
        assert_eq!(ex.blocks[0].kind, Kind::Code);
        assert_eq!(ex.blocks[0].text, "$ brew install ripgrep\n**not bold** [x](y) &amp;");
    }

    #[test]
    fn raw_html_is_its_text_and_the_markdown_in_it_is_still_markdown() {
        // A `<details>` with no blank line after its summary is one HTML block: the site shows its "**this**" bold, and
        // a comment's words are not on the page.
        let md = "<!-- TODO: hide -->\n<details>\n<summary>Question?</summary>\nSee **this** and [that](https://x.example).\n</details>";
        let texts: Vec<_> = markdown(md, &base()).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["Question?\nSee this and that."]);
    }
}

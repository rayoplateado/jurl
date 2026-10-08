//! The markdown path: a page the server already sent as markdown, split into blocks on blank lines, with fenced code
//! kept whole. Links and images are read from the markdown syntax itself.

use url::Url;

use super::join::join_short;
use super::{Block, Extracted, Kind, collapse, push_image, push_link};

/// Server already sent markdown (`Accept: text/markdown`). Split on blank lines,
/// keeping fenced code intact.
pub fn markdown(body: &str, base: &Url) -> Extracted {
    let (mut title, body) = frontmatter(body);
    let mut blocks = Vec::new();
    let mut images = Vec::new();
    let mut links = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut fence: Option<String> = None;

    for line in body.lines() {
        let t = line.trim_start();
        if let Some(f) = &fence {
            cur.push(line);
            if closes_fence(t, f) {
                fence = None;
                push_chunk(&mut cur, &mut blocks);
            }
            continue;
        }
        if opens_fence(t) {
            push_chunk(&mut cur, &mut blocks);
            fence = Some(fence_run(t));
            cur.push(line);
        } else if t.is_empty() || t.starts_with('#') {
            push_chunk(&mut cur, &mut blocks);
            if !t.is_empty() {
                cur.push(line);
                push_chunk(&mut cur, &mut blocks);
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
    push_chunk(&mut cur, &mut blocks);
    if title.is_empty()
        && let Some(h) = blocks.iter().find(|b| b.kind == Kind::Heading)
    {
        title = h.text.clone();
    }
    let site_links = links.clone();
    Extracted { title, blocks: join_short(blocks), images, links, site_links, app_shell: false }
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

/// Turns the lines in `cur` into one block (a heading, a fenced code block, a quote or a paragraph), and empties `cur`.
fn push_chunk(cur: &mut Vec<&str>, blocks: &mut Vec<Block>) {
    let text = cur.join("\n").trim().to_string();
    cur.clear();
    if text.is_empty() {
        return;
    }
    let i = blocks.len();
    if let Some(rest) = text.strip_prefix('#') {
        // Six levels, as in HTML: a longer run of #s is still a heading, at level 6.
        let level = (1 + rest.chars().take_while(|&c| c == '#').count()).min(6) as u8;
        let text = rest.trim_start_matches('#').trim().to_string();
        blocks.push(Block { level: Some(level), ..Block::new(i, Kind::Heading, text) });
    } else if opens_fence(&text) {
        blocks.push(fenced_block(i, &text));
    } else if text.starts_with('>') {
        let t = text.lines().map(|l| l.trim_start_matches('>').trim()).collect::<Vec<_>>().join("\n");
        blocks.push(Block::new(i, Kind::Quote, t));
    } else {
        blocks.push(Block::new(i, Kind::Para, text));
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
        assert_eq!(texts, ["- Free plan\n- Pro plan"]);
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
}

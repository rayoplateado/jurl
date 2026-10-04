//! HTML/markdown → candidate blocks and images. Every emitted string is text that
//! exists in the page; nothing here rewrites content beyond whitespace collapsing.

use scraper::{ElementRef, Html, Node, Selector};
use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Heading,
    Para,
    Code,
    Quote,
    Item,
    Table,
}

#[derive(Debug, Clone, Serialize)]
pub struct Block {
    pub i: usize,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub text: String,
}

impl Block {
    pub fn markdown(&self) -> String {
        match self.kind {
            Kind::Heading => format!("{} {}", "#".repeat(self.level.unwrap_or(2) as usize), self.text),
            Kind::Code => format!("```{}\n{}\n```", self.lang.as_deref().unwrap_or(""), self.text.trim_end()),
            Kind::Quote => self.text.lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n"),
            Kind::Item => format!("- {}", self.text),
            Kind::Para | Kind::Table => self.text.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Image {
    pub i: usize,
    pub url: Url,
    pub alt: String,
    pub caption: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub i: usize,
    pub url: Url,
    pub text: String,
    pub context: String,
}

pub struct Extracted {
    pub title: String,
    pub blocks: Vec<Block>,
    pub images: Vec<Image>,
    pub links: Vec<Link>,
}

const SKIP: &[&str] = &[
    "script", "style", "noscript", "nav", "footer", "aside", "form", "svg", "button", "iframe",
    "template", "select", "input", "textarea", "dialog", "canvas", "video", "audio", "object",
];
const INLINE: &[&str] = &[
    "a", "span", "em", "strong", "b", "i", "code", "small", "sup", "sub", "abbr", "mark", "u",
    "s", "del", "ins", "time", "br", "kbd", "q", "cite", "label", "var", "samp", "dfn", "bdi",
    "wbr", "font", "img", "picture", "data",
];

pub fn html(body: &str, base: &Url) -> Extracted {
    let doc = Html::parse_document(body);
    let title = meta(&doc, "meta[property='og:title']")
        .or_else(|| first_text(&doc, "title"))
        .or_else(|| first_text(&doc, "h1"))
        .unwrap_or_default();

    // Several <article>s means a listing of cards: the container is the content.
    let order: &[&str] = if doc.select(&sel("article")).nth(1).is_some() {
        &["main", "[role=main]", "body"]
    } else {
        &["article", "main", "[role=main]", "body"]
    };
    let root = order
        .iter()
        .find_map(|s| doc.select(&sel(s)).max_by_key(|e| e.text().map(str::len).sum::<usize>()))
        .unwrap_or_else(|| doc.root_element());
    let in_body = root.value().name() == "body";

    let mut w = Walker { blocks: Vec::new(), skip_header: in_body, buf: String::new() };
    w.container(root);
    w.flush();

    let mut images = Vec::new();
    if let Some(og) = meta(&doc, "meta[property='og:image']") {
        push_image(&mut images, base, &og, String::new(), String::new(), None, None);
    }
    for img in root.select(&sel("img")) {
        if hidden(img) || has_skipped_ancestor(img, in_body) {
            continue;
        }
        let a = |k| img.value().attr(k);
        let src = a("srcset")
            .or(a("data-srcset"))
            .and_then(best_srcset)
            .or(a("data-src").or(a("data-lazy-src")).or(a("data-original")).or(a("src")).map(String::from));
        let Some(src) = src else { continue };
        let alt = collapse(a("alt").or(a("title")).unwrap_or(""));
        let caption = figcaption(img);
        let dim = |k| a(k).and_then(|v: &str| v.trim_end_matches("px").parse().ok());
        push_image(&mut images, base, &src, alt, caption, dim("width"), dim("height"));
    }

    let mut links = Vec::new();
    for a in root.select(&sel("a[href]")) {
        if hidden(a) || has_skipped_ancestor(a, in_body) {
            continue;
        }
        let v = a.value();
        let mut text = collapse(&a.text().collect::<String>());
        if text.is_empty() {
            text = v
                .attr("aria-label")
                .or(v.attr("title"))
                .or_else(|| a.select(&sel("img")).next().and_then(|i| i.value().attr("alt")))
                .map(collapse)
                .unwrap_or_default();
        }
        let context = a
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|e| matches!(e.value().name(), "p" | "li" | "td" | "dd" | "blockquote" | "figcaption" | "h1" | "h2" | "h3" | "h4"))
            .map(|e| collapse(&inline_text(e)))
            .filter(|c| *c != text)
            .map(|c| c.chars().take(200).collect())
            .unwrap_or_default();
        push_link(&mut links, base, v.attr("href").unwrap_or(""), text, context);
    }

    Extracted { title: collapse(&title), blocks: w.blocks, images, links }
}

/// Server already sent markdown (`Accept: text/markdown`). Split on blank lines,
/// keeping fenced code intact.
pub fn markdown(body: &str, base: &Url) -> Extracted {
    let mut blocks = Vec::new();
    let mut images = Vec::new();
    let mut links = Vec::new();
    let mut title = String::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut fence: Option<String> = None;

    let flush = |cur: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        let text = cur.join("\n").trim().to_string();
        cur.clear();
        if text.is_empty() {
            return;
        }
        let i = blocks.len();
        let (kind, level, text) = if let Some(rest) = text.strip_prefix('#') {
            let level = 1 + rest.chars().take_while(|&c| c == '#').count() as u8;
            (Kind::Heading, Some(level), rest.trim_start_matches('#').trim().to_string())
        } else if text.starts_with("```") || text.starts_with("~~~") {
            let mut lines = text.lines();
            let lang = lines.next().unwrap_or("").trim_matches(|c| c == '`' || c == '~').trim().to_string();
            let body: Vec<_> = lines.collect();
            let body = body[..body.len().saturating_sub(1)].join("\n");
            blocks.push(Block { i, kind: Kind::Code, level: None, lang: Some(lang).filter(|l| !l.is_empty()), text: body });
            return;
        } else if text.starts_with('>') {
            let t = text.lines().map(|l| l.trim_start_matches('>').trim()).collect::<Vec<_>>().join("\n");
            (Kind::Quote, None, t)
        } else {
            (Kind::Para, None, text)
        };
        blocks.push(Block { i, kind, level, lang: None, text });
    };

    // YAML frontmatter: take the title, don't treat it as content.
    let mut body = body;
    if let Some(rest) = body.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                if let Some(v) = line.strip_prefix("title:") {
                    title = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
                }
            }
            body = rest[end + 4..].trim_start_matches(['-', '\n']);
        }
    }

    for line in body.lines() {
        let t = line.trim_start();
        if let Some(f) = &fence {
            cur.push(line);
            if t.starts_with(f.as_str()) {
                fence = None;
                flush(&mut cur, &mut blocks);
            }
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            flush(&mut cur, &mut blocks);
            fence = Some(t[..3].to_string());
            cur.push(line);
        } else if t.is_empty() || t.starts_with('#') {
            flush(&mut cur, &mut blocks);
            if !t.is_empty() {
                cur.push(line);
                flush(&mut cur, &mut blocks);
            }
        } else {
            cur.push(line);
        }
        for (text, href) in md_links(line) {
            let context = if line.trim() == format!("[{text}]({href})") { String::new() } else { collapse(line).chars().take(200).collect() };
            push_link(&mut links, base, href, collapse(text), context);
        }
        for (alt, src) in md_images(line) {
            push_image(&mut images, base, src, collapse(alt), String::new(), None, None);
        }
    }
    flush(&mut cur, &mut blocks);
    if title.is_empty() && let Some(h) = blocks.iter().find(|b| b.kind == Kind::Heading) {
        title = h.text.clone();
    }
    Extracted { title, blocks, images, links }
}

struct Walker {
    blocks: Vec<Block>,
    skip_header: bool,
    buf: String,
}

impl Walker {
    fn push(&mut self, kind: Kind, level: Option<u8>, lang: Option<String>, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let i = self.blocks.len();
        self.blocks.push(Block { i, kind, level, lang, text });
    }

    fn flush(&mut self) {
        let text = collapse(&std::mem::take(&mut self.buf));
        self.push(Kind::Para, None, None, text);
    }

    fn skipped(&self, el: ElementRef) -> bool {
        let name = el.value().name();
        SKIP.contains(&name) || (self.skip_header && name == "header") || hidden(el)
    }

    /// Generic container: inline runs become paragraphs, block children recurse.
    fn container(&mut self, el: ElementRef) {
        for child in el.children() {
            match child.value() {
                Node::Text(t) => self.buf.push_str(t),
                Node::Element(_) => {
                    let child = ElementRef::wrap(child).unwrap();
                    if self.skipped(child) {
                        continue;
                    }
                    if INLINE.contains(&child.value().name()) && !has_block_desc(child) {
                        self.buf.push_str(&inline_text(child));
                    } else {
                        self.flush();
                        self.element(child);
                    }
                }
                _ => {}
            }
        }
    }

    fn element(&mut self, el: ElementRef) {
        let name = el.value().name();
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name[1..].parse().ok();
                self.push(Kind::Heading, level, None, collapse(&inline_text(el)));
            }
            "p" | "figcaption" | "dt" | "dd" | "summary" => {
                self.push(Kind::Para, None, None, collapse(&inline_text(el)))
            }
            "pre" => {
                let lang = el
                    .select(&sel("code"))
                    .next()
                    .and_then(|c| c.value().attr("class"))
                    .or(el.value().attr("class"))
                    .and_then(|c| c.split_whitespace().find_map(|k| k.strip_prefix("language-").or(k.strip_prefix("lang-"))))
                    .map(String::from);
                self.push(Kind::Code, None, lang, el.text().collect());
            }
            "blockquote" => self.push(Kind::Quote, None, None, collapse(&inline_text(el))),
            "li" => {
                let mut own = String::new();
                let mut nested = Vec::new();
                for c in el.children() {
                    match c.value() {
                        Node::Text(t) => own.push_str(t),
                        Node::Element(e) if matches!(e.name(), "ul" | "ol") => nested.push(ElementRef::wrap(c).unwrap()),
                        Node::Element(_) => {
                            let c = ElementRef::wrap(c).unwrap();
                            if !self.skipped(c) {
                                own.push(' ');
                                own.push_str(&inline_text(c));
                            }
                        }
                        _ => {}
                    }
                }
                self.push(Kind::Item, None, None, collapse(&own));
                for n in nested {
                    self.container(n);
                }
            }
            // Tables holding tables are layout, not data.
            "table" if el.descendants().filter_map(ElementRef::wrap).skip(1).any(|d| d.value().name() == "table") => {
                self.container(el);
                self.flush();
            }
            "table" => {
                let rows: Vec<String> = el
                    .select(&sel("tr"))
                    .map(|tr| {
                        let cells: Vec<_> = tr.select(&sel("th, td")).map(|c| collapse(&inline_text(c))).collect();
                        format!("| {} |", cells.join(" | "))
                    })
                    .collect();
                self.push(Kind::Table, None, None, rows.join("\n"));
            }
            _ => {
                self.container(el);
                self.flush();
            }
        }
    }
}

fn inline_text(el: ElementRef) -> String {
    let mut out = String::new();
    for c in el.children() {
        match c.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) if SKIP.contains(&e.name()) => {}
            Node::Element(e) if e.name() == "br" => out.push(' '),
            Node::Element(_) => {
                let c = ElementRef::wrap(c).unwrap();
                if !hidden(c) {
                    let block = !INLINE.contains(&c.value().name());
                    if block {
                        out.push(' ');
                    }
                    out.push_str(&inline_text(c));
                    if block {
                        out.push(' ');
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn has_block_desc(el: ElementRef) -> bool {
    el.descendants().filter_map(ElementRef::wrap).skip(1).any(|d| !INLINE.contains(&d.value().name()))
}

fn has_skipped_ancestor(el: ElementRef, skip_header: bool) -> bool {
    el.ancestors().filter_map(ElementRef::wrap).any(|a| {
        let n = a.value().name();
        SKIP.contains(&n) || (skip_header && n == "header") || hidden(a)
    })
}

fn hidden(el: ElementRef) -> bool {
    let v = el.value();
    v.attr("hidden").is_some()
        || v.attr("aria-hidden") == Some("true")
        || v.attr("style").is_some_and(|s| s.replace(' ', "").contains("display:none"))
}

fn figcaption(img: ElementRef) -> String {
    img.ancestors()
        .filter_map(ElementRef::wrap)
        .take(4)
        .find(|a| a.value().name() == "figure")
        .and_then(|f| f.select(&sel("figcaption")).next())
        .map(|c| collapse(&c.text().collect::<String>()))
        .unwrap_or_default()
}

fn best_srcset(srcset: &str) -> Option<String> {
    srcset
        .split(',')
        .filter_map(|c| {
            let mut parts = c.split_whitespace();
            let url = parts.next()?;
            let w = parts
                .next()
                .and_then(|d| d.trim_end_matches(['w', 'x']).parse::<f32>().ok())
                .unwrap_or(1.0);
            Some((url.to_string(), w))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(u, _)| u)
}

fn push_image(
    out: &mut Vec<Image>,
    base: &Url,
    src: &str,
    alt: String,
    caption: String,
    width: Option<u32>,
    height: Option<u32>,
) {
    let src = src.trim();
    if src.is_empty() || src.starts_with('#') {
        return;
    }
    let Ok(url) = base.join(src) else { return };
    if !matches!(url.scheme(), "http" | "https") || noise(&url, width, height) {
        return;
    }
    if let Some(existing) = out.iter_mut().find(|i| i.url == url) {
        if existing.alt.is_empty() {
            existing.alt = alt;
        }
        if existing.caption.is_empty() {
            existing.caption = caption;
        }
        return;
    }
    out.push(Image { i: out.len(), url, alt, caption, width, height });
}

/// Cheap pre-filter: things that are never content, so no model needs to see them.
fn noise(url: &Url, width: Option<u32>, height: Option<u32>) -> bool {
    let path = url.path().to_ascii_lowercase();
    if path.ends_with(".svg") || path.ends_with(".ico") {
        return true;
    }
    if width.is_some_and(|w| w < 48) || height.is_some_and(|h| h < 48) {
        return true;
    }
    const WORDS: &[&str] = &["sprite", "pixel", "tracking", "spacer", "blank.gif", "favicon", "1x1", "spinner", "emoji"];
    let full = url.as_str().to_ascii_lowercase();
    WORDS.iter().any(|w| full.contains(w))
}

fn push_link(out: &mut Vec<Link>, base: &Url, href: &str, text: String, context: String) {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return;
    }
    let Ok(mut url) = base.join(href) else { return };
    if !matches!(url.scheme(), "http" | "https") {
        return;
    }
    url.set_fragment(None);
    let mut page = base.clone();
    page.set_fragment(None);
    if url == page {
        return;
    }
    if let Some(existing) = out.iter_mut().find(|l| l.url == url) {
        if existing.text.is_empty() {
            existing.text = text;
        }
        return;
    }
    out.push(Link { i: out.len(), url, text, context });
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

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

fn meta(doc: &Html, s: &str) -> Option<String> {
    doc.select(&sel(s)).next()?.value().attr("content").map(String::from).filter(|s| !s.trim().is_empty())
}

fn first_text(doc: &Html, s: &str) -> Option<String> {
    doc.select(&sel(s)).next().map(|e| e.text().collect::<String>()).filter(|s| !s.trim().is_empty())
}

pub fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/post/").unwrap()
    }

    #[test]
    fn layout_tables_do_not_duplicate_rows() {
        let page = "<body><table><tr><td><table><tr><td>Story one is here and long enough</td></tr></table></td></tr></table></body>";
        let ex = html(page, &base());
        let hits = ex.blocks.iter().filter(|b| b.text.contains("Story one")).count();
        assert_eq!(hits, 1);
    }

    #[test]
    fn skips_nav_and_keeps_article() {
        let page = "<body><nav><p>Home About Contact links go here</p></nav><article><h1>T</h1><p>The real paragraph.</p></article></body>";
        let ex = html(page, &base());
        assert!(ex.blocks.iter().all(|b| !b.text.contains("Home")));
        assert!(ex.blocks.iter().any(|b| b.text == "The real paragraph."));
    }

    #[test]
    fn images_resolve_lazy_src_and_skip_noise() {
        let page = r#"<body><article><figure><img data-src="/a.jpg" src="data:,"><figcaption>A cat</figcaption></figure>
            <img src="/logo.svg"><img src="/t.gif" width="1" height="1"></article></body>"#;
        let ex = html(page, &base());
        assert_eq!(ex.images.len(), 1);
        assert_eq!(ex.images[0].url.as_str(), "https://example.com/a.jpg");
        assert_eq!(ex.images[0].caption, "A cat");
    }

    #[test]
    fn many_articles_use_main() {
        let page = "<body><main><article><p>First card text is long enough.</p></article><article><p>Second card text is long enough.</p></article></main></body>";
        let ex = html(page, &base());
        assert_eq!(ex.blocks.len(), 2);
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
    fn markdown_frontmatter_and_empty_images() {
        let md = "---\ntitle: Hello\n---\n\n# Heading\n\nSome text ![]() and ![pic](img.png)\n\n```rust\nfn x() {}\n```\n";
        let ex = markdown(md, &base());
        assert_eq!(ex.title, "Hello");
        assert_eq!(ex.images.len(), 1);
        assert!(ex.blocks.iter().any(|b| b.kind == Kind::Code && b.text == "fn x() {}"));
        assert!(ex.blocks.iter().all(|b| !b.text.contains("title:")));
    }
}

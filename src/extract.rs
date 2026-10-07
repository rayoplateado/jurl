//! HTML/markdown → candidate blocks and images. Every emitted string is text that
//! exists in the page; nothing here rewrites content beyond whitespace collapsing.

use std::collections::HashMap;

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
    /// A smaller variant when the page offers one: what gets downloaded for Clef.
    pub preview: Url,
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
    /// Scripts plus an empty mount point, `<noscript>` or a heavy shell: with almost no
    /// text, the page is a JS app.
    pub app_shell: bool,
}

const SKIP: &[&str] = &[
    "script", "style", "noscript", "nav", "footer", "aside", "form", "svg", "button", "iframe", "template", "select",
    "input", "textarea", "dialog", "canvas", "video", "audio", "object",
];
const INLINE: &[&str] = &[
    "a", "span", "em", "strong", "b", "i", "code", "small", "sup", "sub", "abbr", "mark", "u", "s", "del", "ins",
    "time", "br", "kbd", "q", "cite", "label", "var", "samp", "dfn", "bdi", "wbr", "font", "img", "picture", "data",
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
    // An <article> or <main> holding a sliver of the page's text isn't its content: a sign-up modal's
    // <main>, a "related" card. Below this share of the body's visible text, try the next candidate.
    let body_len = doc.select(&sel("body")).next().map(visible_len).unwrap_or(0);
    let root = order
        .iter()
        .find_map(|s| {
            doc.select(&sel(s))
                .max_by_key(|e| visible_len(*e))
                .filter(|e| *s == "body" || visible_len(*e) * ROOT_MIN_SHARE_INV >= body_len)
        })
        .unwrap_or_else(|| doc.root_element());
    let in_body = root.value().name() == "body";

    // React streams late content (a Suspense boundary) as `<div hidden id="S:n">` at the end of the body, and a
    // script moves it into `<template id="B:n">`, where it belongs. Read it there.
    let segments: HashMap<String, ElementRef> = doc
        .select(&sel("div[hidden][id^='S:']"))
        .filter_map(|e| Some((e.value().id()?.strip_prefix("S:")?.to_string(), e)))
        .collect();
    let mut w = Walker { blocks: Vec::new(), skip_header: in_body, buf: String::new(), segments };
    w.container(root);
    w.flush();

    let mut images = Vec::new();
    if let Some(og) = meta(&doc, "meta[property='og:image']") {
        push_image(&mut images, base, &og, None, String::new(), String::new(), None, None);
    }
    for img in root.select(&sel("img")) {
        if image_skipped(img, in_body) {
            continue;
        }
        let a = |k| img.value().attr(k);
        let src = a("srcset").or(a("data-srcset")).and_then(best_srcset).or(a("data-src")
            .or(a("data-lazy-src"))
            .or(a("data-original"))
            .or(a("src"))
            .map(String::from));
        let Some(src) = src else { continue };
        let preview = a("srcset").or(a("data-srcset")).and_then(small_srcset).or(a("data-src")
            .or(a("data-lazy-src"))
            .or(a("src"))
            .filter(|s| !s.starts_with("data:"))
            .map(String::from));
        let alt = collapse(a("alt").or(a("title")).unwrap_or(""));
        let caption = figcaption(img);
        let dim = |k| a(k).and_then(|v: &str| v.trim_end_matches("px").parse().ok());
        push_image(&mut images, base, &src, preview.as_deref(), alt, caption, dim("width"), dim("height"));
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
            .find(|e| {
                matches!(
                    e.value().name(),
                    "p" | "li" | "td" | "dd" | "blockquote" | "figcaption" | "h1" | "h2" | "h3" | "h4"
                )
            })
            .map(|e| collapse(&inline_text(e)))
            .filter(|c| *c != text)
            .map(|c| c.chars().take(200).collect())
            .unwrap_or_default();
        push_link(&mut links, base, v.attr("href").unwrap_or(""), text, context);
    }

    let app_shell = doc.select(&sel("script")).next().is_some()
        && (body.len() > 4096
            || doc
                .select(&sel("noscript, #root, #app, #__next, #__nuxt, [data-reactroot], [ng-app]"))
                .next()
                .is_some());
    Extracted { title: collapse(&title), blocks: w.blocks, images, links, app_shell }
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
            blocks.push(Block {
                i,
                kind: Kind::Code,
                level: None,
                lang: Some(lang).filter(|l| !l.is_empty()),
                text: body,
            });
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
    if let Some(rest) = body.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---")
    {
        for line in rest[..end].lines() {
            if let Some(v) = line.strip_prefix("title:") {
                title = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
            }
        }
        body = rest[end + 4..].trim_start_matches(['-', '\n']);
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
            let context = if line.trim() == format!("[{text}]({href})") {
                String::new()
            } else {
                collapse(line).chars().take(200).collect()
            };
            push_link(&mut links, base, href, collapse(text), context);
        }
        for (alt, src) in md_images(line) {
            push_image(&mut images, base, src, None, collapse(alt), String::new(), None, None);
        }
    }
    flush(&mut cur, &mut blocks);
    if title.is_empty()
        && let Some(h) = blocks.iter().find(|b| b.kind == Kind::Heading)
    {
        title = h.text.clone();
    }
    Extracted { title, blocks, images, links, app_shell: false }
}

struct Walker<'a> {
    blocks: Vec<Block>,
    skip_header: bool,
    buf: String,
    segments: HashMap<String, ElementRef<'a>>,
}

impl<'a> Walker<'a> {
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
        SKIP.contains(&name) || (self.skip_header && name == "header") || hidden(el) || chrome(el)
    }

    /// Generic container: inline runs become paragraphs, block children recurse.
    fn container(&mut self, el: ElementRef<'a>) {
        for child in el.children() {
            match child.value() {
                Node::Text(t) => self.buf.push_str(t),
                Node::Element(_) => {
                    let child = ElementRef::wrap(child).unwrap();
                    if let Some(seg) = self.segment(child) {
                        self.flush();
                        self.container(seg);
                        self.flush();
                        continue;
                    }
                    if self.skipped(child) || segment(child) {
                        continue;
                    }
                    if permalink(child) {
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

    /// The streamed content that belongs where this `<template id="B:n">` is.
    fn segment(&self, el: ElementRef) -> Option<ElementRef<'a>> {
        let id = el.value().id()?.strip_prefix("B:")?;
        (el.value().name() == "template").then(|| self.segments.get(id).copied()).flatten()
    }

    fn element(&mut self, el: ElementRef<'a>) {
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
                    .and_then(|c| {
                        c.split_whitespace().find_map(|k| k.strip_prefix("language-").or(k.strip_prefix("lang-")))
                    })
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
                        Node::Element(e) if matches!(e.name(), "ul" | "ol") => {
                            nested.push(ElementRef::wrap(c).unwrap())
                        }
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

/// Where inline text crosses a block boundary (`<li>`s in a table cell, a `<br>`): `collapse` makes it a
/// newline, as the page shows it, so "Dylan Field" and "Evan Wallace" don't read as one name.
const BREAK: char = '\u{1F}';

fn inline_text(el: ElementRef) -> String {
    let mut out = String::new();
    for c in el.children() {
        match c.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) if SKIP.contains(&e.name()) => {}
            Node::Element(_) if permalink(ElementRef::wrap(c).unwrap()) => {}
            Node::Element(e) if e.name() == "br" => out.push(BREAK),
            Node::Element(_) => {
                let c = ElementRef::wrap(c).unwrap();
                if !hidden(c) && !chrome(c) {
                    let block = !INLINE.contains(&c.value().name());
                    if block {
                        out.push(BREAK);
                    }
                    out.push_str(&inline_text(c));
                    if block {
                        out.push(BREAK);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The ¶ / # that docs put next to a heading to link to it: page furniture, not text.
fn permalink(el: ElementRef) -> bool {
    el.value().name() == "a"
        && el.value().attr("href").is_some_and(|h| h.starts_with('#'))
        && matches!(collapse(&el.text().collect::<String>()).as_str(), "" | "¶" | "#" | "§" | "🔗" | "⚓")
}

/// The root must hold at least 1/5 of the body's visible text.
const ROOT_MIN_SHARE_INV: usize = 5;

/// Text a reader could see: skips script, style and the like, which can outweigh the article itself.
fn visible_len(el: ElementRef) -> usize {
    el.descendants()
        .filter_map(|n| n.value().as_text().map(|t| (n, t.len())))
        .filter(|(n, _)| {
            !n.ancestors()
                .filter_map(ElementRef::wrap)
                .any(|a| matches!(a.value().name(), "script" | "style" | "noscript" | "template"))
        })
        .map(|(_, len)| len)
        .sum()
}

fn has_block_desc(el: ElementRef) -> bool {
    el.descendants().filter_map(ElementRef::wrap).skip(1).any(|d| !INLINE.contains(&d.value().name()))
}

fn has_skipped_ancestor(el: ElementRef, skip_header: bool) -> bool {
    el.ancestors().filter_map(ElementRef::wrap).any(|a| {
        let n = a.value().name();
        SKIP.contains(&n) || (skip_header && n == "header") || hidden(a) || chrome(a)
    })
}

/// An image is skipped like any element, except that a hidden carousel slide still counts: sliders hide every slide
/// but the active one with `display:none`, and those are the page's photos (a restaurant's dishes), not chrome.
fn image_skipped(img: ElementRef, skip_header: bool) -> bool {
    let carousel = std::iter::once(img)
        .chain(img.ancestors().filter_map(ElementRef::wrap))
        .take(8)
        .any(|a| a.value().attr("class").is_some_and(is_carousel));
    let skipped_by = |a: ElementRef| {
        let n = a.value().name();
        SKIP.contains(&n) || (skip_header && n == "header") || chrome(a) || (!carousel && hidden(a))
    };
    (!carousel && hidden(img)) || img.ancestors().filter_map(ElementRef::wrap).any(skipped_by)
}

const CAROUSEL: &[&str] = &["slide", "slider", "carousel", "swiper", "splide", "glide", "owl-", "slick", "gallery"];

fn is_carousel(class: &str) -> bool {
    let c = class.to_ascii_lowercase();
    CAROUSEL.iter().any(|w| c.contains(w))
}

/// A React streaming segment: hidden only until its script moves it into place.
fn segment(el: ElementRef) -> bool {
    el.value().name() == "div" && el.value().id().is_some_and(|id| id.starts_with("S:"))
}

/// Page chrome that isn't in a `<nav>`/`<aside>`/`<footer>` tag, found the way reader modes find it: by ARIA role,
/// or by a class or id that is one of these exact words (never a part of one: a restaurant's "menu" stays).
const CHROME_ROLES: &[&str] = &["navigation", "complementary", "contentinfo", "search", "menu", "menubar"];
const CHROME_NAMES: &[&str] = &[
    "navbox",
    "vertical-navbox",
    "sidebar",
    "toc",
    "vector-toc",
    "references",
    "reflist",
    "mw-references-wrap",
    "catlinks",
    "mw-editsection",
    "mw-jump-link",
    "breadcrumb",
    "breadcrumbs",
    "sistersitebox",
    "printfooter",
];

fn chrome(el: ElementRef) -> bool {
    let v = el.value();
    v.attr("role").is_some_and(|r| CHROME_ROLES.contains(&r))
        || v.classes().any(|c| CHROME_NAMES.contains(&c))
        || v.id().is_some_and(|id| CHROME_NAMES.contains(&id))
}

fn hidden(el: ElementRef) -> bool {
    let v = el.value();
    (v.attr("hidden").is_some() && !segment(el))
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
            let w = parts.next().and_then(|d| d.trim_end_matches(['w', 'x']).parse::<f32>().ok()).unwrap_or(1.0);
            Some((url.to_string(), w))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(u, _)| u)
}

/// Smallest srcset candidate that is still big enough to recognise (≥320w).
fn small_srcset(srcset: &str) -> Option<String> {
    srcset
        .split(',')
        .filter_map(|c| {
            let mut parts = c.split_whitespace();
            let url = parts.next()?;
            let w = parts.next()?.strip_suffix('w')?.parse::<u32>().ok()?;
            Some((url.to_string(), w))
        })
        .filter(|(_, w)| *w >= 320)
        .min_by_key(|(_, w)| *w)
        .map(|(u, _)| u)
}

#[allow(clippy::too_many_arguments)]
fn push_image(
    out: &mut Vec<Image>,
    base: &Url,
    src: &str,
    preview: Option<&str>,
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
    let preview = preview.and_then(|p| base.join(p.trim()).ok()).unwrap_or_else(|| url.clone());
    out.push(Image { i: out.len(), url, preview, alt, caption, width, height });
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
    const WORDS: &[&str] =
        &["sprite", "pixel", "tracking", "spacer", "blank.gif", "favicon", "1x1", "spinner", "emoji"];
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

/// Runs of whitespace become one space, or one newline where they hold a block boundary.
pub fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap: Option<bool> = None; // Some(has a break) while inside a run of whitespace
    for c in s.chars() {
        if c.is_whitespace() || c == BREAK {
            gap = Some(gap.unwrap_or(false) || c == BREAK);
        } else {
            if let Some(brk) = gap.take()
                && !out.is_empty()
            {
                out.push(if brk { '\n' } else { ' ' });
            }
            out.push(c);
        }
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
    fn a_modal_main_is_not_the_content() {
        let page = r#"<body><main class="membershipModal"><p>Join our premium membership today.</p></main>
            <div class="article"><h2>Val Best Class</h2><p>The best class for Val is the Rune Knight, thanks to its damage.</p>
            <p>A second paragraph about Val's builds and skills, long enough to matter.</p>
            <p>A third paragraph about Val's builds and skills, long enough to matter.</p></div></body>"#;
        let ex = html(page, &base());
        assert!(ex.blocks.iter().any(|b| b.text.contains("Rune Knight")), "{:?}", ex.blocks);
    }

    #[test]
    fn react_streamed_content_is_read_where_it_goes() {
        let page = r#"<body><h1>Pricing</h1><template id="B:0"></template><p>FAQ</p>
            <div hidden id="S:0"><div><div>Monthly</div><div>Pro $10 / month</div></div></div>
            <script>$RC("B:0","S:0")</script></body>"#;
        let texts: Vec<_> = html(page, &base()).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["Pricing", "Monthly", "Pro $10 / month", "FAQ"]);
    }

    #[test]
    fn list_items_in_a_cell_keep_their_lines() {
        let page = r#"<body><table><tr><th>Founders</th><td><ul><li><a>Dylan Field</a></li><li><a>Evan Wallace</a></li></ul></td></tr></table>
            <p>Line one<br>line   two</p></body>"#;
        let texts: Vec<_> = html(page, &base()).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["| Founders | Dylan Field\nEvan Wallace |", "Line one\nline two"]);
        assert_eq!(collapse("  a \n  b  "), "a b");
    }

    #[test]
    fn reader_mode_drops_chrome_outside_nav_tags() {
        let page = r##"<body><div class="vector-toc"><ul><li><a href="#h">History</a></li></ul></div>
            <h2>History<span class="mw-editsection">[edit]</span></h2><p>Founded in 1890.</p>
            <div role="navigation" class="navbox"><ul><li><a href="/x">Other city</a></li></ul></div>
            <div class="reflist"><ol class="references"><li>Smith, J. (2010).</li></ol></div>
            <div class="menu"><p>Paella 12 €</p></div></body>"##;
        let texts: Vec<_> = html(page, &base()).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["History", "Founded in 1890.", "Paella 12 €"]);
    }

    #[test]
    fn heading_permalinks_are_not_text() {
        let page = r##"<body><article><h2>Methods of File Objects<a class="headerlink" href="#methods">¶</a></h2>
            <p>See <a href="#methods">the methods</a> for more.</p></article></body>"##;
        let ex = html(page, &base());
        assert_eq!(ex.blocks[0].text, "Methods of File Objects");
        assert!(ex.blocks[1].text.contains("the methods"));
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
    fn app_shell_needs_scripts_and_a_shell() {
        let tiny = "<html><body><p>Example Domain text.</p><script>1</script></body></html>";
        assert!(!html(tiny, &base()).app_shell);
        let spa = "<html><body><div id=\"root\"></div><script src=\"/app.js\"></script></body></html>";
        assert!(html(spa, &base()).app_shell);
        let static_page = "<html><body><div id=\"root\"><p>Server text</p></div></body></html>";
        assert!(!html(static_page, &base()).app_shell);
    }

    #[test]
    fn hidden_carousel_slides_keep_their_images() {
        let html_doc = r#"<html><body><article><p>Our kitchen and our dishes, every day.</p>
            <div class='frs-slide-img-wrapper' style='display:none;'><div class='frs-slide-img' style='display:none'>
            <img alt='4' src='https://example.com/amatriciana.jpg'></div></div>
            <div style="display:none"><img src="https://example.com/tracker-banner.jpg"></div>
            </article></body></html>"#;
        let ex = html(html_doc, &base());
        let urls: Vec<&str> = ex.images.iter().map(|i| i.url.as_str()).collect();
        assert!(urls.contains(&"https://example.com/amatriciana.jpg"), "{urls:?}");
        assert!(!urls.contains(&"https://example.com/tracker-banner.jpg"), "{urls:?}");
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

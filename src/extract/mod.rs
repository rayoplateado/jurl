//! HTML/markdown → candidate blocks and images. Every emitted string is text that
//! exists in the page; nothing here rewrites content beyond whitespace collapsing.

use std::collections::{HashMap, HashSet};

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
    /// Which `<ul>`/`<ol>` a list item belongs to, numbered in page order.
    #[serde(skip)]
    pub list: Option<usize>,
}

impl Block {
    pub fn markdown(&self) -> String {
        match self.kind {
            Kind::Heading => format!("{} {}", "#".repeat(self.level.unwrap_or(2) as usize), self.text),
            Kind::Code => {
                // Longer than any fence inside, so code that shows a ``` block prints as one block.
                let inner = self.text.lines().map(|l| l.trim_start().chars().take_while(|&c| c == '`').count()).max();
                let fence = "`".repeat(inner.unwrap_or(0).max(2) + 1);
                format!("{fence}{}\n{}\n{fence}", self.lang.as_deref().unwrap_or(""), self.text.trim_end())
            }
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
    /// Only ever a footnote mark (inside `<sup>`: "[1]", "[clarification needed]") or an image with no text (on a wiki
    /// it opens the photo's own page): beside the text, never a way to the topic. A link to the same page in words
    /// clears it.
    pub marginal: bool,
}

pub struct Extracted {
    pub title: String,
    pub blocks: Vec<Block>,
    pub images: Vec<Image>,
    pub links: Vec<Link>,
    /// Every link on the page, menus and footers included: how `--follow` moves around a site.
    pub site_links: Vec<Link>,
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
    let mut w = Walker { blocks: Vec::new(), skip_header: in_body, buf: String::new(), segments, list: None, lists: 0 };
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
        push_link(&mut links, base, v.attr("href").unwrap_or(""), text, context, marginal(a));
    }

    let app_shell = doc.select(&sel("script")).next().is_some()
        && (body.len() > 4096
            || doc
                .select(&sel("noscript, #root, #app, #__next, #__nuxt, [data-reactroot], [ng-app]"))
                .next()
                .is_some());
    // Menus and footers are where a site keeps "Pricing" and "Docs": only hidden links are left out here.
    let mut site_links = Vec::new();
    for a in doc.select(&sel("a[href]")) {
        if a.ancestors().filter_map(ElementRef::wrap).chain(std::iter::once(a)).any(hidden) {
            continue;
        }
        let v = a.value();
        let text = collapse(&a.text().collect::<String>());
        let text = if text.is_empty() {
            v.attr("aria-label").or(v.attr("title")).map(collapse).unwrap_or_default()
        } else {
            text
        };
        push_link(&mut site_links, base, v.attr("href").unwrap_or(""), text, String::new(), marginal(a));
    }
    Extracted { title: collapse(&title), blocks: join_short(w.blocks), images, links, site_links, app_shell }
}

/// Below this many characters a block says too little to be judged on its own ("Basic", "$10", "per user/month").
pub const SHORT_BLOCK_CHARS: usize = 25;
/// A run of joined short blocks stops growing here.
const JOINED_MAX_CHARS: usize = 400;

/// A heading that is only a price, so it joins its card: it starts with a currency sign, or it is figures, currency
/// signs and separators with letters only as unit words ("$12", "€9.50", "12 €", "$10/mo", "10 per user/month").
/// "Step 2" is a section title.
fn is_price(text: &str) -> bool {
    const CURRENCY: &[char] = &['$', '€', '£', '¥', '₹', '₩', '₽', '₺'];
    const SEPARATORS: &[char] = &['.', ',', '/', '-', '–', '|', '·'];
    // A unit word also matches with a trailing "s" ("users").
    const UNITS: &[&str] = &["a", "per", "mo", "month", "yr", "year", "user", "seat"];
    let t = text.trim();
    if !t.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let figure = |c: char| c.is_ascii_digit() || c.is_whitespace() || CURRENCY.contains(&c) || SEPARATORS.contains(&c);
    let only_figures = t.chars().all(|c| c.is_alphabetic() || figure(c));
    let only_units = t
        .split(|c: char| !c.is_alphabetic())
        .all(|w| w.is_empty() || UNITS.contains(&w.to_lowercase().trim_end_matches('s')));
    t.starts_with(CURRENCY) || (only_figures && only_units)
}

/// Consecutive short paragraphs and list items become one block, a line each, as the page shows them: a pricing
/// card built from bare `<div>`s is then one block that says "Basic / $10 / per user/month" instead of pieces too
/// short to judge. Headings, code, tables and quotes are never joined, except a heading that is a price.
fn join_short(blocks: Vec<Block>) -> Vec<Block> {
    // A heading that is a price ("### $12") is a figure set big on its card, not a section title.
    let value = |b: &Block| b.kind == Kind::Heading && is_price(&b.text);
    let short = |b: &Block| {
        (matches!(b.kind, Kind::Para | Kind::Item) || value(b)) && b.text.chars().count() < SHORT_BLOCK_CHARS
    };
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    let mut run: Vec<Block> = Vec::new();
    let flush = |run: &mut Vec<Block>, out: &mut Vec<Block>| match run.len() {
        0 => {}
        1 => out.push(run.pop().unwrap()),
        _ => {
            let text = run.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
            // Items of one list joined are still part of that list ("1 egg" and "Salt" next to each other).
            let list = run[0].list.filter(|l| run.iter().all(|b| b.kind == Kind::Item && b.list == Some(*l)));
            out.push(Block { i: 0, kind: Kind::Para, level: None, lang: None, text, list });
            run.clear();
        }
    };
    for b in blocks {
        let joined: usize = run.iter().map(|r| r.text.chars().count() + 1).sum();
        if short(&b) && joined + b.text.chars().count() <= JOINED_MAX_CHARS {
            run.push(b);
        } else {
            flush(&mut run, &mut out);
            if short(&b) { run.push(b) } else { out.push(b) }
        }
    }
    flush(&mut run, &mut out);
    for (i, b) in out.iter_mut().enumerate() {
        b.i = i;
    }
    out
}

/// List items too short to be judged alone ("1 teaspoon baking soda") that sit next to a kept item of the same
/// list: a list is read whole, so they go wherever their neighbours go. Without them a recipe loses its salt.
pub fn short_items_of_kept_lists(blocks: &[Block], kept: impl Fn(usize) -> bool) -> HashSet<usize> {
    let short = |b: &Block| b.list.is_some() && b.text.chars().count() < SHORT_BLOCK_CHARS;
    let mut out = HashSet::new();
    // Grows from each kept item outwards, so a run of short items between two kept ones comes along whole.
    loop {
        let before = out.len();
        for (i, b) in blocks.iter().enumerate() {
            if !short(b) || out.contains(&i) {
                continue;
            }
            let neighbour = |j: usize| blocks.get(j).is_some_and(|n| n.list == b.list && (kept(j) || out.contains(&j)));
            if (i > 0 && neighbour(i - 1)) || neighbour(i + 1) {
                out.insert(i);
            }
        }
        if out.len() == before {
            return out;
        }
    }
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
            // Six levels, as in HTML: a longer run of #s is still a heading, at level 6.
            let level = (1 + rest.chars().take_while(|&c| c == '#').count()).min(6) as u8;
            (Kind::Heading, Some(level), rest.trim_start_matches('#').trim().to_string())
        } else if text.starts_with("```") || text.starts_with("~~~") {
            let mut lines = text.lines();
            let first = lines.next().unwrap_or("");
            let mark = first.chars().next().unwrap_or('`');
            let fence: String = first.chars().take_while(|&x| x == mark).collect();
            let lang = first.trim_matches(|c| c == '`' || c == '~').trim().to_string();
            let mut body: Vec<_> = lines.collect();
            // Only a line that closes the fence goes: a fence still open at the end of input keeps its last line.
            if body.last().is_some_and(|l| closes_fence(l.trim_start(), &fence)) {
                body.pop();
            }
            blocks.push(Block {
                i,
                kind: Kind::Code,
                level: None,
                lang: Some(lang).filter(|l| !l.is_empty()),
                text: body.join("\n"),
                list: None,
            });
            return;
        } else if text.starts_with('>') {
            let t = text.lines().map(|l| l.trim_start_matches('>').trim()).collect::<Vec<_>>().join("\n");
            (Kind::Quote, None, t)
        } else {
            (Kind::Para, None, text)
        };
        blocks.push(Block { i, kind, level, lang: None, text, list: None });
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
        // Only the closing `---` line goes: the body after it stays whole, a leading "- " bullet included.
        body = rest[end + 4..].split_once('\n').map_or("", |(_, after)| after);
    }

    for line in body.lines() {
        let t = line.trim_start();
        if let Some(f) = &fence {
            cur.push(line);
            if closes_fence(t, f) {
                fence = None;
                flush(&mut cur, &mut blocks);
            }
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            flush(&mut cur, &mut blocks);
            let c = t.chars().next().unwrap_or('`');
            fence = Some(t.chars().take_while(|&x| x == c).collect());
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
            push_link(&mut links, base, href, collapse(text), context, false);
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
    let site_links = links.clone();
    Extracted { title, blocks: join_short(blocks), images, links, site_links, app_shell: false }
}

/// A fence closes on a line of the same character, at least as long, and nothing else: a ```` block can show
/// a ```js block inside it, and "```js" never closes anything.
fn closes_fence(line: &str, fence: &str) -> bool {
    let Some(c) = fence.chars().next() else { return false };
    let run = line.chars().take_while(|&x| x == c).count();
    run >= fence.chars().count() && line[run * c.len_utf8()..].trim().is_empty()
}

struct Walker<'a> {
    blocks: Vec<Block>,
    skip_header: bool,
    buf: String,
    segments: HashMap<String, ElementRef<'a>>,
    /// The list being walked, and how many lists came before it.
    list: Option<usize>,
    lists: usize,
}

impl<'a> Walker<'a> {
    fn push(&mut self, kind: Kind, level: Option<u8>, lang: Option<String>, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let i = self.blocks.len();
        let list = (kind == Kind::Item).then_some(self.list).flatten();
        self.blocks.push(Block { i, kind, level, lang, text, list });
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
                    self.element(n);
                }
            }
            "ul" | "ol" => {
                let outer = self.list.replace(self.lists);
                self.lists += 1;
                self.container(el);
                self.flush();
                self.list = outer;
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
    // A src that resolves to the page itself ("?q=80" in an og:image left without its path) is the page, not a
    // picture of anything.
    let bare = |u: &Url| u.as_str().split(['?', '#']).next().unwrap_or_default().to_string();
    if bare(&url) == bare(base) {
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

/// A link in a footnote mark (`<sup>`), or an image with no text: see [`Link::marginal`].
fn marginal(a: ElementRef) -> bool {
    let image_only = a.text().all(|t| t.trim().is_empty()) && a.select(&sel("img")).next().is_some();
    image_only || a.ancestors().filter_map(ElementRef::wrap).any(|e| e.value().name() == "sup")
}

fn push_link(out: &mut Vec<Link>, base: &Url, href: &str, text: String, context: String, marginal: bool) {
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
        existing.marginal &= marginal;
        return;
    }
    out.push(Link { i: out.len(), url, text, context, marginal });
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
        assert_eq!(texts, ["Pricing", "Monthly\nPro $10 / month\nFAQ"]);
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
        assert_eq!(texts, ["History", "Founded in 1890.\nPaella 12 €"]);
    }

    #[test]
    fn a_price_set_as_a_heading_joins_its_card() {
        let ex = markdown("### Teams\n\nYEARLY\n\n### $12\n\nper user/month\n\nSave 25%\n", &base());
        let texts: Vec<_> = ex.blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["Teams", "YEARLY\n$12\nper user/month\nSave 25%"]);
    }

    #[test]
    fn a_numbered_heading_is_not_a_price() {
        let want = [
            (Kind::Heading, Some(2), "Step 2"),
            (Kind::Para, None, "Install it."),
            (Kind::Heading, Some(2), "Step 3"),
            (Kind::Para, None, "Run it."),
        ];
        let md = markdown("## Step 2\n\nInstall it.\n\n## Step 3\n\nRun it.\n", &base());
        let page = html("<body><h2>Step 2</h2><p>Install it.</p><h2>Step 3</h2><p>Run it.</p></body>", &base());
        for ex in [md, page] {
            let got: Vec<_> = ex.blocks.iter().map(|b| (b.kind, b.level, b.text.as_str())).collect();
            assert_eq!(got, want);
        }
    }

    #[test]
    fn price_headings_of_each_shape_join_their_card() {
        for price in ["€9.50", "12 €", "$10/mo"] {
            let ex = markdown(&format!("### {price}\n\nper user/month\n"), &base());
            let texts: Vec<_> = ex.blocks.into_iter().map(|b| b.text).collect();
            assert_eq!(texts, [format!("{price}\nper user/month")], "{price}");
        }
    }

    #[test]
    fn only_figures_and_units_are_prices() {
        for price in ["$12", "€9.50", "12 €", "$10/mo", "10 per user/month", "$10 Pro"] {
            assert!(is_price(price), "{price}");
        }
        for title in ["Step 2", "Version 3", "Top 10 tips", "COVID-19", "2FA", "Save 25%", "100%", "Free"] {
            assert!(!is_price(title), "{title}");
        }
    }

    #[test]
    fn short_pieces_of_a_card_are_one_block() {
        let page = r#"<body><h2>Pricing</h2><div><div>Basic</div><div>$10</div><div>per user/month</div></div>
            <p>Everything in Free, plus unlimited teams and private projects for everyone.</p><div>Ok</div></body>"#;
        let texts: Vec<_> = html(page, &base()).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(
            texts,
            [
                "Pricing",
                "Basic\n$10\nper user/month",
                "Everything in Free, plus unlimited teams and private projects for everyone.",
                "Ok"
            ]
        );
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
    fn footnote_marks_and_image_only_links_are_marginal() {
        let page = "<body><a href='/'><img alt='Logo' src='/logo.png'></a><article><p>Built in 1889 \
            <sup><i>[<a href='/wiki/Help:Clarify'>clarification needed</a>]</i></sup> by \
            <a href='/wiki/Gustave_Eiffel'>Eiffel</a>.</p><a href='/wiki/File:Tower.jpg'><img alt='The tower' \
            src='/tower.jpg'></a><p><a href='/'>Home</a></p>\
            </article></body>";
        let ex = html(page, &base());
        let marginal = |path: &str| ex.links.iter().find(|l| l.url.path() == path).map(|l| l.marginal);
        assert_eq!(marginal("/wiki/Help:Clarify"), Some(true));
        assert_eq!(marginal("/wiki/File:Tower.jpg"), Some(true));
        assert_eq!(marginal("/wiki/Gustave_Eiffel"), Some(false));
        // `site_links` carry the mark too: candidates read both lists.
        let site = |path: &str| ex.site_links.iter().find(|l| l.url.path() == path).map(|l| l.marginal);
        assert_eq!(site("/wiki/File:Tower.jpg"), Some(true));
        // The logo is an image, but "Home" links to the same page in words.
        assert_eq!(site("/"), Some(false));
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
    fn short_list_items_go_with_their_list() {
        let page = "<article><p>Some introduction that is long enough to be judged on its own.</p><ul>\
            <li>2 and 1/4 cups (281g) all-purpose flour</li><li>1 teaspoon baking soda</li>\
            <li>1 and 1/2 teaspoons cornstarch*</li><li>1/2 teaspoon salt</li>\
            <li>3/4 cup (170g) unsalted butter, melted</li></ul><ul><li>Pin it</li></ul></article>";
        let ex = html(page, &base());
        let at = |t: &str| ex.blocks.iter().position(|b| b.text == t).unwrap();
        let (soda, salt, pin) = (at("1 teaspoon baking soda"), at("1/2 teaspoon salt"), at("Pin it"));
        let flour = at("2 and 1/4 cups (281g) all-purpose flour");
        let kept = |i| {
            i == flour
                || i == at("1 and 1/2 teaspoons cornstarch*")
                || i == at("3/4 cup (170g) unsalted butter, melted")
        };
        let items = short_items_of_kept_lists(&ex.blocks, kept);
        assert!(items.contains(&soda) && items.contains(&salt));
        // Another list's short item stays out, and nothing comes along when no item of the list is kept.
        assert!(!items.contains(&pin));
        assert!(short_items_of_kept_lists(&ex.blocks, |_| false).is_empty());

        // Two tiny items next to each other are joined into one block, still too short to judge: it goes too.
        let page = "<article><ul><li>2 and 1/4 cups (281g) all-purpose flour</li><li>1 egg</li><li>Salt</li>\
            <li>3/4 cup (170g) unsalted butter, melted</li></ul></article>";
        let ex = html(page, &base());
        let joined = ex.blocks.iter().position(|b| b.text == "1 egg\nSalt").unwrap();
        let flour = ex.blocks.iter().position(|b| b.text.starts_with("2 and")).unwrap();
        assert!(short_items_of_kept_lists(&ex.blocks, |i| i == flour).contains(&joined));
    }

    #[test]
    fn the_page_itself_is_not_an_image() {
        let page = r#"<html><head><meta property="og:image" content="?q=80"></head><body><article>
            <p>Text</p><img src="/blog/post?w=800" width="800" height="400"><img src="/a.png" width="800" height="400">
            </article></body></html>"#;
        let ex = html(page, &Url::parse("https://example.com/blog/post").unwrap());
        let urls: Vec<_> = ex.images.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(urls, ["https://example.com/a.png"]);
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

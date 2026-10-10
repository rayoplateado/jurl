//! The HTML path: find the page's content root, walk it into blocks, and collect the images and links on it. The rules
//! for what a reader never sees (script and form elements, hidden elements, page chrome) are here too.

use std::collections::HashMap;

use scraper::{ElementRef, Html, Node, Selector};
use url::Url;

use super::join::join_short;
use super::{BREAK, Block, Extracted, Image, Kind, Link, collapse, push_image, push_link};

const SKIP: &[&str] = &[
    "script", "style", "noscript", "nav", "footer", "aside", "form", "svg", "button", "iframe", "template", "select",
    "input", "textarea", "dialog", "canvas", "video", "audio", "object",
];
const INLINE: &[&str] = &[
    "a", "span", "em", "strong", "b", "i", "code", "small", "sup", "sub", "abbr", "mark", "u", "s", "del", "ins",
    "time", "br", "kbd", "q", "cite", "label", "var", "samp", "dfn", "bdi", "wbr", "font", "img", "picture", "data",
];

/// Attributes where a lazy loader keeps an image's real address while `src` holds a placeholder, in the order read.
const LAZY_SRC: &[&str] = &["data-src", "data-lazy-src"];
/// The full-size file a lazy loader may keep as well. The image and its preview both read it after `LAZY_SRC` and
/// before `src`, so a placeholder `src` is never the preview of an image that has a lazy one.
const LAZY_FULL_SIZE: &str = "data-original";

pub fn html(body: &str, base: &Url) -> Extracted {
    extract_doc(&Html::parse_document(body), body, base)
}

/// `html`, and whether the page is a template its script has not filled in: see [`has_placeholders`].
pub fn html_with_placeholders(body: &str, base: &Url) -> (Extracted, bool) {
    let doc = Html::parse_document(body);
    (extract_doc(&doc, body, base), has_placeholders(&doc))
}

fn extract_doc(doc: &Html, body: &str, base: &Url) -> Extracted {
    let root = content_root(doc);
    let in_body = root.value().name() == "body";
    Extracted {
        title: collapse(&page_title(doc)),
        blocks: join_short(walk(doc, root, in_body)),
        structured: Vec::new(),
        images: collect_images(doc, root, in_body, base),
        links: collect_links(root, in_body, base),
        site_links: collect_site_links(doc, base),
        app_shell: is_app_shell(doc, body),
    }
}

/// The page's title: `og:title`, else the `<title>`, else the first `<h1>`.
fn page_title(doc: &Html) -> String {
    meta(doc, "meta[property='og:title']")
        .or_else(|| first_text(doc, "title"))
        .or_else(|| first_text(doc, "h1"))
        .unwrap_or_default()
}

/// The element the content is read from: the first candidate, in order, that holds a fair share of the page's text.
fn content_root(doc: &Html) -> ElementRef<'_> {
    // Several <article>s means a listing of cards: the container is the content.
    let order: &[&str] = if doc.select(&sel("article")).nth(1).is_some() {
        &["main", "[role=main]", "body"]
    } else {
        &["article", "main", "[role=main]", "body"]
    };
    // An <article> or <main> holding a sliver of the page's text isn't its content: a sign-up modal's
    // <main>, a "related" card. Below this share of the body's visible text, try the next candidate.
    let body_len = doc.select(&sel("body")).next().map(visible_len).unwrap_or(0);
    order
        .iter()
        .find_map(|s| {
            doc.select(&sel(s))
                .max_by_key(|e| visible_len(*e))
                .filter(|e| *s == "body" || visible_len(*e) * ROOT_MIN_SHARE_INV >= body_len)
        })
        .unwrap_or_else(|| doc.root_element())
}

/// The blocks of the content root, in page order.
fn walk<'a>(doc: &'a Html, root: ElementRef<'a>, in_body: bool) -> Vec<Block> {
    // React streams late content (a Suspense boundary) as `<div hidden id="S:n">` at the end of the body, and a
    // script moves it into `<template id="B:n">`, where it belongs. Read it there.
    let segments: HashMap<String, ElementRef> = doc
        .select(&sel("div[hidden][id^='S:']"))
        .filter_map(|e| Some((e.value().id()?.strip_prefix("S:")?.to_string(), e)))
        .collect();
    let mut w = Walker {
        blocks: Vec::new(),
        skip_header: in_body,
        buf: String::new(),
        segments,
        list: None,
        lists: 0,
        depth: 0,
    };
    w.container(root);
    w.flush();
    w.blocks
}

/// The images of the page: `og:image` first, then a product's photos from its JSON-LD, then the `<img>`s of the content root
/// that a reader can see, then its `<picture>` sources. `push_image` keeps each URL once, so a picture's `<img>` and its
/// `<source>` that name one file are one candidate.
fn collect_images(doc: &Html, root: ElementRef, in_body: bool, base: &Url) -> Vec<Image> {
    let mut images = Vec::new();
    if let Some(og) = meta(doc, "meta[property='og:image']") {
        push_image(&mut images, base, &og, None, String::new(), String::new(), None, None);
    }
    for src in super::json_ld::product_images(doc) {
        push_image(&mut images, base, &src, None, String::new(), String::new(), None, None);
    }
    for img in root.select(&sel("img")) {
        if image_skipped(img, in_body) {
            continue;
        }
        let a = |k| img.value().attr(k);
        let srcsets = a("srcset").or(a("data-srcset"));
        // The image's own address: a lazy loader's attribute if it has one, else `src`, which may be a placeholder.
        let own = first_attr(img, LAZY_SRC).or(a(LAZY_FULL_SIZE)).or(a("src"));
        let src = srcsets.and_then(best_srcset).or(own.map(String::from));
        let Some(src) = src else { continue };
        let preview = srcsets.and_then(small_srcset).or(own.filter(|s| !s.starts_with("data:")).map(String::from));
        let alt = collapse(a("alt").or(a("title")).unwrap_or(""));
        let caption = figcaption(img);
        let dim = |k| a(k).and_then(|v: &str| v.trim_end_matches("px").parse().ok());
        push_image(&mut images, base, &src, preview.as_deref(), alt, caption, dim("width"), dim("height"));
    }
    // A responsive picture's banner is often only in its <source> srcsets, with the <img> as the fallback. Each source of a
    // type the decoder reads is a candidate at its largest size, as an <img>'s srcset is read, with the picture's alt text.
    for source in root.select(&sel("picture > source")) {
        if image_skipped(source, in_body) || !decodable_type(source.value().attr("type")) {
            continue;
        }
        let Some(srcset) = source.value().attr("srcset") else { continue };
        let Some(src) = best_srcset(srcset) else { continue };
        let preview = small_srcset(srcset);
        let alt = source
            .parent()
            .and_then(ElementRef::wrap)
            .and_then(|picture| picture.select(&sel("img")).next())
            .map(|img| collapse(img.value().attr("alt").unwrap_or("")))
            .unwrap_or_default();
        push_image(&mut images, base, &src, preview.as_deref(), alt, String::new(), None, None);
    }
    images
}

/// Whether a `<source>`'s type is one the image decoder reads, by the decoder's own list of MIME types, so a type it has no
/// feature for (AVIF, say) is left out. A type may carry parameters after a `;`. A source with no type is read as it is.
fn decodable_type(t: Option<&str>) -> bool {
    let Some(t) = t else { return true };
    let essence = t.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    image::ImageFormat::from_mime_type(essence).is_some_and(|f| f.reading_enabled())
}

/// The first of these attributes that the element has.
fn first_attr<'a>(el: ElementRef<'a>, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|n| el.value().attr(n))
}

/// The links of the content root. Hidden links, and links inside skipped elements, are left out.
fn collect_links(root: ElementRef, in_body: bool, base: &Url) -> Vec<Link> {
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
    links
}

/// Scripts plus an empty mount point, `<noscript>` or a heavy shell: with almost no text, the page is a JS app.
fn is_app_shell(doc: &Html, body: &str) -> bool {
    doc.select(&sel("script")).next().is_some()
        && (body.len() > 4096
            || doc.select(&sel("noscript, #root, #app, #__next, #__nuxt, [data-reactroot], [ng-app]")).next().is_some())
}

/// A sigil before `{name}` makes a template placeholder: MEGA's `!{freePlanStorage}` and `^{price}`, JavaScript's
/// `${total}`, Ruby's `#{name}`.
const SIGILS: &[char] = &['!', '^', '$', '#', '%', '@'];
/// The placeholders in a page's visible text that make it a template its script has not filled in.
const PLACEHOLDERS_MIN: usize = 2;
/// Elements whose text is not prose a reader takes in: markup, and code, where braces are what the text is about.
const NOT_PROSE: &[&str] =
    &["head", "script", "style", "noscript", "template", "textarea", "pre", "code", "kbd", "samp", "var"];

/// Whether the page is a template its script has not filled in: at least `PLACEHOLDERS_MIN` placeholders in its
/// visible text, outside code. `{{name}}`, and a sigil before `{name}`, are placeholders; a bare `{name}` is not, since
/// prose uses it for regex quantifiers and the like. Only a script fills placeholders in, so a page without one is read
/// as it is.
fn has_placeholders(doc: &Html) -> bool {
    if doc.select(&sel("script")).next().is_none() {
        return false;
    }
    let mut found = 0;
    for node in doc.root_element().descendants() {
        let Some(text) = node.value().as_text().filter(|t| t.contains('{')) else { continue };
        if node.ancestors().filter_map(ElementRef::wrap).any(|a| hidden(a) || NOT_PROSE.contains(&a.value().name())) {
            continue;
        }
        found += count_placeholders(text);
        if found >= PLACEHOLDERS_MIN {
            return true;
        }
    }
    false
}

/// The placeholders in `text`: each `{{name}}`, and each `{name}` with a sigil right before it.
fn count_placeholders(text: &str) -> usize {
    text.match_indices('{')
        .filter(|&(at, _)| match text[at + 1..].strip_prefix('{') {
            Some(inside) => name_then(inside, "}}"),
            None => {
                text[..at].chars().next_back().is_some_and(|c| SIGILS.contains(&c)) && name_then(&text[at + 1..], "}")
            }
        })
        .count()
}

/// Whether `s` starts with a name and then `close`, spaces allowed around the name. A name is an identifier of two or
/// more characters, dots allowed (`price`, `plan.storage`), so the `n` in `x^{n}` is not one.
fn name_then(s: &str, close: &str) -> bool {
    let s = s.trim_start();
    let len = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.')).unwrap_or(s.len());
    len >= 2 && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && s[len..].trim_start().starts_with(close)
}

/// Every link on the page, menus and footers included: only hidden links are left out.
fn collect_site_links(doc: &Html, base: &Url) -> Vec<Link> {
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
    site_links
}

/// How deep the walk recurses into a page before it reads a subtree flat. A page nested this deep is hostile or broken,
/// and the stack is finite: the walk and the inline text both stop here.
const MAX_WALK_DEPTH: usize = 256;

struct Walker<'a> {
    blocks: Vec<Block>,
    skip_header: bool,
    buf: String,
    segments: HashMap<String, ElementRef<'a>>,
    /// The list being walked, and how many lists came before it.
    list: Option<usize>,
    lists: usize,
    /// How many containers the walk is inside of, capped at `MAX_WALK_DEPTH`.
    depth: usize,
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

    /// Generic container: inline runs become paragraphs, block children recurse.
    fn container(&mut self, el: ElementRef<'a>) {
        if self.depth >= MAX_WALK_DEPTH {
            // Too deep to walk into: its text, read flat, carries on the paragraph in progress.
            self.buf.push(BREAK);
            self.buf.push_str(&flat_text(el, self.skip_header));
            self.buf.push(BREAK);
            return;
        }
        self.depth += 1;
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
                    if skipped(child, self.skip_header, true) || segment(child) {
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
        self.depth -= 1;
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
                            if !skipped(c, self.skip_header, true) {
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

fn inline_text(el: ElementRef) -> String {
    inline_text_at(el, 0)
}

/// `inline_text` of `el`, which is `depth` levels down: past `MAX_WALK_DEPTH` a subtree is read flat.
fn inline_text_at(el: ElementRef, depth: usize) -> String {
    let mut out = String::new();
    for c in el.children() {
        match c.value() {
            Node::Text(t) => out.push_str(t),
            // A `<br>` is a line break even where it is hidden, so it is handled before the skip rules.
            Node::Element(e) if e.name() == "br" => out.push(BREAK),
            Node::Element(_) => {
                let c = ElementRef::wrap(c).unwrap();
                // No `<header>` rule here: inline text keeps the words of a header.
                if !skipped(c, false, true) && !permalink(c) {
                    let block = !INLINE.contains(&c.value().name());
                    if block {
                        out.push(BREAK);
                    }
                    if depth < MAX_WALK_DEPTH {
                        out.push_str(&inline_text_at(c, depth + 1));
                    } else {
                        // Inline text keeps the words of a header, as `inline_text` does.
                        out.push_str(&flat_text(c, false));
                    }
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

/// The text under `el`, read without recursion for a subtree the walk won't go into. It leaves out what the walk leaves
/// out (script, hidden elements, page chrome, and a masthead `<header>` when `skip_header`), and a block element starts
/// and ends a line.
fn flat_text(el: ElementRef, skip_header: bool) -> String {
    let mut out = String::new();
    // A stack instead of recursion. A `None` marks the end of a block element.
    let mut stack: Vec<_> = el.children().rev().map(Some).collect();
    while let Some(next) = stack.pop() {
        let Some(node) = next else {
            out.push(BREAK);
            continue;
        };
        match node.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) if e.name() == "br" => out.push(BREAK),
            Node::Element(_) => {
                let Some(child) = ElementRef::wrap(node) else { continue };
                if skipped(child, skip_header, true) || permalink(child) {
                    continue;
                }
                if !INLINE.contains(&child.value().name()) {
                    out.push(BREAK);
                    stack.push(None);
                }
                stack.extend(child.children().rev().map(Some));
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

/// Whether an ancestor of the element is skipped. The element's own `hidden` is the caller's to check, and its own
/// class or role is not checked at all: only its ancestors' are.
fn has_skipped_ancestor(el: ElementRef, skip_header: bool) -> bool {
    el.ancestors().filter_map(ElementRef::wrap).any(|a| skipped(a, skip_header, true))
}

/// What a reader never sees: script-like and form elements, a `<header>` that is the page's masthead (`skip_header`),
/// page chrome, and hidden elements (`skip_hidden`). A hidden carousel slide is the one exception, which
/// `image_skipped` makes by passing `skip_hidden = false`.
fn skipped(el: ElementRef, skip_header: bool, skip_hidden: bool) -> bool {
    let name = el.value().name();
    SKIP.contains(&name) || (skip_header && name == "header") || chrome(el) || (skip_hidden && hidden(el))
}

/// An image is skipped like any element, except that a hidden carousel slide still counts: sliders hide every slide
/// but the active one with `display:none`, and those are the page's photos (a restaurant's dishes), not chrome.
/// As for a link, the image's own class or role is not checked, only its ancestors'.
fn image_skipped(img: ElementRef, skip_header: bool) -> bool {
    let carousel = std::iter::once(img)
        .chain(img.ancestors().filter_map(ElementRef::wrap))
        .take(8)
        .any(|a| a.value().attr("class").is_some_and(is_carousel));
    (!carousel && hidden(img))
        || img.ancestors().filter_map(ElementRef::wrap).any(|a| skipped(a, skip_header, !carousel))
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

/// Whether a reader can't see the element itself: its `hidden` attribute, or an inline style that hides it. An ancestor's
/// are checked where the walk reaches the ancestor. `aria-hidden` is not one of them: it hides the element from assistive
/// technology, and the page still shows it.
fn hidden(el: ElementRef) -> bool {
    let v = el.value();
    (v.attr("hidden").is_some() && !segment(el)) || v.attr("style").is_some_and(style_hides)
}

/// Whether an inline style hides its element: `display: none`, or `visibility: hidden`. `visibility: collapse` hides a
/// non-table element the same way. CSS reads property names and keywords without case, and `!important` ends a value.
fn style_hides(style: &str) -> bool {
    style.split(';').any(|decl| {
        let Some((name, value)) = decl.split_once(':') else { return false };
        let value = match value.rsplit_once('!') {
            Some((v, flag)) if flag.trim().eq_ignore_ascii_case("important") => v,
            _ => value,
        };
        let (name, value) = (name.trim(), value.trim());
        (name.eq_ignore_ascii_case("display") && value.eq_ignore_ascii_case("none"))
            || (name.eq_ignore_ascii_case("visibility")
                && (value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("collapse")))
    })
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

/// The candidates of a `srcset`, split as the HTML standard splits them. A candidate is a URL, a run of characters with no
/// space in it (a CDN's URL may hold commas, as in `c_scale,w_400`), then an optional descriptor, `640w` or `2x`, which runs
/// to the next comma. Splitting on every comma would cut such a URL in two.
fn srcset_candidates(srcset: &str) -> Vec<(&str, Option<&str>)> {
    let mut out = Vec::new();
    let mut rest = srcset;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
        let end = rest.find(|c: char| c.is_ascii_whitespace()).unwrap_or(rest.len());
        let (url, after) = rest.split_at(end);
        if url.is_empty() {
            return out;
        }
        let bare = url.trim_end_matches(',');
        if bare.len() < url.len() {
            // Commas right after the URL end the candidate, so it has no descriptor.
            out.push((bare, None));
            rest = after;
        } else {
            let stop = after.find(',').unwrap_or(after.len());
            let descriptor = after[..stop].trim();
            out.push((url, Some(descriptor).filter(|d| !d.is_empty())));
            rest = &after[stop..];
        }
    }
}

/// What a descriptor gives: `640w` is 640 and `2x` is 2. No descriptor, or one it cannot read, counts as 1x.
fn scale(descriptor: Option<&str>) -> f32 {
    descriptor
        .and_then(|d| d.split_whitespace().next())
        .and_then(|d| d.trim_end_matches(['w', 'x']).parse::<f32>().ok())
        .unwrap_or(1.0)
}

/// The largest candidate of a srcset: one picture at several sizes, so the largest is the one to look at.
fn best_srcset(srcset: &str) -> Option<String> {
    srcset_candidates(srcset)
        .into_iter()
        .map(|(url, d)| (url, scale(d)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(u, _)| u.to_string())
}

/// Smallest srcset candidate that is still big enough to recognise (≥320w).
fn small_srcset(srcset: &str) -> Option<String> {
    srcset_candidates(srcset)
        .into_iter()
        .filter_map(|(url, d)| {
            let w = d?.split_whitespace().next()?.strip_suffix('w')?.parse::<u32>().ok()?;
            Some((url, w))
        })
        .filter(|(_, w)| *w >= 320)
        .min_by_key(|(_, w)| *w)
        .map(|(u, _)| u.to_string())
}

/// A link in a footnote mark (`<sup>`), or an image with no text: see [`Link::marginal`].
fn marginal(a: ElementRef) -> bool {
    let image_only = a.text().all(|t| t.trim().is_empty()) && a.select(&sel("img")).next().is_some();
    image_only || a.ancestors().filter_map(ElementRef::wrap).any(|e| e.value().name() == "sup")
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
    fn unfilled_template_placeholders_need_a_render() {
        // MEGA's pricing page: the text is long, but its script has not filled in the placeholders.
        let page = r#"<body><main><p>Compare the plans and pick the one that fits your team. Every plan includes encrypted
            storage, file sharing and apps for every device, and you can change plans at any time from your account.</p>
            <div class="plan-feature">!{freePlanStorage}</div><p>Get {{ planStorage }} free, then save up to ^{price} a month.</p>
            </main><script src="/app.js"></script></body>"#;
        assert!(html_with_placeholders(page, &base()).1);
    }

    #[test]
    fn braces_in_code_and_bare_braces_in_prose_are_not_placeholders() {
        let page = r#"<body><article><p>Set <code>${HOME}</code> and <code>{{ name }}</code>, then run the sample.</p>
            <pre><code>cd ${HOME}/bin
            echo {name} !{freePlanStorage} !{freePlanStorage}</code></pre>
            <p>The quantifier {n} repeats the atom, x^{n} is a power, and pass {min} and {max} as the bounds.</p>
            <p>A stray !{freePlanStorage} is one placeholder, not a template.</p><div hidden>!{freePlanStorage} ^{price}</div>
            </article><script src="/app.js"></script></body>"#;
        assert!(!html_with_placeholders(page, &base()).1);
    }

    #[test]
    fn placeholders_need_a_script_to_fill_them_in() {
        let page =
            r#"<body><p>Plan storage is !{freePlanStorage}, and transfer is !{freePlanTransfer} a month.</p></body>"#;
        assert!(!html_with_placeholders(page, &base()).1);
        let with_script = page.replace("</body>", "<script>load()</script></body>");
        assert!(html_with_placeholders(&with_script, &base()).1);
    }

    #[test]
    fn a_placeholder_is_a_sigil_or_double_braces_around_a_name() {
        assert_eq!(count_placeholders("!{freePlanStorage} ^{price} ${plan.total} {{ name }}"), 4);
        // Bare braces, one-letter names, digits, empty braces and braces after punctuation are not placeholders.
        assert_eq!(count_placeholders("{ab} x^{n} {min} !{a} ({ab}) {0} ${}"), 0);
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
    fn aria_hidden_alone_keeps_the_content() {
        // aria-hidden hides content from assistive technology only. A banner marked that way is on the page, so a reader
        // sees it and the walk keeps it.
        let page = r#"<body><article><p>The store opens at nine every morning.</p>
            <div aria-hidden="true"><p>Summer sale, up to 50% off.</p><img src="/banner.jpg" alt="Sale"></div></article></body>"#;
        let ex = html(page, &base());
        assert!(ex.blocks.iter().any(|b| b.text.contains("Summer sale")), "{:?}", ex.blocks);
        assert!(ex.images.iter().any(|i| i.url.path() == "/banner.jpg"), "{:?}", ex.images);
    }

    #[test]
    fn a_hidden_attribute_drops_the_content_below_it() {
        let page = r#"<body><article><p>The store opens at nine every morning.</p>
            <div hidden><p>Hidden promotion text.</p><img src="/banner.jpg" alt="Sale"></div></article></body>"#;
        let ex = html(page, &base());
        assert!(ex.blocks.iter().all(|b| !b.text.contains("Hidden promotion")), "{:?}", ex.blocks);
        assert!(ex.images.is_empty(), "{:?}", ex.images);
    }

    #[test]
    fn inline_display_none_on_an_ancestor_drops_the_content_below_it() {
        let page = r#"<body><article><p>The store opens at nine every morning.</p>
            <section style="display: none"><p>Hidden promotion text.</p><img src="/banner.jpg" alt="Sale"></section></article></body>"#;
        let ex = html(page, &base());
        assert!(ex.blocks.iter().all(|b| !b.text.contains("Hidden promotion")), "{:?}", ex.blocks);
        assert!(ex.images.is_empty(), "{:?}", ex.images);
    }

    #[test]
    fn visibility_hidden_on_an_ancestor_drops_the_content_below_it() {
        let page = r#"<body><article><p>The store opens at nine every morning.</p>
            <div style="visibility:hidden"><p>Hidden promotion text.</p><img src="/banner.jpg" alt="Sale"></div></article></body>"#;
        let ex = html(page, &base());
        assert!(ex.blocks.iter().all(|b| !b.text.contains("Hidden promotion")), "{:?}", ex.blocks);
        assert!(ex.images.is_empty(), "{:?}", ex.images);
    }

    #[test]
    fn an_inline_style_hides_as_css_reads_it() {
        assert!(style_hides("DISPLAY: None !important"));
        assert!(style_hides("color: red; visibility : hidden"));
        assert!(style_hides("visibility:collapse"));
        assert!(!style_hides("display: block; color: red"));
        assert!(!style_hides("visibility: visible"));
        // A property name inside a value is not a declaration.
        assert!(!style_hides("content: \"display:none\""));
    }

    #[test]
    fn a_picture_source_is_a_candidate_at_its_largest_size_and_its_smallest_big_one_is_the_preview() {
        let page = r#"<body><main><picture><source srcset="/s/banner-160.webp 160w, /s/banner-320.webp 320w, /s/banner-1280.webp 1280w" type="image/webp"><img src="/fallback.jpg" alt="Banner"></picture></main></body>"#;
        let ex = html(page, &base());
        let banner = ex.images.iter().find(|i| i.url.path() == "/s/banner-1280.webp").expect("the largest source");
        assert_eq!(banner.preview.path(), "/s/banner-320.webp");
        assert_eq!(banner.alt, "Banner");
        assert!(ex.images.iter().any(|i| i.url.path() == "/fallback.jpg"), "{:?}", ex.images);
    }

    #[test]
    fn a_picture_source_and_its_img_naming_one_file_are_one_candidate() {
        let page = r#"<body><main><picture><source srcset="/banner.webp 1x" type="image/webp"><img src="/banner.webp" alt="Banner"></picture></main></body>"#;
        let ex = html(page, &base());
        let urls: Vec<&str> = ex.images.iter().map(|i| i.url.path()).collect();
        assert_eq!(urls, ["/banner.webp"]);
        assert_eq!(ex.images[0].alt, "Banner");
    }

    #[test]
    fn a_picture_source_of_a_type_the_decoder_cannot_read_is_not_a_candidate() {
        let page = r#"<body><main><picture><source srcset="/banner.avif 1x" type="image/avif"><source srcset="/banner.webp 1x" type="image/webp"><img src="/fallback.jpg" alt="Banner"></picture></main></body>"#;
        let ex = html(page, &base());
        let urls: Vec<&str> = ex.images.iter().map(|i| i.url.path()).collect();
        assert!(urls.contains(&"/banner.webp") && urls.contains(&"/fallback.jpg"), "{urls:?}");
        assert!(!urls.iter().any(|u| u.ends_with(".avif")), "{urls:?}");
    }

    #[test]
    fn a_srcset_splits_into_its_urls_and_descriptors_as_the_standard_does() {
        assert_eq!(
            srcset_candidates("a.jpg, b.jpg 2x,c.jpg,d.jpg 640w"),
            [("a.jpg", None), ("b.jpg", Some("2x")), ("c.jpg,d.jpg", Some("640w"))]
        );
        // A CDN's transform list holds commas: each URL is still one candidate, and the largest one is chosen.
        let cdn =
            "https://cdn.example.test/c_scale,w_400/a.jpg 400w, https://cdn.example.test/c_scale,w_800/a.jpg 800w";
        assert_eq!(best_srcset(cdn).as_deref(), Some("https://cdn.example.test/c_scale,w_800/a.jpg"));
        assert_eq!(small_srcset(cdn).as_deref(), Some("https://cdn.example.test/c_scale,w_400/a.jpg"));
    }

    #[test]
    fn a_products_photo_is_an_image_candidate_after_og_image() {
        let page = r#"<html><head><meta property="og:image" content="https://example.com/og.jpg"><script type="application/ld+json">{"@type":"Product","name":"Asana","image":"https://cdn.example.test/product.jpg"}</script></head>
            <body><article><p>Asana is a to-do app for teams.</p><img src="/in-page.jpg" alt="Screenshot"></article></body></html>"#;
        let ex = html(page, &base());
        let urls: Vec<&str> = ex.images.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://example.com/og.jpg", "https://cdn.example.test/product.jpg", "https://example.com/in-page.jpg"]
        );
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

    /// The stack the deep-nesting tests run on: a tokio worker's, which is where the walk runs in production. In a debug
    /// build the walk at its depth cap needs between 0.5 and 1 MB of it, and the worst case (the walk at its cap, then
    /// inline text at its cap) between 1 and 1.5 MB.
    const DEEP_STACK: usize = 2 << 20;

    /// The block texts of `page`, read on a thread of `DEEP_STACK` bytes. A recursion too deep for that aborts the test.
    fn texts_on_a_deep_stack(page: String) -> Vec<String> {
        std::thread::Builder::new()
            .stack_size(DEEP_STACK)
            .spawn(move || html(&page, &base()).blocks.into_iter().map(|b| b.text).collect())
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn a_page_nested_deeper_than_the_walk_recurses_is_read_flat() {
        // Without the cap, 600 levels overflow DEEP_STACK in a debug build. This is 1,000, which parses in about 60 ms:
        // html5ever scans the open elements for each <div>, so parsing costs the square of the nesting.
        let page = format!("<body>{}Deep text is still read.{}</body>", "<div>".repeat(1_000), "</div>".repeat(1_000));
        let texts = texts_on_a_deep_stack(page);
        assert!(texts.iter().any(|t| t == "Deep text is still read."), "{texts:?}");
    }

    #[test]
    fn inline_elements_nested_deeper_than_the_walk_recurses_are_read_flat() {
        // Without the cap, 4,000 spans overflow DEEP_STACK in a debug build. This is 8,000.
        let page = format!("<body><p>{}Deep words.{}</p></body>", "<span>".repeat(8_000), "</span>".repeat(8_000));
        let texts = texts_on_a_deep_stack(page);
        assert!(texts.iter().any(|t| t == "Deep words."), "{texts:?}");
    }

    #[test]
    fn the_walk_and_the_inline_text_at_their_caps_fit_the_stack_together() {
        // The walk reaches its cap, then a paragraph there reaches the inline cap: the worst case. Without the caps it
        // overflows DEEP_STACK in a debug build; with them it fits.
        let page = format!(
            "<body>{}<p>{}Deep words.{}</p>{}</body>",
            "<div>".repeat(255),
            "<span>".repeat(4_000),
            "</span>".repeat(4_000),
            "</div>".repeat(255)
        );
        let texts = texts_on_a_deep_stack(page);
        assert!(texts.iter().any(|t| t == "Deep words."), "{texts:?}");
    }

    #[test]
    fn a_deep_subtree_keeps_what_a_reader_sees_and_no_more() {
        // Past the depth cap the text is read flat, which must leave out script, hidden text and a masthead header as
        // the walk does.
        let page = format!(
            r#"<body>{}<p>Shown text.</p><script>evil()</script><div style="display: none">Hidden text.</div><header>Masthead words.</header>{}</body>"#,
            "<div>".repeat(300),
            "</div>".repeat(300)
        );
        let text: String = html(&page, &base()).blocks.into_iter().map(|b| b.text + "|").collect();
        assert!(text.contains("Shown text."), "{text}");
        assert!(!text.contains("evil()") && !text.contains("Hidden text.") && !text.contains("Masthead"), "{text}");
    }

    #[test]
    fn a_lazy_image_previews_its_full_file_not_the_placeholder() {
        let ex = html(r#"<img src="/img/blank.gif" data-original="/photos/full.jpg">"#, &base());
        assert_eq!(ex.images.len(), 1);
        assert_eq!(ex.images[0].url.as_str(), "https://example.com/photos/full.jpg");
        assert_eq!(ex.images[0].preview.as_str(), "https://example.com/photos/full.jpg");
    }

    #[test]
    fn a_lazy_image_with_a_srcset_previews_the_small_candidate() {
        let page = r#"<img srcset="/s/small.jpg 320w, /s/big.jpg 1280w" src="/img/blank.gif" data-original="/photos/full.jpg">"#;
        let ex = html(page, &base());
        assert_eq!(ex.images[0].url.as_str(), "https://example.com/s/big.jpg");
        assert_eq!(ex.images[0].preview.as_str(), "https://example.com/s/small.jpg");
    }
}

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
/// The full-size file a lazy loader may keep as well. The image reads it after `LAZY_SRC`, never its preview does.
const LAZY_FULL_SIZE: &str = "data-original";

pub fn html(body: &str, base: &Url) -> Extracted {
    let doc = Html::parse_document(body);
    let root = content_root(&doc);
    let in_body = root.value().name() == "body";
    Extracted {
        title: collapse(&page_title(&doc)),
        blocks: join_short(walk(&doc, root, in_body)),
        images: collect_images(&doc, root, in_body, base),
        links: collect_links(root, in_body, base),
        site_links: collect_site_links(&doc, base),
        app_shell: is_app_shell(&doc, body),
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
    let mut w = Walker { blocks: Vec::new(), skip_header: in_body, buf: String::new(), segments, list: None, lists: 0 };
    w.container(root);
    w.flush();
    w.blocks
}

/// The images of the page: `og:image` first, then the `<img>`s of the content root that a reader can see.
fn collect_images(doc: &Html, root: ElementRef, in_body: bool, base: &Url) -> Vec<Image> {
    let mut images = Vec::new();
    if let Some(og) = meta(doc, "meta[property='og:image']") {
        push_image(&mut images, base, &og, None, String::new(), String::new(), None, None);
    }
    for img in root.select(&sel("img")) {
        if image_skipped(img, in_body) {
            continue;
        }
        let a = |k| img.value().attr(k);
        let srcsets = a("srcset").or(a("data-srcset"));
        let src = srcsets
            .and_then(best_srcset)
            .or(first_attr(img, LAZY_SRC).or(a(LAZY_FULL_SIZE)).or(a("src")).map(String::from));
        let Some(src) = src else { continue };
        let preview = srcsets
            .and_then(small_srcset)
            .or(first_attr(img, LAZY_SRC).or(a("src")).filter(|s| !s.starts_with("data:")).map(String::from));
        let alt = collapse(a("alt").or(a("title")).unwrap_or(""));
        let caption = figcaption(img);
        let dim = |k| a(k).and_then(|v: &str| v.trim_end_matches("px").parse().ok());
        push_image(&mut images, base, &src, preview.as_deref(), alt, caption, dim("width"), dim("height"));
    }
    images
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
    fn the_page_itself_is_not_an_image() {
        let page = r#"<html><head><meta property="og:image" content="?q=80"></head><body><article>
            <p>Text</p><img src="/blog/post?w=800" width="800" height="400"><img src="/a.png" width="800" height="400">
            </article></body></html>"#;
        let ex = html(page, &Url::parse("https://example.com/blog/post").unwrap());
        let urls: Vec<_> = ex.images.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(urls, ["https://example.com/a.png"]);
    }
}

//! HTML/markdown → candidate blocks and images. Every emitted string is text that
//! exists in the page; nothing here rewrites content beyond whitespace collapsing.
//!
//! `html` reads an HTML page, `markdown` a page the server sent as markdown, and `join` turns their short pieces into
//! blocks a reader can judge. This file holds the types both paths return and the helpers they share.

use serde::Serialize;
use url::Url;

mod block_images;
mod cards;
mod html;
mod join;
pub mod json_ld;
mod markdown;
pub mod sources;

pub use html::{html, html_with_placeholders};
pub use join::{SHORT_BLOCK_CHARS, short_items_of_kept_lists};
pub use markdown::markdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Heading,
    Para,
    Code,
    Quote,
    Item,
    Table,
    /// A value from the page's JSON-LD (see `json_ld`): structured data, not text the page shows. Only --precise answers
    /// from one, and the answer's link has no text fragment, since its words are not on the page.
    Structured,
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
    /// The page a headline links to, absolute (see `html::headline_href`). A heading, or a card (see `cards`) that holds a
    /// headline, can have one. Jev is asked about a headline as about a paragraph, where a section title is shown with its
    /// section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// A record of a repeated list (see `cards`): one card of it, its pieces (a title, a date, a price…) joined by lines in page
    /// order. It is the record's boundary in the output: numbered from 1 over the whole run, in page order, and printed under a
    /// `[record N]` line in the text. A record is always a candidate for Jev, however short its text is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<usize>,
    /// Which page of a list the block is from, counted from 1, when `--pages` read more than one (see `paging`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    /// The web addresses the block's links go to, in page order, at most `MAX_BLOCK_LINKS` (see `html::Walker::links_of`). A list's item
    /// often names its own site only as a link ("Web", or its name linked), so a list read takes the site from here: the text
    /// alone doesn't carry it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// The picture of what the block is about, when the page shows one beside it: an image whose card's title (see
    /// `Image::card`) is the block's first line, as a shop's product card shows its photo above its name, or, for a card that
    /// shows its price first, the one line of the block its title or alt names (see `block_images`). A list read takes
    /// an item's photo from here: the text alone doesn't carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

impl Block {
    /// A block with no heading level, code language, list, link, record or page: callers set those with struct update syntax.
    pub fn new(i: usize, kind: Kind, text: String) -> Block {
        Block {
            i,
            kind,
            level: None,
            lang: None,
            text,
            list: None,
            href: None,
            record: None,
            page: None,
            links: Vec::new(),
            image: None,
        }
    }

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
            Kind::Para | Kind::Table | Kind::Structured => self.text.clone(),
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
    /// The words around the image that name what it is of, when its own alt text and file name don't: its `title`, the
    /// text of the link it sits in (led by the link's one heading), or its card's heading (see `html::set_card`).
    pub card: String,
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

/// Another version of the page that the page declares (`<link rel="alternate" hreflang>`): the same page in another
/// language or market (see [`html`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Alternate {
    /// The `hreflang` as the page writes it (`en-GB`, `x-default`).
    pub hreflang: String,
    pub url: url::Url,
}

#[derive(Default)]
pub struct Extracted {
    pub title: String,
    /// The language the page declares (BCP 47), as written: its `<html lang>`, else its `xml:lang`. None when it
    /// declares none, and always for markdown.
    pub lang: Option<String>,
    /// The other versions of the page that it declares, in the order it lists them. Empty for markdown.
    pub alternates: Vec<Alternate>,
    pub blocks: Vec<Block>,
    /// The page's JSON-LD values (see [`json_ld`]): --precise answers from them only when its text gives no answer.
    pub structured: Vec<Block>,
    pub images: Vec<Image>,
    pub links: Vec<Link>,
    /// Every link on the page, menus and footers included: how `--follow` moves around a site.
    pub site_links: Vec<Link>,
    /// The page after this one in its list, when the page links to one (see `paging`): what `--pages` reads next.
    pub next_page: Option<url::Url>,
    /// The pages a list was read from, in order, when `--pages` read more than one (see `paging::merge`); empty otherwise.
    pub pages: Vec<url::Url>,
    /// Scripts plus an empty mount point, `<noscript>` or a heavy shell: with almost no
    /// text, the page is a JS app.
    pub app_shell: bool,
}

/// Where inline text crosses a block boundary (`<li>`s in a table cell, a `<br>`): `collapse` makes it a
/// newline, as the page shows it, so "Dylan Field" and "Evan Wallace" don't read as one name.
const BREAK: char = '\u{1F}';

/// Adds an image, or fills in what an earlier copy of it lacks, and returns its place in `out`; none when it is left out.
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
) -> Option<usize> {
    let src = src.trim();
    if src.is_empty() || src.starts_with('#') {
        return None;
    }
    let url = base.join(src).ok()?;
    if !matches!(url.scheme(), "http" | "https") || noise(&url, width, height) {
        return None;
    }
    // A src that resolves to the page itself ("?q=80" in an og:image left without its path) is the page, not a
    // picture of anything.
    let bare = |u: &Url| u.as_str().split(['?', '#']).next().unwrap_or_default().to_string();
    if bare(&url) == bare(base) {
        return None;
    }
    if let Some(existing) = out.iter_mut().find(|i| i.url == url) {
        if existing.alt.is_empty() {
            existing.alt = alt;
        }
        if existing.caption.is_empty() {
            existing.caption = caption;
        }
        return Some(existing.i);
    }
    let preview = preview.and_then(|p| base.join(p.trim()).ok()).unwrap_or_else(|| url.clone());
    let i = out.len();
    out.push(Image { i, url, preview, alt, caption, card: String::new(), width, height });
    Some(i)
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

fn push_link(out: &mut Vec<Link>, base: &Url, href: &str, text: String, context: String, marginal: bool) {
    let Some(mut url) = link_url(base, href) else { return };
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

/// The most links one block keeps (see `Block::links`): a card or a row names a site or two, and a long table is cut short.
pub(crate) const MAX_BLOCK_LINKS: usize = 20;

/// The web address a link goes to: its `href` resolved against the page, `None` for an empty one, a `#fragment` or another
/// scheme (`mailto:`, `tel:`, `javascript:`). An `href` that starts with `www.` is a site whose `https://` the page left out:
/// resolved as written it would be a path of the page ("/madrid/www.albaonline.org"), which no reader means.
pub(crate) fn link_url(base: &Url, href: &str) -> Option<Url> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let url = if href.get(..4).is_some_and(|w| w.eq_ignore_ascii_case("www.")) {
        Url::parse(&format!("https://{href}")).ok()?
    } else {
        base.join(href).ok()?
    };
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// Adds `more` to a block's links, in order, each once, up to `MAX_BLOCK_LINKS`.
pub(crate) fn add_links(links: &mut Vec<String>, more: impl IntoIterator<Item = String>) {
    for link in more {
        if links.len() >= MAX_BLOCK_LINKS {
            return;
        }
        if !links.contains(&link) {
            links.push(link);
        }
    }
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

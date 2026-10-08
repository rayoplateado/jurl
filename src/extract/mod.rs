//! HTML/markdown → candidate blocks and images. Every emitted string is text that
//! exists in the page; nothing here rewrites content beyond whitespace collapsing.

use serde::Serialize;
use url::Url;

mod html;
mod join;
mod markdown;

pub use html::html;
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

/// Where inline text crosses a block boundary (`<li>`s in a table cell, a `<br>`): `collapse` makes it a
/// newline, as the page shows it, so "Dylan Field" and "Evan Wallace" don't read as one name.
const BREAK: char = '\u{1F}';

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

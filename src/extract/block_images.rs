//! The picture beside a block. A shop's product card, a venue's event card or a shelter's dog card shows its photo next to its
//! name, but the image is no text, so a block of the card doesn't carry it. An image already knows the words that name it (its
//! card, see `Image::card`: its link's text or its card's one heading). A block whose first line is the first line of those
//! words, the card's title, is the one the image shows: a card's other lines (a format, a price) are shared by many cards and
//! name none. Two cards with one name ("Aceite de oliva 0,4º Hacendado", a 5 L and a 1 L) take their images in page order.
//! Images are kept once per address, so cards that share a placeholder give it to the first of them only.
//!
//! A card that puts its price before its name ("18,95 €", "3,79 €/l", then the name) starts its block with the price, so no
//! title matches its first line. Its photo's alt text still names it: an image left over is given to the one block with a
//! line that is its alt, when the alt is a name (two words or more, not a word like "logo") and no other block has that line.

use super::{Block, Image};

/// The fewest characters a card's title needs to name a block: a word or two of a button ("Ver") names nothing.
const MIN_NAME_CHARS: usize = 4;

/// The fewest words an alt text needs to name a block by itself: one word ("Aceite", "Producto") names a kind, not a thing.
const MIN_ALT_WORDS: usize = 2;

/// Words that make an alt text describe the picture rather than name what it shows ("Company logo", "Product image").
const GENERIC_ALT_WORDS: &[&str] =
    &["logo", "icon", "icono", "banner", "image", "imagen", "photo", "foto", "picture", "placeholder", "thumbnail"];

/// Gives each block the first image, in page order, whose card's title is the block's first line, then each image left
/// over to the one block with a line that is its alt text; each image goes to one block at most.
pub fn attach(blocks: &mut [Block], images: &[Image]) {
    let first_lines: Vec<String> =
        blocks.iter().map(|b| b.text.lines().next().unwrap_or_default().trim().to_string()).collect();
    let mut used = vec![false; images.len()];
    for (n, image) in images.iter().enumerate() {
        let title = image.card.lines().next().unwrap_or_default().trim();
        if title.chars().count() < MIN_NAME_CHARS {
            continue;
        }
        let found = (0..blocks.len()).find(|&at| blocks[at].image.is_none() && first_lines[at] == title);
        if let Some(at) = found {
            blocks[at].image = Some(image.url.to_string());
            used[n] = true;
        }
    }
    let lines: Vec<Vec<String>> = blocks.iter().map(|b| b.text.lines().map(normalise).collect()).collect();
    for (image, _) in images.iter().zip(used).filter(|(_, used)| !used) {
        let Some(alt) = name(&image.alt) else { continue };
        let mut found = (0..blocks.len()).filter(|&at| lines[at].contains(&alt));
        if let (Some(at), None) = (found.next(), found.next())
            && blocks[at].image.is_none()
        {
            blocks[at].image = Some(image.url.to_string());
        }
    }
}

/// The alt text as a name to look for, normalised, or None when it names nothing (see `MIN_ALT_WORDS`, `GENERIC_ALT_WORDS`).
fn name(alt: &str) -> Option<String> {
    let alt = normalise(alt);
    let words: Vec<&str> = alt.split(' ').filter(|w| !w.is_empty()).collect();
    let generic = words.iter().any(|w| GENERIC_ALT_WORDS.contains(w));
    (words.len() >= MIN_ALT_WORDS && !generic).then_some(alt)
}

/// A line as it is compared: lower case, with its runs of white space made one space.
fn normalise(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

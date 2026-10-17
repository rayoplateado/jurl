//! The picture beside a block. A shop's product card, a venue's event card or a shelter's dog card shows its photo next to its
//! name, but the image is no text, so a block of the card doesn't carry it. An image already knows the words that name it (its
//! card, see `Image::card`: its link's text or its card's one heading). A block whose first line is the card's title (the
//! first line of those words that names something: a price or a measure names nothing) is the one the image shows: a card's
//! other lines (a format, a price) are shared by many cards and name none. Two cards with one name ("Aceite de oliva 0,4º
//! Hacendado", a 5 L and a 1 L) take their images in page order. Images are kept once per address, so cards that share a
//! placeholder give it to the first of them only.
//!
//! A card that shows its price before its name ("18,95 €", "3,79 €/l", then the name) starts its block with the price, so
//! no title is its first line. An image left over then goes to the one block with a line that is its card's title, or,
//! when the card has none or no block has it, its alt text when that is a name (two words or more, not "Product image").
//! A line two blocks have names neither. Headings are left out of this: a page's heading is not a card, though a banner's
//! alt may repeat it. When several images name one block, the one naming its longest line (the product's name, not a
//! brand strip's) wins, else the first in page order.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::{Block, Image, Kind};

/// The fewest characters a card's title needs to name a block: a word or two of a button ("Ver") names nothing.
const MIN_NAME_CHARS: usize = 4;

/// The fewest letters in a row a line needs to name something: "3,79 €/l", "500 g" or "4,50 EU" are a price or a measure.
const MIN_WORD_LETTERS: usize = 3;

/// The fewest words an alt text needs to name a block by itself: one word ("Aceite", "Producto") names a kind, not a thing.
const MIN_ALT_WORDS: usize = 2;

/// Alt texts that describe the picture rather than name what it shows, compared whole, as `alt_name` normalises them.
const GENERIC_ALTS: &[&str] = &[
    "product image",
    "product photo",
    "product picture",
    "company logo",
    "no image",
    "image not available",
    "imagen del producto",
    "foto del producto",
    "imagen de producto",
    "foto de producto",
    "sin imagen",
    "imagen no disponible",
];

/// Gives each block the first image, in page order, whose card's title is the block's first line, then each image left
/// over to the one block that has its title or its alt as a line (see the module's docs); each image goes to one block
/// at most, and each block takes one image.
pub fn attach(blocks: &mut [Block], images: &[Image]) {
    let first_lines: Vec<String> =
        blocks.iter().map(|b| normalise(b.text.lines().next().unwrap_or_default())).collect();
    let mut left = Vec::new();
    for image in images {
        let title = title(&image.card);
        let found = title
            .as_ref()
            .and_then(|t| (0..blocks.len()).find(|&at| blocks[at].image.is_none() && first_lines[at] == *t));
        match found {
            Some(at) => blocks[at].image = Some(image.url.to_string()),
            None => left.push((image, title)),
        }
    }
    if left.is_empty() {
        return;
    }
    let owners = line_owners(blocks);
    let mut chosen: HashMap<usize, (&Image, bool)> = HashMap::new();
    for (image, title) in left {
        let named =
            title.into_iter().chain(alt_name(&image.alt)).find_map(|name| Some((owners.get(&name).copied()??, name)));
        let Some((at, name)) = named else { continue };
        if blocks[at].image.is_some() {
            continue;
        }
        let longest = name == normalise(longest_line(&blocks[at].text));
        match chosen.entry(at) {
            Entry::Vacant(e) => {
                e.insert((image, longest));
            }
            Entry::Occupied(mut e) if longest && !e.get().1 => {
                e.insert((image, longest));
            }
            Entry::Occupied(_) => {}
        }
    }
    for (at, (image, _)) in chosen {
        blocks[at].image = Some(image.url.to_string());
    }
}

/// Each normalised line of the blocks that aren't headings, with the one block that has it, or None when two do.
fn line_owners(blocks: &[Block]) -> HashMap<String, Option<usize>> {
    let mut owners: HashMap<String, Option<usize>> = HashMap::new();
    for (at, block) in blocks.iter().enumerate().filter(|(_, b)| b.kind != Kind::Heading) {
        for line in block.text.lines().map(normalise).filter(|l| !l.is_empty()) {
            owners.entry(line).and_modify(|owner| *owner = owner.filter(|&o| o == at)).or_insert(Some(at));
        }
    }
    owners
}

/// A card's title, normalised: its first line that names something (see `names_something`), or None.
fn title(card: &str) -> Option<String> {
    card.lines().map(normalise).find(|l| l.chars().count() >= MIN_NAME_CHARS && names_something(l))
}

/// The alt text as a name to look for, normalised, or None when it names nothing: fewer than `MIN_ALT_WORDS` words, no
/// word (a price or a measure), or a description of the picture (`GENERIC_ALTS`, punctuation aside).
fn alt_name(alt: &str) -> Option<String> {
    let alt = normalise(alt);
    let bare: String = alt.chars().filter(|c| !c.is_ascii_punctuation()).collect();
    let bare = normalise(&bare);
    let named =
        bare.split(' ').count() >= MIN_ALT_WORDS && names_something(&bare) && !GENERIC_ALTS.contains(&bare.as_str());
    named.then_some(alt)
}

/// Whether a line holds a word (`MIN_WORD_LETTERS` letters in a row), not only a price, a number or a measure.
fn names_something(line: &str) -> bool {
    let mut run = 0;
    line.chars().any(|c| {
        run = if c.is_alphabetic() { run + 1 } else { 0 };
        run >= MIN_WORD_LETTERS
    })
}

/// The block's longest line, by characters.
fn longest_line(text: &str) -> &str {
    text.lines().max_by_key(|l| l.trim().chars().count()).unwrap_or_default()
}

/// A line as both passes compare it: lower case, trimmed, with its runs of white space made one space.
fn normalise(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_first_card_line_that_names_something() {
        assert_eq!(title("4,50 €\n0,90 €/l\nAceite  de Oliva 1 L").as_deref(), Some("aceite de oliva 1 l"));
        assert_eq!(title("500 g\n3,79 €/kg"), None);
        assert_eq!(title("Ver"), None);
    }

    #[test]
    fn an_alt_names_a_block_when_it_is_a_name() {
        assert_eq!(alt_name(" Aceite de girasol  1 l. ").as_deref(), Some("aceite de girasol 1 l."));
        assert_eq!(alt_name("Aceite"), None);
        assert_eq!(alt_name("Product image."), None);
        assert_eq!(alt_name("Imagen del producto"), None);
        assert_eq!(alt_name("18,95 € 3,79 €/l"), None);
        // A generic word inside a name doesn't make it generic.
        assert_eq!(alt_name("Logo bordado gorra azul").as_deref(), Some("logo bordado gorra azul"));
    }

    #[test]
    fn a_line_two_blocks_have_has_no_owner_and_headings_own_none() {
        let block = |kind, text: &str| Block::new(0, kind, text.to_string());
        let blocks = [
            block(Kind::Para, "1 €\nUno dos\nUno dos"),
            block(Kind::Para, "2 €\nTres cuatro\nuno  DOS"),
            block(Kind::Heading, "Cinco seis"),
        ];
        let owners = line_owners(&blocks);
        assert_eq!(owners.get("uno dos"), Some(&None));
        assert_eq!(owners.get("tres cuatro"), Some(&Some(1)));
        assert_eq!(owners.get("cinco seis"), None);
    }
}

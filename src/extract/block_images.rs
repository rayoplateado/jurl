//! The picture beside a block. A shop's product card, a venue's event card or a shelter's dog card shows its photo next to its
//! name, but the image is no text, so a block of the card doesn't carry it. An image already knows the words that name it (its
//! card, see `Image::card`: its link's text or its card's one heading). A block whose first line is the first line of those
//! words, the card's title, is the one the image shows: a card's other lines (a format, a price) are shared by many cards and
//! name none. Two cards with one name ("Aceite de oliva 0,4º Hacendado", a 5 L and a 1 L) take their images in page order.
//! Images are kept once per address, so cards that share a placeholder give it to the first of them only.

use super::{Block, Image};

/// The fewest characters a card's title needs to name a block: a word or two of a button ("Ver") names nothing.
const MIN_NAME_CHARS: usize = 4;

/// Gives each block the first image, in page order, whose card's title is the block's first line, and each image to one
/// block at most.
pub fn attach(blocks: &mut [Block], images: &[Image]) {
    let first_lines: Vec<String> =
        blocks.iter().map(|b| b.text.lines().next().unwrap_or_default().trim().to_string()).collect();
    for image in images {
        let title = image.card.lines().next().unwrap_or_default().trim();
        if title.chars().count() < MIN_NAME_CHARS {
            continue;
        }
        let found = (0..blocks.len()).find(|&at| blocks[at].image.is_none() && first_lines[at] == title);
        if let Some(at) = found {
            blocks[at].image = Some(image.url.to_string());
        }
    }
}

//! The picture beside a block. A shop's product card, a venue's event card or a shelter's dog card shows its photo next to its
//! name, but the image is no text, so a block of the card doesn't carry it. An image already knows the words that name it (its
//! card, see `Image::card`: its link's text or its card's one heading). A block whose first line is those words is the one the
//! image shows. Two cards with one name ("Aceite de oliva 0,4º Hacendado", a 5 L and a 1 L) take their images in page order.

use super::{Block, Image};

/// The fewest characters a card's words need to name a block: a word or two of a button ("Ver") names nothing.
const MIN_NAME_CHARS: usize = 4;

/// Gives each block the first image, in page order, whose card names it, and each image to one block at most.
pub fn attach(blocks: &mut [Block], images: &[Image]) {
    for image in images {
        let names: Vec<&str> =
            image.card.lines().map(str::trim).filter(|n| n.chars().count() >= MIN_NAME_CHARS).collect();
        if names.is_empty() {
            continue;
        }
        let first_line = |b: &Block| b.text.lines().next().unwrap_or_default().trim().to_string();
        if let Some(block) = blocks.iter_mut().find(|b| b.image.is_none() && names.contains(&first_line(b).as_str())) {
            block.image = Some(image.url.to_string());
        }
    }
}

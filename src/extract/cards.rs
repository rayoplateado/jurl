//! Cards: a repeated list (a concert calendar, a job board, a price grid) read one block per card. On many pages a card's
//! date comes before its title, or its title is last, so a page read as a flat list of blocks puts each title next to the
//! next card's date. A card is one block here, its pieces in page order, so a reader pairs the fields of a card where they are.

use std::collections::HashMap;

use super::{Block, Kind, SHORT_BLOCK_CHARS, add_links};

/// The most text a card holds. The judge reads a block's first 1,200 characters, so a card within that is read whole. A
/// longer run of siblings is article text, not a card list, and is read as it was.
const CARD_MAX_CHARS: usize = 1_200;
/// The fewest members a repeated structure needs to be a card list: two of anything is a pair.
const MIN_MEMBERS: usize = 3;

/// A child of a container, as the walk read it: its shape (its tags, see `html::shape`), and the blocks it produced, which are
/// `start..end` in the container's block list.
pub struct Member {
    pub shape: String,
    pub start: usize,
    pub end: usize,
}

/// The blocks of one container, with each card list among its children merged: every member of a list becomes one card
/// block. A list is merged as a whole or not at all, so cards and plain text are never mixed in one list: a member that is not
/// card-shaped (see `card_shaped`) keeps the whole list as it was.
pub fn merge_cards(blocks: Vec<Block>, members: &[Member]) -> Vec<Block> {
    let mut lists: HashMap<&str, Vec<&Member>> = HashMap::new();
    for m in members.iter().filter(|m| m.end > m.start) {
        lists.entry(m.shape.as_str()).or_default().push(m);
    }
    let mut cards: Vec<&Member> = lists
        .into_values()
        .filter(|list| list.len() >= MIN_MEMBERS && list.iter().all(|m| card_shaped(&blocks[m.start..m.end])))
        .flatten()
        .collect();
    if cards.is_empty() {
        return blocks;
    }
    cards.sort_by_key(|m| m.start);
    let mut out = Vec::with_capacity(blocks.len());
    let mut cards = cards.into_iter().peekable();
    let mut pieces = Vec::new();
    for (at, block) in blocks.into_iter().enumerate() {
        match cards.peek() {
            Some(m) if at >= m.start => {
                pieces.push(block);
                if at + 1 == m.end {
                    out.push(card_of(std::mem::take(&mut pieces)));
                    cards.next();
                }
            }
            _ => out.push(block),
        }
    }
    out
}

/// One record: its pieces' text, one piece to a line, the first headline a piece holds as its link, and its pieces' links. Its
/// number is set by `number_records`, once the whole page is read.
fn card_of(pieces: Vec<Block>) -> Block {
    let href = pieces.iter().find_map(|b| b.href.clone());
    let text = pieces.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
    let mut links = Vec::new();
    add_links(&mut links, pieces.into_iter().flat_map(|b| b.links));
    Block { href, record: Some(0), links, ..Block::new(0, Kind::Para, text) }
}

/// Numbers the records of a page from 1 in page order, so each record's boundary is a number the output and the JSON share.
pub fn number_records(blocks: &mut [Block]) {
    let mut n = 0;
    for b in blocks.iter_mut().filter(|b| b.record.is_some()) {
        n += 1;
        b.record = Some(n);
    }
}

/// Whether a member's blocks can be a card: at least two pieces of text a reader sees, no more than `CARD_MAX_CHARS`, and at
/// most one piece with a line too long to be a field. A card holds one title, which may be long, and short fields: a date, a
/// price, a venue. A teaser (a headline and a summary) and a section of a document (a heading and paragraphs) hold more prose
/// than that, and are read as they were.
fn card_shaped(pieces: &[Block]) -> bool {
    pieces.len() >= 2
        && pieces.iter().all(|b| matches!(b.kind, Kind::Heading | Kind::Para | Kind::Item))
        && pieces.iter().map(|b| b.text.chars().count() + 1).sum::<usize>() <= CARD_MAX_CHARS
        && pieces.iter().filter(|b| !is_field(b)).count() <= 1
}

/// A piece that is a field: every line of it is short, so it is a date, a price, a venue or a short title, and a run of such
/// lines (a date, a time, a day) is a field too.
fn is_field(b: &Block) -> bool {
    b.text.lines().all(|l| l.chars().count() < SHORT_BLOCK_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(kind: Kind, text: &str) -> Block {
        Block::new(0, kind, text.to_string())
    }

    /// Blocks of `pieces` per member, with each member's range.
    fn lists_of(members: &[(&str, Vec<Block>)]) -> (Vec<Block>, Vec<Member>) {
        let (mut blocks, mut out) = (Vec::new(), Vec::new());
        for (shape, pieces) in members {
            let start = blocks.len();
            blocks.extend(pieces.iter().cloned());
            out.push(Member { shape: shape.to_string(), start, end: blocks.len() });
        }
        (blocks, out)
    }

    fn card(text: &str) -> Vec<Block> {
        vec![piece(Kind::Para, "16 Oct"), piece(Kind::Heading, text)]
    }

    #[test]
    fn each_member_of_a_card_list_is_one_card_block_with_its_pieces_in_order() {
        let (blocks, members) = lists_of(&[
            ("div(div)", card("Carmen Lillo")),
            ("div(div)", card("9louro")),
            ("div(div)", card("Baile Europa")),
        ]);
        let merged = merge_cards(blocks, &members);
        let texts: Vec<_> = merged.iter().map(|b| (b.record.is_some(), b.text.as_str())).collect();
        assert_eq!(texts, [(true, "16 Oct\nCarmen Lillo"), (true, "16 Oct\n9louro"), (true, "16 Oct\nBaile Europa")]);
    }

    #[test]
    fn a_card_keeps_the_headline_it_holds_as_its_link() {
        let mut title = piece(Kind::Heading, "Riyadh airport hit again after a deadly attack");
        title.href = Some("https://example.com/world/riyadh".into());
        let (blocks, members) = lists_of(&[
            ("article(div)", vec![piece(Kind::Para, "Sat"), title.clone()]),
            ("article(div)", vec![piece(Kind::Para, "Sun"), piece(Kind::Heading, "Second story, a headline")]),
            ("article(div)", vec![piece(Kind::Para, "Mon"), piece(Kind::Heading, "Third story, a headline")]),
        ]);
        let merged = merge_cards(blocks, &members);
        assert_eq!(merged[0].href.as_deref(), Some("https://example.com/world/riyadh"));
        assert_eq!(merged[1].href, None);
    }

    #[test]
    fn fewer_than_three_members_or_a_list_without_short_text_is_left_as_it_was() {
        let (blocks, members) = lists_of(&[("div(div)", card("Carmen Lillo")), ("div(div)", card("9louro"))]);
        assert_eq!(merge_cards(blocks.clone(), &members).len(), blocks.len(), "two members are a pair");

        let long = |t: &str| {
            vec![piece(Kind::Heading, t), piece(Kind::Para, "A summary of the story that runs on for a while.")]
        };
        let (blocks, members) = lists_of(&[
            ("article(div)", long("First story headline, long enough")),
            ("article(div)", long("Second story headline, long enough")),
            ("article(div)", long("Third story headline, long enough")),
        ]);
        assert!(
            merge_cards(blocks.clone(), &members).iter().all(|b| b.record.is_none()),
            "teasers with no short field are not cards"
        );
    }

    #[test]
    fn a_card_has_one_long_title_and_short_fields_even_when_a_field_has_lines() {
        // A date of three lines is short field by field, and the title is the one long piece.
        let card = |title: &str| {
            vec![
                piece(Kind::Para, "Concert Dilluns\n12.10.2026\n20:30"),
                piece(Kind::Para, title),
                piece(Kind::Para, "18€"),
            ]
        };
        let (blocks, members) = lists_of(&[
            ("article(div,div,div)", card("BIGWÉ @Doun Doun Mondays afrobeat")),
            ("article(div,div,div)", card("MC LUANNA rap carioca")),
            ("article(div,div,div)", card("MEMOCRACIA + Hollow Empty Minds + Los Valientes punk")),
        ]);
        let merged = merge_cards(blocks, &members);
        assert_eq!(merged.iter().filter(|b| b.record.is_some()).count(), 3, "{merged:?}");
    }

    #[test]
    fn document_sections_are_not_cards() {
        // Sections of a document: a short heading, then long paragraphs and a subsection. Only the first piece is short.
        let section = |title: &str| {
            vec![
                piece(Kind::Heading, title),
                piece(Kind::Para, "Methods of file objects are described on this page, with an example for each one."),
                piece(
                    Kind::Para,
                    "Each method reads or writes the file in a different way, as its own paragraph says.",
                ),
                piece(Kind::Heading, "7.1.1. Formatted string literals"),
            ]
        };
        let (blocks, members) = lists_of(&[
            ("section(div(h2),p,p,div(h3))", section("7. Input and output")),
            ("section(div(h2),p,p,div(h3))", section("8. Errors")),
            ("section(div(h2),p,p,div(h3))", section("9. Classes")),
        ]);
        assert!(merge_cards(blocks.clone(), &members).iter().all(|b| b.record.is_none()));
    }

    #[test]
    fn a_list_is_merged_whole_or_not_at_all() {
        // One member has a code block: no card in the list, and every other member is read as it was.
        let mut odd = card("Odd one");
        odd.push(piece(Kind::Code, "npm install"));
        let (blocks, members) = lists_of(&[
            ("div(div)", card("Carmen Lillo")),
            ("div(div)", odd),
            ("div(div)", card("9louro")),
            ("div(div)", card("Baile Europa")),
        ]);
        let merged = merge_cards(blocks.clone(), &members);
        assert_eq!(merged.len(), blocks.len());
        assert!(merged.iter().all(|b| b.record.is_none()));
    }

    #[test]
    fn a_member_with_no_blocks_does_not_count_against_its_list() {
        // A hidden card (no blocks) is left out of the list, and the three cards that have text still form one.
        let (blocks, mut members) = lists_of(&[
            ("div(div)", card("Carmen Lillo")),
            ("div(div)", Vec::new()),
            ("div(div)", card("9louro")),
            ("div(div)", card("Baile Europa")),
        ]);
        members[1].end = members[1].start;
        let merged = merge_cards(blocks, &members);
        assert_eq!(merged.iter().filter(|b| b.record.is_some()).count(), 3);
    }

    #[test]
    fn two_lists_of_one_container_are_merged_each_on_its_own() {
        let (blocks, members) = lists_of(&[
            ("div(div)", card("A1")),
            ("p", vec![piece(Kind::Para, "A paragraph in between, not a card at all.")]),
            ("div(div)", card("A2")),
            ("div(div)", card("A3")),
            ("span(b)", card("B1")),
            ("span(b)", card("B2")),
        ]);
        let merged = merge_cards(blocks, &members);
        // The paragraph is one block of the page, and the three `div`s are a list; the two `span`s are too few.
        let kinds: Vec<_> = merged.iter().map(|b| b.record.is_some()).collect();
        assert_eq!(kinds, [true, false, true, true, false, false, false, false]);
    }

    #[test]
    fn records_are_numbered_from_one_in_page_order() {
        let record = |i: usize, text: &str| Block { record: Some(0), ..Block::new(i, Kind::Para, text.to_string()) };
        let mut blocks = vec![
            Block::new(0, Kind::Para, "Intro".into()),
            record(1, "A"),
            Block::new(2, Kind::Para, "B".into()),
            record(3, "C"),
        ];
        number_records(&mut blocks);
        let got: Vec<_> = blocks.iter().map(|b| b.record).collect();
        assert_eq!(got, [None, Some(1), None, Some(2)]);
    }
}

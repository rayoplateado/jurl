//! Cards: a repeated list (a concert calendar, a job board, a price grid) read one block per card. On many pages a card's
//! date comes before its title, or its title is last, so a page read as a flat list of blocks puts each title next to the
//! next card's date. A card is one block here, its pieces in page order, so a reader pairs the fields of a card where they are.
//! A card that holds prose (a description under its title) is too long to be one block the judge reads whole: its pieces
//! stay blocks, and each carries the card's record number, so a reader still knows where the card ends.

use std::collections::HashMap;

use super::{Block, Kind, SHORT_BLOCK_CHARS, add_links};

/// The most text a card holds. The judge reads a block's first 1,200 characters, so a card within that is read whole. A
/// longer run of siblings is article text, not a card list, and is read as it was.
const CARD_MAX_CHARS: usize = 1_200;
/// The most text a record holds when its pieces stay blocks: one card of a list, with its description.
const RECORD_MAX_CHARS: usize = 6_000;
/// The fewest members a repeated structure needs to be a card list: two of anything is a pair.
const MIN_MEMBERS: usize = 3;
/// The fewest pieces a card with prose has: a headline and its summary are a teaser.
const RECORD_MIN_PIECES: usize = 3;
/// ...and the most: a card with a description has a few paragraphs, not a section's worth of blocks.
const RECORD_MAX_PIECES: usize = 24;
/// The most nodes two shapes may differ by and still be one list: a badge or a rating that only some cards have.
const NEAR_NODES: usize = 2;
/// ...and by at most this share of the smaller shape's nodes: one in this many.
const NEAR_SHARE: usize = 3;
/// A line is prose when it has this many words and ends as a sentence does, or when it is this long.
const PROSE_WORDS: usize = 7;
const PROSE_CHARS: usize = 90;

/// A child of a container, as the walk read it: its shape (its tags, see `html::shape`), and the blocks it produced, which are
/// `start..end` in the container's block list.
pub struct Member {
    pub shape: String,
    pub start: usize,
    pub end: usize,
}

/// What a list of members is read as: each member one block (`Card`), or each member's blocks kept and numbered (`Record`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tier {
    Card,
    Record,
}

/// What the card rule remembers over the whole page: the ids it has given, and the shapes that made a list somewhere, so a
/// row of a grid with one or two cards left (the last one) is read as the cards of the rows before it.
#[derive(Default)]
pub struct Lists {
    next_id: usize,
    /// The container that held the container of the members (a grid, for its rows), their shape, and what they made.
    known: Vec<(usize, String, String, Tier)>,
}

impl Lists {
    fn id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }
}

/// The blocks of one container, with each card list among its children merged: every member of a list becomes one card
/// block, or, for a card that holds prose, its blocks all take one record number. A list is merged as a whole or not at all,
/// so cards and plain text are never mixed in one list: a member that is not card-shaped (see `card_shaped`) keeps the whole
/// list as it was. The members of a group of fewer than three join a list, though: one of this container whose shape is
/// near theirs (a card with a badge the others lack), or one of the same shape in a container of the same shape and parent (a grid's
/// last row, `grid` being the container of its rows and `container` the shape of the one that holds these members).
pub fn merge_cards(
    blocks: Vec<Block>,
    members: &[Member],
    grid: usize,
    container: &str,
    lists: &mut Lists,
) -> Vec<Block> {
    let container = loose(container);
    let members: Vec<&Member> = members.iter().filter(|m| m.end > m.start).collect();
    let mut taken: Vec<(&Member, Tier)> = Vec::new();
    let mut made: Vec<(String, Tier)> = Vec::new();
    let mut rest: Vec<&Member> = Vec::new();

    // Cards: three or more members of one shape, with the same number of parts in each, that are all card-shaped.
    let mut left: Vec<&Member> = Vec::new();
    for (shape, list) in grouped(members, |m| m.shape.clone()) {
        if list.len() >= MIN_MEMBERS && list.iter().all(|m| card_shaped(&blocks[m.start..m.end])) {
            made.push((shape, Tier::Card));
            taken.extend(list.into_iter().map(|m| (m, Tier::Card)));
        } else {
            left.extend(list);
        }
    }
    // Records: members that differ only in how many paragraphs (or other plain parts) they hold, whose pieces stay blocks. A
    // group of three or more that is not records as a whole stays as it was: none of its members joins another list.
    // A member that is a row of cells (it holds a run of containers of one shape, as a grid's row does) is no record: its cells are.
    left.retain(|m| !is_row(&m.shape));
    for (shape, list) in grouped(left, |m| loose(&m.shape)) {
        if list.len() < MIN_MEMBERS {
            rest.extend(list);
        } else if list.iter().all(|m| record_shaped(&blocks[m.start..m.end])) {
            made.push((shape, Tier::Record));
            taken.extend(list.into_iter().map(|m| (m, Tier::Record)));
        }
    }
    // The members of a group too small to be a list join a list of this container whose shape is near theirs (a card with a
    // badge the others lack), or one of the same shape found earlier on the page (a grid's last row, a tab's card).
    for m in rest {
        // A card of blocks of its own is a record wherever it is on the page once a list of them has been found (a listing's
        // tabs each hold one or two of the cards of the first); a card read as one block needs the grid of its list as well.
        let known = lists
            .known
            .iter()
            .find(|(of, held, shape, tier)| match tier {
                Tier::Record => *shape == loose(&m.shape),
                Tier::Card => *of == grid && *held == container && *shape == m.shape,
            })
            .map(|(_, _, _, tier)| *tier);
        let tier = known.or_else(|| made.iter().find(|(shape, _)| near(&m.shape, shape)).map(|(_, tier)| *tier));
        if let Some(tier) = tier.filter(|tier| fits(*tier, &blocks[m.start..m.end])) {
            taken.push((m, tier));
        }
    }
    for (shape, tier) in made {
        if !lists.known.iter().any(|(of, held, known, t)| {
            *known == shape && *t == tier && (tier == Tier::Record || (*of == grid && *held == container))
        }) {
            lists.known.push((grid, container.clone(), shape, tier));
        }
    }
    if taken.is_empty() {
        return blocks;
    }
    taken.sort_by_key(|(m, _)| m.start);
    let mut out = Vec::with_capacity(blocks.len());
    let mut taken = taken.into_iter().peekable();
    let mut pieces = Vec::new();
    for (at, block) in blocks.into_iter().enumerate() {
        match taken.peek() {
            Some((m, tier)) if at >= m.start => {
                pieces.push(block);
                if at + 1 == m.end {
                    let id = lists.id();
                    let pieces = std::mem::take(&mut pieces);
                    match tier {
                        Tier::Card => out.push(card_of(pieces, id)),
                        Tier::Record => out.extend(pieces.into_iter().map(|b| Block { record: Some(id), ..b })),
                    }
                    taken.next();
                }
            }
            _ => out.push(block),
        }
    }
    out
}

/// The members grouped by `key`, in the order their keys first appear.
fn grouped(members: Vec<&Member>, key: impl Fn(&Member) -> String) -> Vec<(String, Vec<&Member>)> {
    let mut groups: Vec<(String, Vec<&Member>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for m in members {
        let key = key(m);
        let i = *at.entry(key.clone()).or_insert_with(|| {
            groups.push((key, Vec::new()));
            groups.len() - 1
        });
        groups[i].1.push(m);
    }
    groups
}

/// A shape without how many parts of one shape a run holds: `div(h3,p*3)` is `div(h3,p)`.
fn loose(shape: &str) -> String {
    let mut out = String::with_capacity(shape.len());
    let mut chars = shape.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '*' {
            while chars.next_if(char::is_ascii_digit).is_some() {}
        } else {
            out.push(c);
        }
    }
    out
}

fn fits(tier: Tier, pieces: &[Block]) -> bool {
    match tier {
        Tier::Card => card_shaped(pieces),
        Tier::Record => record_shaped(pieces),
    }
}

/// One record: its pieces' text, one piece to a line, the first headline a piece holds as its link, and its pieces' links. Its
/// id is the walk's own, and `number_records` replaces it once the whole page is read.
fn card_of(pieces: Vec<Block>, id: usize) -> Block {
    let href = pieces.iter().find_map(|b| b.href.clone());
    let text = pieces.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
    let mut links = Vec::new();
    add_links(&mut links, pieces.into_iter().flat_map(|b| b.links));
    Block { href, record: Some(id), card: true, links, ..Block::new(0, Kind::Para, text) }
}

/// Numbers the records of a page from 1 in page order, so each record's boundary is a number the output and the JSON share.
/// The blocks that follow one another with one id (the pieces of a card that stayed blocks) are one record.
pub fn number_records(blocks: &mut [Block]) {
    let (mut n, mut previous) = (0, None);
    for b in blocks.iter_mut() {
        let id = b.record;
        if id.is_some() && id != previous {
            n += 1;
        }
        previous = id;
        if id.is_some() {
            b.record = Some(n);
        }
    }
}

/// Whether a member's blocks can be a card: at least two pieces of text a reader sees, no more than `CARD_MAX_CHARS`, and at
/// most one piece with a line too long to be a field. A card holds one title, which may be long, and short fields: a date, a
/// price, a venue. A card with more than that (a title and a venue that are both long, a description) is read as blocks of its
/// own, numbered (see `record_shaped`): joined, it was fewer items for a reader that asks for a few blocks an item. A teaser (a
/// headline and a summary) and a section of a document hold more prose than a card does, and are read as they were. The
/// cards of a plan or a row are pieces of it: a card of cards is a card.
fn card_shaped(pieces: &[Block]) -> bool {
    pieces.len() >= 2
        && pieces.iter().all(|b| matches!(b.kind, Kind::Heading | Kind::Para | Kind::Item))
        && pieces.iter().map(|b| b.text.chars().count() + 1).sum::<usize>() <= CARD_MAX_CHARS
        && pieces.iter().filter(|b| !is_field(b)).count() <= 1
}

/// Whether a member's blocks can be a record whose pieces stay blocks: a card with more than a title and short fields, as a
/// listing of shows has (a title, a venue, a description, a date, a button), and no list: a section that holds one is a section. It has at least `RECORD_MIN_PIECES` pieces of text, a field among them, and a
/// link that is not in a paragraph of prose (its title's, its button's): a section of a document has no such link. A piece that
/// is a record already (a card in it, a card of a list inside it) makes the member no record: a record never holds another,
/// so a section of three courses is not one more record over the courses', and a record holds at most `RECORD_MAX_PIECES`.
fn record_shaped(pieces: &[Block]) -> bool {
    (RECORD_MIN_PIECES..=RECORD_MAX_PIECES).contains(&pieces.len())
        && pieces.iter().all(|b| matches!(b.kind, Kind::Heading | Kind::Para) && b.record.is_none())
        && pieces.iter().map(|b| b.text.chars().count() + 1).sum::<usize>() <= RECORD_MAX_CHARS
        && pieces.iter().any(is_field)
        && pieces.iter().any(|b| !b.links.is_empty() && !is_prose(b))
}

/// A piece that is a field: every line of it is short, so it is a date, a price, a venue or a short title, and a run of such
/// lines (a date, a time, a day) is a field too, as is a line of short fields set apart by a separator ("Male | 12 kg | Born
/// 2023", "Jazz · 20:30").
fn is_field(b: &Block) -> bool {
    let fields = |l: &str| FIELD_SEPARATORS.iter().fold(l.to_string(), |l, sep| l.replace(sep, " | "));
    b.text.lines().all(|l| fields(l).split(" | ").all(|f| f.trim().chars().count() < SHORT_BLOCK_CHARS))
}

/// What a card sets between the fields it puts on one line, spaced on both sides: a bare `·` is also a letter's (`l·l`).
const FIELD_SEPARATORS: [&str; 2] = [" · ", " • "];

/// A piece that holds prose: a line of it is a sentence (several words, ending as a sentence ends) or is as long as one.
fn is_prose(b: &Block) -> bool {
    b.text.lines().any(|l| {
        let l = l.trim();
        let ended = l.trim_end_matches(CLOSERS).ends_with(SENTENCE_ENDS);
        l.chars().count() >= PROSE_CHARS || (ended && l.split_whitespace().count() >= PROSE_WORDS)
    })
}

/// How a sentence ends (the Unicode Sentence_Terminal characters of the scripts jurl reads), and what may close after it.
const SENTENCE_ENDS: [char; 7] = ['.', '!', '?', '…', '。', '！', '？'];
const CLOSERS: [char; 7] = ['"', '\'', '”', '’', '»', ')', '」'];

/// Whether a shape is that of a row of cells: it holds a run of containers of one shape (`div(div(h3,p)*3)`), not just of plain parts
/// (`div(h3,p*3)`).
fn is_row(shape: &str) -> bool {
    Node::parse(shape).kids.iter().any(|k| k.run && !k.kids.is_empty())
}

/// Whether two shapes (see `html::shape`) differ by little: by at most `NEAR_NODES` nodes, and by at most a `NEAR_SHARE`rd
/// of the smaller one. The same card with a part that some cards lack is near; a layout `div` is near no card.
fn near(a: &str, b: &str) -> bool {
    let (a, b) = (Node::parse(a), Node::parse(b));
    a.distance(&b).is_some_and(|d| d == 0 || (d <= NEAR_NODES && d * NEAR_SHARE <= a.size().min(b.size())))
}

/// A shape as a tree: its tag, whether it stands for a run of siblings (`div*3`), and its children.
struct Node {
    tag: String,
    run: bool,
    kids: Vec<Node>,
}

impl Node {
    fn parse(s: &str) -> Node {
        fn parse_at(s: &[u8], at: &mut usize) -> Node {
            let from = *at;
            while *at < s.len() && !matches!(s[*at], b'(' | b')' | b',' | b'*') {
                *at += 1;
            }
            let tag = String::from_utf8_lossy(&s[from..*at]).into_owned();
            let mut kids = Vec::new();
            if *at < s.len() && s[*at] == b'(' {
                *at += 1;
                loop {
                    kids.push(parse_at(s, at));
                    match s.get(*at) {
                        Some(b',') => *at += 1,
                        Some(b')') => {
                            *at += 1;
                            break;
                        }
                        _ => break,
                    }
                }
            }
            let mut run = false;
            if s.get(*at) == Some(&b'*') {
                *at += 1;
                let from = *at;
                while s.get(*at).is_some_and(u8::is_ascii_digit) {
                    *at += 1;
                }
                run = String::from_utf8_lossy(&s[from..*at]).parse::<usize>().is_ok_and(|n| n > 1);
            }
            Node { tag, run, kids }
        }
        parse_at(s.as_bytes(), &mut 0)
    }

    fn size(&self) -> usize {
        1 + self.kids.iter().map(Node::size).sum::<usize>()
    }

    /// The fewest nodes to add or take away to make one the other, None when the tags of the roots differ: children are
    /// matched in order, each match costing the difference of the two, each left over its size.
    fn distance(&self, other: &Node) -> Option<usize> {
        if self.tag != other.tag {
            return None;
        }
        let (a, b) = (&self.kids, &other.kids);
        let mut row = vec![0];
        for n in b {
            row.push(row[row.len() - 1] + n.size());
        }
        for x in a {
            let mut next = vec![row[0] + x.size()];
            for (j, y) in b.iter().enumerate() {
                let skip = (row[j + 1] + x.size()).min(next[j] + y.size());
                next.push(x.distance(y).map_or(skip, |d| skip.min(row[j] + d)));
            }
            row = next;
        }
        Some(row[b.len()])
    }
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
        let merged = merge_cards(blocks, &members, 0, "div", &mut Lists::default());
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
        let merged = merge_cards(blocks, &members, 0, "div", &mut Lists::default());
        assert_eq!(merged[0].href.as_deref(), Some("https://example.com/world/riyadh"));
        assert_eq!(merged[1].href, None);
    }

    #[test]
    fn fewer_than_three_members_or_a_list_without_short_text_is_left_as_it_was() {
        let (blocks, members) = lists_of(&[("div(div)", card("Carmen Lillo")), ("div(div)", card("9louro"))]);
        assert_eq!(
            merge_cards(blocks.clone(), &members, 0, "div", &mut Lists::default()).len(),
            blocks.len(),
            "two members are a pair"
        );

        let long = |t: &str| {
            vec![piece(Kind::Heading, t), piece(Kind::Para, "A summary of the story that runs on for a while.")]
        };
        let (blocks, members) = lists_of(&[
            ("article(div)", long("First story headline, long enough")),
            ("article(div)", long("Second story headline, long enough")),
            ("article(div)", long("Third story headline, long enough")),
        ]);
        assert!(
            merge_cards(blocks.clone(), &members, 0, "div", &mut Lists::default()).iter().all(|b| b.record.is_none()),
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
        let merged = merge_cards(blocks, &members, 0, "div", &mut Lists::default());
        assert_eq!(merged.iter().filter(|b| b.record.is_some()).count(), 3, "{merged:?}");
    }

    #[test]
    fn a_line_of_short_fields_set_apart_by_a_separator_is_a_field() {
        // Each card has a line of fields ("Male | 12.5 kg | Born…") and one longer line: one piece that is not a field.
        let card = |name: &str, other: &str| {
            vec![
                piece(Kind::Para, name),
                piece(Kind::Para, "Male | 12.5 kg | Born: 10/05/2021"),
                piece(Kind::Para, other),
            ]
        };
        let (blocks, members) = lists_of(&[
            ("div(div)", card("Ades", "Sponsors: Maria Diaz, Loreto")),
            ("div(div)", card("Dina", "Sponsor: Jorge Juan Perez")),
            ("div(div)", card("Amazon", "Mixed")),
        ]);
        assert_eq!(
            merge_cards(blocks, &members, 0, "div", &mut Lists::default())
                .iter()
                .filter(|b| b.record.is_some())
                .count(),
            3
        );
        assert!(!is_field(&piece(Kind::Para, "A sentence that runs well past a field | and on")));
        assert!(
            !is_field(&piece(Kind::Para, "Col·legi de Periodistes de Catalunya")),
            "a letter's dot is no separator"
        );
        assert!(is_field(&piece(Kind::Para, "Jazz · 20:30 · Sala Paral·lel 62")));
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
        assert!(
            merge_cards(blocks.clone(), &members, 0, "div", &mut Lists::default()).iter().all(|b| b.record.is_none())
        );
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
        let merged = merge_cards(blocks.clone(), &members, 0, "div", &mut Lists::default());
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
        let merged = merge_cards(blocks, &members, 0, "div", &mut Lists::default());
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
        let merged = merge_cards(blocks, &members, 0, "div", &mut Lists::default());
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

    fn field_link(kind: Kind, text: &str) -> Block {
        Block { links: vec!["https://example.com/show".into()], ..piece(kind, text) }
    }

    /// The text of each record of `out`, a card's pieces (or its one block) joined by lines, in order.
    fn records_in(out: &[Block]) -> Vec<(usize, String)> {
        let mut records: Vec<(usize, String)> = Vec::new();
        for b in out {
            let Some(id) = b.record else { continue };
            match records.last_mut() {
                Some((last, text)) if *last == id => *text = format!("{text}\n{}", b.text),
                _ => records.push((id, b.text.clone())),
            }
        }
        records
    }

    #[test]
    fn a_long_title_and_a_long_venue_keep_their_blocks_and_share_a_number() {
        // The title and the venue are both over 25 characters, and neither is a sentence: the card is numbered, and its pieces
        // stay the blocks they were, as a reader that asks for a few blocks an item reads them.
        let card = |title: &str, venue: &str| {
            vec![
                piece(Kind::Para, "-25%\nDesde\n28 €21 €"),
                piece(Kind::Heading, title),
                Block { links: vec!["https://example.com/show".into()], ..piece(Kind::Para, venue) },
            ]
        };
        let (blocks, members) = lists_of(&[
            ("a(div,h3,div)", card("Los miserables, el musical", "Teatro Apolo Madrid")),
            ("a(div,h3,div)", card("Drácula, una comedia terrorífica", "Teatro Calderón. Rooftop Lírico")),
            ("a(div,h3,div)", card("Mi reno de peluche", "Teatro Infanta Isabel")),
        ]);
        assert!(!card_shaped(&blocks[3..6]));
        let merged = merge_cards(blocks.clone(), &members, 0, "div", &mut Lists::default());
        assert_eq!(merged.len(), blocks.len());
        assert!(merged.iter().all(|b| !b.card && b.record.is_some()));
        let records = records_in(&merged);
        assert_eq!(records.len(), 3);
        assert_eq!(
            records[1].1,
            "-25%\nDesde\n28 €21 €\nDrácula, una comedia terrorífica\nTeatro Calderón. Rooftop Lírico"
        );
    }

    #[test]
    fn a_sentence_among_the_pieces_is_prose_and_not_a_card() {
        let sentence = "Cada semana, un grupo de amigos organiza una cena privada con una regla infame.";
        assert!(is_prose(&piece(Kind::Para, sentence)));
        assert!(is_prose(&piece(Kind::Para, "¿Te adentras en el multiverso de Wiz Problema?")));
        assert!(!is_prose(&piece(Kind::Para, "Desde: 24 de octubre de 2026\nHasta: 31 de enero de 2027")));
        assert!(!is_prose(&piece(Kind::Para, "Teatro Calderón. Rooftop Lírico")));
        assert!(!is_prose(&piece(Kind::Heading, "MEMOCRACIA + Hollow Empty Minds + Los Valientes punk")));
        // Two long pieces and one of them a sentence: a teaser with a date, as a news front page has.
        let teaser = |title: &str| {
            vec![piece(Kind::Para, "3 hrs ago"), piece(Kind::Heading, title), piece(Kind::Para, sentence)]
        };
        assert!(!card_shaped(&teaser("Two Italians killed in strike in Kuwait")));
    }

    #[test]
    fn a_card_with_a_description_keeps_its_pieces_and_they_share_one_number() {
        // A listing of shows: the date, the title, a description of a few paragraphs (their number varies), a button. The
        // description makes each card too long for one block, so its pieces stay blocks and the card is numbered.
        let sentence = "Cada semana, un grupo de amigos organiza una cena privada con una regla infame.";
        let card = |title: &str, paragraphs: usize| {
            let mut pieces =
                vec![piece(Kind::Para, "04/11/2026 - 10/01/2027\nTeatro Marquina"), piece(Kind::Heading, title)];
            pieces.extend((0..paragraphs).map(|_| piece(Kind::Para, sentence)));
            pieces.push(field_link(Kind::Para, "+ información"));
            pieces
        };
        let (blocks, members) = lists_of(&[
            ("div(div,div(h3,p*2,p))", card("LA CENA DE LOS IDIOTAS", 2)),
            ("div(div,div(h3,p,p))", card("TURISMO RURAL", 1)),
            ("div(div,div(h3,p*2,p))", card("YUNEZ CHAIB EN DIRECTO", 3)),
        ]);
        let merged = merge_cards(blocks.clone(), &members, 0, "div", &mut Lists::default());
        assert_eq!(merged.len(), blocks.len(), "no piece is joined or dropped");
        assert!(merged.iter().all(|b| b.record.is_some() && !b.card), "{merged:?}");
        let records = records_in(&merged);
        assert_eq!(records.len(), 3);
        assert!(records[1].1.contains("TURISMO RURAL") && !records[1].1.contains("LA CENA"), "{records:?}");

        let mut numbered = merged;
        number_records(&mut numbered);
        let numbers: Vec<_> = numbered.iter().filter_map(|b| b.record).collect();
        assert_eq!(numbers, [1, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3], "{numbered:?}");
    }

    #[test]
    fn sections_of_a_document_with_links_in_their_text_are_no_records() {
        // A link inside a paragraph of prose is no button: the sections stay as they were.
        let section = |title: &str| {
            vec![
                piece(Kind::Heading, title),
                Block {
                    links: vec!["https://example.com/more".into()],
                    ..piece(
                        Kind::Para,
                        "Methods of file objects are described on this page, with an example for each one.",
                    )
                },
                piece(
                    Kind::Para,
                    "Each method reads or writes the file in a different way, as its own paragraph says.",
                ),
            ]
        };
        let (blocks, members) = lists_of(&[
            ("section(h2,p,p)", section("7. Input and output")),
            ("section(h2,p,p)", section("8. Errors")),
            ("section(h2,p,p)", section("9. Classes")),
        ]);
        assert!(merge_cards(blocks, &members, 0, "div", &mut Lists::default()).iter().all(|b| b.record.is_none()));
    }

    #[test]
    fn a_card_with_a_part_the_others_lack_joins_their_list() {
        // Three cards of one shape, and two that have a badge (a promotion, a featured show): five cards, not three.
        let (blocks, members) = lists_of(&[
            ("div(div,h3,p)", vec![piece(Kind::Heading, "La función"), piece(Kind::Para, "20€")]),
            (
                "div(div,div,h3,p)",
                vec![piece(Kind::Para, "-20%"), piece(Kind::Heading, "El Rey León"), piece(Kind::Para, "30€")],
            ),
            ("div(div,h3,p)", vec![piece(Kind::Heading, "Wicked"), piece(Kind::Para, "25€")]),
            (
                "div(div,div,h3,p)",
                vec![piece(Kind::Para, "Destacado"), piece(Kind::Heading, "Los miserables"), piece(Kind::Para, "35€")],
            ),
            ("div(div,h3,p)", vec![piece(Kind::Heading, "Cabaret"), piece(Kind::Para, "28€")]),
        ]);
        let merged = merge_cards(blocks, &members, 0, "div", &mut Lists::default());
        assert_eq!(merged.iter().filter(|b| b.card).count(), 5, "{merged:?}");
    }

    #[test]
    fn shapes_far_apart_are_not_one_list() {
        assert!(near("div(h3,p)", "div(h3,p)"));
        assert!(near("div(div,h3,p)", "div(div,div,h3,p)"));
        assert!(near("a(div,div(h3,div,div,div(div),div(div)))", "a(div,div(h3,div,div,div(div)))"));
        assert!(!near("div(h3,p)", "article(h3,p)"));
        assert!(!near("div(div(h3,p,p),div(a))", "div(ul(li),ul(li),ul(li))"));
        assert!(near("div(h3,p*3)", "div(h3,p)"));
    }

    #[test]
    fn a_row_of_cells_is_no_card_and_its_cells_are() {
        assert!(is_row("div(div(h3,p)*3)"));
        assert!(is_row("div(div(div(h5,p))*2,p)"));
        assert!(!is_row("div(h3,p*3)"), "a run of paragraphs is a card's text");
        assert!(!is_row("div(div,div(h3,p*2,div))"));
    }

    #[test]
    fn a_pair_with_the_shape_of_a_list_found_earlier_is_read_as_its_cards() {
        // A grid's rows: three cards, three cards, and the last row's two.
        let card =
            |name: &str| vec![piece(Kind::Heading, name), piece(Kind::Para, "Desde 12 Oct"), piece(Kind::Para, "90€")];
        let mut lists = Lists::default();
        let (blocks, members) = lists_of(&[
            ("div(div(h5,p))", card("Cocina italiana")),
            ("div(div(h5,p))", card("Cocina japonesa")),
            ("div(div(h5,p))", card("Panadería")),
        ]);
        assert_eq!(merge_cards(blocks, &members, 0, "div", &mut lists).iter().filter(|b| b.card).count(), 3);
        let (blocks, members) =
            lists_of(&[("div(div(h5,p))", card("Cócteles")), ("div(div(h5,p))", card("Vinos del mundo"))]);
        let merged = merge_cards(blocks, &members, 0, "div", &mut lists);
        assert_eq!(merged.iter().filter(|b| b.card).count(), 2, "{merged:?}");
        // Without the earlier list, a pair is a pair.
        let (blocks, members) =
            lists_of(&[("div(div(h5,p))", card("Cócteles")), ("div(div(h5,p))", card("Vinos del mundo"))]);
        assert!(merge_cards(blocks, &members, 0, "div", &mut Lists::default()).iter().all(|b| b.record.is_none()));
    }

    #[test]
    fn a_card_that_holds_cards_is_still_a_card_but_no_record() {
        // A plan of rows read as cards is one card, as it always was; a record's pieces never hold a whole card.
        let row = || -> Vec<Block> {
            (1..=3).map(|n| Block { record: Some(n), card: true, ..piece(Kind::Para, "Basic\n$10") }).collect()
        };
        let (blocks, members) = lists_of(&[("div(div*3)", row()), ("div(div*3)", row()), ("div(div*3)", row())]);
        assert!(card_shaped(&blocks[members[0].start..members[0].end]));
        assert!(!record_shaped(&blocks[members[0].start..members[0].end]));
    }

    #[test]
    fn a_list_that_is_not_cards_as_a_whole_does_not_leave_some_of_its_members_numbered() {
        // Five members of one shape and two are not cards (a code block in one, nothing but a sentence in the other): no
        // member is numbered, and none is absorbed by the shape of another list.
        let mut odd = card("Odd one");
        odd.push(piece(Kind::Code, "npm install"));
        let (blocks, members) = lists_of(&[
            ("div(div)", card("Carmen Lillo")),
            ("div(div)", odd),
            ("div(div)", card("9louro")),
            ("div(div)", vec![piece(Kind::Para, "A sentence that is all the card has, and no field at all.")]),
            ("div(div)", card("Baile Europa")),
        ]);
        let mut lists = Lists::default();
        lists.known.push((0, "div".into(), "div(div)".into(), Tier::Card));
        assert!(merge_cards(blocks, &members, 0, "div", &mut lists).iter().all(|b| b.record.is_none()));
    }

    #[test]
    fn a_layout_part_is_not_near_a_card_and_a_shape_known_is_matched_exactly() {
        assert!(!near("div", "div(div,div)"));
        assert!(!near("div(div)", "div(div,div,div)"));
        assert!(!near("div(div(h3,p),div)", "div(div(h3,p),div,div,div,div)"));
        let (blocks, members) =
            lists_of(&[("div(div,div)", card("Cócteles")), ("div(div,div)", card("Vinos del mundo"))]);
        let mut lists = Lists::default();
        lists.known.push((0, "div".into(), "div(div)".into(), Tier::Card));
        assert!(
            merge_cards(blocks, &members, 0, "div", &mut lists).iter().all(|b| b.record.is_none()),
            "near is not the same shape"
        );
    }
}

//! Short paragraphs and list items, joined into blocks a reader can judge: a price card's pieces read as one block,
//! and a short list item is kept with the list it belongs to.

use std::collections::HashSet;

use super::{Block, Kind};

/// Below this many characters a block says too little to be judged on its own ("Basic", "$10", "per user/month").
pub const SHORT_BLOCK_CHARS: usize = 25;
/// A run of joined short blocks stops growing here.
const JOINED_MAX_CHARS: usize = 400;

/// A heading that is only a price, so it joins its card: it starts with a currency sign, or it is figures, currency
/// signs and separators with letters only as unit words ("$12", "€9.50", "12 €", "$10/mo", "10 per user/month").
/// "Step 2" is a section title.
fn is_price(text: &str) -> bool {
    const CURRENCY: &[char] = &['$', '€', '£', '¥', '₹', '₩', '₽', '₺'];
    const SEPARATORS: &[char] = &['.', ',', '/', '-', '–', '|', '·'];
    // A unit word also matches with a trailing "s" ("users").
    const UNITS: &[&str] = &["a", "per", "mo", "month", "yr", "year", "user", "seat"];
    let t = text.trim();
    if !t.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let figure = |c: char| c.is_ascii_digit() || c.is_whitespace() || CURRENCY.contains(&c) || SEPARATORS.contains(&c);
    let only_figures = t.chars().all(|c| c.is_alphabetic() || figure(c));
    let only_units = t
        .split(|c: char| !c.is_alphabetic())
        .all(|w| w.is_empty() || UNITS.contains(&w.to_lowercase().trim_end_matches('s')));
    t.starts_with(CURRENCY) || (only_figures && only_units)
}

/// Consecutive short paragraphs and list items become one block, a line each, as the page shows them: a pricing
/// card built from bare `<div>`s is then one block that says "Basic / $10 / per user/month" instead of pieces too
/// short to judge. Headings, code, tables and quotes are never joined, except a heading that is a price.
pub(super) fn join_short(blocks: Vec<Block>) -> Vec<Block> {
    // A heading that is a price ("### $12") is a figure set big on its card, not a section title.
    let value = |b: &Block| b.kind == Kind::Heading && is_price(&b.text);
    let short = |b: &Block| {
        (matches!(b.kind, Kind::Para | Kind::Item) || value(b)) && b.text.chars().count() < SHORT_BLOCK_CHARS
    };
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    let mut run: Vec<Block> = Vec::new();
    let flush = |run: &mut Vec<Block>, out: &mut Vec<Block>| match run.len() {
        0 => {}
        1 => out.push(run.pop().unwrap()),
        _ => {
            let text = run.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
            // Items of one list joined are still part of that list ("1 egg" and "Salt" next to each other).
            let list = run[0].list.filter(|l| run.iter().all(|b| b.kind == Kind::Item && b.list == Some(*l)));
            out.push(Block { i: 0, kind: Kind::Para, level: None, lang: None, text, list });
            run.clear();
        }
    };
    for b in blocks {
        let joined: usize = run.iter().map(|r| r.text.chars().count() + 1).sum();
        if short(&b) && joined + b.text.chars().count() <= JOINED_MAX_CHARS {
            run.push(b);
        } else {
            flush(&mut run, &mut out);
            if short(&b) { run.push(b) } else { out.push(b) }
        }
    }
    flush(&mut run, &mut out);
    for (i, b) in out.iter_mut().enumerate() {
        b.i = i;
    }
    out
}

/// List items too short to be judged alone ("1 teaspoon baking soda") that sit next to a kept item of the same
/// list: a list is read whole, so they go wherever their neighbours go. Without them a recipe loses its salt.
pub fn short_items_of_kept_lists(blocks: &[Block], kept: impl Fn(usize) -> bool) -> HashSet<usize> {
    let short = |b: &Block| b.list.is_some() && b.text.chars().count() < SHORT_BLOCK_CHARS;
    let mut out = HashSet::new();
    // Grows from each kept item outwards, so a run of short items between two kept ones comes along whole.
    loop {
        let before = out.len();
        for (i, b) in blocks.iter().enumerate() {
            if !short(b) || out.contains(&i) {
                continue;
            }
            let neighbour = |j: usize| blocks.get(j).is_some_and(|n| n.list == b.list && (kept(j) || out.contains(&j)));
            if (i > 0 && neighbour(i - 1)) || neighbour(i + 1) {
                out.insert(i);
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{html, markdown};
    use url::Url;

    fn base() -> Url {
        Url::parse("https://example.com/post/").unwrap()
    }

    #[test]
    fn a_price_set_as_a_heading_joins_its_card() {
        let ex = markdown("### Teams\n\nYEARLY\n\n### $12\n\nper user/month\n\nSave 25%\n", &base());
        let texts: Vec<_> = ex.blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["Teams", "YEARLY\n$12\nper user/month\nSave 25%"]);
    }

    #[test]
    fn a_numbered_heading_is_not_a_price() {
        let want = [
            (Kind::Heading, Some(2), "Step 2"),
            (Kind::Para, None, "Install it."),
            (Kind::Heading, Some(2), "Step 3"),
            (Kind::Para, None, "Run it."),
        ];
        let md = markdown("## Step 2\n\nInstall it.\n\n## Step 3\n\nRun it.\n", &base());
        let page = html("<body><h2>Step 2</h2><p>Install it.</p><h2>Step 3</h2><p>Run it.</p></body>", &base());
        for ex in [md, page] {
            let got: Vec<_> = ex.blocks.iter().map(|b| (b.kind, b.level, b.text.as_str())).collect();
            assert_eq!(got, want);
        }
    }

    #[test]
    fn price_headings_of_each_shape_join_their_card() {
        for price in ["€9.50", "12 €", "$10/mo"] {
            let ex = markdown(&format!("### {price}\n\nper user/month\n"), &base());
            let texts: Vec<_> = ex.blocks.into_iter().map(|b| b.text).collect();
            assert_eq!(texts, [format!("{price}\nper user/month")], "{price}");
        }
    }

    #[test]
    fn only_figures_and_units_are_prices() {
        for price in ["$12", "€9.50", "12 €", "$10/mo", "10 per user/month", "$10 Pro"] {
            assert!(is_price(price), "{price}");
        }
        for title in ["Step 2", "Version 3", "Top 10 tips", "COVID-19", "2FA", "Save 25%", "100%", "Free"] {
            assert!(!is_price(title), "{title}");
        }
    }

    #[test]
    fn short_pieces_of_a_card_are_one_block() {
        let page = r#"<body><h2>Pricing</h2><div><div>Basic</div><div>$10</div><div>per user/month</div></div>
            <p>Everything in Free, plus unlimited teams and private projects for everyone.</p><div>Ok</div></body>"#;
        let texts: Vec<_> = html(page, &base()).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(
            texts,
            [
                "Pricing",
                "Basic\n$10\nper user/month",
                "Everything in Free, plus unlimited teams and private projects for everyone.",
                "Ok"
            ]
        );
    }

    #[test]
    fn short_list_items_go_with_their_list() {
        let page = "<article><p>Some introduction that is long enough to be judged on its own.</p><ul>\
            <li>2 and 1/4 cups (281g) all-purpose flour</li><li>1 teaspoon baking soda</li>\
            <li>1 and 1/2 teaspoons cornstarch*</li><li>1/2 teaspoon salt</li>\
            <li>3/4 cup (170g) unsalted butter, melted</li></ul><ul><li>Pin it</li></ul></article>";
        let ex = html(page, &base());
        let at = |t: &str| ex.blocks.iter().position(|b| b.text == t).unwrap();
        let (soda, salt, pin) = (at("1 teaspoon baking soda"), at("1/2 teaspoon salt"), at("Pin it"));
        let flour = at("2 and 1/4 cups (281g) all-purpose flour");
        let kept = |i| {
            i == flour
                || i == at("1 and 1/2 teaspoons cornstarch*")
                || i == at("3/4 cup (170g) unsalted butter, melted")
        };
        let items = short_items_of_kept_lists(&ex.blocks, kept);
        assert!(items.contains(&soda) && items.contains(&salt));
        // Another list's short item stays out, and nothing comes along when no item of the list is kept.
        assert!(!items.contains(&pin));
        assert!(short_items_of_kept_lists(&ex.blocks, |_| false).is_empty());

        // Two tiny items next to each other are joined into one block, still too short to judge: it goes too.
        let page = "<article><ul><li>2 and 1/4 cups (281g) all-purpose flour</li><li>1 egg</li><li>Salt</li>\
            <li>3/4 cup (170g) unsalted butter, melted</li></ul></article>";
        let ex = html(page, &base());
        let joined = ex.blocks.iter().position(|b| b.text == "1 egg\nSalt").unwrap();
        let flour = ex.blocks.iter().position(|b| b.text.starts_with("2 and")).unwrap();
        assert!(short_items_of_kept_lists(&ex.blocks, |i| i == flour).contains(&joined));
    }
}

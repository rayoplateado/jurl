//! The values a page gives in its JSON-LD (schema.org): an event's dates, an offer's price, a course's duration. A precise
//! answer may be one of them, when the page's text gives none (see `structured_or` in blocks.rs). Each is a block of its own
//! that names its source, so the quote shows the field: "Event VivaTech 2027 (JSON-LD): startDate 2027-06-16, endDate 2027-06-19."

use scraper::{Html, Selector};
use serde_json::{Map, Value};

use super::{Block, Extracted, Kind};

/// The most blocks one page gives from its JSON-LD: a long catalogue of offers stays a short list.
const MAX_BLOCKS: usize = 40;

/// Sets the page's JSON-LD values as `ex.structured`. They are not in `ex.blocks`: the page's text is scored as it is, and
/// the values only answer when the text does not.
pub fn add_to(ex: &mut Extracted, html: &str) {
    ex.structured = blocks(html, 0);
}

/// The JSON-LD values of an HTML page as blocks, numbered from `first`. A value repeated on the page is kept once.
pub fn blocks(html: &str, first: usize) -> Vec<Block> {
    let doc = Html::parse_document(html);
    let script = Selector::parse(r#"script[type="application/ld+json"]"#).expect("static selector");
    let mut texts: Vec<String> = Vec::new();
    for el in doc.select(&script) {
        let body: String = el.text().collect();
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            visit(&value, None, &mut texts);
        }
        if texts.len() >= MAX_BLOCKS {
            break;
        }
    }
    texts.truncate(MAX_BLOCKS);
    texts.into_iter().enumerate().map(|(k, text)| Block::new(first + k, Kind::Para, text)).collect()
}

/// Walks a JSON-LD value depth first. A typed object gives its own text (see [`describe`]); what is inside it is visited
/// too, with the object's name as its parent: a product's offers name the product they belong to.
fn visit(value: &Value, parent: Option<&str>, out: &mut Vec<String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|v| visit(v, parent, out)),
        Value::Object(map) => {
            let name = text(map.get("name"));
            for kind in types(map) {
                if let Some(s) = describe(&kind, map, name.as_deref(), parent).filter(|s| !out.contains(s)) {
                    out.push(s);
                }
            }
            let inner = name.as_deref().or(parent);
            for (key, child) in map {
                if key != "@type" && matches!(child, Value::Object(_) | Value::Array(_)) {
                    visit(child, inner, out);
                }
            }
        }
        _ => {}
    }
}

/// The text of a typed object whose type a precise answer may use: an event's dates, a course's duration, an offer's
/// price. Other types give none. The fields are the schema.org ones, so no word list decides which values count.
fn describe(kind: &str, map: &Map<String, Value>, name: Option<&str>, parent: Option<&str>) -> Option<String> {
    let head = match name {
        Some(n) => format!("{kind} {n}"),
        None => kind.to_string(),
    };
    let source = match parent {
        Some(p) => format!("(JSON-LD, {p})"),
        None => "(JSON-LD)".to_string(),
    };
    let body = match kind {
        "Event" => field_list(map, &["startDate", "endDate"]),
        "Course" => field_list(map, &["duration"]),
        "Offer" => {
            let price = text(map.get("price"))?;
            Some(match text(map.get("priceCurrency")) {
                Some(currency) => format!("price {price} {currency}"),
                None => format!("price {price}"),
            })
        }
        _ => None,
    }?;
    Some(format!("{head} {source}: {body}."))
}

/// `key value` for each of `keys` the object has, joined: "startDate 2027-06-16, endDate 2027-06-19".
fn field_list(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    let parts: Vec<String> = keys.iter().filter_map(|k| text(map.get(*k)).map(|v| format!("{k} {v}"))).collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// A JSON-LD value as text: a string or a number as it is, or an object's name (a location's, say).
fn text(value: Option<&Value>) -> Option<String> {
    let s = match value? {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Object(o) => o.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_string(),
        _ => String::new(),
    };
    (!s.is_empty()).then_some(s)
}

/// An object's types: "@type" is one type name or a list of them.
fn types(map: &Map<String, Value>) -> Vec<String> {
    match map.get("@type") {
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(ts)) => ts.iter().filter_map(|t| t.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(html: &str) -> Vec<String> {
        blocks(html, 0).into_iter().map(|b| b.text).collect()
    }

    #[test]
    fn an_event_gives_its_dates_with_its_name_and_its_source() {
        let html = r#"<script type="application/ld+json">{"@type":"Event","name":"VivaTech 2027","startDate":"2027-06-16","endDate":"2027-06-19"}</script>"#;
        assert_eq!(texts(html), ["Event VivaTech 2027 (JSON-LD): startDate 2027-06-16, endDate 2027-06-19."]);
    }

    #[test]
    fn a_product_gives_each_offer_its_price_and_currency_with_the_product_it_belongs_to() {
        let html = r#"<script type="application/ld+json">{"@type":"Product","name":"Asana Pricing Plans","offers":[{"@type":"Offer","name":"Starter","price":10.99,"priceCurrency":"USD"},{"@type":"Offer","name":"Free","price":"0","priceCurrency":"USD"}]}</script>"#;
        assert_eq!(
            texts(html),
            [
                "Offer Starter (JSON-LD, Asana Pricing Plans): price 10.99 USD.",
                "Offer Free (JSON-LD, Asana Pricing Plans): price 0 USD."
            ]
        );
    }

    #[test]
    fn a_graph_and_a_course_are_read_while_other_types_and_bad_json_are_not() {
        let html = r#"<script type="application/ld+json">{"@graph":[{"@type":"Course","name":"Data","duration":"PT10W"},{"@type":"Article","name":"News"}]}</script><script type="application/ld+json">{not json</script>"#;
        assert_eq!(texts(html), ["Course Data (JSON-LD): duration PT10W."]);
    }

    #[test]
    fn the_values_are_numbered_from_the_given_index_and_a_repeated_value_is_kept_once() {
        let one =
            r#"<script type="application/ld+json">{"@type":"Event","name":"X","startDate":"2027-01-01"}</script>"#;
        let blocks = blocks(&format!("{one}{one}"), 7);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].i, 7);
        assert_eq!(blocks[0].text, "Event X (JSON-LD): startDate 2027-01-01.");
    }

    #[test]
    fn the_page_keeps_its_values_apart_from_its_text() {
        let mut ex = Extracted::default();
        add_to(
            &mut ex,
            r#"<script type="application/ld+json">{"@type":"Event","name":"X","startDate":"2027-01-01"}</script>"#,
        );
        assert!(ex.blocks.is_empty());
        assert_eq!(ex.structured.len(), 1);
    }
}

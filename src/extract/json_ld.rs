//! The values a page gives in its JSON-LD (schema.org): an event's dates, an offer's price, a course's duration. A precise
//! answer may be one of them, when the page's text gives none (see `structured_or` in blocks.rs). Each is a block of its own
//! that names its source, so the quote shows the field: "Event VivaTech 2027 (JSON-LD): startDate 2027-06-16, endDate 2027-06-19."
//! A product's photos are not text: `product_images` gives them to the image candidates instead.

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
    texts.into_iter().enumerate().map(|(k, text)| Block::new(first + k, Kind::Structured, text)).collect()
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

/// The most items one page gives from its JSON-LD: as many as a sheet keeps from one site's list.
const MAX_ITEMS: usize = 300;
/// The longest description an item keeps, in characters: enough to tell items apart, short enough for a long catalogue.
const MAX_DESCRIPTION_CHARS: usize = 300;
/// Types that describe the page or the site, not a thing a list is of.
const PAGE_TYPES: &[&str] = &[
    "WebSite",
    "WebPage",
    "CollectionPage",
    "ItemPage",
    "SearchResultsPage",
    "BreadcrumbList",
    "SiteNavigationElement",
    "SearchAction",
    "ImageObject",
    "Offer",
    "AggregateOffer",
    "PostalAddress",
];
/// The keys under which a thing lists other things of its own: a venue's events, a list's elements, a page's main entity.
const LIST_KEYS: &[&str] = &["@graph", "itemListElement", "item", "mainEntity", "event", "subEvent"];

/// The things a page's JSON-LD lists, each one block, numbered from `first`: the events of a listing, the products of a
/// catalogue. Each gives its type, name, and the schema.org fields a row of a list takes, in words a check can find in it:
/// "Event Lola Índigo (JSON-LD): url https://…; startDate 2027-03-14T20:00; location WiZink Center, Madrid; price 25 EUR."
/// A thing inside another (an event's performer, an offer's seller) is a field of it, not an item. Read for `--with-pages`,
/// so a list read can take its rows from the page's own data.
pub fn items(html: &str, first: usize) -> Vec<Block> {
    let doc = Html::parse_document(html);
    let script = Selector::parse(r#"script[type="application/ld+json"]"#).expect("static selector");
    let mut texts: Vec<String> = Vec::new();
    for el in doc.select(&script) {
        let body: String = el.text().collect();
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            collect_items(&value, &mut texts);
        }
        if texts.len() >= MAX_ITEMS {
            break;
        }
    }
    texts.truncate(MAX_ITEMS);
    texts.into_iter().enumerate().map(|(k, text)| Block::new(first + k, Kind::Structured, text)).collect()
}

/// Walks the things a JSON-LD value lists (see [`LIST_KEYS`]) and describes each named one.
fn collect_items(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Array(values) => values.iter().for_each(|v| collect_items(v, out)),
        Value::Object(map) => {
            let kinds = types(map);
            // A thing that lists others (a venue's events) is where they are, not one of them.
            let lists_others =
                ["event", "subEvent", "itemListElement"].iter().any(|k| map.get(*k).is_some_and(|v| !v.is_null()));
            let is_item = !kinds.is_empty()
                && !lists_others
                && !kinds
                    .iter()
                    .any(|k| PAGE_TYPES.contains(&k.as_str()) || k == "ItemList" || k == "ListItem" || k == "Thing");
            if is_item && let Some(s) = describe_item(&kinds[0], map).filter(|s| !out.contains(s)) {
                out.push(s);
            }
            // A trail of links to the page's parents lists no things.
            if kinds.iter().any(|k| k == "BreadcrumbList" || k == "SiteNavigationElement") {
                return;
            }
            for key in LIST_KEYS {
                if let Some(child) = map.get(*key) {
                    collect_items(child, out);
                }
            }
        }
        _ => {}
    }
}

/// An item's text, or None when it has no name. A type written as a full IRI ("https://schema.org/Event") is named by its
/// last part, and a name given in several languages by its first.
fn describe_item(kind: &str, map: &Map<String, Value>) -> Option<String> {
    let kind = kind.rsplit('/').next().unwrap_or(kind);
    let name = match map.get("name") {
        Some(Value::Array(names)) => names.iter().find_map(|n| text(Some(n))),
        name => text(name),
    }?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(url) = map.get("url").and_then(Value::as_str) {
        parts.push(format!("url {}", url.trim()));
    }
    let mut images = Vec::new();
    if let Some(image) = map.get("image") {
        image_values(image, &mut images);
    }
    if let Some(image) = images.into_iter().find(|i| !i.is_empty()) {
        parts.push(format!("image {image}"));
    }
    for key in ["startDate", "endDate"] {
        if let Some(v) = text(map.get(key)) {
            parts.push(format!("{key} {v}"));
        }
    }
    for key in ["location", "address"] {
        if let Some(v) = place(map.get(key)) {
            parts.push(format!("{key} {v}"));
        }
    }
    if let Some(offer) = first_offer(map.get("offers")) {
        parts.extend(offer_parts(offer, map.get("url").and_then(Value::as_str)));
    }
    if let Some(d) = text(map.get("description")) {
        let d: String = d.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches('.').to_string();
        let cut: String = d.chars().take(MAX_DESCRIPTION_CHARS).collect();
        parts.push(format!("description {}", if cut.len() < d.len() { format!("{cut}…") } else { cut }));
    }
    Some(if parts.is_empty() {
        format!("{kind} {name} (JSON-LD).")
    } else {
        format!("{kind} {name} (JSON-LD): {}.", parts.join("; "))
    })
}

/// A place as words: its name, and the street and town of its address, as far as it gives them ("WiZink Center, Madrid").
fn place(value: Option<&Value>) -> Option<String> {
    let value = match value? {
        Value::Array(values) => values.first()?,
        v => v,
    };
    let parts: Vec<String> = match value {
        Value::String(s) => vec![s.trim().to_string()],
        Value::Object(map) => {
            let mut parts: Vec<String> = text(map.get("name")).into_iter().collect();
            let address = match map.get("address") {
                Some(Value::Object(a)) => a.clone(),
                _ => map.clone(),
            };
            for key in ["streetAddress", "addressLocality"] {
                if let Some(v) = address.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()) {
                    parts.push(v.to_string());
                }
            }
            if let Some(Value::String(a)) = map.get("address") {
                parts.push(a.trim().to_string());
            }
            parts
        }
        _ => Vec::new(),
    };
    let parts: Vec<String> = parts.into_iter().filter(|p| !p.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The first offer of an `offers` value: an Offer, an AggregateOffer, or a list of them.
fn first_offer(value: Option<&Value>) -> Option<&Map<String, Value>> {
    match value? {
        Value::Object(map) => Some(map),
        Value::Array(values) => values.iter().find_map(Value::as_object),
        _ => None,
    }
}

/// An offer's prices with its currency ("price 25 EUR", "lowPrice 18, highPrice 60 EUR"), and its URL when it is not the
/// item's own: where the ticket is bought.
fn offer_parts(offer: &Map<String, Value>, item_url: Option<&str>) -> Vec<String> {
    let currency = text(offer.get("priceCurrency"));
    let prices: Vec<String> = ["price", "lowPrice", "highPrice"]
        .iter()
        .filter_map(|k| text(offer.get(*k)).map(|v| format!("{k} {v}")))
        .collect();
    let mut parts = Vec::new();
    if !prices.is_empty() {
        let joined = prices.join(", ");
        parts.push(match currency {
            Some(c) => format!("{joined} {c}"),
            None => joined,
        });
    }
    if let Some(url) =
        offer.get("url").and_then(Value::as_str).map(str::trim).filter(|u| Some(*u) != item_url.map(str::trim))
    {
        parts.push(format!("offer url {url}"));
    }
    parts
}

/// The most product photos one page gives from its JSON-LD: a listing of many products stays short.
const MAX_IMAGES: usize = 8;

/// The photos a page's JSON-LD gives its products: the `image` of each Product or Offer, as a URL, a list of URLs, or an
/// ImageObject. They are image candidates, not text: `html` reads them after og:image.
pub fn product_images(doc: &Html) -> Vec<String> {
    let script = Selector::parse(r#"script[type="application/ld+json"]"#).expect("static selector");
    let mut out: Vec<String> = Vec::new();
    for el in doc.select(&script) {
        let body: String = el.text().collect();
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            visit_images(&value, &mut out);
        }
        if out.len() >= MAX_IMAGES {
            break;
        }
    }
    out.truncate(MAX_IMAGES);
    out
}

/// Walks a JSON-LD value depth first, and takes the `image` of each Product or Offer it finds. An image counts where it is
/// written: nothing is inherited from a product downwards, since a product's review has an author with an avatar of their
/// own, and that is not a photo of the product.
fn visit_images(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|v| visit_images(v, out)),
        Value::Object(map) => {
            if types(map).iter().any(|t| matches!(t.as_str(), "Product" | "Offer"))
                && let Some(image) = map.get("image")
            {
                image_values(image, out);
            }
            for (key, child) in map {
                if key != "@type" && matches!(child, Value::Object(_) | Value::Array(_)) {
                    visit_images(child, out);
                }
            }
        }
        _ => {}
    }
}

/// The URLs of an `image` value: a string, a list of them, or an ImageObject. An ImageObject's `contentUrl` is the file
/// itself, and its `url` may be the page that shows the file, so `contentUrl` is read first.
fn image_values(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.trim().to_string()),
        Value::Array(items) => items.iter().for_each(|v| image_values(v, out)),
        Value::Object(map) => {
            if let Some(url) = map.get("contentUrl").or_else(|| map.get("url")).and_then(Value::as_str) {
                out.push(url.trim().to_string());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(html: &str) -> Vec<String> {
        blocks(html, 0).into_iter().map(|b| b.text).collect()
    }

    #[test]
    fn a_value_is_structured_data_not_the_page_text() {
        // Its kind says so in the JSON the precise answer prints: "structured", never "para".
        let html =
            r#"<script type="application/ld+json">{"@type":"Event","name":"X","startDate":"2027-01-01"}</script>"#;
        let b = blocks(html, 0);
        assert_eq!(b[0].kind, Kind::Structured);
        assert_eq!(serde_json::to_value(b[0].kind).unwrap(), "structured");
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

    fn photos(html: &str) -> Vec<String> {
        product_images(&Html::parse_document(html))
    }

    #[test]
    fn a_products_image_is_a_candidate_as_a_string_a_list_or_an_image_object() {
        let html = r#"<script type="application/ld+json">{"@type":"Product","name":"Asana","image":"https://cdn.example.test/a.jpg"}</script>
            <script type="application/ld+json">{"@type":"Product","name":"Mug","image":["https://cdn.example.test/b.jpg","https://cdn.example.test/c.jpg"]}</script>
            <script type="application/ld+json">{"@graph":[{"@type":"Product","name":"Desk","image":{"@type":"ImageObject","contentUrl":"https://cdn.example.test/d.jpg"}}]}</script>"#;
        assert_eq!(
            photos(html),
            [
                "https://cdn.example.test/a.jpg",
                "https://cdn.example.test/b.jpg",
                "https://cdn.example.test/c.jpg",
                "https://cdn.example.test/d.jpg"
            ]
        );
    }

    #[test]
    fn an_image_object_gives_its_file_and_otherwise_its_url() {
        // The page that shows a photo is the ImageObject's `url`; its file is the `contentUrl`, which is read first.
        let html = r#"<script type="application/ld+json">{"@type":"Product","image":{"@type":"ImageObject","url":"https://cdn.example.test/page.html","contentUrl":"https://cdn.example.test/file.jpg"}}</script>
            <script type="application/ld+json">{"@type":"Product","image":{"@type":"ImageObject","url":"https://cdn.example.test/only.jpg"}}</script>"#;
        assert_eq!(photos(html), ["https://cdn.example.test/file.jpg", "https://cdn.example.test/only.jpg"]);
    }

    #[test]
    fn an_offers_image_counts_but_a_reviewers_avatar_does_not() {
        let html = r#"<script type="application/ld+json">{"@type":"Product","name":"Asana","image":"https://cdn.example.test/a.jpg",
            "review":{"@type":"Review","author":{"@type":"Person","name":"Ana","image":"https://cdn.example.test/avatar.jpg"}},
            "offers":{"@type":"Offer","price":10,"image":"https://cdn.example.test/offer.jpg"}}</script>"#;
        assert_eq!(photos(html), ["https://cdn.example.test/a.jpg", "https://cdn.example.test/offer.jpg"]);
    }

    #[test]
    fn the_image_of_anything_but_a_product_or_an_offer_is_not_a_product_photo() {
        let html = r#"<script type="application/ld+json">{"@type":"Article","image":"https://cdn.example.test/news.jpg"}</script>"#;
        assert!(photos(html).is_empty());
    }

    fn item_texts(html: &str) -> Vec<String> {
        items(html, 0).into_iter().map(|b| b.text).collect()
    }

    #[test]
    fn a_listing_gives_each_event_with_the_fields_a_row_takes() {
        let html = r#"<script type="application/ld+json">{"@type":"ItemList","itemListElement":[
            {"@type":"ListItem","position":1,"item":{"@type":"MusicEvent","name":"Lola Índigo","url":"https://tickets.example.test/e/1",
              "image":["https://cdn.example.test/1.jpg"],"startDate":"2027-03-14T20:00","location":{"@type":"Place","name":"WiZink Center",
              "address":{"@type":"PostalAddress","streetAddress":"Av. Felipe II","addressLocality":"Madrid"}},
              "offers":{"@type":"Offer","price":25,"priceCurrency":"EUR","url":"https://tickets.example.test/buy/1"},
              "performer":{"@type":"Person","name":"Lola Índigo"},"description":"  Gira   2027. "}},
            {"@type":"ListItem","position":2,"item":{"@type":"Event","name":"Hamlet","startDate":"2027-03-20",
              "offers":[{"@type":"AggregateOffer","lowPrice":"18","highPrice":"60","priceCurrency":"EUR"}]}}]}</script>"#;
        assert_eq!(
            item_texts(html),
            [
                "MusicEvent Lola Índigo (JSON-LD): url https://tickets.example.test/e/1; image https://cdn.example.test/1.jpg; startDate 2027-03-14T20:00; location WiZink Center, Av. Felipe II, Madrid; price 25 EUR; offer url https://tickets.example.test/buy/1; description Gira 2027.",
                "Event Hamlet (JSON-LD): startDate 2027-03-20; lowPrice 18, highPrice 60 EUR."
            ]
        );
    }

    #[test]
    fn a_venues_events_are_items_but_the_venue_the_page_its_breadcrumbs_and_a_lone_offer_are_not() {
        let html = r#"<script type="application/ld+json">{"@graph":[{"@type":"WebPage","name":"Agenda"},
            {"@type":"BreadcrumbList","itemListElement":[{"@type":"ListItem","position":1,"item":{"@type":"Thing","name":"Inicio"}}]},
            {"@type":"Place","name":"Teatro Real","event":[{"@type":"Event","name":"Tosca","startDate":"2027-03-02"}]}]}</script>
            <script type="application/ld+json">[{"@type":"Product","name":"Mug"},{"@type":"Product"},{"@type":"Offer","name":"Gift","price":5}]</script>"#;
        assert_eq!(item_texts(html), ["Event Tosca (JSON-LD): startDate 2027-03-02.", "Product Mug (JSON-LD)."]);
    }

    #[test]
    fn a_full_iri_type_is_named_by_its_last_part_and_a_name_in_several_languages_by_its_first() {
        let html = r#"<script type="application/ld+json">{"@type":"https://schema.org/Event","name":[{"@value":"x"},"Tosca","Tosca (en)"]}</script>"#;
        assert_eq!(item_texts(html), ["Event Tosca (JSON-LD)."]);
    }

    #[test]
    fn items_are_structured_numbered_from_the_given_index_and_a_long_description_is_cut() {
        let long = "word ".repeat(100);
        let html = format!(
            r#"<script type="application/ld+json">{{"@type":"Event","name":"X","description":"{long}"}}</script>"#
        );
        let b = items(&html, 5);
        assert_eq!((b[0].i, b[0].kind), (5, Kind::Structured));
        assert!(b[0].text.ends_with("…."));
        assert!(b[0].text.chars().count() < 340);
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

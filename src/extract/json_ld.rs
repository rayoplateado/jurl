//! The values a page gives in its JSON-LD (schema.org): an event's dates, an offer's price, a course's duration. A precise
//! answer may be one of them, when the page's text gives none (see `structured_or` in blocks.rs). Each is a block of its own
//! that names its source, so the quote shows the field: "Event VivaTech 2027 (JSON-LD): startDate 2027-06-16, endDate 2027-06-19."
//! The photos of what a page offers are not text: `product_images` gives them to the image candidates instead.

use scraper::{Html, Selector};
use serde_json::{Map, Value};
use url::Url;

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
/// The longest text an item keeps of a value (a link is kept whole, or it could not be followed), and the longest item, in
/// characters: a long catalogue stays readable.
const MAX_VALUE_CHARS: usize = 200;
const MAX_ITEM_CHARS: usize = 1200;
/// How many typed objects make a list of things inside a thing (a venue's events, a list's elements). Fewer are the thing's
/// own values: an event's offers or performers, a product's photos.
const MIN_LIST: usize = 3;
/// Types that are the site's machinery, not anything on the page: they give nothing.
const MACHINERY_TYPES: &[&str] = &["BreadcrumbList", "SiteNavigationElement", "SearchAction"];
/// Keys that name a node or its language in JSON-LD rather than say something of it. An object's `@type` says what kind of
/// value it is (a Place, an Offer), which its path already says.
const NODE_KEYS: &[&str] = &["@context", "@id", "@type", "@language"];

/// The things a page's JSON-LD holds, each one block, numbered from `first`. Nothing here knows what a type means: a thing
/// gives every value it holds under its path, each value once, and the reader decides which matter. "MusicEvent (JSON-LD):
/// name: Lola Índigo; location.name: WiZink Center; offers.price: 25; offers.priceCurrency: EUR; startDate: 2027-03-14".
///
/// A typed object at the top, in `@graph` or in the page's own object (a WebPage's `mainEntity`) is a thing. A list of
/// MIN_LIST or more typed objects inside a thing is a list of things, each one a thing of its own, and a list's wrapper (a
/// ListItem around its `item`) gives way to what it wraps. Read for `--with-pages`, so a list read can take its rows from the
/// page's own data.
pub fn items(html: &str, first: usize) -> Vec<Block> {
    let doc = Html::parse_document(html);
    let script = Selector::parse(r#"script[type="application/ld+json"]"#).expect("static selector");
    let mut texts: Vec<String> = Vec::new();
    for el in doc.select(&script) {
        let body: String = el.text().collect();
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            collect_things(&value, &mut texts);
        }
        if texts.len() >= MAX_ITEMS {
            break;
        }
    }
    texts.truncate(MAX_ITEMS);
    texts.into_iter().enumerate().map(|(k, text)| Block::new(first + k, Kind::Structured, text)).collect()
}

/// A type's own name: "https://schema.org/Event" is an Event.
fn short(kind: &str) -> &str {
    kind.rsplit('/').next().unwrap_or(kind)
}

/// The things a JSON-LD value holds at its top. An object with no type (a `@graph`'s holder), the page itself or the site
/// is not a thing, but what it holds may be; the site's machinery gives nothing.
fn collect_things(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Array(values) => values.iter().for_each(|v| collect_things(v, out)),
        Value::Object(map) => {
            let kinds = types(map);
            if kinds.iter().any(|k| MACHINERY_TYPES.contains(&short(k))) {
                return;
            }
            if kinds.is_empty() || kinds.iter().any(|k| short(k).ends_with("Page") || short(k) == "WebSite") {
                for (key, child) in map.iter().filter(|(k, _)| !NODE_KEYS.contains(&k.as_str())) {
                    if child.is_object() || child.is_array() || key == "@graph" {
                        collect_things(child, out);
                    }
                }
                return;
            }
            thing(map, false, out);
        }
        _ => {}
    }
}

/// The typed objects of a value that is a list of MIN_LIST or more of them.
fn list_of(value: &Value) -> Option<Vec<&Map<String, Value>>> {
    let things: Vec<&Map<String, Value>> =
        value.as_array()?.iter().filter_map(Value::as_object).filter(|o| !types(o).is_empty()).collect();
    (things.len() >= MIN_LIST).then_some(things)
}

/// What a list's element stands for: the one typed object it wraps, when it has no name of its own (a ListItem's `item`),
/// else itself.
fn unwrapped(map: &Map<String, Value>) -> &Map<String, Value> {
    if map.contains_key("name") {
        return map;
    }
    let mut wrapped = map.values().filter_map(Value::as_object).filter(|o| !types(o).is_empty());
    match (wrapped.next(), wrapped.next()) {
        (Some(inner), None) => inner,
        _ => map,
    }
}

/// A thing's block, then each thing it lists, when it is not itself in a list.
fn thing(map: &Map<String, Value>, in_list: bool, out: &mut Vec<String>) {
    let map = if in_list { unwrapped(map) } else { map };
    let kind = types(map).first().map(|k| short(k).to_string()).unwrap_or_else(|| "Thing".into());
    let mut keys: Vec<&String> = map.keys().filter(|k| !NODE_KEYS.contains(&k.as_str())).collect();
    // The name first, then the other keys in the map's order.
    keys.sort_by_key(|k| *k != "name");
    let lists: Vec<&String> =
        if in_list { Vec::new() } else { keys.iter().copied().filter(|k| list_of(&map[*k]).is_some()).collect() };
    let mut values = Vec::new();
    for key in keys.iter().filter(|k| !lists.contains(k)) {
        flatten(key, &map[*key], &mut values);
    }
    if let Some(text) = said(&format!("{kind} (JSON-LD): "), &values)
        && !out.contains(&text)
    {
        out.push(text);
    }
    for key in lists {
        for element in list_of(&map[key]).into_iter().flatten() {
            thing(element, true, out);
        }
    }
}

/// A thing's text: `head`, then each of its values with its path, as long as the text stays within MAX_ITEM_CHARS. A value said
/// again under a path of the same last name (an offer's url that is the event's) is said once. None when it says no value.
fn said(head: &str, values: &[(String, String)]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let mut text = head.to_string();
    let mut said = 0;
    for (path, value) in values {
        if !seen.insert((path.rsplit('.').next().unwrap_or(path), value.as_str())) {
            continue;
        }
        let part = format!("{}{path}: {value}", if said > 0 { "; " } else { "" });
        if said > 0 && text.chars().count() + part.chars().count() > MAX_ITEM_CHARS {
            continue;
        }
        text.push_str(&part);
        said += 1;
    }
    (said > 0).then_some(text)
}

/// The text of one thing of a page's own data, as a JSON-LD thing's (see `items`): "Thing (data): name: …; prices.price: 5.75".
/// The data writes its own addresses from the site's root ("/product_images/1.jpg"), so a value that reads as such a path is
/// given whole, on `base` (see `site_path`).
pub fn data_thing(map: &Map<String, Value>, base: &Url) -> Option<String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by_key(|k| *k != "name");
    let mut values = Vec::new();
    for key in keys {
        flatten(key, &map[key], &mut values);
    }
    for (_, value) in values.iter_mut() {
        if site_path(value)
            && let Ok(url) = base.join(value)
        {
            *value = url.to_string();
        }
    }
    said("Thing (data): ", &values)
}

/// Whether a value reads as an address on the site written from its root: no spaces, not cut (see `flatten`), and a file
/// ("/img/1.jpg") or two steps or more ("/aceites/p/188385"). A unit ("/kg") or the root alone is no address.
fn site_path(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('/') else { return false };
    !rest.starts_with('/')
        && !value.contains(char::is_whitespace)
        && !value.ends_with('…')
        && (rest.trim_end_matches('/').contains('/') || rest.rsplit('/').next().is_some_and(|last| last.contains('.')))
}

/// Every value under `path`, with its path: a text with its spaces collapsed and cut to MAX_VALUE_CHARS (a link kept whole),
/// a number or a yes/no as written, each value of a list, and an object's values under their own paths. A schema.org term
/// ("https://schema.org/InStock") is the vocabulary's word, not the page's, and a language-tagged value (`@value`) is its
/// object's.
fn flatten(path: &str, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::String(s) => {
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            let link = s.starts_with("https://") || s.starts_with("http://");
            if s.is_empty() || ["https://schema.org/", "http://schema.org/"].iter().any(|p| s.starts_with(p)) {
                return;
            }
            let s = if link || s.chars().count() <= MAX_VALUE_CHARS {
                s
            } else {
                format!("{}…", s.chars().take(MAX_VALUE_CHARS).collect::<String>())
            };
            out.push((path.to_string(), s));
        }
        Value::Number(n) => out.push((path.to_string(), n.to_string())),
        Value::Bool(b) => out.push((path.to_string(), b.to_string())),
        Value::Object(map) => {
            for (key, child) in map.iter().filter(|(k, _)| !NODE_KEYS.contains(&k.as_str())) {
                let inner = if key == "@value" { path.to_string() } else { format!("{path}.{key}") };
                flatten(&inner, child, out);
            }
        }
        Value::Array(values) => values.iter().for_each(|v| flatten(path, v, out)),
        Value::Null => {}
    }
}

/// The most photos one page gives from its JSON-LD: a flat's or a product's whole gallery, as `pages` lists at most.
const MAX_IMAGES: usize = 40;

/// The types whose `image` is a photo of the thing the page offers: a product or an offer, a home or a place to stay, a vehicle,
/// an event (any type named `…Event`), a place to visit, a recipe. Not a person, an organization or a page, whose image is a
/// portrait, a logo or a banner.
const PHOTO_TYPES: &[&str] = &[
    "Product",
    "Offer",
    "IndividualProduct",
    "ProductModel",
    "RealEstateListing",
    "Accommodation",
    "Apartment",
    "House",
    "SingleFamilyResidence",
    "Residence",
    "ApartmentComplex",
    "Room",
    "HotelRoom",
    "Suite",
    "LodgingBusiness",
    "Hotel",
    "Hostel",
    "Resort",
    "BedAndBreakfast",
    "VacationRental",
    "Campground",
    "Vehicle",
    "Car",
    "Motorcycle",
    "Event",
    "TouristAttraction",
    "LandmarksOrHistoricalBuildings",
    "Museum",
    "Recipe",
];

fn photo_type(t: &str) -> bool {
    PHOTO_TYPES.contains(&t) || t.ends_with("Event")
}

/// The photos a page's JSON-LD gives the things it offers: the `image` of each node of a [`PHOTO_TYPES`] type, as a URL, a list
/// of URLs, or an ImageObject. They are image candidates, not text: `html` reads them after og:image.
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

/// Walks a JSON-LD value depth first, and takes the `image` of each node of a photo type it finds. An image counts where it is
/// written: nothing is inherited from a product downwards, since a product's review has an author with an avatar of their
/// own, and that is not a photo of the product.
fn visit_images(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|v| visit_images(v, out)),
        Value::Object(map) => {
            if types(map).iter().any(|t| photo_type(t))
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
    fn the_image_of_a_page_a_person_or_an_organization_is_not_a_photo_of_what_it_offers() {
        let html = r#"<script type="application/ld+json">{"@graph":[{"@type":"Article","image":"https://cdn.example.test/news.jpg"},
            {"@type":"Organization","image":"https://cdn.example.test/logo.png"},{"@type":"WebPage","image":"https://cdn.example.test/hero.jpg"},
            {"@type":"Person","image":"https://cdn.example.test/face.jpg"}]}</script>"#;
        assert!(photos(html).is_empty());
    }

    #[test]
    fn a_listings_whole_gallery_is_a_candidate_whatever_kind_of_thing_it_offers() {
        // A flat's listing gives its eleven photos only here; an event's type is any `…Event`.
        let gallery: Vec<String> = (0..11).map(|n| format!("https://cdn.example.test/flat/{n}.jpg")).collect();
        let html = format!(
            r#"<script type="application/ld+json">{{"@graph":[{{"@type":"Organization","logo":"https://cdn.example.test/logo.png"}},
            {{"@type":"RealEstateListing","name":"Piso en venta","image":{}}}]}}</script>
            <script type="application/ld+json">{{"@type":"MusicEvent","image":"https://cdn.example.test/gig.jpg"}}</script>"#,
            serde_json::to_string(&gallery).unwrap()
        );
        let mut expected = gallery.clone();
        expected.push("https://cdn.example.test/gig.jpg".to_string());
        assert_eq!(photos(&html), expected);
    }

    #[test]
    fn a_page_gives_at_most_max_images_photos() {
        let gallery: Vec<String> = (0..MAX_IMAGES + 5).map(|n| format!("https://cdn.example.test/{n}.jpg")).collect();
        let html = format!(
            r#"<script type="application/ld+json">{{"@type":"Product","image":{}}}</script>"#,
            serde_json::to_string(&gallery).unwrap()
        );
        assert_eq!(photos(&html).len(), MAX_IMAGES);
    }

    fn item_texts(html: &str) -> Vec<String> {
        items(html, 0).into_iter().map(|b| b.text).collect()
    }

    #[test]
    fn a_listing_gives_each_element_with_every_value_it_holds_under_its_path() {
        // Each ListItem gives way to its item. The offer stays the event's, and the performer's name, the event's own, is said once.
        let html = r#"<script type="application/ld+json">{"@context":"https://schema.org","@type":"ItemList","numberOfItems":3,"itemListElement":[
            {"@type":"ListItem","position":1,"item":{"@type":"MusicEvent","name":"Lola Índigo","startDate":"2027-03-14","endDate":"2027-03-14",
              "location":{"@type":"Place","name":"WiZink Center","address":{"addressLocality":"Madrid"}},
              "offers":[{"@type":"Offer","price":25,"priceCurrency":"EUR","availability":"https://schema.org/InStock"}],
              "performer":[{"@type":"Person","name":"Lola Índigo"}],"description":"  Gira   2027. "}},
            {"@type":"ListItem","position":2,"item":{"@type":"JobPosting","title":"Cook","baseSalary":{"value":{"minValue":1500}}}},
            {"@type":"ListItem","position":3,"name":"Tosca","url":"https://venue.example.test/tosca"}]}</script>"#;
        assert_eq!(
            item_texts(html),
            [
                "ItemList (JSON-LD): numberOfItems: 3",
                "MusicEvent (JSON-LD): name: Lola Índigo; description: Gira 2027.; endDate: 2027-03-14; location.address.addressLocality: Madrid; location.name: WiZink Center; offers.price: 25; offers.priceCurrency: EUR; startDate: 2027-03-14",
                "JobPosting (JSON-LD): baseSalary.value.minValue: 1500; title: Cook",
                "ListItem (JSON-LD): name: Tosca; position: 3; url: https://venue.example.test/tosca"
            ]
        );
    }

    #[test]
    fn a_product_keeps_its_offers_and_photos_and_a_pages_main_entity_and_graph_are_read() {
        let html = r#"<script type="application/ld+json">{"@type":"CollectionPage","name":"Agenda","mainEntity":
            {"@type":"Product","name":[{"@value":"Taza","@language":"es"}],"image":[{"@type":"ImageObject","url":"https://cdn.example.test/1.jpg"}],
             "offers":[{"@type":"Offer","price":5},{"@type":"Offer","price":7}]}}</script>
            <script type="application/ld+json">{"@type":"WebPage","@graph":[{"@type":"BreadcrumbList","itemListElement":[
              {"@type":"ListItem","name":"A"},{"@type":"ListItem","name":"B"},{"@type":"ListItem","name":"C"}]},
              {"@type":"Place","name":"Teatro Real","event":[{"@type":"Event","name":"Tosca"},{"@type":"Event","name":"Aida"},{"@type":"Event","name":"Norma"}]}]}</script>"#;
        assert_eq!(
            item_texts(html),
            [
                "Product (JSON-LD): name: Taza; image.url: https://cdn.example.test/1.jpg; offers.price: 5; offers.price: 7",
                "Place (JSON-LD): name: Teatro Real",
                "Event (JSON-LD): name: Tosca",
                "Event (JSON-LD): name: Aida",
                "Event (JSON-LD): name: Norma"
            ]
        );
    }

    #[test]
    fn items_are_structured_numbered_from_the_given_index_a_long_text_is_cut_but_not_a_link_and_a_repeat_is_kept_once()
    {
        let long = "word ".repeat(100);
        let link = format!("https://cdn.example.test/{}", "x".repeat(300));
        let one = format!(
            r#"<script type="application/ld+json">{{"@type":"Event","name":"X","description":"{long}","url":"{link}"}}</script>"#
        );
        let b = items(&format!("{one}{one}"), 5);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].i, b[0].kind), (5, Kind::Structured));
        assert!(b[0].text.contains("…;") && b[0].text.ends_with(&link));
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

    #[test]
    fn a_data_thing_gives_its_site_paths_whole_and_leaves_other_values() {
        let base = Url::parse("https://shop.test/api/c?page=2").unwrap();
        let thing =
            serde_json::json!({ "name": "Oil", "image": "/img/1.jpg", "url": "/oils/p/1", "per": "/kg", "root": "/" });
        assert_eq!(
            data_thing(thing.as_object().unwrap(), &base).unwrap(),
            "Thing (data): name: Oil; image: https://shop.test/img/1.jpg; per: /kg; root: /; url: https://shop.test/oils/p/1"
        );
    }
}

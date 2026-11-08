//! The variants an add-to-cart form offers: its `<select>`s ("Size: 500g (other options: 1kg, 5 Kg)"). A shop puts a
//! product's sizes and weights in a drop-down of the form that buys it, and the reader skips forms, so a page showing
//! "from £8.20" would give no size at all. Only a form that buys is read: a search's or a filter's drop-down says nothing
//! about the product. A page of many products, each with a quick-add form, gives none: its lines would be the list's.

use std::collections::HashMap;

use scraper::{ElementRef, Html, Selector};

use super::chosen::labels_of;
use super::collapse;

/// The most options a drop-down lists: past these it is a list of content (countries, years), not a product's variants.
const MAX_OPTIONS: usize = 30;
/// The most characters of an option's or a label's text.
const MAX_CHARS: usize = 60;
/// The most buying forms with variants a page may have and be a product's page (a main form and a sticky bar).
const MAX_FORMS: usize = 3;
/// Words that make a button, a name or a form's address one of a cart.
const CART_WORDS: &[&str] = &[
    "add to cart",
    "add to basket",
    "add to bag",
    "buy now",
    "add-to-cart",
    "add_to_cart",
    "/cart/add",
    "añadir al carrito",
    "añadir a la cesta",
    "comprar",
    "ajouter au panier",
    "in den warenkorb",
];

/// The page's variant lines in page order, each once. `unseen` says whether a reader never sees an element.
pub(super) fn variant_lines(doc: &Html, unseen: impl Fn(ElementRef) -> bool) -> Vec<String> {
    let form = Selector::parse("form").expect("static selector");
    let select = Selector::parse("select").expect("static selector");
    let labels = labels_of(doc);
    let mut lines: Vec<String> = Vec::new();
    let mut forms = 0;
    for f in doc.select(&form).filter(|f| !unseen(*f) && buys(*f)) {
        // Its drop-downs: those inside it, and those elsewhere that name it in `form="id"` (a theme's variant picker).
        let id = f.value().id();
        let outside = doc.select(&select).filter(|s| id.is_some() && s.value().attr("form") == id);
        let found: Vec<String> =
            f.select(&select).chain(outside).filter(|s| !unseen(*s)).filter_map(|s| line_of(f, s, &labels)).collect();
        if found.is_empty() {
            continue;
        }
        forms += 1;
        for line in found {
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    if forms > MAX_FORMS { Vec::new() } else { lines }
}

/// Whether the form buys: it posts to a cart, has a quantity, or has a control that adds to one.
fn buys(form: ElementRef) -> bool {
    let words = |t: &str| {
        let t = t.to_ascii_lowercase();
        CART_WORDS.iter().any(|w| t.contains(w))
    };
    if form.value().attr("action").is_some_and(words) {
        return true;
    }
    form.descendants().filter_map(ElementRef::wrap).any(|e| {
        let v = e.value();
        let named = |attr: &str| v.attr(attr).is_some_and(words);
        (v.name() == "input" && v.attr("name").is_some_and(|n| matches!(n, "quantity" | "qty") || words(n)))
            || v.attr("data-button-action") == Some("add-to-cart")
            || (matches!(v.name(), "button" | "input")
                && (named("name") || named("value") || named("id") || words(&e.text().collect::<String>())))
    })
}

/// A drop-down's line: its label, the selected option, then the other options. None for fewer than two options, a
/// quantity, or a drop-down of numbers only.
fn line_of(form: ElementRef, select: ElementRef, labels: &HashMap<&str, String>) -> Option<String> {
    let option = Selector::parse("option").expect("static selector");
    let mut options: Vec<(String, bool)> = Vec::new();
    for o in select.select(&option) {
        let v = o.value();
        let text = collapse(&o.text().collect::<String>());
        let placeholder = v.attr("value").is_none() && placeholder(&text);
        if v.attr("value").is_some_and(|x| x.trim().is_empty()) || v.attr("disabled").is_some() || placeholder {
            continue;
        }
        if text.is_empty() || text.chars().count() > MAX_CHARS || options.iter().any(|(t, _)| *t == text) {
            continue;
        }
        options.push((text, v.attr("selected").is_some()));
    }
    if options.len() < 2
        || options.len() > MAX_OPTIONS
        || options.iter().all(|(t, _)| t.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let label = label_of(form, select, labels);
    let sv = select.value();
    let quantity = |t: &str| matches!(t.to_ascii_lowercase().as_str(), "quantity" | "qty" | "quantité" | "cantidad");
    if [sv.attr("name"), sv.id(), label.as_deref()].into_iter().flatten().any(|t| quantity(t.trim_end_matches(':'))) {
        return None;
    }
    let at = options.iter().position(|(_, s)| *s).unwrap_or(0);
    let chosen = options.remove(at).0;
    let others = options.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(", ");
    let head = label.map(|l| format!("{}: ", l.trim_end_matches(':'))).unwrap_or_default();
    Some(format!("{head}{chosen} (other options: {others})"))
}

/// An option that asks for a choice rather than being one ("Choose an option", "Select size", "-- Please select --").
fn placeholder(text: &str) -> bool {
    let t = text.trim_start_matches(['-', ' ']).to_ascii_lowercase();
    ["choose", "select", "please", "elige", "seleccion", "choisir"].iter().any(|w| t.starts_with(w))
}

/// A drop-down's label: a `<label for>` in its form, else on the page, its ARIA label, or the label that wraps it.
fn label_of(form: ElementRef, select: ElementRef, labels: &HashMap<&str, String>) -> Option<String> {
    let v = select.value();
    let label = Selector::parse("label[for]").expect("static selector");
    let in_form = v.id().and_then(|id| {
        form.select(&label).find(|l| l.value().attr("for") == Some(id)).map(|l| collapse(&l.text().collect::<String>()))
    });
    in_form
        .or_else(|| v.id().and_then(|id| labels.get(id).cloned()))
        .or_else(|| v.attr("aria-label").map(collapse))
        .or_else(|| {
            let wrapper = select.ancestors().filter_map(ElementRef::wrap).find(|a| a.value().name() == "label")?;
            let own: String = wrapper.children().filter_map(|c| c.value().as_text().map(|t| t.to_string())).collect();
            Some(collapse(&own))
        })
        .filter(|t| !t.is_empty() && t.chars().count() <= MAX_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(body: &str) -> Vec<String> {
        variant_lines(&Html::parse_document(body), |_| false)
    }

    #[test]
    fn a_buying_forms_dropdown_names_its_selected_option_and_the_others() {
        let body = r#"<form action="/cart/add"><label for="s">Size</label><select id="s"><option value="500g" selected>500g</option>
            <option value="1kg">1kg</option><option value="5 Kg">5 Kg</option></select><button name="add">Add to cart</button></form>"#;
        assert_eq!(lines(body), ["Size: 500g (other options: 1kg, 5 Kg)"]);
    }

    #[test]
    fn a_placeholder_is_no_variant_and_the_first_is_the_default() {
        let body = r#"<form class="cart"><input name="quantity"><label for="w">Weight</label><select id="w">
            <option value="">Choose an option</option><option value="12kg">12kg</option><option value="24kg">24kg</option></select></form>"#;
        assert_eq!(lines(body), ["Weight: 12kg (other options: 24kg)"]);
        let bare = r#"<form action="/cart/add"><select aria-label="Size"><option>Choose size</option><option>500g</option><option>1kg</option></select></form>"#;
        assert_eq!(lines(bare), ["Size: 500g (other options: 1kg)"]);
    }

    #[test]
    fn a_search_or_filter_form_is_not_read() {
        let body = r#"<form action="/search"><select aria-label="Sort"><option>Price</option><option>Name</option></select><button>Go</button></form>"#;
        assert!(lines(body).is_empty());
    }

    #[test]
    fn a_long_list_a_single_option_or_a_quantity_is_no_variants() {
        let many: String = (1..=31).map(|i| format!("<option>Size {i}</option>")).collect();
        assert!(
            lines(&format!(r#"<form action="/cart/add"><select aria-label="Year">{many}</select></form>"#)).is_empty()
        );
        assert!(lines(r#"<form action="/cart/add"><select><option>Only</option></select></form>"#).is_empty());
        let qty =
            r#"<form action="/cart/add"><select name="quantity"><option>1</option><option>2</option></select></form>"#;
        assert!(lines(qty).is_empty());
        let nums = r#"<form action="/cart/add"><select aria-label="Amount"><option>10</option><option>20</option></select></form>"#;
        assert!(lines(nums).is_empty());
    }

    #[test]
    fn a_wrapping_label_names_the_dropdown_without_its_options_and_a_picker_outside_the_form_is_read() {
        let wrap = "<form action=\"/cart/add\"><label>Size <select><option>500g</option>\n   <option>1kg</option></select></label></form>";
        assert_eq!(lines(wrap), ["Size: 500g (other options: 1kg)"]);
        let outside = r#"<select form="pf" aria-label="Weight"><option>1kg</option><option>5kg</option></select>
            <form id="pf" action="/cart/add"><button>Add to cart</button></form>"#;
        assert_eq!(lines(outside), ["Weight: 1kg (other options: 5kg)"]);
    }

    #[test]
    fn a_hidden_dropdown_a_repeated_form_and_a_page_of_quick_adds_are_left_out() {
        let form = r#"<form action="/cart/add"><select aria-label="Size"><option>500g</option><option>1kg</option></select></form>"#;
        assert_eq!(lines(&format!("{form}{form}")), ["Size: 500g (other options: 1kg)"]);
        let hidden = |el: ElementRef| el.value().name() == "select";
        assert!(variant_lines(&Html::parse_document(form), hidden).is_empty());
        let grid: String = (0..4).map(|i| form.replace("500g", &format!("{i}00g"))).collect();
        assert!(lines(&grid).is_empty());
    }
}

//! The options a page was read with chosen: the selected one of each choice the page offers (a pricing page's "Pay
//! monthly / Pay yearly", a tab of plans, a size). A page shows what its chosen options show, so what is read from it is
//! for them: under "Pay yearly" a price is the yearly one, even where the page writes no period beside it. A caller that
//! asks for another option knows from these that the page, as read, shows a different one.
//!
//! A choice is a group of at least two options of one kind of control, exactly one of them selected: radio buttons that
//! share a `name` (`checked`), ARIA radios, tabs or options in one group (`aria-checked`, `aria-selected`), or toggle
//! buttons side by side (`aria-pressed`). A `<select>` is not one: on the pages read it is a search's filter or a
//! form's field, not what the page shows. Nothing here reads classes: a choice that marks its selected option only by
//! its look is not seen.

use std::collections::HashMap;

use scraper::{ElementRef, Html, node::Element};

use super::collapse;

/// The most choices a page gives: past these, a page of filters says nothing more about the content it shows.
const MAX_CHOICES: usize = 6;
/// The most other options a choice names.
const MAX_OTHERS: usize = 5;
/// The most characters of an option's or a choice's name: past these it is content, not a control's label.
const MAX_NAME_CHARS: usize = 60;

/// One choice: its name (its legend or ARIA label, when it has one) and its options in page order, each with whether it
/// is the selected one.
#[derive(Default)]
struct Choice {
    name: Option<String>,
    options: Vec<(String, bool)>,
}

/// The page's choices as lines, in page order: "Select payment mode: Pay yearly (other options: Pay monthly)". `unseen`
/// says whether a reader never sees an element (page chrome, or hidden): a choice in one is left out.
pub(super) fn chosen(doc: &Html, unseen: impl Fn(ElementRef) -> bool) -> Vec<String> {
    let labels = labels_of(doc);
    let mut order: Vec<String> = Vec::new();
    let mut choices: HashMap<String, Choice> = HashMap::new();
    for el in doc.root_element().descendants().filter_map(ElementRef::wrap) {
        let Some((key, selected)) = option_of(el) else { continue };
        if unseen(el) {
            continue;
        }
        let Some(text) = option_name(&labels, el) else { continue };
        let choice = choices.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            Choice { name: choice_name(doc, el), options: Vec::new() }
        });
        choice.options.push((text, selected));
    }
    let mut lines: Vec<String> = Vec::new();
    for line in order.iter().filter_map(|key| line_of(&choices[key])) {
        if !lines.contains(&line) {
            lines.push(line);
        }
        if lines.len() == MAX_CHOICES {
            break;
        }
    }
    lines
}

/// Whether the element is an option of a choice: the key of its choice, and whether it is selected.
fn option_of(el: ElementRef) -> Option<(String, bool)> {
    let v = el.value();
    let is = |name: &str, value: &str| v.attr(name).is_some_and(|a| a.trim().eq_ignore_ascii_case(value));
    if v.name() == "input" && is("type", "radio") {
        let name = v.attr("name").filter(|n| !n.is_empty())?;
        return Some((format!("radio {name}"), v.attr("checked").is_some()));
    }
    if let Some(role) = v.attr("role").map(str::trim)
        && matches!(role, "radio" | "tab" | "option" | "menuitemradio")
    {
        let group = el
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|a| {
                a.value()
                    .attr("role")
                    .is_some_and(|r| matches!(r.trim(), "radiogroup" | "tablist" | "listbox" | "menu" | "group"))
            })
            .or_else(|| el.parent().and_then(ElementRef::wrap))?;
        let selected = is("aria-checked", "true") || is("aria-selected", "true");
        return Some((format!("{role} {:?}", group.id()), selected));
    }
    if v.name() == "button" && v.attr("aria-pressed").is_some() {
        let parent = el.parent().and_then(ElementRef::wrap)?;
        return Some((format!("pressed {:?}", parent.id()), is("aria-pressed", "true")));
    }
    None
}

/// What an option says: its `<label>` (by `for`, or around it), its ARIA label, its text, or a radio's value.
fn option_name(labels: &HashMap<&str, String>, el: ElementRef) -> Option<String> {
    let v = el.value();
    let label = v
        .id()
        .and_then(|id| labels.get(id).cloned())
        .or_else(|| el.ancestors().filter_map(ElementRef::wrap).find(|a| a.value().name() == "label").map(text_of));
    let name = label
        .filter(|t| !t.is_empty())
        .or_else(|| v.attr("aria-label").map(collapse))
        .filter(|t| !t.is_empty())
        .or_else(|| Some(text_of(el)))
        .filter(|t| !t.is_empty())
        .or_else(|| v.attr("value").map(collapse))?;
    (!name.is_empty() && name.chars().count() <= MAX_NAME_CHARS).then_some(name)
}

/// The text of each `<label for>`, by the id it names; the first label of an id is its own.
pub(super) fn labels_of(doc: &Html) -> HashMap<&str, String> {
    let mut labels = HashMap::new();
    for label in doc.root_element().descendants().filter_map(ElementRef::wrap).filter(|e| e.value().name() == "label") {
        if let Some(id) = label.value().attr("for") {
            labels.entry(id).or_insert_with(|| text_of(label));
        }
    }
    labels
}

/// A choice's own name: the legend of the fieldset its option is in, else the ARIA label of the group around it.
fn choice_name(doc: &Html, el: ElementRef) -> Option<String> {
    let name = el.ancestors().filter_map(ElementRef::wrap).find_map(|a| {
        let v = a.value();
        if v.name() == "fieldset" {
            return a.children().filter_map(ElementRef::wrap).find(|c| c.value().name() == "legend").map(text_of);
        }
        let group =
            v.attr("role").is_some_and(|r| matches!(r.trim(), "radiogroup" | "tablist" | "listbox" | "menu" | "group"));
        group.then(|| aria_label(doc, v)).flatten()
    })?;
    (!name.is_empty() && name.chars().count() <= MAX_NAME_CHARS).then_some(name)
}

/// An element's `aria-label`, or the text of the element its `aria-labelledby` names.
fn aria_label(doc: &Html, v: &Element) -> Option<String> {
    v.attr("aria-label").map(collapse).filter(|t| !t.is_empty()).or_else(|| {
        let id = v.attr("aria-labelledby")?.split_whitespace().next()?;
        doc.root_element().descendants().filter_map(ElementRef::wrap).find(|e| e.value().id() == Some(id)).map(text_of)
    })
}

fn text_of(el: ElementRef) -> String {
    collapse(&el.text().collect::<String>())
}

/// A choice's line, when it is one: two options at least, exactly one of them selected.
fn line_of(choice: &Choice) -> Option<String> {
    if choice.options.len() < 2 || choice.options.iter().filter(|(_, selected)| *selected).count() != 1 {
        return None;
    }
    let selected = choice.options.iter().find(|(_, s)| *s).map(|(t, _)| t.as_str())?;
    let mut others: Vec<&str> = Vec::new();
    for (text, _) in choice.options.iter().filter(|(t, s)| !*s && t != selected) {
        if !others.contains(&text.as_str()) && others.len() < MAX_OTHERS {
            others.push(text);
        }
    }
    if others.is_empty() {
        return None;
    }
    let head = choice.name.as_ref().map(|n| format!("{}: ", n.trim_end_matches(':'))).unwrap_or_default();
    Some(format!("{head}{selected} (other options: {})", others.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(body: &str) -> Vec<String> {
        chosen(&Html::parse_document(body), |_| false)
    }

    #[test]
    fn a_checked_radio_names_its_choice_and_the_other_options() {
        let body = r#"<fieldset><legend>Select payment mode:</legend>
            <input type="radio" id="m" name="billing" value="month"><label for="m">Pay monthly</label>
            <input type="radio" id="y" name="billing" value="year" checked><label for="y">Pay yearly</label></fieldset>
            <p>Plus</p><p>€9.50 per member / month</p>"#;
        assert_eq!(lines(body), ["Select payment mode: Pay yearly (other options: Pay monthly)"]);
    }

    #[test]
    fn aria_tabs_and_pressed_buttons_are_choices_too() {
        let body = r#"<div role="tablist" aria-label="Billing">
              <button role="tab" aria-selected="false">Monthly</button><button role="tab" aria-selected="true">Annual</button></div>
            <div><button aria-pressed="true">EUR</button><button aria-pressed="false">USD</button></div>"#;
        assert_eq!(lines(body), ["Billing: Annual (other options: Monthly)", "EUR (other options: USD)"]);
    }

    #[test]
    fn a_select_is_no_choice() {
        assert!(
            lines(r#"<select aria-label="Seats"><option>1</option><option selected>5</option></select>"#).is_empty()
        );
    }

    #[test]
    fn a_group_with_no_selected_option_or_one_option_is_no_choice() {
        let none = r#"<input type="radio" name="a" value="x"><input type="radio" name="a" value="y">"#;
        assert!(lines(none).is_empty());
        let one = r#"<input type="radio" name="b" value="x" checked>"#;
        assert!(lines(one).is_empty());
        let two_selected = r#"<div role="radiogroup"><span role="radio" aria-checked="true">A</span><span role="radio" aria-checked="true">B</span></div>"#;
        assert!(lines(two_selected).is_empty());
    }

    #[test]
    fn a_choice_a_reader_never_sees_is_left_out() {
        let body =
            r#"<nav><input type="radio" name="l" value="en" checked><input type="radio" name="l" value="de"></nav>"#;
        let unseen = |el: ElementRef| el.ancestors().filter_map(ElementRef::wrap).any(|a| a.value().name() == "nav");
        assert!(chosen(&Html::parse_document(body), unseen).is_empty());
    }
}

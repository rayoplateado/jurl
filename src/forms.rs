//! A page's own search, as its HTML writes it: the forms that send what is typed in them to an address (method GET), and
//! the links that lead to a search page. Many sites keep their lists behind such a form (an official gazette, a register, a
//! catalogue): the front door links no list, and the list is the page the form's answer is. jurl only describes them, each
//! field with its name, its kind, its label, its value and its choices: what to type in them is the reader's call, made
//! from its own question (`--with-pages` gives them as a page's `forms` and `search_links`).

use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use url::Url;

/// The most forms, fields and choices given for a page: a search form has a few fields, and a page a few forms.
const MAX_FORMS: usize = 4;
const MAX_FIELDS: usize = 60;
const MAX_OPTIONS: usize = 40;
/// The most links to a search page given for a page.
const MAX_SEARCH_LINKS: usize = 4;
/// The longest label kept, in characters.
const MAX_LABEL: usize = 120;
/// The longest name or value a form may give, in characters: a longer one is the page's state (a token, a serialized view), and the form
/// is left out, as cutting it would send a wrong search.
const MAX_VALUE: usize = 256;

/// A form that sends its fields to `action` as a query (GET).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Form {
    pub(crate) action: String,
    pub(crate) fields: Vec<Field>,
}

/// One field of a form: `kind` is the input's type (`text`, `search`, `date`, `checkbox`, `radio`, `hidden`, `submit`…),
/// or `select`. `value` is what it sends as the page leaves it; a box sends it only when `checked`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) label: String,
    pub(crate) value: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) checked: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) options: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Choice {
    pub(crate) value: String,
    pub(crate) label: String,
}

/// The page's search forms: those sent by GET, with a field to type in, and none for a password or a file (a sign-in, an
/// upload), and no name or value over MAX_VALUE. None for a page with no form.
pub(crate) fn search_forms(html: &Html, base: &Url) -> Vec<Form> {
    let form_sel = Selector::parse("form").expect("static selector");
    html.select(&form_sel).filter_map(|form| form_of(html, form, base)).take(MAX_FORMS).collect()
}

fn form_of(html: &Html, form: ElementRef<'_>, base: &Url) -> Option<Form> {
    let el = form.value();
    if !el.attr("method").unwrap_or("get").trim().eq_ignore_ascii_case("get") {
        return None;
    }
    let action = base.join(el.attr("action").unwrap_or("").trim()).ok()?;
    if !matches!(action.scheme(), "http" | "https") {
        return None;
    }
    let field_sel = Selector::parse("input, select, textarea").expect("static selector");
    let mut fields = Vec::new();
    for control in form.select(&field_sel) {
        let c = control.value();
        let Some(name) = c.attr("name").map(str::trim).filter(|n| !n.is_empty()) else { continue };
        if c.attr("disabled").is_some() {
            continue;
        }
        let kind = match c.name() {
            "select" => "select".to_string(),
            "textarea" => "text".to_string(),
            _ => c.attr("type").unwrap_or("text").trim().to_ascii_lowercase(),
        };
        if matches!(kind.as_str(), "password" | "file") {
            return None;
        }
        if matches!(kind.as_str(), "button" | "reset" | "image") {
            continue;
        }
        let (value, options) = if kind == "select" { select_of(control) } else { (input_value(control), Vec::new()) };
        let too_long = |s: &str| s.chars().count() > MAX_VALUE;
        if too_long(name) || too_long(&value) || options.iter().any(|o| too_long(&o.value)) {
            return None;
        }
        fields.push(Field {
            name: name.to_string(),
            label: label_of(html, control),
            value,
            checked: c.attr("checked").is_some(),
            options,
            kind,
        });
        if fields.len() == MAX_FIELDS {
            break;
        }
    }
    let typed = fields.iter().any(|f| matches!(f.kind.as_str(), "text" | "search" | "date" | "number" | "month"));
    typed.then(|| Form { action: action.to_string(), fields })
}

fn input_value(control: ElementRef<'_>) -> String {
    match control.value().name() {
        "textarea" => control.text().collect::<String>().trim().to_string(),
        _ => control.value().attr("value").unwrap_or_default().to_string(),
    }
}

/// A select's value as the page leaves it (its selected choice, else its first) and its choices.
fn select_of(control: ElementRef<'_>) -> (String, Vec<Choice>) {
    let option_sel = Selector::parse("option").expect("static selector");
    let mut value = None;
    let mut options = Vec::new();
    for option in control.select(&option_sel) {
        let label = words(&option.text().collect::<String>());
        let v = option.value().attr("value").map(str::to_string).unwrap_or_else(|| label.clone());
        if option.value().attr("selected").is_some() || value.is_none() && options.is_empty() {
            value = Some(v.clone());
        }
        if options.len() < MAX_OPTIONS {
            options.push(Choice { value: v, label: short(&label) });
        }
    }
    (value.unwrap_or_default(), options)
}

/// What names a field for a person: its `<label for>`, the label around it, else its aria-label, title or placeholder.
fn label_of(html: &Html, control: ElementRef<'_>) -> String {
    let c = control.value();
    let by_for = c.attr("id").and_then(|id| {
        let sel = Selector::parse("label[for]").expect("static selector");
        html.select(&sel).find(|l| l.value().attr("for") == Some(id)).map(|l| words(&l.text().collect::<String>()))
    });
    let around = || {
        control
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|a| a.value().name() == "label")
            .map(|l| words(&l.text().collect::<String>()))
    };
    let named = || ["aria-label", "title", "placeholder"].iter().find_map(|a| c.attr(a)).map(words);
    short(
        &by_for.filter(|l| !l.is_empty()).or_else(around).filter(|l| !l.is_empty()).or_else(named).unwrap_or_default(),
    )
}

/// The links on the page that lead to a search page of its own site: their words, their title or their address name a
/// search ("Buscar", "Search", `/search/`). Their page may hold the form the front door does not.
pub(crate) fn search_links(html: &Html, base: &Url) -> Vec<String> {
    let sel = Selector::parse("a[href]").expect("static selector");
    let mut found: Vec<String> = Vec::new();
    for a in html.select(&sel) {
        let el = a.value();
        let Some(url) = el.attr("href").and_then(|h| base.join(h.trim()).ok()) else { continue };
        // The page itself again (in another language, at another anchor) is no other page.
        if url.host_str() != base.host_str() || url.path() == base.path() {
            continue;
        }
        let said = [
            words(&a.text().collect::<String>()),
            el.attr("title").unwrap_or("").to_string(),
            el.attr("aria-label").unwrap_or("").to_string(),
        ];
        let path = url.path().to_ascii_lowercase();
        let named = said.iter().any(|s| s.split_whitespace().any(is_search_word))
            || path.split(|c: char| !c.is_ascii_alphanumeric()).any(is_search_word);
        let mut bare = url.clone();
        bare.set_fragment(None);
        if named && !found.contains(&bare.to_string()) {
            found.push(bare.to_string());
            if found.len() == MAX_SEARCH_LINKS {
                break;
            }
        }
    }
    found
}

/// A word that names a search, in the languages jurl reads.
fn is_search_word(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    matches!(
        w.as_str(),
        "search"
            | "buscar"
            | "buscador"
            | "búsqueda"
            | "busqueda"
            | "cerca"
            | "ricerca"
            | "recherche"
            | "rechercher"
            | "suche"
            | "suchen"
            | "pesquisa"
            | "pesquisar"
            | "zoeken"
            | "cercar"
            | "bilatu"
    )
}

fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn short(s: &str) -> String {
    s.chars().take(MAX_LABEL).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://gazette.example/buscar/").unwrap()
    }

    #[test]
    fn a_get_form_gives_its_fields_with_their_labels_values_and_choices() {
        let html = Html::parse_document(
            &r#"<form method="GET" action="docs.php">
              <input type="hidden" name="field[0]" value="SECTION">
              <label><input type="checkbox" name="section[1]" value="1" checked> I. General</label>
              <input type="checkbox" name="section[5]" value="5" id="s5"><label for="s5">V. Notices</label>
              <label for="t">Title</label><input type="text" name="title" id="t">
              <input type="date" name="from" title="From">
              <select name="per_page" aria-label="Per page"><option>10</option><option value="50" selected>50 a page</option></select>
              <input type="submit" name="go" value="Search"><input type="reset" name="clear" value="Clear">
            </form>
            <form action="/state"><input name="q"><input type="hidden" name="view" value="{state}"></form>"#.replace("{state}", &"x".repeat(300)),
        );
        let forms = search_forms(&html, &base());
        assert_eq!(forms.len(), 1);
        let form = &forms[0];
        assert_eq!(form.action, "https://gazette.example/buscar/docs.php");
        let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["field[0]", "section[1]", "section[5]", "title", "from", "per_page", "go"]);
        assert!(form.fields[1].checked && form.fields[1].label == "I. General");
        assert!(!form.fields[2].checked && form.fields[2].label == "V. Notices");
        assert_eq!(form.fields[3].label, "Title");
        assert_eq!((form.fields[4].kind.as_str(), form.fields[4].label.as_str()), ("date", "From"));
        assert_eq!(form.fields[5].value, "50");
        assert_eq!(form.fields[5].options[0], Choice { value: "10".into(), label: "10".into() });
    }

    #[test]
    fn a_post_form_a_sign_in_and_a_form_with_nothing_to_type_are_no_search() {
        let html = Html::parse_document(
            r#"<form method="post" action="/s"><input name="q"></form>
               <form action="/login"><input name="user"><input type="password" name="pass"></form>
               <form action="/cart"><input type="hidden" name="id" value="3"><input type="submit" name="add" value="Add"></form>
               <form action="/s"><input name="q" placeholder="Search the site"></form>"#,
        );
        let forms = search_forms(&html, &base());
        assert_eq!(forms.len(), 1);
        assert_eq!(
            (forms[0].action.as_str(), forms[0].fields[0].label.as_str()),
            ("https://gazette.example/s", "Search the site")
        );
    }

    #[test]
    fn links_that_name_a_search_on_the_same_site_are_its_search_pages() {
        let html = Html::parse_document(
            r#"<a href="/buscar/docs.php">BOE</a><a href="/buscar/docs.php#top">again</a>
               <a href="/agenda" title="Buscar en la agenda">Agenda</a><a href="/news">News</a>
               <a href="https://other.example/search">Search elsewhere</a><a href="/s?q=" aria-label="Search">🔍</a>"#,
        );
        assert_eq!(
            search_links(&html, &Url::parse("https://gazette.example/").unwrap()),
            [
                "https://gazette.example/buscar/docs.php",
                "https://gazette.example/agenda",
                "https://gazette.example/s?q="
            ]
        );
    }
}

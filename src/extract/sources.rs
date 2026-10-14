//! The lists of things a page's own data holds, and the rows a recipe takes from one of them. A page's data is its JSON-LD,
//! its `<script type="application/json">` (an app's state, such as `__NEXT_DATA__`) and the objects its scripts assign
//! (`window.__SERVER_DATA__ = {…}`). Nothing here knows what a thing is: a list is any array of 3 or more objects, and a
//! recipe names a list and, for each of its fields, the path to a value inside each element.
//!
//! `--with-sources` gives the lists, so a reader can write a recipe from their samples; `--recipe` applies one, with no model:
//! every value it gives is the page's own value, as written, under the path the recipe names.

use std::collections::{BTreeMap, HashSet};

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The fewest objects an array needs to count as a list of things.
const MIN_LIST: usize = 3;
/// The most lists a page gives: the largest ones.
const MAX_LISTS: usize = 8;
/// The elements of an array that are looked into for lists inside them: a long list's lists are its elements' own.
const MAX_WALKED: usize = 50;
/// The values a sample element gives, and the characters of each: a sample shows paths, not whole texts.
const MAX_SAMPLE_VALUES: usize = 60;
const MAX_SAMPLE_CHARS: usize = 120;
/// The elements shown of each list.
const SAMPLES: usize = 2;
/// The assignments of one page that are parsed as JSON, and the size of a script that is not looked into. A minified bundle
/// holds many `e={}`: those fail at once, so the cap is high and only guards against a page made of them.
const MAX_ASSIGNMENTS: usize = 5_000;
const MAX_SCRIPT_BYTES: usize = 8_000_000;
/// The most rows a recipe gives from one page. A longer list says so with `rows_truncated`.
const MAX_ROWS: usize = 1_000;

/// A list of things in a page's data: which data (`ld:0`, `json:__NEXT_DATA__`, `var:__SERVER_DATA__`), the path to the
/// array (one key or index per step), how many objects it holds, and its first elements as `path: value` lines.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    pub source: String,
    pub path: Vec<String>,
    pub count: usize,
    pub sample: Vec<Vec<String>>,
}

/// Where a recipe's rows are and what each field takes. A step that ends in `*` is the one key that starts with what comes
/// before it (a key that carries the page's query, such as `cityEvents({"page":0})`); two such keys give no value.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub source: String,
    pub path: Vec<String>,
    pub fields: BTreeMap<String, Vec<String>>,
}

/// What a recipe gave on one page: a row per element of its list, each with the fields that have a value, and whether the
/// list had more than `MAX_ROWS`; or why there are no rows.
#[derive(Debug, Clone, PartialEq)]
pub enum Applied {
    Rows { rows: Vec<Map<String, Value>>, truncated: bool },
    RecipeError(String),
}

/// A page's data, read once for both `lists` and `apply`: each piece under the name a recipe gives it.
pub struct Data(Vec<(String, Value)>);

/// Reads `--recipe`: a JSON object with `source`, `path` (empty for data that is itself the list) and `fields`, each field a
/// path.
pub fn parse_recipe(text: &str) -> Result<Recipe, String> {
    let recipe: Recipe = serde_json::from_str(text).map_err(|e| format!("not a recipe: {e}"))?;
    if let Some(name) = recipe.fields.iter().find(|(_, path)| path.is_empty()).map(|(name, _)| name) {
        return Err(format!("not a recipe: the path of `{name}` is empty"));
    }
    Ok(recipe)
}

/// A list found on the way, before only the largest are kept: its score, its data's name, its path and its objects.
type Candidate<'d> = (usize, &'d str, Vec<String>, Vec<&'d Map<String, Value>>);

/// The page's lists of things, the largest first: those with more objects and more distinct keys.
pub fn lists(data: &Data) -> Vec<Source> {
    let mut found: Vec<Candidate> = Vec::new();
    for (name, root) in &data.0 {
        walk(root, &mut Vec::new(), &mut |path, objects| {
            let keys: HashSet<&String> = objects.iter().flat_map(|o| o.keys()).collect();
            found.push((objects.len() * keys.len().min(15), name, path.to_vec(), objects));
            // Only the largest are kept as the walk goes, so a large app state with many small arrays stays small.
            if found.len() > 4 * MAX_LISTS {
                found.sort_by_key(|f| std::cmp::Reverse(f.0));
                found.truncate(MAX_LISTS);
            }
        });
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found
        .into_iter()
        .take(MAX_LISTS)
        .map(|(_, name, path, objects)| Source {
            source: name.to_string(),
            path,
            count: objects.len(),
            sample: objects.iter().take(SAMPLES).map(|o| sample_of(o)).collect(),
        })
        .collect()
}

/// The rows a recipe takes from a page.
pub fn apply(data: &Data, recipe: &Recipe) -> Applied {
    let Some(root) = data.0.iter().find(|(name, _)| *name == recipe.source).map(|(_, v)| v) else {
        return Applied::RecipeError(format!("the page has no {}", recipe.source));
    };
    let Some(Value::Array(elements)) = at(root, &recipe.path) else {
        return Applied::RecipeError(format!("no list at {} {}", recipe.source, recipe.path.join(" ")));
    };
    let objects: Vec<&Value> = elements.iter().filter(|e| e.is_object()).collect();
    let rows = objects
        .iter()
        .take(MAX_ROWS)
        .map(|element| {
            let mut row = Map::new();
            for (name, path) in &recipe.fields {
                if let Some(text) = at(element, path).and_then(scalar) {
                    row.insert(name.clone(), Value::String(text));
                }
            }
            row
        })
        .collect();
    Applied::Rows { rows, truncated: objects.len() > MAX_ROWS }
}

/// Every piece of data the page holds, named as a recipe names it. A script that is not JSON is skipped. JSON-LD is found
/// as `json_ld` finds it, so `ld:N` is the Nth script it reads.
pub fn data_of(html: &str) -> Data {
    let doc = Html::parse_document(html);
    let script = Selector::parse("script").expect("static selector");
    let (mut out, mut ld, mut json, mut assignments): (Vec<(String, Value)>, usize, usize, usize) =
        (Vec::new(), 0, 0, 0);
    let keep = |out: &mut Vec<(String, Value)>, name: String, value: Value| {
        if !out.iter().any(|(n, _)| *n == name) {
            out.push((name, value));
        }
    };
    for el in doc.select(&script) {
        let kind = el.value().attr("type").unwrap_or("");
        let body: String = el.text().collect();
        match kind {
            "application/ld+json" => {
                if let Ok(v) = serde_json::from_str(&body) {
                    keep(&mut out, format!("ld:{ld}"), v);
                }
                ld += 1;
            }
            "application/json" => {
                let name = el.value().attr("id").map(String::from).unwrap_or_else(|| json.to_string());
                if let Ok(v) = serde_json::from_str(&body) {
                    keep(&mut out, format!("json:{name}"), v);
                }
                json += 1;
            }
            "" | "text/javascript" | "application/javascript" | "module" if body.len() <= MAX_SCRIPT_BYTES => {
                for (name, v) in assigned(&body, &mut assignments) {
                    keep(&mut out, name, v);
                }
            }
            _ => {}
        }
    }
    Data(out)
}

/// The JSON objects and arrays a script assigns to a name (`window.__STATE__ = {…}`, `var data = […]`), as `var:NAME`
/// without a `window.`, `self.` or `globalThis.` in front. `tried` counts the parses across the page's scripts.
fn assigned(body: &str, tried: &mut usize) -> Vec<(String, Value)> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    for (eq, _) in body.match_indices('=') {
        if *tried >= MAX_ASSIGNMENTS {
            break;
        }
        let before = bytes.get(eq.wrapping_sub(1)).copied();
        if matches!(before, Some(b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'&' | b'|' | b'%' | b'^')) {
            continue;
        }
        let rest = &body[eq + 1..];
        let start = eq + 1 + (rest.len() - rest.trim_start().len());
        if !matches!(bytes.get(start), Some(b'{' | b'[')) {
            continue;
        }
        let head = body[..eq].trim_end();
        let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.';
        // The name starts after the last character that can't be in one, which may take more than one byte.
        let name_at = head.char_indices().rev().find(|&(_, c)| !is_name(c)).map_or(0, |(i, c)| i + c.len_utf8());
        let name = &head[name_at..];
        let name = ["window.", "self.", "globalThis."].iter().find_map(|p| name.strip_prefix(p)).unwrap_or(name);
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
            continue;
        }
        *tried += 1;
        if let Some(Ok(v)) = serde_json::Deserializer::from_str(&body[start..]).into_iter::<Value>().next() {
            out.push((format!("var:{name}"), v));
        }
    }
    out
}

/// What `walk` calls with each list it finds: the list's path and its objects.
type Found<'a, 'v> = dyn FnMut(&[String], Vec<&'v Map<String, Value>>) + 'a;

/// Calls `found` with the path and objects of every array of 3 or more objects inside `value`.
fn walk<'v>(value: &'v Value, path: &mut Vec<String>, found: &mut Found<'_, 'v>) {
    match value {
        Value::Array(items) => {
            let objects: Vec<&Map<String, Value>> = items.iter().filter_map(Value::as_object).collect();
            if objects.len() >= MIN_LIST {
                found(path, objects);
            }
            for (i, item) in items.iter().take(MAX_WALKED).enumerate() {
                path.push(i.to_string());
                walk(item, path, found);
                path.pop();
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                path.push(key.clone());
                walk(child, path, found);
                path.pop();
            }
        }
        _ => {}
    }
}

/// An element as `path: value` lines, its keys joined with dots: what a recipe's field paths are written from.
fn sample_of(element: &Map<String, Value>) -> Vec<String> {
    let mut lines = Vec::new();
    for (key, value) in element {
        leaves(value, key, &mut lines);
    }
    lines
}

fn leaves(value: &Value, path: &str, out: &mut Vec<String>) {
    if out.len() >= MAX_SAMPLE_VALUES {
        return;
    }
    match value {
        Value::Object(map) => map.iter().for_each(|(k, v)| leaves(v, &format!("{path}.{k}"), out)),
        Value::Array(items) => items.iter().enumerate().for_each(|(i, v)| leaves(v, &format!("{path}.{i}"), out)),
        _ => {
            if let Some(text) = scalar(value) {
                let cut: String = text.chars().take(MAX_SAMPLE_CHARS).collect();
                let more = if cut.len() < text.len() { "…" } else { "" };
                out.push(format!("{path}: {cut}{more}"));
            }
        }
    }
}

/// The value at a path: a key of an object, an index of an array, or the one key that starts with a step's text before
/// its `*`.
fn at<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(value, |v, step| match v {
        Value::Array(items) => step.parse::<usize>().ok().and_then(|i| items.get(i)),
        Value::Object(map) => match step.strip_suffix('*') {
            Some(prefix) => {
                let mut matching = map.iter().filter(|(k, _)| k.starts_with(prefix));
                match (matching.next(), matching.next()) {
                    (Some((_, v)), None) => Some(v),
                    _ => None,
                }
            }
            None => map.get(step),
        },
        _ => None,
    })
}

/// A value as the page wrote it, as text: a text, a number or a yes/no. An object, a list or a null has none.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head>
<script type="application/ld+json">{"@type":"ItemList","itemListElement":[{"name":"A"},{"name":"B"},{"name":"C"}]}</script>
<script id="__NEXT_DATA__" type="application/json">{"props":{"queries":{"cityEvents({\"page\":0})":{"events":[
  {"title":"Banksy","dates":{"start":"2026-10-14T08:00:00Z"},"price":12.5,"venue":{"name":"Museo"}},
  {"title":"Dinolandia","dates":{"start":"2026-10-14T08:30:00Z"},"venue":{"name":"Bosque"}},
  {"title":"Rosalía","dates":{"start":"2026-10-15T20:00:00Z"},"price":45,"venue":{"name":"Wizink"}}]}}}}</script>
<script>if (a == b) {}; var año={}; x…={}; window.__SERVER_DATA__ = {"jsonld":[{"itemListElement":[{"item":{"name":"X"}},{"item":{"name":"Y"}},{"item":{"name":"Z"}},{"item":{"name":"W"}}]}]}; var code = {not: json};</script>
</head><body><p>Agenda</p></body></html>"#;

    fn recipe(source: &str, path: &[&str], fields: &[(&str, &[&str])]) -> Recipe {
        let steps = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let fields = fields.iter().map(|(n, p)| (n.to_string(), steps(p))).collect();
        Recipe { source: source.to_string(), path: steps(path), fields }
    }

    fn rows_of(applied: Applied) -> Vec<Map<String, Value>> {
        match applied {
            Applied::Rows { rows, truncated: false } => rows,
            other => panic!("not whole rows: {other:?}"),
        }
    }

    #[test]
    fn a_page_gives_the_lists_its_json_ld_its_app_state_and_its_scripts_hold_with_samples_by_path() {
        let found = lists(&data_of(PAGE));
        let named: Vec<(&str, String, usize)> =
            found.iter().map(|s| (s.source.as_str(), s.path.join(" "), s.count)).collect();
        assert!(named.contains(&("ld:0", "itemListElement".to_string(), 3)), "{named:?}");
        assert!(
            named.contains(&("json:__NEXT_DATA__", r#"props queries cityEvents({"page":0}) events"#.to_string(), 3)),
            "{named:?}"
        );
        assert!(named.contains(&("var:__SERVER_DATA__", "jsonld 0 itemListElement".to_string(), 4)), "{named:?}");
        let events = found.iter().find(|s| s.source == "json:__NEXT_DATA__").unwrap();
        assert_eq!(events.sample.len(), 2);
        assert!(events.sample[0].contains(&"dates.start: 2026-10-14T08:00:00Z".to_string()), "{:?}", events.sample[0]);
        assert!(events.sample[0].contains(&"price: 12.5".to_string()), "{:?}", events.sample[0]);
    }

    #[test]
    fn a_recipe_gives_each_elements_values_as_written_and_leaves_out_a_field_with_none() {
        let r = recipe(
            "json:__NEXT_DATA__",
            &["props", "queries", "cityEvents(*", "events"],
            &[
                ("title", &["title"]),
                ("date", &["dates", "start"]),
                ("price", &["price"]),
                ("venue", &["venue", "name"]),
            ],
        );
        let rows = rows_of(apply(&data_of(PAGE), &r));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["title"], "Banksy");
        assert_eq!(rows[0]["date"], "2026-10-14T08:00:00Z");
        assert_eq!(rows[0]["price"], "12.5");
        assert_eq!(rows[2]["price"], "45");
        assert_eq!(rows[1].get("price"), None);
        assert_eq!(rows[1]["venue"], "Bosque");
    }

    #[test]
    fn a_recipe_reads_a_scripts_assignment_past_non_ascii_names_and_says_why_it_has_no_rows() {
        let data = data_of(PAGE);
        let r = recipe("var:__SERVER_DATA__", &["jsonld", "0", "itemListElement"], &[("name", &["item", "name"])]);
        let rows = rows_of(apply(&data, &r));
        assert_eq!(rows.iter().map(|r| r["name"].as_str().unwrap()).collect::<Vec<_>>(), ["X", "Y", "Z", "W"]);
        let missing = recipe("json:nope", &["a"], &[("name", &["name"])]);
        assert_eq!(apply(&data, &missing), Applied::RecipeError("the page has no json:nope".to_string()));
        let moved = recipe("json:__NEXT_DATA__", &["props", "events"], &[("name", &["name"])]);
        assert_eq!(
            apply(&data, &moved),
            Applied::RecipeError("no list at json:__NEXT_DATA__ props events".to_string())
        );
    }

    #[test]
    fn a_list_longer_than_the_cap_gives_the_first_rows_and_says_so() {
        let many: Vec<Value> = (0..MAX_ROWS + 5).map(|i| serde_json::json!({ "n": i })).collect();
        let page = format!(r#"<script type="application/json" id="d">{}</script>"#, Value::from(many));
        let Applied::Rows { rows, truncated } = apply(&data_of(&page), &recipe("json:d", &[], &[("n", &["n"])])) else {
            panic!("no rows")
        };
        assert_eq!((rows.len(), truncated), (MAX_ROWS, true));
    }

    #[test]
    fn a_step_ending_in_a_star_takes_the_one_key_it_starts_and_none_when_two_do() {
        let one: Value = serde_json::json!({"q(page=1)": {"v": 1}, "other": 2});
        let two: Value = serde_json::json!({"q(page=1)": {"v": 1}, "q(page=2)": {"v": 2}});
        let path = vec!["q(*".to_string(), "v".to_string()];
        assert_eq!(at(&one, &path), Some(&Value::from(1)));
        assert_eq!(at(&two, &path), None);
    }

    #[test]
    fn a_recipe_is_refused_when_a_path_is_not_a_list_of_keys() {
        assert!(parse_recipe(r#"{"source":"ld:0","path":["a"],"fields":{"name":["name"]}}"#).is_ok());
        assert!(parse_recipe(r#"{"source":"ld:0","path":["a"],"fields":{"name":"name"}}"#).is_err());
        assert!(parse_recipe(r#"{"source":"ld:0","path":["a"],"fields":{"name":[]}}"#).unwrap_err().contains("`name`"));
        // An empty path is the data itself: a JSON-LD script that is a list of things.
        assert!(parse_recipe(r#"{"source":"ld:0","path":[],"fields":{"name":["name"]}}"#).is_ok());
        assert!(parse_recipe(r#"{"source":"ld:0","path":["a"],"fields":{},"extra":1}"#).is_err());
    }
}

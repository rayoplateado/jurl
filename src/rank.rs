//! Which of a site's pages get asked about. `--follow` reads the site's own map (`llms.txt`, `sitemap.xml`) and asks
//! about at most a fixed number of the pages it lists: the ones whose URLs best match the question. Ranked here by BM25
//! over the character 3-grams of the words in each URL (and in its link text, where the site gives one). A word is its
//! 3-grams with a space at each end, so "price" and "pricing" share " pr", "pri" and "ric", and "fundó" and "fundada"
//! share " fu", "fun" and "und": no word lists and no stemmer. A gram most pages have counts for little (its IDF), which
//! is what a stop list would do by hand.

use std::collections::{BTreeSet, HashMap};

use crate::extract::Link;

/// BM25's constants: how quickly a repeated gram stops adding to a page's score, and how much a long URL is discounted.
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// A character 3-gram.
type Gram = [char; 3];

/// The `max` of `links` whose URLs (and link text) best match `question`, best first. A list that fits in `max` is
/// returned as it is. Equal scores keep the list's order: llms.txt first, then the shallowest sitemap pages.
pub fn most_relevant(question: &str, links: Vec<Link>, max: usize) -> Vec<Link> {
    let mut links = links;
    if links.len() > max {
        let query: BTreeSet<Gram> = terms(question).into_iter().collect();
        let docs: Vec<Vec<Gram>> = links.iter().map(page_terms).collect();
        let mut ranked: Vec<(f64, Link)> = bm25(&query, &docs).into_iter().zip(links).collect();
        // A stable sort, so equal scores keep the list's order.
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        ranked.truncate(max);
        links = ranked.into_iter().map(|(_, l)| l).collect();
    }
    links.into_iter().enumerate().map(|(i, l)| Link { i, ..l }).collect()
}

/// Each page's BM25 score for the query's grams: for each gram of the query that the page has, its IDF (how few of the
/// pages have it) times how often the page has it, discounted for the page's length against the average.
fn bm25(query: &BTreeSet<Gram>, docs: &[Vec<Gram>]) -> Vec<f64> {
    let n = docs.len() as f64;
    let avg_len = docs.iter().map(Vec::len).sum::<usize>() as f64 / n;
    // How often each gram of the query is on each page, and on how many pages it is at all.
    let mut df: HashMap<Gram, usize> = HashMap::new();
    let counts: Vec<HashMap<Gram, usize>> = docs
        .iter()
        .map(|doc| {
            let mut count: HashMap<Gram, usize> = HashMap::new();
            for g in doc.iter().filter(|g| query.contains(*g)) {
                *count.entry(*g).or_insert(0) += 1;
            }
            for g in count.keys() {
                *df.entry(*g).or_insert(0) += 1;
            }
            count
        })
        .collect();
    // Summed in the query's order, so a page's score doesn't depend on the order its grams came in.
    let weights: Vec<(Gram, f64)> = query
        .iter()
        .map(|g| {
            let d = df.get(g).copied().unwrap_or(0) as f64;
            (*g, ((n - d + 0.5) / (d + 0.5) + 1.0).ln())
        })
        .collect();
    counts
        .iter()
        .zip(docs)
        .map(|(count, doc)| {
            let len = doc.len() as f64;
            weights
                .iter()
                .map(|&(g, idf)| match count.get(&g) {
                    Some(&tf) => {
                        let tf = tf as f64;
                        idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * len / avg_len))
                    }
                    None => 0.0,
                })
                .sum::<f64>()
        })
        .collect()
}

/// The grams a page is matched by: its URL path (percent-decoded) and its link text. A sitemap's entries have their
/// path as their text, so the path is counted once.
fn page_terms(link: &Link) -> Vec<Gram> {
    let path = link.url.path();
    let mut grams = terms(&percent_decode(path));
    if link.text != path {
        grams.extend(terms(&link.text));
    }
    grams
}

/// The character 3-grams of the words in `text`.
fn terms(text: &str) -> Vec<Gram> {
    words(text).iter().flat_map(|w| grams(w)).collect()
}

/// The words of `text`, lowercase: split on whatever isn't a letter or a digit (so `/`, `-`, `_` and `.` all split).
/// Words of one letter are left out: a possessive's `s` says nothing about a page. Accents are kept.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.chars().flat_map(char::to_lowercase).collect::<String>())
        .filter(|w| w.chars().count() >= 2)
        .collect()
}

/// The character 3-grams of a word, with a space at each end, so a word's start and end are grams too: "pricing" is
/// " pr", "pri", "ric", "ici", "cin", "ing", "ng ".
fn grams(word: &str) -> Vec<Gram> {
    let chars: Vec<char> = std::iter::once(' ').chain(word.chars()).chain(std::iter::once(' ')).collect();
    chars.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
}

/// `s` with its `%XX` escapes read as the bytes they stand for, as UTF-8 (`/fundaci%C3%B3n` is `/fundación`). An escape
/// that isn't two hex digits stays as it is.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |i: usize| b.get(i).and_then(|&c| (c as char).to_digit(16));
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn link(path: &str, text: &str) -> Link {
        Link {
            i: 0,
            url: Url::parse(&format!("https://x.com{path}")).unwrap(),
            text: text.to_string(),
            context: String::new(),
            marginal: false,
        }
    }

    fn links(paths: &[&str]) -> Vec<Link> {
        paths.iter().map(|p| link(p, "")).collect()
    }

    fn kept_paths(kept: &[Link]) -> Vec<&str> {
        kept.iter().map(|l| l.url.path()).collect()
    }

    /// A word's grams as strings, for the tests to read.
    fn gram_strings(word: &str) -> Vec<String> {
        grams(word).iter().map(|g| g.iter().collect::<String>()).collect()
    }

    #[test]
    fn a_word_is_its_padded_character_grams() {
        assert_eq!(gram_strings("pricing"), [" pr", "pri", "ric", "ici", "cin", "ing", "ng "]);
        assert_eq!(gram_strings("ai"), [" ai", "ai "]);
    }

    #[test]
    fn inflections_and_accents_share_grams() {
        let shared = |a: &str, b: &str| -> Vec<String> {
            let b = gram_strings(b);
            gram_strings(a).into_iter().filter(|g| b.contains(g)).collect()
        };
        assert!(shared("price", "pricing").contains(&"pri".to_string()));
        assert!(shared("prices", "pricing").contains(&"ric".to_string()));
        assert_eq!(shared("fundó", "fundada"), [" fu", "fun", "und"]);
    }

    #[test]
    fn words_are_lowercase_and_split_on_punctuation() {
        assert_eq!(words("/Workers-Limits_v2.html, a's"), ["workers", "limits", "v2", "html"]);
    }

    #[test]
    fn percent_escapes_are_read_as_utf8() {
        assert_eq!(percent_decode("/fundaci%C3%B3n"), "/fundación");
        assert_eq!(percent_decode("/100%/x%2"), "/100%/x%2");
        // The escaped path has the gram of "ón" that the escapes stand for.
        assert!(page_terms(&link("/fundaci%C3%B3n", "")).contains(&['ó', 'n', ' ']));
    }

    #[test]
    fn a_sitemap_entry_counts_its_path_once() {
        // A sitemap's text is its path, so the path's grams are counted once; a llms.txt text adds its own.
        let path_only = page_terms(&link("/pricing", ""));
        assert_eq!(page_terms(&link("/pricing", "/pricing")), path_only);
        assert_eq!(page_terms(&link("/pricing", "Pricing")).len(), 2 * path_only.len());
    }

    #[test]
    fn the_monthly_price_question_ranks_pricing_first() {
        let list = links(&["/", "/about", "/careers", "/blog/launch-week", "/pricing", "/customers", "/docs/api/keys"]);
        let kept = most_relevant("What is the monthly price of the cheapest paid plan?", list, 3);
        // The rest share no grams with the question, so they keep the order they came in.
        assert_eq!(kept_paths(&kept), ["/pricing", "/", "/about"]);
        assert_eq!(kept.iter().map(|l| l.i).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn a_gram_most_pages_have_counts_for_little() {
        // "the" is in every page here, so its grams have a low IDF; "price" is in one page, so its grams count for much.
        let mut list: Vec<Link> = (0..20).map(|i| link(&format!("/the-team-{i}"), "")).collect();
        list.push(link("/pricing", ""));
        let kept = most_relevant("What is the price?", list, 1);
        assert_eq!(kept_paths(&kept), ["/pricing"]);
    }

    #[test]
    fn ties_keep_the_list_order_and_a_list_that_fits_is_unchanged() {
        let kept = most_relevant("zzz", links(&["/x1", "/x2", "/x3", "/x4"]), 2);
        assert_eq!(kept_paths(&kept), ["/x1", "/x2"]);
        // Within the cap nothing is ranked, even when a word matches.
        let kept = most_relevant("What is the price?", links(&["/a", "/pricing"]), 2);
        assert_eq!(kept_paths(&kept), ["/a", "/pricing"]);
    }

    #[test]
    fn the_cap_keeps_the_pages_that_match_wherever_they_are() {
        let mut list: Vec<Link> = (0..200).map(|i| link(&format!("/page/{i}"), "")).collect();
        list[180] = link("/workers/limits", "");
        let kept = most_relevant("What are the Workers limits?", list, 150);
        assert_eq!(kept.len(), 150);
        assert_eq!(kept[0].url.path(), "/workers/limits");
        assert_eq!(kept[0].i, 0);
    }

    #[test]
    fn a_word_in_the_link_text_counts_too() {
        // llms.txt names this page "Enterprise plan": the question's words are in its text, not in its URL.
        let mut list: Vec<Link> = (0..200).map(|i| link(&format!("/page/{i}"), "")).collect();
        list.push(link("/p/42", "Enterprise plan"));
        let kept = most_relevant("What does the Enterprise plan cost?", list, 150);
        assert_eq!(kept.len(), 150);
        assert_eq!(kept[0].url.path(), "/p/42");
    }

    #[test]
    fn spanish_words_match_their_spanish_forms() {
        // "plan" and "Empresa" share grams with "planes" and "empresa": the page with both comes first.
        let list = links(&["/", "/planes", "/empresa", "/planes/empresa", "/contacto"]);
        let kept = most_relevant("¿Cuánto cuesta el plan Empresa?", list, 2);
        assert_eq!(kept[0].url.path(), "/planes/empresa");
    }

    #[test]
    fn fundo_shares_grams_with_fundacion_and_not_with_historia() {
        // "fundó" and "fundacion" share " fu", "fun" and "und"; "historia" shares no gram with the question.
        let list = links(&["/", "/historia", "/fundacion", "/contacto"]);
        let kept = most_relevant("¿En qué año se fundó la empresa?", list, 2);
        assert_eq!(kept[0].url.path(), "/fundacion");
    }
}

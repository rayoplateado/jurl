//! The next page of a list, from the page's own pagination. The standard comes first: a link with `rel="next"` (in the head,
//! or on an anchor) is the page the site says comes next. Without one, the address says it: a link to the same list whose
//! page number is the current page's plus one. A page number is a `page` or `paged` query parameter (`?page=3`,
//! `?e5273e04_page=2`), or the segments `page/3` at the end of the path (`/blog/page/3/`), and the link differs from the
//! current page in nothing else. The text of a link is never read, so this holds on a page in any language.

use url::Url;

use crate::extract::{Block, Extracted};

/// A URL with its page number taken out: the scheme and host, the path without the page's segments, the other query
/// parameters sorted, and the number. A URL with no page number is page 1 of itself.
#[derive(Debug, PartialEq, Eq)]
struct Slot {
    scheme: String,
    host: String,
    path: Vec<String>,
    query: Vec<(String, String)>,
    page: u32,
}

/// Whether a query parameter names a page: `page`, `paged`, or a name ending in one of them (Webflow's `e5273e04_page`).
fn is_page_param(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with("page") || name.ends_with("paged")
}

/// The page number a string is, when it is one: a whole number from 1.
fn number_of(v: &str) -> Option<u32> {
    v.parse::<u32>().ok().filter(|n| *n >= 1)
}

fn slot_of(u: &Url) -> Option<Slot> {
    let mut query: Vec<(String, String)> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let mut path: Vec<String> =
        u.path_segments().map(|s| s.filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default();
    let in_query: Vec<usize> = query
        .iter()
        .enumerate()
        .filter(|(_, (k, v))| is_page_param(k) && number_of(v).is_some())
        .map(|(at, _)| at)
        .collect();
    let in_path = match path.as_slice() {
        [.., word, number] if word.eq_ignore_ascii_case("page") && number_of(number).is_some() => Some(path.len() - 2),
        _ => None,
    };
    // A URL with more than one page number is not one list's pagination, so it has no slot.
    let page = match (in_query.as_slice(), in_path) {
        ([], None) => 1,
        ([at], None) => {
            let n = number_of(&query[*at].1)?;
            query.remove(*at);
            n
        }
        ([], Some(at)) => {
            let n = number_of(&path[at + 1])?;
            path.truncate(at);
            n
        }
        _ => return None,
    };
    query.sort();
    Some(Slot { scheme: u.scheme().to_string(), host: u.host_str()?.to_string(), path, query, page })
}

/// The first of `links` that is the page after `current` in its list: the same list, one page number on. `links` are in the
/// page's own order, so the first such link is the one the page shows first.
pub fn numbered_next<'a>(current: &Url, mut links: impl Iterator<Item = &'a Url>) -> Option<Url> {
    let here = slot_of(current)?;
    let next = here.page.checked_add(1)?;
    links.find(|link| slot_of(link).is_some_and(|s| s.page == next && same_list(&s, &here))).cloned()
}

/// Whether two slots name the same list: the same scheme, host, path and other parameters, whatever their page numbers.
fn same_list(a: &Slot, b: &Slot) -> bool {
    a.scheme == b.scheme && a.host == b.host && a.path == b.path && a.query == b.query
}

/// Whether two URLs are on the same site: the same scheme, and a host that is the same once `www.` is taken off both.
pub fn same_site(a: &Url, b: &Url) -> bool {
    let host = |u: &Url| u.host_str().map(|h| h.trim_start_matches("www.").to_string());
    a.scheme() == b.scheme() && host(a).is_some() && host(a) == host(b)
}

/// The most pages `--pages` reads: each is a request to the site.
pub const MAX_PAGES: usize = 50;

/// The pages of one list as one page of blocks: the first page's title, links and images, and every page's blocks in order,
/// numbered on from the first page's. Each block says which page it is from (`Block::page`, counted from 1), `pages` says
/// where each page is, and the ids of a page's lists stay apart from the next page's.
pub fn merge(pages: Vec<(Url, Extracted)>) -> Extracted {
    let urls: Vec<Url> = pages.iter().map(|(u, _)| u.clone()).collect();
    let mut merged: Option<Extracted> = None;
    let (mut lists, mut records) = (0, 0);
    for (n, (_, mut ex)) in pages.into_iter().enumerate() {
        let blocks = std::mem::take(&mut ex.blocks);
        let page_lists = blocks.iter().filter_map(|b| b.list).map(|l| l + 1).max().unwrap_or(0);
        let page_records = blocks.iter().filter_map(|b| b.record).max().unwrap_or(0);
        let out = merged.get_or_insert_with(|| Extracted { blocks: Vec::new(), ..ex });
        out.blocks.extend(blocks.into_iter().map(|b| Block {
            page: Some(n + 1),
            list: b.list.map(|l| l + lists),
            // Each page numbers its records from 1: the list goes on from the records before it.
            record: b.record.map(|r| r + records),
            ..b
        }));
        lists += page_lists;
        records += page_records;
    }
    let mut out = merged.unwrap_or_default();
    for (i, b) in out.blocks.iter_mut().enumerate() {
        b.i = i;
    }
    out.pages = urls;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    /// The next page the links name, from `current`.
    fn next(current: &str, links: &[&str]) -> Option<String> {
        let links: Vec<Url> = links.iter().map(|l| u(l)).collect();
        numbered_next(&u(current), links.iter()).map(String::from)
    }

    #[test]
    fn a_page_number_in_the_query_goes_up_by_one_from_the_page_it_is_on() {
        // No number is page 1: `?page=2` is the next page of the bare list, and a jump of two is not a next page.
        assert_eq!(
            next("https://x.test/agenda", &["https://x.test/agenda?page=2"]).as_deref(),
            Some("https://x.test/agenda?page=2")
        );
        assert_eq!(next("https://x.test/agenda", &["https://x.test/agenda?page=3"]), None);
        assert_eq!(
            next("https://x.test/agenda?page=2", &["https://x.test/agenda?page=3", "https://x.test/agenda?page=1"])
                .as_deref(),
            Some("https://x.test/agenda?page=3")
        );
        assert_eq!(next("https://x.test/agenda?page=3", &["https://x.test/agenda?page=2"]), None, "the previous page");
    }

    #[test]
    fn webflow_style_and_wordpress_style_parameters_count() {
        assert_eq!(
            next("https://www.salaclamores.es/calendario", &["https://www.salaclamores.es/calendario?e5273e04_page=2"])
                .as_deref(),
            Some("https://www.salaclamores.es/calendario?e5273e04_page=2")
        );
        assert_eq!(
            next("https://x.test/news/", &["https://x.test/news/?paged=2"]).as_deref(),
            Some("https://x.test/news/?paged=2")
        );
    }

    #[test]
    fn a_page_number_at_the_end_of_the_path_counts() {
        assert_eq!(
            next("https://x.test/blog/", &["https://x.test/blog/page/2/"]).as_deref(),
            Some("https://x.test/blog/page/2/")
        );
        assert_eq!(
            next("https://x.test/blog/page/2/", &["https://x.test/blog/page/3"]).as_deref(),
            Some("https://x.test/blog/page/3")
        );
        assert_eq!(next("https://x.test/blog/page/2/", &["https://x.test/blog/page/2/"]), None, "its own page");
    }

    #[test]
    fn other_numbers_in_a_link_are_not_pages() {
        // Product, article and variant numbers are not a list's pages: no `page` word, another list, or another site.
        let links = [
            "https://x.test/products/3",
            "https://x.test/shop?id=2",
            "https://x.test/shop?variant=2",
            "https://x.test/posts/2019/",
            "https://x.test/other?page=2",
            "https://x.test/shop?page=2&sort=asc",
            "https://y.test/shop?page=2",
        ];
        assert_eq!(next("https://x.test/shop", &links), None);
        assert_eq!(next("https://x.test/shop?id=1", &["https://x.test/shop?id=2"]), None);
    }

    #[test]
    fn the_other_parameters_must_match_in_any_order() {
        assert_eq!(
            next("https://x.test/agenda?page=1&city=bcn", &["https://x.test/agenda?city=bcn&page=2"]).as_deref(),
            Some("https://x.test/agenda?city=bcn&page=2")
        );
        assert_eq!(next("https://x.test/agenda?sort=desc", &["https://x.test/agenda?page=2&sort=asc"]), None);
    }

    #[test]
    fn a_url_with_two_page_numbers_has_no_next_page() {
        assert_eq!(next("https://x.test/a?page=1&e1_page=1", &["https://x.test/a?page=2&e1_page=1"]), None);
    }

    #[test]
    fn the_pages_of_a_list_are_one_list_of_blocks_in_page_order() {
        let page = |texts: &[&str]| Extracted {
            title: "Agenda".into(),
            blocks: texts
                .iter()
                .enumerate()
                .map(|(i, t)| Block { list: Some(0), ..Block::new(i, crate::extract::Kind::Item, t.to_string()) })
                .collect(),
            ..Default::default()
        };
        let merged = merge(vec![
            (u("https://x.test/agenda"), page(&["A", "B"])),
            (u("https://x.test/agenda?page=2"), page(&["C"])),
        ]);
        let got: Vec<_> = merged.blocks.iter().map(|b| (b.i, b.text.as_str(), b.page, b.list)).collect();
        // Page 2's list is a list of its own, numbered after page 1's, and the blocks are numbered on from page 1's.
        assert_eq!(got, [(0, "A", Some(1), Some(0)), (1, "B", Some(1), Some(0)), (2, "C", Some(2), Some(1))]);
        assert_eq!(merged.title, "Agenda");
        assert_eq!(merged.pages, [u("https://x.test/agenda"), u("https://x.test/agenda?page=2")]);
    }

    #[test]
    fn records_go_on_numbering_from_one_page_to_the_next() {
        let page = |records: &[usize]| Extracted {
            blocks: records
                .iter()
                .enumerate()
                .map(|(i, &n)| Block {
                    record: Some(n),
                    ..Block::new(i, crate::extract::Kind::Para, format!("record {n}"))
                })
                .chain(std::iter::once(Block::new(9, crate::extract::Kind::Para, "A paragraph".into())))
                .collect(),
            ..Default::default()
        };
        let merged = merge(vec![
            (u("https://x.test/agenda"), page(&[1, 2])),
            (u("https://x.test/agenda?page=2"), page(&[1, 2, 3])),
        ]);
        let got: Vec<_> = merged.blocks.iter().map(|b| (b.record, b.page)).collect();
        assert_eq!(
            got,
            [
                (Some(1), Some(1)),
                (Some(2), Some(1)),
                (None, Some(1)),
                (Some(3), Some(2)),
                (Some(4), Some(2)),
                (Some(5), Some(2)),
                (None, Some(2)),
            ]
        );
    }

    #[test]
    fn a_fragment_is_not_part_of_the_page() {
        assert_eq!(
            next("https://x.test/agenda", &["https://x.test/agenda?page=2#top"]).as_deref(),
            Some("https://x.test/agenda?page=2#top")
        );
    }

    #[test]
    fn same_site_ignores_a_www_prefix_but_not_a_scheme() {
        assert!(same_site(&u("https://www.sala-apolo.com/es/agenda"), &u("https://sala-apolo.com/es/agenda?page=2")));
        assert!(!same_site(&u("https://x.test/post"), &u("https://other.test/post-2")));
        assert!(!same_site(&u("https://x.test/post"), &u("http://x.test/post-2")));
    }
}

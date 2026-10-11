//! The language a page or a sitemap declares, as a BCP 47 tag, and how it relates to the start page's language. Tags are
//! checked against the IANA Language Subtag Registry (the `language-tags` crate): it lists the ISO 639 languages, the
//! ISO 3166 regions and the UN M.49 regions (`es-419`). Pure: the reading of the pages is `follow.rs`'s.

use language_tags::LanguageTag;
use url::Url;

/// How a page's language relates to the start page's, best first: the order `--follow` puts pages and sitemaps in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Relation {
    /// The start page's own tag: `es-es` for a start page that declares `es-ES`.
    Exact,
    /// Another tag of the start page's language: `es` or `es-mx` for `es-es`.
    Same,
    /// No language declared.
    Undeclared,
    /// Another language.
    Other,
}

/// A tag written the way the registry lists it: the language lowercase, a two-letter region uppercase, a four-letter
/// script capitalized, and `-` between subtags (`zh_CN` is `zh-CN`).
fn canonical(tag: &str) -> String {
    tag.split(['-', '_'])
        .enumerate()
        .map(|(i, part)| {
            let alpha = part.bytes().all(|b| b.is_ascii_alphabetic());
            match (i, part.len()) {
                (0, _) => part.to_ascii_lowercase(),
                (_, 2) if alpha => part.to_ascii_uppercase(),
                (_, 4) if alpha => {
                    let lower = part.to_ascii_lowercase();
                    format!("{}{}", lower[..1].to_ascii_uppercase(), &lower[1..])
                }
                _ => part.to_ascii_lowercase(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// `canon` as a tag the registry lists, or None: the crate's parse, then its validation of the primary language, the
/// script, the region and the variants against the registry.
fn registered(canon: &str) -> Option<LanguageTag> {
    let tag = LanguageTag::parse(canon).ok()?;
    tag.validate().ok()?;
    Some(tag)
}

/// The language a page or a sitemap declares (its `<html lang>`, or an `hreflang`), normalized: any language the registry
/// lists, with an optional script and region (`es-ES`, `zh-Hant-TW`, `fil`). None for anything else, `x-default`
/// among them.
pub fn declared(tag: &str) -> Option<String> {
    let canon = canonical(tag);
    let parsed = registered(&canon)?;
    (2..=3).contains(&parsed.primary_language().len()).then(|| canon.to_ascii_lowercase())
}

/// The language a path segment names, normalized: a language (`de`), or a language and a region (`es-es`, `es_ES`,
/// `de-ch`, `pt-BR`, `es-419`), and nothing else (`my-account` is not one: `account` is no region).
///
/// Only a two-letter language counts. A three-letter code is an ordinary word too (`car`, `bin`, `pro`), and a site's
/// locale is two letters in practice. The region is an ISO 3166-1 code or a UN M.49 region as the registry lists them;
/// the private-use regions (`XX`, `QM`) are not codes.
pub fn segment(segment: &str) -> Option<String> {
    let canon = canonical(segment);
    let mut parts = canon.split('-');
    let language = parts.next()?;
    let region = parts.next();
    if parts.next().is_some() || language.len() != 2 || !language.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    if let Some(region) = region {
        let iso = region.len() == 2
            && region.bytes().all(|b| b.is_ascii_uppercase())
            && !matches!(region.as_bytes()[0], b'X' | b'Q');
        let m49 = region.len() == 3 && region.bytes().all(|b| b.is_ascii_digit());
        if !(iso || m49) {
            return None;
        }
    }
    registered(&canon)?;
    Some(canon.to_ascii_lowercase())
}

/// The language a URL's path names, normalized: its first two segments are read, and the first that names a language
/// (see [`segment`]) is the locale (`/es-es/pricing`, `/us/es/pricing`). None when neither does (`/my-account`).
pub fn path_tag(url: &Url) -> Option<String> {
    url.path_segments()?.filter(|s| !s.is_empty()).take(2).find_map(segment)
}

/// The primary language subtag of a normalized tag: `es` for `es-es`.
pub fn primary(tag: &str) -> &str {
    tag.split('-').next().unwrap_or_default()
}

/// How the language of a page (`page`, normalized, None when it declares none) relates to the start page's (`start`,
/// normalized).
pub fn relation(start: &str, page: Option<&str>) -> Relation {
    match page {
        None => Relation::Undeclared,
        Some(page) if page == start => Relation::Exact,
        Some(page) if primary(page) == primary(start) => Relation::Same,
        Some(_) => Relation::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_segment_is_a_language_or_a_language_and_a_region() {
        for s in ["es", "es-es", "es_ES", "de-ch", "pt-BR", "zh-cn", "en-gb", "es-419"] {
            assert!(segment(s).is_some(), "{s}");
        }
        assert_eq!(segment("es_ES").as_deref(), Some("es-es"));
        assert_eq!(segment("pt-BR").as_deref(), Some("pt-br"));
        assert_eq!(segment("de-CH").as_deref(), Some("de-ch"));
    }

    #[test]
    fn a_path_segment_that_is_no_language_and_region_is_none() {
        for s in [
            "my-account", // `my` is Burmese, but `account` is no region
            "blog",       // four letters: no language
            "en-xx-yy",   // three subtags
            "xx-zz",      // `xx` is no language
            "images",
            "2024",
            "123",
            "2024-05",
            "en-xx", // a private-use region
            "car",   // a three-letter word, not a two-letter locale
            "us",    // a country, not a language
            "es-",   // no region after the dash
            "-es",   // no language before it
            "",
        ] {
            assert_eq!(segment(s), None, "{s:?}");
        }
    }

    #[test]
    fn a_declared_language_is_any_registered_tag_but_not_x_default() {
        assert_eq!(declared("es-ES").as_deref(), Some("es-es"));
        assert_eq!(declared("zh-Hant-TW").as_deref(), Some("zh-hant-tw"));
        assert_eq!(declared("fil").as_deref(), Some("fil"));
        for s in ["x-default", "xx", "", "i-klingon", "123"] {
            assert_eq!(declared(s), None, "{s:?}");
        }
    }

    #[test]
    fn a_path_names_its_language_in_its_first_two_segments() {
        let tag = |s: &str| path_tag(&Url::parse(s).unwrap());
        assert_eq!(tag("https://x.com/es-es/pricing").as_deref(), Some("es-es"));
        assert_eq!(tag("https://x.com/us/es/pricing").as_deref(), Some("es"), "the second segment");
        assert_eq!(tag("https://x.com/a/b/es/pricing"), None, "the third is not read");
        assert_eq!(tag("https://x.com/my-account"), None);
        assert_eq!(tag("https://x.com/"), None);
    }

    #[test]
    fn a_page_is_the_start_pages_tag_or_its_language_or_another() {
        assert_eq!(relation("es-es", Some("es-es")), Relation::Exact);
        assert_eq!(relation("es", Some("es")), Relation::Exact);
        assert_eq!(relation("es-es", Some("es")), Relation::Same);
        assert_eq!(relation("es", Some("es-mx")), Relation::Same);
        assert_eq!(relation("es", None), Relation::Undeclared);
        assert_eq!(relation("es", Some("de")), Relation::Other);
        assert!(Relation::Exact < Relation::Same);
        assert!(Relation::Same < Relation::Undeclared);
        assert!(Relation::Undeclared < Relation::Other);
    }
}

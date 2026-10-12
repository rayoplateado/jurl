//! The language a page or a sitemap declares, as a BCP 47 tag, and how it relates to the start page's language. Tags are
//! checked against the IANA Language Subtag Registry (the `language-tags` crate): it lists the ISO 639 languages, the
//! ISO 3166 regions and the UN M.49 regions (`es-419`). A region is a country a run can be read as, and the region a bare
//! language implies is CLDR's likely subtags (`icu_locale`). Pure: the reading of the pages is `follow.rs`'s.

use icu_locale::{LanguageIdentifier, LocaleExpander};
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

/// Where a page's region stands against the run's country, best first: the country's own, a page that declares no region,
/// and another region (see [`market`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Market {
    /// The run's country: `es-es` for `ES`.
    Country,
    /// No region declared (`es`), or no country named, which makes every page this.
    Undeclared,
    /// Another region than the run's country: `es-ar`, `es-419`.
    Elsewhere,
}

/// Where a page sits in the order `--follow` reads a site's pages in, best first: a page in the start page's language, then a
/// page that declares no language, then a page in another language. Within the start page's language, the run's country comes
/// first ([`Market`]), then the start page's own tag ([`Relation::Exact`]) before another tag of the language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    /// In the start page's language: the market of its region, and whether its tag is the start page's own (`Exact`) or another
    /// of the language (`Same`).
    Language(Market, Relation),
    /// No language declared.
    Undeclared,
    /// Another language.
    Other,
}

/// The place of a page whose language relates to the start page's as `relation` says, and whose region is in `market`.
pub fn place(relation: Relation, market: Market) -> Place {
    match relation {
        Relation::Exact | Relation::Same => Place::Language(market, relation),
        Relation::Undeclared => Place::Undeclared,
        Relation::Other => Place::Other,
    }
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

/// The language a run asks for, from an Accept-Language value (what `JURL_ACCEPT_LANGUAGE` sets): the primary subtag of its first
/// tag, when the registry lists that tag (`en` for `en-GB,en;q=0.9`). None for an empty value, `*`, or a tag the registry does
/// not list.
pub fn run_language(accept_language: &str) -> Option<String> {
    let first = accept_language.split(',').next()?.split(';').next()?.trim();
    let tag = declared(first)?;
    tag.split('-').next().map(str::to_string)
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
    if region.is_some_and(|region| !region_shaped(region)) {
        return None;
    }
    registered(&canon)?;
    Some(canon.to_ascii_lowercase())
}

/// The region a path segment names, as the registry lists it: an ISO 3166-1 code (`at`) or a UN M.49 one (`419`), lowercase.
/// The codes the standard keeps for private use (`XX`, `QM`) are none.
fn region_code(s: &str) -> Option<String> {
    let canon = canonical(&format!("und-{s}"));
    let region = canon.strip_prefix("und-")?;
    if !region_shaped(region) {
        return None;
    }
    registered(&canon)?;
    Some(region.to_ascii_lowercase())
}

/// Whether `region` (as [`canonical`] writes it) is shaped as a region code: an ISO 3166-1 alpha-2 code (`AT`), but not one of
/// the private-use codes that begin with `X` or `Q`, or a UN M.49 code (`419`).
fn region_shaped(region: &str) -> bool {
    let iso = region.len() == 2
        && region.bytes().all(|b| b.is_ascii_uppercase())
        && !matches!(region.as_bytes()[0], b'X' | b'Q');
    let m49 = region.len() == 3 && region.bytes().all(|b| b.is_ascii_digit());
    iso || m49
}

/// Whether `code` is an ISO 3166-1 alpha-2 country code the registry lists (`ES`, `at`): not a UN M.49 region (`419`), and not a
/// code the registry does not list (`XX`, `QQ`).
pub fn is_country_code(code: &str) -> bool {
    region_code(code).is_some_and(|region| region.len() == 2)
}

/// A path segment that names a language and no region (`de`, not `de-at`).
fn bare_language(s: &str) -> Option<String> {
    segment(s).filter(|tag| !tag.contains('-'))
}

/// The language and the region a URL's path names (see [`path_locale`]), both normalized and both optional.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Locale {
    /// The language: `es` for `/es-ar/` and `/us/es/`.
    pub language: Option<String>,
    /// The region, lowercase: `ar` for `/es-ar/`, `at` for `/at/de/` and `/at/`, `419` for `/es-419/`.
    pub region: Option<String>,
}

/// The locale a normalized tag of the form [`segment`] reads names: `es`, or `es-ar`.
fn locale_of(tag: &str) -> Locale {
    let mut parts = tag.split('-');
    Locale { language: parts.next().map(str::to_string), region: parts.next().map(str::to_string) }
}

/// The language and the region a URL's path names, read from its first two segments:
///
/// - A segment that names a language and a region together is the locale on its own (`/es-ar/pricing`).
/// - Two segments name a region and a language in either order: `/at/de/` (region, then language) and `/de/at/` (language,
///   then region). A pair that reads both ways names neither: `/ar/es/` is Arabic in Spain or Spanish in Argentina, the
///   registry lists both segments as either, and only the sitemap's hreflang can tell them apart (see [`page_tag`]). A pair
///   that reads both ways alike (`/de/de/`) names that locale.
/// - Otherwise the first segment that names a language is the language (`/de/pricing`), and a first segment that names only
///   a region is the region (`/at/flights`).
/// - A lone segment is a language when the registry lists one (`/de/`), else a region (`/us/`).
pub fn path_locale(url: &Url) -> Locale {
    let segments: Vec<&str> = url.path_segments().into_iter().flatten().filter(|s| !s.is_empty()).take(2).collect();
    match segments.as_slice() {
        [] => Locale::default(),
        [one] => match segment(one) {
            Some(tag) => locale_of(&tag),
            None => Locale { language: None, region: region_code(one) },
        },
        [first, second, ..] => pair(first, second),
    }
}

/// [`path_locale`] for a URL whose first two segments are `first` and `second`.
fn pair(first: &str, second: &str) -> Locale {
    if let Some(tag) = segment(first).filter(|tag| tag.contains('-')) {
        return locale_of(&tag);
    }
    let lang_region = bare_language(first)
        .zip(region_code(second))
        .map(|(language, region)| Locale { language: Some(language), region: Some(region) });
    let region_lang = region_code(first)
        .zip(bare_language(second))
        .map(|(region, language)| Locale { language: Some(language), region: Some(region) });
    match (lang_region, region_lang) {
        (Some(both), Some(other)) if both == other => both,
        (Some(_), Some(_)) => Locale::default(),
        (Some(locale), None) | (None, Some(locale)) => locale,
        (None, None) => match segment(first).or_else(|| segment(second)) {
            Some(tag) => locale_of(&tag),
            None => Locale { language: None, region: region_code(first) },
        },
    }
}

/// The language a URL's path names, normalized, with the region it names as well (`es-ar`, `de-at`): see [`path_locale`].
/// None when it names no language (`/my-account`, `/at/`).
pub fn path_tag(url: &Url) -> Option<String> {
    let Locale { language, region } = path_locale(url);
    let language = language?;
    Some(match region {
        Some(region) => format!("{language}-{region}"),
        None => language,
    })
}

/// The tag a page declares, normalized: its sitemap's hreflang when the sitemap gives one (authoritative: `es-AR`), else the
/// one its path names (see [`path_tag`]).
pub fn page_tag(url: &Url, declared: Option<&str>) -> Option<String> {
    declared.map(str::to_string).or_else(|| path_tag(url))
}

/// The region a normalized tag declares, lowercase: `ar` for `es-ar`, `419` for `es-419`. None for a tag that declares none
/// (`es`).
pub fn region(tag: &str) -> Option<String> {
    LanguageTag::parse(tag).ok()?.region().map(str::to_ascii_lowercase)
}

/// Where the region of `tag` (normalized, None when the page declares none) stands against the run's `country` (two uppercase
/// letters). Without a country no region counts: every page is [`Market::Undeclared`].
pub fn market(country: Option<&str>, tag: Option<&str>) -> Market {
    match (country, tag.and_then(region)) {
        (Some(country), Some(region)) if region.eq_ignore_ascii_case(country) => Market::Country,
        (Some(_), Some(_)) => Market::Elsewhere,
        _ => Market::Undeclared,
    }
}

/// The primary language subtag of a normalized tag: `es` for `es-es`.
pub fn primary(tag: &str) -> &str {
    tag.split('-').next().unwrap_or_default()
}

/// The region CLDR's likely subtags give a bare language, lowercase: `de` gives `de` (Germany), `en` gives `us`. None when the
/// language is not in the compiled-in data, or is no language. The data is that of CLDR's commonly used locales (Basic
/// coverage or better): a language outside them gets no likely region.
pub fn likely_region(language: &str) -> Option<String> {
    let mut id: LanguageIdentifier = language.parse().ok()?;
    // Built per call: the expander is not `Sync`, so it cannot be a static, and its constructor only points at the data.
    LocaleExpander::new_common().maximize(&mut id);
    id.region.map(|region| region.as_str().to_ascii_lowercase())
}

/// How a page's language relates to the start page's (`page`, normalized, None when it declares none) and the start page's
/// (`start`, normalized).
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
    fn the_language_a_run_asks_for_is_the_primary_subtag_of_its_first_tag() {
        assert_eq!(run_language("en").as_deref(), Some("en"));
        assert_eq!(run_language("en-GB,en;q=0.9").as_deref(), Some("en"));
        assert_eq!(run_language(" es-ES ; q=1").as_deref(), Some("es"));
        assert_eq!(run_language("zh_CN").as_deref(), Some("zh"));
        assert_eq!(run_language("*"), None);
        assert_eq!(run_language(""), None);
        assert_eq!(run_language("english-GB"), None);
    }

    fn url(s: &str) -> Url {
        Url::parse(s).expect("a test URL")
    }

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
        let tag = |s: &str| path_tag(&url(s));
        assert_eq!(tag("https://x.com/es-es/pricing").as_deref(), Some("es-es"));
        assert_eq!(tag("https://x.com/us/es/pricing").as_deref(), Some("es-us"), "the region, then the language");
        assert_eq!(tag("https://x.com/a/b/es/pricing"), None, "the third is not read");
        assert_eq!(tag("https://x.com/my-account"), None);
        assert_eq!(tag("https://x.com/"), None);
    }

    #[test]
    fn a_pair_of_segments_names_a_region_and_a_language_in_either_order() {
        let tag = |s: &str| path_tag(&url(s));
        // The Ryanair and Air Europa forms: `/at/de` is Austria in German, `/es-ar/` is Spanish in Argentina.
        assert_eq!(tag("https://x.com/at/de").as_deref(), Some("de-at"));
        assert_eq!(tag("https://x.com/at/de/flights").as_deref(), Some("de-at"));
        assert_eq!(tag("https://x.com/de/at/").as_deref(), Some("de-at"));
        assert_eq!(tag("https://x.com/es-ar/aea").as_deref(), Some("es-ar"));
        assert_eq!(tag("https://x.com/en/de/").as_deref(), Some("en-de"), "`en` is no region");
        assert_eq!(tag("https://x.com/de/de/").as_deref(), Some("de-de"), "both readings give the same locale");
        // `ar` and `es` are both a language and a region: the pair reads both ways, so it names no locale.
        assert_eq!(tag("https://x.com/ar/es/aea"), None);
        assert_eq!(tag("https://x.com/es/ar/"), None);
        // One segment that is a language, and another that is none: the language, as it always was.
        assert_eq!(tag("https://x.com/de/pricing").as_deref(), Some("de"));
        assert_eq!(tag("https://x.com/blog/es/post").as_deref(), Some("es"));
    }

    #[test]
    fn a_path_region_alone_is_a_region_and_no_language() {
        let locale = |s: &str| path_locale(&url(s));
        assert_eq!(locale("https://x.com/at/").region.as_deref(), Some("at"));
        assert_eq!(locale("https://x.com/at/").language, None);
        assert_eq!(locale("https://x.com/at/flights").region.as_deref(), Some("at"));
        assert_eq!(locale("https://x.com/us/").region.as_deref(), Some("us"));
        // `de` alone is a language (German), though DE is also a region: a lone language segment is read as one.
        assert_eq!(locale("https://x.com/de/").region, None);
        assert_eq!(locale("https://x.com/de/").language.as_deref(), Some("de"));
        assert_eq!(locale("https://x.com/xx/").region, None, "not a registered region");
    }

    #[test]
    fn a_region_is_one_the_registry_lists_and_no_private_use_code() {
        assert_eq!(region_code("at").as_deref(), Some("at"));
        assert_eq!(region_code("419").as_deref(), Some("419"), "a UN M.49 region");
        for s in ["xx", "qm", "xa", "2024", "123", "en-gb", "at-x"] {
            assert_eq!(region_code(s), None, "{s:?}");
        }
        assert!(is_country_code("ES") && is_country_code("at"));
        assert!(!is_country_code("419"), "a UN M.49 region is no country code");
        assert!(!is_country_code("XX") && !is_country_code("QQ"), "private-use codes are none");
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

    #[test]
    fn a_page_region_is_the_countrys_or_none_or_another() {
        assert_eq!(market(Some("ES"), Some("es-es")), Market::Country);
        assert_eq!(market(Some("ES"), Some("es-ES")), Market::Country, "the region's case does not matter");
        assert_eq!(market(Some("ES"), Some("es-ar")), Market::Elsewhere);
        assert_eq!(market(Some("ES"), Some("es-419")), Market::Elsewhere, "a UN M.49 region is another one");
        assert_eq!(market(Some("ES"), Some("es")), Market::Undeclared);
        assert_eq!(market(Some("ES"), None), Market::Undeclared);
        assert_eq!(market(None, Some("es-ar")), Market::Undeclared, "without a country no region counts");
    }

    #[test]
    fn a_sitemap_hreflang_is_the_pages_tag_before_its_path() {
        // The sitemap says es-AR for a page under `/es-es/`: the hreflang is authoritative.
        assert_eq!(page_tag(&url("https://x.com/es-es/a"), Some("es-ar")).as_deref(), Some("es-ar"));
        assert_eq!(page_tag(&url("https://x.com/at/de/a"), None).as_deref(), Some("de-at"));
        assert_eq!(region("zh-hant-tw").as_deref(), Some("tw"));
        assert_eq!(region("es"), None);
    }

    #[test]
    fn a_place_puts_the_country_first_within_the_start_pages_language() {
        let es_country = place(Relation::Same, Market::Country);
        let exact_undeclared = place(Relation::Exact, Market::Undeclared);
        let exact_elsewhere = place(Relation::Exact, Market::Elsewhere);
        let same_elsewhere = place(Relation::Same, Market::Elsewhere);
        assert!(es_country < exact_undeclared, "the country's region before the start page's own tag");
        assert!(exact_undeclared < exact_elsewhere);
        assert!(exact_elsewhere < same_elsewhere, "the start page's own tag, within a market");
        assert!(same_elsewhere < place(Relation::Undeclared, Market::Undeclared));
        assert!(place(Relation::Undeclared, Market::Undeclared) < place(Relation::Other, Market::Country));
        assert_eq!(place(Relation::Other, Market::Country), Place::Other);
    }

    #[test]
    fn cldr_gives_the_likely_region_of_a_bare_language() {
        assert_eq!(likely_region("de").as_deref(), Some("de"));
        assert_eq!(likely_region("en").as_deref(), Some("us"));
        assert_eq!(likely_region("es").as_deref(), Some("es"));
        assert_eq!(likely_region("fr").as_deref(), Some("fr"));
        assert_eq!(likely_region("pt").as_deref(), Some("br"));
        assert_eq!(likely_region("und"), None, "no language, no likely region");
    }
}

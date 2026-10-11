//! The sitemaps a site names itself: the `Sitemap:` lines of its robots.txt (sitemaps.org, "Submit your sitemap using
//! robots.txt"), the order a sitemap index lists its sitemaps in, by `<lastmod>` (sitemaps.org, "Sitemap index files"),
//! and the language a page's own entry declares for it (Google's hreflang extension in sitemaps). Pure text: the reading
//! of the files is `follow.rs`'s.

/// How many of a sitemap index's sitemaps are read: the first [`index_children`] ranks them.
pub const CHILD_SITEMAPS: usize = 3;
/// How many of robots.txt's `Sitemap:` lines are read, besides /sitemap.xml: in file order.
pub const ROBOTS_SITEMAPS: usize = 3;

/// The values of the `Sitemap:` lines of a robots.txt, in file order. The line is not part of any group: it applies to the
/// whole file, whichever `User-agent` it follows (sitemaps.org). A comment after `#` is not read.
pub fn robots_sitemaps(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default();
            let (field, value) = line.split_once(':')?;
            let value = value.trim();
            (field.trim().eq_ignore_ascii_case("sitemap") && !value.is_empty()).then(|| value.to_string())
        })
        .collect()
}

/// The sitemaps a sitemap index lists that are worth reading: the [`CHILD_SITEMAPS`] that rank first by `rank` (a child's
/// rank, lower first: the caller says what it ranks by, say its language), and of those, the newest by their `<lastmod>`.
/// One with no `<lastmod>`, or one that isn't a date, comes after the dated ones, in the order the index lists them.
pub fn index_children<K: Ord>(xml: &str, rank: impl Fn(&str) -> K) -> Vec<String> {
    let mut entries: Vec<(K, Option<i64>, String)> = index_entries(xml)
        .into_iter()
        .map(|(loc, lastmod)| (rank(&loc), lastmod.as_deref().and_then(lastmod_key), loc))
        .collect();
    // A stable sort: the rank first, then the newest first (`Reverse` puts the larger time first and `None`, no date, last).
    entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| std::cmp::Reverse(a.1).cmp(&std::cmp::Reverse(b.1))));
    entries.into_iter().take(CHILD_SITEMAPS).map(|(_, _, loc)| loc).collect()
}

/// Each `<url>` entry of a sitemap: its `<loc>`, and the language the entry declares for it. That is the `hreflang` of
/// its own alternate: the `xhtml:link` whose `href` is the `<loc>` (the alternates an entry lists include the page
/// itself). None when no alternate names the `<loc>`.
pub fn url_entries(xml: &str) -> Vec<(String, Option<String>)> {
    xml.split("<url>")
        .skip(1)
        .filter_map(|entry| {
            let entry = entry.split("</url>").next()?;
            let loc = text_of(entry, "loc")?;
            let lang = entry.split("<xhtml:link").skip(1).find_map(|link| {
                let attrs = link.split('>').next()?.trim_end_matches('/');
                let is_own = attr(attrs, "rel")? == "alternate" && attr(attrs, "href")? == loc;
                // `x-default` is the fallback page, and it can be the page itself, listed before its own language.
                if is_own { attr(attrs, "hreflang").filter(|h| !h.eq_ignore_ascii_case("x-default")) } else { None }
            });
            Some((loc, lang))
        })
        .collect()
}

/// The value of attribute `name` in the attributes of an XML tag (`name="value"` or `name='value'`), with `&amp;` read
/// back as `&`.
fn attr(attrs: &str, name: &str) -> Option<String> {
    attrs.split_whitespace().find_map(|token| {
        let (key, value) = token.split_once('=')?;
        (key == name).then(|| value.trim_matches(['"', '\'']).replace("&amp;", "&"))
    })
}

/// Each `<sitemap>` entry of a sitemap index: its `<loc>`, and its `<lastmod>` text when it has one.
fn index_entries(xml: &str) -> Vec<(String, Option<String>)> {
    xml.split("<sitemap>")
        .skip(1)
        .filter_map(|entry| {
            let entry = entry.split("</sitemap>").next()?;
            let loc = text_of(entry, "loc")?;
            Some((loc, text_of(entry, "lastmod")))
        })
        .collect()
}

/// The text of the first `<tag>…</tag>` in `s`, with the XML entity `&amp;` read back as `&`.
fn text_of(s: &str, tag: &str) -> Option<String> {
    let inner = s.split(&format!("<{tag}>")).nth(1)?.split(&format!("</{tag}>")).next()?;
    Some(inner.trim().replace("&amp;", "&"))
}

/// A `<lastmod>` value as a time, in seconds since 1970 (UTC). sitemaps.org's W3C datetime: `YYYY-MM-DD`, optionally
/// followed by `Thh:mm`, `:ss` and a fraction, and a zone (`Z`, or `±hh:mm`). A zone is taken into account, and a missing
/// one is UTC. None for anything else.
pub fn lastmod_key(text: &str) -> Option<i64> {
    let s = text.trim();
    let (date, rest) = s.split_at_checked(10)?;
    let mut parts = date.split('-');
    let (y, m, d): (i64, i64, i64) =
        (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    if parts.next().is_some() || date.len() != 10 || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let day_secs = days_from_civil(y, m, d) * 86_400;
    if rest.is_empty() {
        return Some(day_secs);
    }
    let rest = rest.strip_prefix('T')?;
    let (time, offset_minutes) = match rest.strip_suffix('Z') {
        Some(time) => (time, 0),
        None if rest.len() >= 6 && matches!(rest.as_bytes()[rest.len() - 6], b'+' | b'-') => {
            let (time, zone) = rest.split_at(rest.len() - 6);
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (zh, zm) = zone[1..].split_once(':')?;
            (time, sign * (zh.parse::<i64>().ok()? * 60 + zm.parse::<i64>().ok()?))
        }
        None => (rest, 0),
    };
    let time = time.split('.').next()?;
    let mut hms = time.split(':');
    let (h, mi): (i64, i64) = (hms.next()?.parse().ok()?, hms.next()?.parse().ok()?);
    let se: i64 = match hms.next() {
        Some(x) => x.parse().ok()?,
        None => 0,
    };
    if hms.next().is_some() || !(0..=23).contains(&h) || !(0..=59).contains(&mi) || !(0..=60).contains(&se) {
        return None;
    }
    Some(day_secs + (h * 3600 + mi * 60 + se) - offset_minutes * 60)
}

/// Days from 1970-01-01 to the civil date (proleptic Gregorian; Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_sitemap_lines_are_read_in_any_group_and_case() {
        let body = "User-agent: GoogleBot\nDisallow: /x\nSitemap: https://a.test/one.xml\n\nuser-agent: *\n\
                    Disallow: /private # not a sitemap\nSITEMAP:   https://a.test/two.xml  \n#Sitemap: https://a.test/no.xml\n\
                    Sitemap:\nSitemap-ish: https://a.test/nope.xml\n";
        assert_eq!(robots_sitemaps(body), ["https://a.test/one.xml", "https://a.test/two.xml"]);
    }

    #[test]
    fn a_robots_file_without_sitemap_lines_names_none() {
        assert!(robots_sitemaps("User-agent: *\nAllow: /\n").is_empty());
        assert!(robots_sitemaps("").is_empty());
    }

    #[test]
    fn lastmod_is_read_as_a_w3c_datetime_with_or_without_a_time_and_zone() {
        let day = lastmod_key("2024-03-01").unwrap();
        assert_eq!(lastmod_key("2024-03-01T00:00:00Z"), Some(day));
        // 10:00 at +02:00 is 08:00 UTC: the same instant as 08:00Z, and two hours before 10:00Z.
        assert_eq!(lastmod_key("2024-03-01T10:00:00+02:00"), lastmod_key("2024-03-01T08:00:00Z"));
        assert_eq!(lastmod_key("2024-03-01T10:00:00Z").unwrap() - lastmod_key("2024-03-01T08:00:00Z").unwrap(), 7200);
        assert_eq!(lastmod_key("2024-03-01T12:30:00.123-05:00"), lastmod_key("2024-03-01T17:30:00Z"));
        assert_eq!(lastmod_key("2024-03-01T12:30Z").unwrap() - day, 12 * 3600 + 30 * 60);
        // Leap years and the epoch.
        assert_eq!(lastmod_key("1970-01-01"), Some(0));
        assert_eq!(lastmod_key("2024-03-01").unwrap() - lastmod_key("2024-02-28").unwrap(), 2 * 86_400);
    }

    #[test]
    fn a_lastmod_that_is_not_a_date_has_no_time() {
        for bad in
            ["", "yesterday", "2024-13-01", "2024-02-30x", "2024/03/01", "2024-03-01T25:00Z", "2024-03-01T10:00+0200"]
        {
            assert_eq!(lastmod_key(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn an_index_lists_its_three_newest_sitemaps_first() {
        let xml = "<?xml version=\"1.0\"?><sitemapindex>\
            <sitemap><loc>https://a.test/s/2019.xml</loc><lastmod>2019-12-31</lastmod></sitemap>\
            <sitemap><loc>https://a.test/s/2024.xml</loc><lastmod>2024-06-02T09:00:00Z</lastmod></sitemap>\
            <sitemap><loc>https://a.test/s/undated.xml</loc></sitemap>\
            <sitemap><loc>https://a.test/s/2023.xml</loc><lastmod>2023-01-15</lastmod></sitemap>\
            <sitemap><loc>https://a.test/s/2025.xml?a=1&amp;b=2</loc><lastmod>2025-02-01T00:00:00+01:00</lastmod></sitemap>\
            </sitemapindex>";
        assert_eq!(
            index_children(xml, |_| ()),
            ["https://a.test/s/2025.xml?a=1&b=2", "https://a.test/s/2024.xml", "https://a.test/s/2023.xml"]
        );
    }

    #[test]
    fn an_index_ranks_its_children_before_it_takes_the_newest() {
        // The Spanish children are read (rank 0) before a newer German or English one, and of the Spanish, the newest.
        // The undated Spanish child is read after the dated ones.
        let xml = "<sitemapindex>\
            <sitemap><loc>https://a.test/de-de/sitemap.xml</loc><lastmod>2026-09-16</lastmod></sitemap>\
            <sitemap><loc>https://a.test/es-es/sitemap.xml</loc><lastmod>2019-01-01</lastmod></sitemap>\
            <sitemap><loc>https://a.test/en-gb/sitemap.xml</loc><lastmod>2025-01-01</lastmod></sitemap>\
            <sitemap><loc>https://a.test/es-ar/sitemap.xml</loc><lastmod>2024-01-01</lastmod></sitemap>\
            <sitemap><loc>https://a.test/es-mx/sitemap.xml</loc></sitemap>\
            </sitemapindex>";
        let rank = |loc: &str| if loc.contains("/es-") { 0 } else { 1 };
        assert_eq!(
            index_children(xml, rank),
            [
                "https://a.test/es-ar/sitemap.xml",
                "https://a.test/es-es/sitemap.xml",
                "https://a.test/es-mx/sitemap.xml"
            ]
        );
        // Ranked alike, the children are the three newest, as before.
        assert_eq!(
            index_children(xml, |_| ()),
            [
                "https://a.test/de-de/sitemap.xml",
                "https://a.test/en-gb/sitemap.xml",
                "https://a.test/es-ar/sitemap.xml"
            ]
        );
    }

    #[test]
    fn a_url_entry_declares_the_language_of_its_own_alternate() {
        let xml = "<urlset xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">\
            <url><loc>https://a.test/es/precios</loc>\
            <xhtml:link rel=\"alternate\" hreflang=\"en\" href=\"https://a.test/en/pricing\"/>\
            <xhtml:link rel=\"alternate\" hreflang=\"es\" href=\"https://a.test/es/precios\"/>\
            <xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"https://a.test/en/pricing\"/></url>\
            <url><loc>https://a.test/q?a=1&amp;b=2</loc>\
            <xhtml:link rel=\"alternate\" href=\"https://a.test/q?a=1&amp;b=2\" hreflang='de-DE'/></url>\
            <url><loc>https://a.test/x</loc>\
            <xhtml:link rel=\"alternate\" hreflang=\"fr\" href=\"https://a.test/fr/x\"/></url>\
            <url><loc>https://a.test/home</loc>\
            <xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"https://a.test/home\"/>\
            <xhtml:link rel=\"alternate\" hreflang=\"en-GB\" href=\"https://a.test/home\"/></url>\
            </urlset>";
        assert_eq!(
            url_entries(xml),
            [
                ("https://a.test/es/precios".to_string(), Some("es".to_string())),
                ("https://a.test/q?a=1&b=2".to_string(), Some("de-DE".to_string())),
                ("https://a.test/x".to_string(), None),
                // Its own x-default is listed first, and the page's own language is the one it declares.
                ("https://a.test/home".to_string(), Some("en-GB".to_string())),
            ]
        );
    }

    #[test]
    fn an_index_without_dates_keeps_its_file_order() {
        let xml = "<sitemapindex><sitemap><loc>https://a.test/1.xml</loc></sitemap>\
                   <sitemap><loc>https://a.test/2.xml</loc></sitemap><sitemap><loc>https://a.test/3.xml</loc></sitemap>\
                   <sitemap><loc>https://a.test/4.xml</loc></sitemap></sitemapindex>";
        assert_eq!(
            index_children(xml, |_| ()),
            ["https://a.test/1.xml", "https://a.test/2.xml", "https://a.test/3.xml"]
        );
    }
}

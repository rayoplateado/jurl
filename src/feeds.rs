//! A web feed read as the page its entries make. A feed (RSS 2.0, RSS 1.0, Atom) is a site's own list in structured form:
//! each entry has its title, its address, its date and its summary. Read as HTML, its `<link>` is a void element, so an
//! entry falls apart into loose lines and loses its address. So a body whose root is a feed becomes an HTML page with one
//! list item per entry: the entry's title linked to its address, its date, its summary as text and its first image, each one
//! the feed's own words. The rest of the reading is the same as any page's.

use scraper::{Html, Selector};

/// The HTML page a feed makes, or None when `body` is no feed.
pub fn as_html(body: &str) -> Option<String> {
    let root = root_name(body)?;
    let (entry_tag, head_end) = match root {
        "rss" => ("item", "<item"),
        "feed" => ("entry", "<entry"),
        name if name.ends_with(":RDF") || name == "RDF" => ("item", "<item"),
        _ => return None,
    };
    let head = body.split(head_end).next().unwrap_or_default();
    let title = field(head, "title").map(|t| plain(&t)).unwrap_or_default();
    let about = field(head, "description").or_else(|| field(head, "subtitle")).map(|t| plain(&t)).unwrap_or_default();
    // RSS names its language in `<language>`, Atom on its root's `xml:lang`.
    let lang =
        field(head, "language").or_else(|| tags(head, root).into_iter().find_map(|attrs| attr(attrs, "xml:lang")));
    let lang = lang.map(|l| format!(" lang=\"{}\"", esc(l.trim()))).unwrap_or_default();
    let mut html = format!("<html{lang}><head><title>{}</title></head><body><h1>{}</h1>", esc(&title), esc(&title));
    if !about.is_empty() {
        html.push_str(&format!("<p>{}</p>", esc(&about)));
    }
    html.push_str("<ul>");
    for entry in elements(body, entry_tag) {
        html.push_str(&entry_item(entry));
    }
    html.push_str("</ul></body></html>");
    Some(html)
}

/// One entry as a list item, which a page reads as one block: its linked title, its date, its summary and its image, the
/// parts it has, set apart by " · " as a card sets its fields.
fn entry_item(entry: &str) -> String {
    let title = field(entry, "title").map(|t| plain(&t)).unwrap_or_default();
    let link = entry_link(entry);
    let date =
        ["pubDate", "dc:date", "published", "updated"].iter().find_map(|tag| field(entry, tag)).map(|d| plain(&d));
    // A summary is the entry's own short text; an entry with only its whole content is summed up by that content's first
    // paragraph, as the entry's page opens.
    let content = || ["content:encoded", "content"].iter().find_map(|tag| field(entry, tag));
    let summary = ["description", "summary"].iter().find_map(|tag| field(entry, tag));
    let (summary, image_source) = match summary {
        Some(summary) => (Some(plain(&summary)), Some(summary)),
        None => content().map(|whole| (Some(first_paragraph(&whole)), Some(whole))).unwrap_or((None, None)),
    };
    let image = entry_image(entry, image_source.as_deref());
    let mut out = match &link {
        Some(link) => format!("<li><a href=\"{}\">{}</a>", esc(link), esc(&title)),
        None => format!("<li>{}", esc(&title)),
    };
    if let Some(date) = date.filter(|d| !d.is_empty()) {
        out.push_str(&format!(" · <time datetime=\"{}\">{}</time>", esc(&date), esc(&date)));
    }
    if let Some(summary) = summary.filter(|s| !s.is_empty() && *s != title) {
        out.push_str(&format!(" · {}", esc(&summary)));
    }
    if let Some(image) = image {
        out.push_str(&format!(" <img src=\"{}\" alt=\"{}\">", esc(&image), esc(&title)));
    }
    out.push_str("</li>");
    out
}

/// The name of the document's root element: the first tag after the XML declaration, processing instructions, comments
/// and a doctype. None when the body starts with anything else.
fn root_name(body: &str) -> Option<&str> {
    let mut rest = body.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("<?") {
            rest = after.split_once("?>")?.1.trim_start();
        } else if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.split_once("-->")?.1.trim_start();
        } else if let Some(after) = rest.strip_prefix("<!") {
            rest = after.split_once('>')?.1.trim_start();
        } else {
            break;
        }
    }
    let tag = rest.strip_prefix('<')?;
    let end = tag.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
    Some(&tag[..end])
}

/// The elements named `tag` in `xml`, each its inner text, in document order. Entries do not nest.
fn elements<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        // `<item>` or `<item attr…>`, not `<itemref>`.
        if !after.starts_with(|c: char| c == '>' || c.is_whitespace()) {
            rest = after;
            continue;
        }
        let Some(start) = after.find('>') else { break };
        let inner = &after[start + 1..];
        let Some(end) = inner.find(&close) else { break };
        out.push(&inner[..end]);
        rest = &inner[end + close.len()..];
    }
    out
}

/// The text of the first `<tag>` element of `xml`, its CDATA sections kept as written and its entities read: an empty
/// element gives an empty text. None when there is no such element.
fn field(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut rest = xml;
    loop {
        let at = rest.find(&open)?;
        let after = &rest[at + open.len()..];
        if !after.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace()) {
            rest = after;
            continue;
        }
        let end = after.find('>')?;
        if after[..end].ends_with('/') {
            return Some(String::new());
        }
        let inner = &after[end + 1..];
        let close = inner.find(&format!("</{tag}>"))?;
        return Some(xml_text(&inner[..close]));
    }
}

/// The entry's address: RSS's `<link>` text, else a `<guid>` that is an address, else Atom's `<link href>` that is the
/// entry's own page (no `rel`, or `rel="alternate"`).
fn entry_link(entry: &str) -> Option<String> {
    if let Some(link) = field(entry, "link").map(|l| l.trim().to_string()).filter(|l| !l.is_empty()) {
        return Some(link);
    }
    if let Some(guid) = field(entry, "guid").map(|g| g.trim().to_string()).filter(|g| g.starts_with("http")) {
        return Some(guid);
    }
    tags(entry, "link").into_iter().find_map(|attrs| {
        let rel = attr(attrs, "rel");
        matches!(rel.as_deref(), None | Some("alternate")).then(|| attr(attrs, "href")).flatten()
    })
}

/// The entry's image: a `media:thumbnail` or `media:content`, an image `<enclosure>`, else the first `<img>` of its summary.
fn entry_image(entry: &str, summary: Option<&str>) -> Option<String> {
    let media = ["media:thumbnail", "media:content"].iter().flat_map(|tag| tags(entry, tag)).find_map(|attrs| {
        let medium = attr(attrs, "medium").or_else(|| attr(attrs, "type"));
        medium.is_none_or(|m| m.starts_with("image")).then(|| attr(attrs, "url")).flatten()
    });
    let enclosure = || {
        tags(entry, "enclosure").into_iter().find_map(|attrs| {
            attr(attrs, "type").is_some_and(|t| t.starts_with("image/")).then(|| attr(attrs, "url")).flatten()
        })
    };
    let inline = || {
        let fragment = Html::parse_fragment(summary?);
        let img = Selector::parse("img[src]").expect("static selector");
        fragment.select(&img).find_map(|el| el.value().attr("src")).map(str::to_string)
    };
    media.or_else(enclosure).or_else(inline)
}

/// The attributes of each `<tag …>` start tag in `xml`.
fn tags<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    xml.match_indices(&open)
        .filter_map(|(at, _)| {
            let after = &xml[at + open.len()..];
            after.starts_with(char::is_whitespace).then(|| after.split('>').next()).flatten()
        })
        .map(|attrs| attrs.trim_end_matches('/'))
        .collect()
}

/// The value of attribute `name` in a start tag's attributes, its entities read.
fn attr(attrs: &str, name: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(at) = rest.find(name) {
        let before_ok = at == 0 || rest[..at].ends_with(char::is_whitespace);
        let after = rest[at + name.len()..].trim_start();
        if before_ok && let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start();
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let value = &value[1..];
            return Some(entities(&value[..value.find(quote)?]));
        }
        rest = &rest[at + name.len()..];
    }
    None
}

/// An element's text: CDATA sections as written, the rest with its entities read.
fn xml_text(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(at) = rest.find("<![CDATA[") {
        out.push_str(&entities(&rest[..at]));
        let data = &rest[at + 9..];
        let end = data.find("]]>").unwrap_or(data.len());
        out.push_str(&data[..end]);
        rest = data.get(end + 3..).unwrap_or_default();
    }
    out.push_str(&entities(rest));
    out.trim().to_string()
}

/// XML's five entities and character references read back as text. HTML's named entities in a summary are read with it
/// (see [`plain`]).
fn entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let name_ok = |end: &usize| *end <= 10 && tail[1..*end].chars().all(|c| c.is_ascii_alphanumeric() || c == '#');
        let Some(end) = tail.find(';').filter(name_ok) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(ch) => out.push(ch),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A text that may hold HTML (a summary, an Atom `type="html"` title) as its plain words, on one line.
fn plain(s: &str) -> String {
    if !s.contains('<') && !s.contains('&') {
        return crate::extract::collapse(s);
    }
    let fragment = Html::parse_fragment(s);
    crate::extract::collapse(&fragment.root_element().text().collect::<Vec<_>>().join(" "))
}

/// The plain words of the first paragraph of an HTML text, or of all of it when it has no paragraph.
fn first_paragraph(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    let p = Selector::parse("p").expect("static selector");
    let first = fragment
        .select(&p)
        .map(|el| crate::extract::collapse(&el.text().collect::<Vec<_>>().join(" ")))
        .find(|t| !t.is_empty());
    first.unwrap_or_else(|| plain(html))
}

/// `s` escaped for HTML text and attribute values.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<?xml-stylesheet type="text/css" href="/rss.css"?>
<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/">
  <channel>
    <title>Convenios colectivos</title>
    <link>https://example.org/</link>
    <description>Publicados en los últimos dos meses</description>
    <language>es-es</language>
    <item>
      <title>Convenio colectivo del sector de pastas, papel y cartón.</title>
      <link>https://example.org/doc?id=1&amp;lang=es</link>
      <description>Ministerio de Trabajo - Publicado el 14/10/2026</description>
      <pubDate>Wed, 14 Oct 2026 00:00:00 +0200</pubDate>
    </item>
    <item>
      <title><![CDATA[Tablas salariales <2026>]]></title>
      <guid isPermaLink="true">https://example.org/doc?id=2</guid>
      <description>&lt;p&gt;Acuerdo &amp;amp; tablas&lt;/p&gt;&lt;img src="https://example.org/t.jpg"&gt;</description>
    </item>
  </channel>
</rss>"#;

    const ATOM: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom" xml:lang="es">
  <title type="text">Ofertas</title>
  <subtitle>Las últimas</subtitle>
  <entry>
    <title type="html">Analista &lt;b&gt;de datos&lt;/b&gt;</title>
    <link rel="edit" href="https://example.org/edit/7"/>
    <link rel="alternate" type="text/html" href="https://example.org/jobs/7"/>
    <published>2026-10-20T09:00:00Z</published>
    <summary>Madrid, 40.000 €</summary>
    <media:thumbnail url="https://example.org/7.png"/>
  </entry>
</feed>"#;

    #[test]
    fn a_page_is_no_feed() {
        assert_eq!(as_html("<!doctype html><html><body><p>Hi</p></body></html>"), None);
        assert_eq!(as_html(r#"<?xml version="1.0"?><urlset><url><loc>https://a.b/</loc></url></urlset>"#), None);
        assert_eq!(as_html("{\"items\": []}"), None);
    }

    #[test]
    fn each_rss_item_is_one_list_item_with_its_link_date_and_summary() {
        let html = as_html(RSS).expect("a feed");
        assert!(html.starts_with(r#"<html lang="es-es"><head><title>Convenios colectivos</title>"#));
        assert!(html.contains("<p>Publicados en los últimos dos meses</p>"));
        assert_eq!(html.matches("<li>").count(), 2);
        assert!(html.contains(concat!(
            r#"<li><a href="https://example.org/doc?id=1&amp;lang=es">Convenio colectivo del sector de pastas, papel y cartón.</a>"#,
            r#" · <time datetime="Wed, 14 Oct 2026 00:00:00 +0200">Wed, 14 Oct 2026 00:00:00 +0200</time>"#,
            " · Ministerio de Trabajo - Publicado el 14/10/2026</li>"
        )));
    }

    #[test]
    fn cdata_guid_and_an_html_summary_are_read_as_the_feed_writes_them() {
        let html = as_html(RSS).expect("a feed");
        assert!(html.contains(concat!(
            r#"<li><a href="https://example.org/doc?id=2">Tablas salariales &lt;2026&gt;</a> · Acuerdo &amp; tablas"#,
            r#" <img src="https://example.org/t.jpg" alt="Tablas salariales &lt;2026&gt;"></li>"#
        )));
    }

    #[test]
    fn an_atom_entry_links_its_alternate_page_not_its_edit_link() {
        let html = as_html(ATOM).expect("a feed");
        assert!(html.starts_with(r#"<html lang="es">"#));
        assert!(html.contains("<h1>Ofertas</h1><p>Las últimas</p>"));
        assert!(html.contains(concat!(
            r#"<li><a href="https://example.org/jobs/7">Analista de datos</a> · <time datetime="2026-10-20T09:00:00Z">"#,
            r#"2026-10-20T09:00:00Z</time> · Madrid, 40.000 € <img src="https://example.org/7.png" alt="Analista de datos"></li>"#
        )));
    }

    #[test]
    fn an_rdf_feed_is_read_by_its_items() {
        let rdf = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><channel><title>B</title></channel>
            <item rdf:about="https://a.b/1"><title>Uno</title><link>https://a.b/1</link><dc:date>2026-10-01</dc:date></item></rdf:RDF>"#;
        let html = as_html(rdf).expect("a feed");
        assert!(
            html.contains(
                r#"<li><a href="https://a.b/1">Uno</a> · <time datetime="2026-10-01">2026-10-01</time></li>"#
            )
        );
    }

    #[test]
    fn a_feed_reads_as_one_block_per_entry_with_its_link() {
        let base = url::Url::parse("https://example.org/feed").unwrap();
        let page = crate::extract::html(&as_html(RSS).expect("a feed"), &base);
        assert_eq!(page.lang.as_deref(), Some("es-es"));
        let items: Vec<_> = page.blocks.iter().filter(|b| b.kind == crate::extract::Kind::Item).collect();
        assert_eq!(items.len(), 2);
        assert!(
            items[0].text.starts_with("Convenio colectivo del sector de pastas, papel y cartón. · Wed, 14 Oct 2026")
        );
        assert_eq!(items[0].links, ["https://example.org/doc?id=1&lang=es"]);
    }

    #[test]
    fn an_entry_with_only_its_content_is_summed_up_by_its_first_paragraph() {
        let atom = r#"<feed><title>Blog</title><entry><title>Release</title><link href="https://a.b/r"/>
            <content type="html">&lt;p&gt;&lt;img src="https://a.b/r.png"&gt;&lt;/p&gt;&lt;p&gt;The release is out.&lt;/p&gt;&lt;p&gt;Much more.&lt;/p&gt;</content></entry></feed>"#;
        let html = as_html(atom).expect("a feed");
        assert!(html.contains(r#"<li><a href="https://a.b/r">Release</a> · The release is out. <img src="https://a.b/r.png" alt="Release"></li>"#));
    }

    #[test]
    fn a_stray_ampersand_is_kept() {
        assert_eq!(entities("R&D &amp; más &#233; &unknown;"), "R&D & más é &unknown;");
    }
}

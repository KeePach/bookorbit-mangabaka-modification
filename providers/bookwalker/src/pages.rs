//! Reading bookwalker.com's pages. Everything here keys on what the site publishes on purpose:
//! schema.org JSON-LD, `og:` meta tags, link targets (`/series/…`, `/browse?genres.include…`) and
//! the visible labels of the details list ("PUBLISHER", "AUTHOR"). Never on CSS class names,
//! which are build hashes that change with every deploy.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::label::split_volume;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

static JSON_LD: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"(?s)<script type="application/ld\+json">(.*?)</script>"#));
static META: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"<meta\s+(?:property|name)="([a-z:_]+)"\s+content="([^"]*)"\s*/?>"#));
// The page's own format badge comes before any recommended title's.
static FORMAT: LazyLock<Regex> =
    LazyLock::new(|| regex(r">\s*(MANGA|NOVEL|WEBTOON|AUDIOBOOK)\s*</p>"));
// A row of the details list: its label, then its values.
static DETAIL_ROW: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?s)<li\b[^>]*>\s*<p\b[^>]*>\s*([A-Z][A-Z /&-]*?)\s*</p>\s*<p\b[^>]*>(.*?)</p>\s*</li>")
});
static ANCHOR: LazyLock<Regex> = LazyLock::new(|| regex(r"(?s)<a\b([^>]*)>(.*?)</a>"));
static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| regex(r#"([a-zA-Z][\w:-]*)="([^"]*)""#));
static TAG: LazyLock<Regex> = LazyLock::new(|| regex(r"<[^>]*>"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+"));
static SERIES_HREF: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^/series/([0-9A-Z]{12})/([a-z0-9-]+)$"));
static ITEM_HREF: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^/(volume|chapter)/([0-9A-Z]{12})/([a-z0-9-]+)$"));
static ENTITY: LazyLock<Regex> =
    LazyLock::new(|| regex(r"&(#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[a-zA-Z]{2,8});"));

/// What bookwalker.com files a series under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Manga,
    Novel,
    Webtoon,
    Audiobook,
    Unknown,
}

impl Format {
    fn parse(word: &str) -> Self {
        match word.trim().to_ascii_lowercase().as_str() {
            "manga" => Self::Manga,
            "novel" => Self::Novel,
            "webtoon" => Self::Webtoon,
            "audiobook" => Self::Audiobook,
            _ => Self::Unknown,
        }
    }

    /// BookOrbit's media type for it: manga and webtoons are comics.
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Manga | Self::Webtoon => "comic",
            Self::Audiobook => "audiobook",
            Self::Novel | Self::Unknown => "book",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Volume,
    Chapter,
}

impl ItemKind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Volume => "volume",
            Self::Chapter => "chapter",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeriesRef {
    pub id: String,
    pub slug: String,
    pub name: String,
}

/// A volume or chapter page.
#[derive(Clone, Debug)]
pub struct ItemPage {
    pub kind: ItemKind,
    pub id: String,
    /// "Blue Exorcist, Vol. 5", "Booty Royale: Never Go Down Without a Fight! Vols. 9-10".
    pub name: String,
    pub url: String,
    pub description: String,
    /// Empty when the site hides the cover (mature titles show a placeholder when signed out).
    pub cover: String,
    pub isbn: String,
    pub authors: Vec<String>,
    pub publisher: String,
    /// `2024-11-26`.
    pub published: String,
    pub language: String,
    pub format: Format,
    pub series: Option<SeriesRef>,
}

impl ItemPage {
    /// The volume/chapter the page's own name says: "Vol. 5", "Vol. 9-10", "Ch. 51".
    pub fn label(&self) -> String {
        split_volume(&self.name).1
    }
}

/// A link to one of a series' volumes, as its page lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemLink {
    pub id: String,
    pub slug: String,
    /// "Vol. 5", "Vol. 9-10": the link's own text read as a label. Empty when it names no number.
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct SeriesPage {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub url: String,
    pub description: String,
    pub cover: String,
    pub format: Format,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub authors: Vec<String>,
    pub publisher: String,
    pub volumes: Vec<ItemLink>,
}

/// A volume or chapter page, or `None` when it carries no book record.
pub fn parse_item(kind: ItemKind, id: &str, html: &str) -> Option<ItemPage> {
    let records = json_ld(html);
    let book = records.iter().find(|record| has_type(record, "Book"))?;
    let text = |key: &str| {
        book.get(key)
            .and_then(Value::as_str)
            .map(decode)
            .unwrap_or_default()
    };
    let details = details(html);
    let mut authors = people(&details);
    if authors.is_empty() {
        authors = ld_people(book.get("author"));
    }
    let publisher = details
        .iter()
        .find(|row| row.label == "PUBLISHER")
        .and_then(|row| row.values.first().cloned())
        .or_else(|| {
            book.get("brand")
                .and_then(|brand| brand.get("name"))
                .and_then(Value::as_str)
                .map(decode)
        })
        .unwrap_or_default();
    let series = details
        .iter()
        .find(|row| row.label == "SERIES")
        .and_then(|row| {
            let (href, name) = row.links.first()?;
            let captures = SERIES_HREF.captures(href)?;
            Some(SeriesRef {
                id: captures[1].to_string(),
                slug: captures[2].to_string(),
                name: name.clone(),
            })
        });
    let format = breadcrumb_format(&records)
        .or_else(|| FORMAT.captures(html).map(|c| Format::parse(&c[1])))
        .unwrap_or(Format::Unknown);
    let isbn: String = text("isbn").chars().filter(char::is_ascii_digit).collect();
    let published = text("datePublished");
    Some(ItemPage {
        kind,
        id: id.to_string(),
        name: text("name"),
        url: text("url"),
        description: tidy_description(&text("description")),
        cover: visible_cover(&text("image")),
        isbn: if isbn.len() == 13 {
            isbn
        } else {
            String::new()
        },
        authors,
        publisher,
        published: published.get(..10).map(str::to_string).unwrap_or_default(),
        language: text("inLanguage"),
        format,
        series,
    })
}

/// A series page, or `None` when it does not look like one.
pub fn parse_series(id: &str, slug: &str, html: &str) -> Option<SeriesPage> {
    let metas = metas(html);
    let meta = |key: &str| {
        metas
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| decode(value))
            .unwrap_or_default()
    };
    let records = json_ld(html);
    // The breadcrumb's last step names the series even when the page has no og: tags, as for
    // mature titles.
    let crumb_name = records
        .iter()
        .find(|record| has_type(record, "BreadcrumbList"))
        .and_then(|record| record.get("itemListElement")?.as_array()?.last().cloned())
        .and_then(|step| step.get("name")?.as_str().map(decode));
    let name = Some(meta("og:title"))
        .filter(|name| !name.is_empty())
        .or(crumb_name)?;
    let description = Some(meta("og:description"))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| meta("description"));
    let details = details(html);
    let mut seen = HashSet::new();
    let volumes = ANCHOR
        .captures_iter(html)
        .filter_map(|captures| {
            let attributes = attributes(&captures[1]);
            let href = attributes
                .iter()
                .find(|(name, _)| name == "href")?
                .1
                .as_str();
            let item = ITEM_HREF.captures(href)?;
            if &item[1] != "volume" || !seen.insert(item[2].to_string()) {
                return None;
            }
            let text = attributes
                .iter()
                .find(|(name, _)| name == "aria-label")
                .map(|(_, value)| decode(value))
                .unwrap_or_else(|| plain(&captures[2]));
            let (_, label) = split_volume(&text);
            Some(ItemLink {
                id: item[2].to_string(),
                slug: item[3].to_string(),
                label,
            })
        })
        .collect();
    Some(SeriesPage {
        id: id.to_string(),
        slug: slug.to_string(),
        name,
        url: format!("https://bookwalker.com/series/{id}/{slug}"),
        description: tidy_description(&description),
        cover: visible_cover(&meta("og:image")),
        format: FORMAT
            .captures(html)
            .map_or(Format::Unknown, |c| Format::parse(&c[1])),
        genres: link_texts(html, "/browse?genres.include"),
        tags: link_texts(html, "/browse?filter.tags.include"),
        authors: people(&details),
        publisher: details
            .iter()
            .find(|row| row.label == "PUBLISHER")
            .and_then(|row| row.values.first().cloned())
            .unwrap_or_default(),
        volumes,
    })
}

struct Detail {
    label: String,
    values: Vec<String>,
    links: Vec<(String, String)>,
}

fn details(html: &str) -> Vec<Detail> {
    DETAIL_ROW
        .captures_iter(html)
        .map(|captures| {
            let links: Vec<(String, String)> = ANCHOR
                .captures_iter(&captures[2])
                .filter_map(|anchor| {
                    let href = attributes(&anchor[1])
                        .into_iter()
                        .find(|(name, _)| name == "href")?
                        .1;
                    Some((href, plain(&anchor[2])))
                })
                .collect();
            let values = if links.is_empty() {
                vec![plain(&captures[2])]
            } else {
                links.iter().map(|(_, text)| text.clone()).collect()
            };
            Detail {
                label: captures[1].to_string(),
                values: values.into_iter().filter(|v| !v.is_empty()).collect(),
                links,
            }
        })
        .collect()
}

/// Story credits first, then the art: "AUTHOR", "STORY", "ARTIST", "ILLUSTRATOR". Translators,
/// letterers and editors are left out.
fn people(details: &[Detail]) -> Vec<String> {
    let mut seen = HashSet::new();
    [
        "AUTHOR",
        "STORY",
        "ORIGINAL STORY",
        "ARTIST",
        "ART",
        "ILLUSTRATOR",
    ]
    .iter()
    .flat_map(|role| details.iter().filter(move |row| row.label == *role))
    .flat_map(|row| row.values.iter().cloned())
    .filter(|name| seen.insert(name.to_lowercase()))
    .collect()
}

fn ld_people(value: Option<&Value>) -> Vec<String> {
    let one = |v: &Value| v.get("name").and_then(Value::as_str).map(decode);
    match value {
        Some(Value::Array(list)) => list.iter().filter_map(one).collect(),
        Some(person) => one(person).into_iter().collect(),
        None => Vec::new(),
    }
}

fn link_texts(html: &str, prefix: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    ANCHOR
        .captures_iter(html)
        .filter(|captures| {
            attributes(&captures[1])
                .iter()
                .any(|(name, value)| name == "href" && value.starts_with(prefix))
        })
        .map(|captures| plain(&captures[2]))
        .filter(|text| !text.is_empty() && seen.insert(text.to_lowercase()))
        .collect()
}

fn json_ld(html: &str) -> Vec<Value> {
    JSON_LD
        .captures_iter(html)
        .filter_map(|captures| serde_json::from_str(&captures[1]).ok())
        .collect()
}

fn has_type(record: &Value, wanted: &str) -> bool {
    match record.get("@type") {
        Some(Value::String(kind)) => kind == wanted,
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind.as_str() == Some(wanted)),
        _ => false,
    }
}

/// A volume's breadcrumb goes BookWalker > Manga > the volume.
fn breadcrumb_format(records: &[Value]) -> Option<Format> {
    let crumbs = records
        .iter()
        .find(|record| has_type(record, "BreadcrumbList"))?
        .get("itemListElement")?
        .as_array()?;
    let second = crumbs.get(1)?;
    let format = Format::parse(second.get("name")?.as_str()?);
    (format != Format::Unknown).then_some(format)
}

fn metas(html: &str) -> Vec<(String, String)> {
    META.captures_iter(html)
        .map(|captures| (captures[1].to_string(), captures[2].to_string()))
        .collect()
}

fn attributes(tag: &str) -> Vec<(String, String)> {
    ATTRIBUTE
        .captures_iter(tag)
        .map(|captures| (captures[1].to_string(), decode(&captures[2])))
        .collect()
}

/// The text of a bit of markup: tags dropped, entities decoded, spaces collapsed.
fn plain(markup: &str) -> String {
    SPACES
        .replace_all(&decode(&TAG.replace_all(markup, " ")), " ")
        .trim()
        .to_string()
}

/// BookWalker's descriptions are plain text, sometimes with a no-break space or two.
fn tidy_description(text: &str) -> String {
    text.replace('\u{a0}', " ").trim().to_string()
}

fn visible_cover(url: &str) -> String {
    if url.starts_with("https://") && !url.contains("restricted-cover") {
        url.to_string()
    } else {
        String::new()
    }
}

/// HTML entities, as the site writes them: `&#x27;`, `&amp;`, `&#160;`. Run twice, because some
/// descriptions arrive escaped twice (`&amp;#160;`).
pub fn decode(text: &str) -> String {
    let once = decode_once(text);
    if once.contains('&') && once != text {
        decode_once(&once)
    } else {
        once
    }
}

fn decode_once(text: &str) -> String {
    ENTITY
        .replace_all(text, |captures: &regex::Captures| {
            let entity = &captures[1];
            let decoded = if let Some(hex) = entity.strip_prefix("#x").or(entity.strip_prefix("#X"))
            {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(decimal) = entity.strip_prefix('#') {
                decimal.parse().ok().and_then(char::from_u32)
            } else {
                match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    "hellip" => Some('…'),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "rsquo" => Some('’'),
                    "lsquo" => Some('‘'),
                    "rdquo" => Some('”'),
                    "ldquo" => Some('“'),
                    _ => None,
                }
            };
            decoded.map_or_else(|| captures[0].to_string(), |c| c.to_string())
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIES_BLUE_EXORCIST: &str = include_str!("../testdata/series-blue-exorcist.html");
    const SERIES_BOOTY_ROYALE: &str = include_str!("../testdata/series-booty-royale.html");
    const SERIES_GOBLIN_NOVEL: &str =
        include_str!("../testdata/series-goblin-slayer-year-one-novel.html");
    const VOLUME_BLUE_EXORCIST_5: &str = include_str!("../testdata/volume-blue-exorcist-5.html");
    const VOLUME_BOOTY_9_10: &str = include_str!("../testdata/volume-booty-royale-9-10.html");
    const CHAPTER_GOBLIN_51: &str =
        include_str!("../testdata/chapter-goblin-slayer-year-one-51.html");

    #[test]
    fn reads_a_volume_page() {
        let page = parse_item(ItemKind::Volume, "0B08EGZVR4NG", VOLUME_BLUE_EXORCIST_5).unwrap();
        assert_eq!(page.name, "Blue Exorcist, Vol. 5");
        assert_eq!(page.label(), "Vol. 5");
        assert_eq!(page.isbn, "9781421546957");
        assert_eq!(page.authors, ["Kazue Kato"]);
        assert_eq!(page.publisher, "VIZ Media");
        assert_eq!(page.published, "2024-11-26");
        assert_eq!(page.language, "en");
        assert_eq!(page.format, Format::Manga);
        assert_eq!(
            page.cover,
            "https://img.sos-dan.net/1200/01K/G/B/DQ9YT7JH3B92SQ38Z8X4V.webp"
        );
        assert!(page.description.starts_with("In the late Edo Period"));
        assert!(page.description.contains("friends' knowledge"));
        assert_eq!(
            page.series,
            Some(SeriesRef {
                id: "1M9B28BP9B40".into(),
                slug: "blue-exorcist".into(),
                name: "Blue Exorcist".into(),
            })
        );
    }

    #[test]
    fn a_hidden_cover_is_left_out() {
        let page = parse_item(ItemKind::Volume, "1K4M8FFM0DZG", VOLUME_BOOTY_9_10).unwrap();
        assert_eq!(page.label(), "Vol. 9-10");
        assert_eq!(page.cover, "");
        assert_eq!(page.isbn, "9781685799922");
    }

    #[test]
    fn reads_a_chapter_page_with_several_artists() {
        let page = parse_item(ItemKind::Chapter, "3EW93XA7Z120", CHAPTER_GOBLIN_51).unwrap();
        assert_eq!(page.label(), "Ch. 51");
        assert_eq!(
            page.authors,
            [
                "Kumo Kagyu",
                "Noboru Kannatuki",
                "Kento Sakaeda",
                "Shingo Adachi"
            ]
        );
        assert_eq!(page.publisher, "Yen Press");
        assert_eq!(page.series.unwrap().id, "0MNCZ95SPE80");
        assert!(page
            .description
            .contains("Read the next chapter of Goblin Slayer"));
    }

    #[test]
    fn reads_a_series_page() {
        let page = parse_series("1M9B28BP9B40", "blue-exorcist", SERIES_BLUE_EXORCIST).unwrap();
        assert_eq!(page.name, "Blue Exorcist");
        assert_eq!(page.format, Format::Manga);
        assert_eq!(
            page.genres,
            ["Drama", "Comedy", "Fantasy", "Action", "Shounen", "Mature"]
        );
        assert_eq!(
            page.tags,
            ["Male Lead", "School", "Supernatural", "Adventure"]
        );
        assert_eq!(page.authors, ["Kazue Kato"]);
        assert_eq!(page.publisher, "VIZ Media");
        assert!(page.description.contains("Rin's veins"));
        assert_eq!(
            page.cover,
            "https://img.sos-dan.net/720/01K/G/B/D18E4G66K3PBBD3VF0VQB.webp"
        );
        assert_eq!(page.volumes.len(), 6);
        assert_eq!(
            page.volumes[2],
            ItemLink {
                id: "0B08EGZVR4NG".into(),
                slug: "blue-exorcist-vol-5".into(),
                label: "Vol. 5".into(),
            }
        );
    }

    #[test]
    fn a_mature_series_is_named_by_its_breadcrumb() {
        let page = parse_series(
            "1KJFRS6F0V2G",
            "booty-royale-never-go-down-without-a-fight",
            SERIES_BOOTY_ROYALE,
        )
        .unwrap();
        assert_eq!(page.name, "Booty Royale: Never Go Down Without a Fight!");
        assert_eq!(page.cover, "");
        let labels: Vec<&str> = page.volumes.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels[4], "Vol. 9-10");
        assert_eq!(page.volumes[4].id, "1K4M8FFM0DZG");
    }

    #[test]
    fn a_novel_series_skips_translators_and_reads_its_format() {
        let page = parse_series(
            "3K8N7ZS3PM40",
            "goblin-slayer-side-story-year-one",
            SERIES_GOBLIN_NOVEL,
        )
        .unwrap();
        assert_eq!(page.format, Format::Novel);
        assert_eq!(
            page.authors,
            ["Kumo Kagyu", "Noboru Kannatuki", "Shingo Adachi"]
        );
        assert_eq!(page.publisher, "Yen Press");
    }

    #[test]
    fn a_page_without_a_book_record_is_none() {
        assert!(parse_item(ItemKind::Volume, "X", "<html><body>Not found</body></html>").is_none());
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode("Rin&#x27;s &amp; &lt;b&gt; &#160;x"),
            "Rin's & <b> \u{a0}x"
        );
        assert_eq!(decode("of&amp;#160;Goblin"), "of\u{a0}Goblin");
        assert_eq!(decode("&unknown; & stays"), "&unknown; & stays");
    }
}

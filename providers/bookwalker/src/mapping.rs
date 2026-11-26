//! BookWalker pages as BookOrbit records, titled the way the MangaBaka provider titles them:
//! "Series - Vol. 9-10", "Series - Vol. 4 Ch. 5", with the chapter (else the volume) as the
//! series index.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::label::{id_suffix, index_label, provider_index, ONESHOT_LABEL};
use crate::pages::{ItemKind, ItemPage, SeriesPage};

/// The spec's schema. Fields after the blank line are proposed extensions that a client which
/// does not know them ignores.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookMetadata {
    pub media_type: &'static str,
    pub provider_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub publisher: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub published_year: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cover: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub isbn13: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub series: Vec<SeriesEntry>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language_code: String,

    #[serde(skip_serializing_if = "String::is_empty")]
    pub published_date: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct SeriesEntry {
    pub series: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sequence: String,
}

/// What a provider id points at.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Item(ItemKind, String),
    Series(String),
}

/// A provider id read back: the page, and the volume/chapter label the record was titled with.
#[derive(Debug, PartialEq, Eq)]
pub struct ProviderId {
    pub target: Target,
    pub label: String,
}

const RANGE: &str = r"[0-9]{1,4}(?:\.[0-9]{1,2})?(?:-[0-9]{1,4}(?:\.[0-9]{1,2})?)?";
static PROVIDER_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^([vcs])-([0-9A-Z]{{12}})(?:_(?:(os)|(?:vol({RANGE}))?(?:v({RANGE}))?))?$"
    ))
    .expect("valid id pattern")
});

pub const MAX_ID_LENGTH: usize = 48;

/// `v-0B08EGZVR4NG` is a volume, `c-…` a chapter, `s-…` a series; `_vol9-10`, `_vol4v5`, `_v51`
/// or `_os` after it says which volume/chapter the record was titled with, as MangaBaka's ids do.
pub fn provider_id(target: &Target, label: &str) -> String {
    let (code, id) = match target {
        Target::Item(ItemKind::Volume, id) => ('v', id),
        Target::Item(ItemKind::Chapter, id) => ('c', id),
        Target::Series(id) => ('s', id),
    };
    let suffix = id_suffix(label);
    if suffix.is_empty() {
        format!("{code}-{id}")
    } else {
        format!("{code}-{id}_{suffix}")
    }
}

pub fn parse_provider_id(raw: &str) -> Option<ProviderId> {
    if raw.len() > MAX_ID_LENGTH {
        return None;
    }
    let captures = PROVIDER_ID.captures(raw)?;
    let id = captures[2].to_string();
    let target = match &captures[1] {
        "v" => Target::Item(ItemKind::Volume, id),
        "c" => Target::Item(ItemKind::Chapter, id),
        _ => Target::Series(id),
    };
    let label = if captures.get(3).is_some() {
        ONESHOT_LABEL.to_string()
    } else {
        index_label(
            captures.get(4).map_or("", |m| m.as_str()),
            captures.get(5).map_or("", |m| m.as_str()),
        )
    };
    // An id with "_" and nothing it understands after it is not one of ours.
    if raw.contains('_') && label.is_empty() {
        return None;
    }
    Some(ProviderId { target, label })
}

fn titled(series_name: &str, label: &str, fallback: &str) -> String {
    if label.is_empty() || series_name.is_empty() {
        fallback.to_string()
    } else {
        format!("{series_name} - {label}")
    }
}

/// A volume or chapter. `label` is what the search asked for ("Vol. 4 Ch. 5"); empty takes the
/// page's own ("Vol. 5").
pub fn map_item(item: &ItemPage, series: Option<&SeriesPage>, label: &str) -> BookMetadata {
    let label = if label.is_empty() {
        item.label()
    } else {
        label.to_string()
    };
    let series_name = series
        .map(|s| s.name.clone())
        .or_else(|| item.series.as_ref().map(|s| s.name.clone()))
        .unwrap_or_else(|| crate::label::split_volume(&item.name).0);
    let or_series = |own: &str, from_series: fn(&SeriesPage) -> &str| -> String {
        if own.is_empty() {
            series
                .map(|s| from_series(s).to_string())
                .unwrap_or_default()
        } else {
            own.to_string()
        }
    };
    let format = if item.format == crate::pages::Format::Unknown {
        series.map_or(item.format, |s| s.format)
    } else {
        item.format
    };
    BookMetadata {
        media_type: format.media_type(),
        provider_id: provider_id(&Target::Item(item.kind, item.id.clone()), &label),
        title: titled(&series_name, &label, &item.name),
        authors: if item.authors.is_empty() {
            series.map(|s| s.authors.clone()).unwrap_or_default()
        } else {
            item.authors.clone()
        },
        publisher: or_series(&item.publisher, |s| &s.publisher),
        published_year: item.published.get(..4).unwrap_or_default().to_string(),
        description: or_series(&item.description, |s| &s.description),
        cover: or_series(&item.cover, |s| &s.cover),
        isbn13: item.isbn.clone(),
        genres: series.map(|s| s.genres.clone()).unwrap_or_default(),
        tags: series.map(|s| s.tags.clone()).unwrap_or_default(),
        series: if series_name.is_empty() {
            Vec::new()
        } else {
            vec![SeriesEntry {
                series: series_name.clone(),
                sequence: provider_index(&label),
            }]
        },
        language_code: item.language.clone(),
        published_date: item.published.clone(),
        url: if item.url.is_empty() {
            format!("https://bookwalker.com/{}/{}", item.kind.path(), item.id)
        } else {
            item.url.clone()
        },
    }
}

/// The series itself, for a volume or chapter BookWalker does not sell on its own: the record
/// still says which one it is ("Blue Exorcist - Ch. 5"), like MangaBaka's records do.
pub fn map_series(series: &SeriesPage, label: &str) -> BookMetadata {
    BookMetadata {
        media_type: series.format.media_type(),
        provider_id: provider_id(&Target::Series(series.id.clone()), label),
        title: titled(&series.name, label, &series.name),
        authors: series.authors.clone(),
        publisher: series.publisher.clone(),
        published_year: String::new(),
        description: series.description.clone(),
        cover: series.cover.clone(),
        isbn13: String::new(),
        genres: series.genres.clone(),
        tags: series.tags.clone(),
        series: vec![SeriesEntry {
            series: series.name.clone(),
            sequence: provider_index(label),
        }],
        language_code: "en".to_string(),
        published_date: String::new(),
        url: series.url.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::{parse_item, parse_series};

    fn series() -> SeriesPage {
        parse_series(
            "1M9B28BP9B40",
            "blue-exorcist",
            include_str!("../testdata/series-blue-exorcist.html"),
        )
        .unwrap()
    }

    fn volume() -> ItemPage {
        parse_item(
            ItemKind::Volume,
            "0B08EGZVR4NG",
            include_str!("../testdata/volume-blue-exorcist-5.html"),
        )
        .unwrap()
    }

    #[test]
    fn ids_round_trip() {
        for (target, label, id) in [
            (
                Target::Item(ItemKind::Volume, "0B08EGZVR4NG".into()),
                "Vol. 5",
                "v-0B08EGZVR4NG_vol5",
            ),
            (
                Target::Item(ItemKind::Volume, "1K4M8FFM0DZG".into()),
                "Vol. 9-10",
                "v-1K4M8FFM0DZG_vol9-10",
            ),
            (
                Target::Item(ItemKind::Chapter, "3EW93XA7Z120".into()),
                "Vol. 4 Ch. 51",
                "c-3EW93XA7Z120_vol4v51",
            ),
            (
                Target::Item(ItemKind::Chapter, "3EW93XA7Z120".into()),
                "Ch. 51",
                "c-3EW93XA7Z120_v51",
            ),
            (Target::Series("1M9B28BP9B40".into()), "", "s-1M9B28BP9B40"),
            (
                Target::Series("1M9B28BP9B40".into()),
                "Oneshot",
                "s-1M9B28BP9B40_os",
            ),
        ] {
            assert_eq!(provider_id(&target, label), id);
            assert_eq!(
                parse_provider_id(id),
                Some(ProviderId {
                    target,
                    label: label.to_string()
                }),
                "{id}"
            );
        }
        for bad in [
            "",
            "0B08EGZVR4NG",
            "x-0B08EGZVR4NG",
            "v-0b08egzvr4ng",
            "v-0B08EGZVR4NG_",
            "v-0B08EGZVR4NG_zz",
            "v-../../etc",
        ] {
            assert_eq!(parse_provider_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn maps_a_volume() {
        let book = map_item(&volume(), Some(&series()), "");
        assert_eq!(book.provider_id, "v-0B08EGZVR4NG_vol5");
        assert_eq!(book.title, "Blue Exorcist - Vol. 5");
        assert_eq!(book.media_type, "comic");
        assert_eq!(
            book.series,
            [SeriesEntry {
                series: "Blue Exorcist".into(),
                sequence: "5".into()
            }]
        );
        assert_eq!(book.isbn13, "9781421546957");
        assert_eq!(
            (book.published_year.as_str(), book.published_date.as_str()),
            ("2024", "2024-11-26")
        );
        assert_eq!(book.genres[0], "Drama");
        assert_eq!(book.tags[0], "Male Lead");
        assert_eq!(
            book.url,
            "https://bookwalker.com/volume/0B08EGZVR4NG/blue-exorcist-vol-5"
        );
    }

    #[test]
    fn the_asked_for_label_names_the_record() {
        // A chapter file of a series sold in volumes keeps saying which chapter it is.
        let book = map_item(&volume(), Some(&series()), "Vol. 5 Ch. 38");
        assert_eq!(book.title, "Blue Exorcist - Vol. 5 Ch. 38");
        assert_eq!(book.series[0].sequence, "38");
        assert_eq!(book.provider_id, "v-0B08EGZVR4NG_vol5v38");
    }

    #[test]
    fn maps_a_series() {
        let book = map_series(&series(), "Ch. 5");
        assert_eq!(book.provider_id, "s-1M9B28BP9B40_v5");
        assert_eq!(book.title, "Blue Exorcist - Ch. 5");
        assert_eq!(book.series[0].sequence, "5");
        assert!(book.description.starts_with("Raised by Father Fujimoto"));
        let plain = map_series(&series(), "");
        assert_eq!(
            (plain.title.as_str(), plain.provider_id.as_str()),
            ("Blue Exorcist", "s-1M9B28BP9B40")
        );
    }
}

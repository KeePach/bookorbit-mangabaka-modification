use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::mangabaka::{Publisher, Series, Title};

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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub series: Vec<SeriesEntry>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language_code: String,

    #[serde(skip_serializing_if = "String::is_empty")]
    pub published_date: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub cover_width: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub cover_height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// What BookOrbit compares the searched title against, when one of the series' other names
    /// fits the query better than the English `title` does. Left out otherwise.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub display_title: String,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct SeriesEntry {
    pub series: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sequence: String,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

// Accept common volume and chapter aliases such as "Vol.", "Volume", "V", "VL", "Ch",
// "Chapter", "Chptr", and "C#". Allow common typos like "volum" and "chaptr" too.
// Rust's regex engine does not support look-ahead, so we use a non-lookaround end condition.
static VOLUME: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?i)(?:^|[\s\-:(\[_])(?:vol(?:ume|um)?|vl|v)\.?\s*(?:#\s*)?([0-9]{1,4}(?:\.[0-9]+)?)(?:$|[\s\-:,)\](\[_])")
});
static CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?i)(?:^|[\s\-:(\[_])(?:chap(?:ter|tr)?|chptr|ch|c#)\.?\s*(?:#\s*)?([0-9]{1,4}(?:\.[0-9]+)?)(?:$|[\s\-:,)\](\[_])")
});
static EMPTY_BRACKETS: LazyLock<Regex> = LazyLock::new(|| regex(r"\(\s*\)|\[\s*\]"));
static TRAILING_JUNK: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[\s,.:;_\-\x{2013}\x{2014}(\[]+$"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+"));
// A trailing edition word a file name adds but the series' own names do not: "... the Comic".
static EDITION_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?i)[\s\-:(\[]+(?:the\s+)?(?:light\s+novel|web\s+novel|comics?|manga|novel|webtoon|manhwa|manhua)[\s)\]]*$")
});

// MangaBaka descriptions are Markdown; clients expect plain text.
static MARKDOWN_LINK: LazyLock<Regex> = LazyLock::new(|| regex(r"\[([^\]]+)\]\([^)]*\)"));
static MARKDOWN_STRONG: LazyLock<Regex> = LazyLock::new(|| regex(r"\*\*(.+?)\*\*|__(.+?)__"));
static MARKDOWN_EM: LazyLock<Regex> = LazyLock::new(|| regex(r"\*(.+?)\*"));
static HARD_BREAK: LazyLock<Regex> = LazyLock::new(|| regex(r" {2,}\n"));
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| regex(r"\n{3,}"));

fn strip_title_suffix(rest: &str) -> String {
    let rest = EMPTY_BRACKETS.replace_all(rest, " ");
    let rest = SPACES.replace_all(&rest, " ");
    TRAILING_JUNK
        .replace_all(rest.trim(), "")
        .trim()
        .to_string()
}

fn trim_title_prefix(title: &str) -> String {
    title
        .trim()
        .trim_matches(['-', '–', '—', ':'])
        .trim()
        .to_string()
}

fn cleanup_title_bits(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        return String::new();
    }

    let mut bits = title.split_whitespace().collect::<Vec<_>>();
    while !bits.is_empty()
        && matches!(
            bits.last(),
            Some(value)
                if matches!(
                    value.to_ascii_lowercase().as_str(),
                    "vol" | "volume" | "ch" | "chapter" | "c#" | "v" | "vl"
                )
        )
    {
        bits.pop();
    }
    bits.join(" ")
}

/// "0006" reads as "6" and "12.50" as "12.5": leading zeros and meaningless trailing decimal
/// zeros go, so the same chapter always produces the same number.
fn normalize_number(raw: &str) -> String {
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    }
}

/// Cuts the first match of `pattern` out of `text`, joining what was on either side.
fn remove_match(text: &str, pattern: &Regex) -> String {
    let Some(whole) = pattern.find(text) else {
        return text.to_string();
    };
    let before = text[..whole.start()].trim();
    let after = text[whole.end()..].trim();
    match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => after.to_string(),
        (false, true) => before.to_string(),
        (false, false) => format!("{before} {after}"),
    }
}

/// Takes the volume and chapter off a query, keeping the title clean enough to search against
/// MangaBaka. Numbers are normalized ("Ch.0006" is "Ch. 6"). When the title comes first, the
/// first volume/chapter marker ends it: whatever follows (the chapter's own name, the author,
/// the language, the scanlation group) is not part of the series name and would only make the
/// search come back empty. When the marker leads ("Vol. 3 Berserk"), the rest is the title.
pub fn split_volume(query: &str) -> (String, String) {
    let query = query.trim();
    let volume_match = VOLUME.captures(query);
    let chapter_match = CHAPTER.captures(query);
    let number = |found: &Option<regex::Captures>| {
        found
            .as_ref()
            .and_then(|captures| captures.get(1))
            .map(|value| normalize_number(value.as_str()))
            .unwrap_or_default()
    };
    let volume = number(&volume_match);
    let chapter = number(&chapter_match);

    let first_marker = [&volume_match, &chapter_match]
        .into_iter()
        .flatten()
        .filter_map(|captures| captures.get(0))
        .map(|whole| whole.start())
        .min();
    let cleaned = match first_marker {
        Some(start) if !query[..start].trim().is_empty() => query[..start].trim().to_string(),
        Some(_) => remove_match(&remove_match(query, &VOLUME), &CHAPTER),
        None => query.to_string(),
    };

    let cleaned = trim_title_prefix(&strip_title_suffix(&cleaned));
    let normalized = match (volume.is_empty(), chapter.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("Vol. {volume}"),
        (true, false) => format!("Ch. {chapter}"),
        (false, false) => format!("Vol. {volume} Ch. {chapter}"),
    };

    if cleaned.is_empty() {
        (query.to_string(), normalized)
    } else {
        (cleaned, normalized)
    }
}

fn series_index_numbers(series_index: &str) -> (String, String) {
    let volume = VOLUME
        .captures(series_index)
        .and_then(|captures| captures.get(1))
        .map(|value| normalize_number(value.as_str()))
        .unwrap_or_default();
    let chapter = CHAPTER
        .captures(series_index)
        .and_then(|captures| captures.get(1))
        .map(|value| normalize_number(value.as_str()))
        .unwrap_or_default();
    (volume, chapter)
}

fn provider_index(series_index: &str) -> String {
    let (volume, chapter) = series_index_numbers(series_index);
    if !chapter.is_empty() {
        chapter
    } else {
        volume
    }
}

pub fn map_series(s: &Series, volume: &str) -> BookMetadata {
    let series_title = tidy_title(&english_title(s));
    let raw_cover = s.cover.as_ref().and_then(|cover| cover.raw.as_ref());
    let mut authors = unique(s.authors.as_deref().unwrap_or_default());
    if authors.is_empty() {
        authors = unique(s.artists.as_deref().unwrap_or_default());
    }
    let authors: Vec<String> = authors.iter().map(|name| given_name_first(name)).collect();
    let publishers = s.publishers.as_deref().unwrap_or_default();
    let publisher = pick_publisher(publishers);
    // The English publisher stands for the English edition, so its language goes with it.
    let english = publishers.iter().any(|p| {
        p.kind.as_deref() == Some("English") && p.name.as_deref() == Some(publisher.as_str())
    });
    let volume_number = series_index_numbers(volume).0;
    // The series' start says when volume 1 came out, not a later one.
    let dated = volume.is_empty() || volume_number == "1";
    let published_date = s
        .published
        .as_ref()
        .and_then(|published| published.start_date.clone())
        .filter(|date| dated && date.len() == "2006-01-02".len())
        .unwrap_or_default();
    let provider_index = provider_index(volume);
    BookMetadata {
        media_type: if s.kind.as_deref() == Some("novel") {
            "book"
        } else {
            "comic"
        },
        provider_id: if provider_index.is_empty() {
            s.id.to_string()
        } else {
            format!("{}v{provider_index}", s.id)
        },
        title: if volume.is_empty() {
            series_title.clone()
        } else {
            format!("{series_title} - {volume}")
        },
        // The series index is a bare number: the chapter if there is one, else the volume, else
        // nothing at all (the full "Vol. 2 Ch. 8" label stays in the title only).
        series: vec![SeriesEntry {
            series: series_title,
            sequence: provider_index.clone(),
        }],
        authors,
        publisher,
        published_year: s
            .year
            .filter(|year| dated && *year > 0)
            .map(|year| year.to_string())
            .unwrap_or_default(),
        description: s.description.as_deref().map(plain_text).unwrap_or_default(),
        cover: raw_cover
            .and_then(|raw| raw.url.clone())
            .unwrap_or_default(),
        genres: genre_labels(s.genres.as_deref().unwrap_or_default()),
        tags: unique(s.tags.as_deref().unwrap_or_default()),
        language_code: if english {
            "en".to_string()
        } else {
            s.original_language.clone().unwrap_or_default()
        },
        published_date,
        cover_width: raw_cover.and_then(|raw| raw.width).unwrap_or(0),
        cover_height: raw_cover.and_then(|raw| raw.height).unwrap_or(0),
        rating: s
            .rating
            .filter(|rating| *rating > 0.0)
            .map(|rating| (rating.min(100.0) / 20.0 * 100.0).round() / 100.0),
        url: s.canonical_url.clone().unwrap_or_default(),
        display_title: String::new(),
    }
}

/// The series name that best matches what was searched, written like the title ("Name - Ch. 10"),
/// or nothing when the English title is already the best match. BookOrbit drops a result whose
/// title shares too little with the query, so a romanized or native query would lose a series
/// whose English name it never uses. `title` stays English; this is only what gets compared.
pub fn display_title(s: &Series, query: &str, volume: &str) -> String {
    let wanted: HashSet<String> = title_words(query)
        .into_iter()
        .filter(|word| !SMALL_WORDS.contains(&word.as_str()))
        .collect();
    let overlap = |name: &str| {
        title_words(name)
            .into_iter()
            .filter(|word| wanted.contains(word))
            .collect::<HashSet<String>>()
            .len()
    };
    let english = tidy_title(&english_title(s));
    let mut best = english.clone();
    let mut best_overlap = overlap(&english);
    for name in all_titles(s) {
        let name_overlap = overlap(name);
        if name_overlap > best_overlap {
            best = tidy_title(name);
            best_overlap = name_overlap;
        }
    }
    if best == english {
        String::new()
    } else if volume.is_empty() {
        best
    } else {
        format!("{best} - {volume}")
    }
}

/// Prefer any readable English title, even if MangaBaka did not label it as official. A title may
/// still be the best English display name even when it is only an accepted alias.
fn english_title(s: &Series) -> String {
    let english: Vec<&Title> = s
        .titles
        .iter()
        .flatten()
        .filter(|t| {
            t.language.as_deref() == Some("en")
                && t.title.as_deref().is_some_and(|t| !t.trim().is_empty())
        })
        .collect();

    english
        .iter()
        .find(|t| t.is_primary == Some(true))
        .or_else(|| {
            english
                .iter()
                .find(|t| t.traits.iter().flatten().any(|tr| tr == "official"))
        })
        .or_else(|| english.first())
        .and_then(|t| t.title.clone())
        .or_else(|| s.title.clone().filter(|t| !t.trim().is_empty()))
        .or_else(|| s.romanized_title.clone())
        .unwrap_or_default()
}

const SMALL_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "but", "by", "for", "in", "nor", "of", "on", "or", "the", "to",
    "vs", "x",
];

/// Catalogue capitals ("ONE PIECE", "SPY×FAMILY") read as the title a library shelves: "One
/// Piece", "Spy x Family". A title that already mixes case is left as its publisher wrote it.
pub fn tidy_title(title: &str) -> String {
    let title = SPACES
        .replace_all(&title.replace('×', " x "), " ")
        .trim()
        .to_string();
    let letters: Vec<char> = title
        .split(' ')
        .filter(|word| word.chars().filter(|c| c.is_alphabetic()).count() > 1)
        .flat_map(str::chars)
        .filter(|c| c.is_alphabetic())
        .collect();
    let shouted = letters.len() > 3
        && letters.iter().all(|c| !c.is_lowercase())
        && letters.iter().any(|c| c.is_uppercase());
    if !shouted {
        return title;
    }
    title
        .split(' ')
        .enumerate()
        .map(|(i, word)| {
            let lower = word.to_lowercase();
            if i > 0 && SMALL_WORDS.contains(&lower.as_str()) {
                return lower;
            }
            let mut chars = lower.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// "MIURA Kentaro" is the family-name-first, family-name-in-capitals way MangaBaka writes some
/// creators; a library files the same person as "Kentaro Miura".
pub fn given_name_first(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let is_upper = |w: &str| {
        w.chars().filter(|c| c.is_alphabetic()).count() > 1 && !w.chars().any(|c| c.is_lowercase())
    };
    if words.len() < 2 || !is_upper(words[0]) || words[1..].iter().any(|w| is_upper(w)) {
        return name.trim().to_string();
    }
    let mut family = words[0].to_lowercase();
    if let Some(first) = family.chars().next() {
        family = first.to_uppercase().chain(family.chars().skip(1)).collect();
    }
    format!("{} {family}", words[1..].join(" "))
}

/// Lowercase words of a title, with "×" read as "x", for comparing against what was asked for.
pub fn title_words(title: &str) -> Vec<String> {
    title
        .replace('×', " x ")
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn all_titles(s: &Series) -> Vec<&str> {
    [&s.title, &s.romanized_title, &s.native_title]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .chain(s.titles.iter().flatten().filter_map(|t| t.title.as_deref()))
        .collect()
}

/// One of the series' names is exactly what was asked for.
pub fn names_exactly(s: &Series, query: &str) -> bool {
    let wanted = title_words(query);
    !wanted.is_empty()
        && all_titles(s)
            .iter()
            .any(|title| title_words(title) == wanted)
}

/// One of the series' names shares a whole word with the query. MangaBaka's search is fuzzy, so
/// "Dune" also brings back "June"; a record no name of which has any asked-for word is not it.
pub fn shares_a_word(s: &Series, query: &str) -> bool {
    let wanted = title_words(query);
    let significant: Vec<&String> = wanted.iter().filter(|w| w.chars().count() >= 3).collect();
    let wanted: Vec<&String> = if significant.is_empty() {
        wanted.iter().collect()
    } else {
        significant
    };
    all_titles(s).iter().any(|title| {
        let words = title_words(title);
        wanted.iter().any(|w| words.contains(w))
    })
}

/// The wordings to search MangaBaka with, most specific first: the title as given, then without a
/// trailing edition word ("... the Comic", "(Light Novel)"), then its first half. MangaBaka's
/// search answers nothing when the query has words none of the series' names have, which a
/// romanized or fan-translated file name often does, so a longer query that finds nothing is
/// retried shorter.
pub fn search_queries(title: &str) -> Vec<String> {
    let title = title.trim();
    let bare = strip_title_suffix(&EDITION_SUFFIX.replace(title, ""));
    let words: Vec<&str> = bare.split_whitespace().collect();
    let mut queries = vec![title.to_string(), bare.clone()];
    if words.len() >= 6 {
        queries.push(words[..(words.len() / 2).max(3)].join(" "));
    }
    let mut seen = HashSet::new();
    queries.retain(|query| !query.is_empty() && seen.insert(query.to_lowercase()));
    queries
}

/// How many of the query's distinct words (small words like "the" aside) the best-matching of the
/// series' names has. Orders the results of a shortened search by how much of the title they match.
pub fn shared_words(s: &Series, query: &str) -> usize {
    let wanted: HashSet<String> = title_words(query)
        .into_iter()
        .filter(|word| !SMALL_WORDS.contains(&word.as_str()))
        .collect();
    all_titles(s)
        .iter()
        .map(|title| {
            title_words(title)
                .into_iter()
                .filter(|word| wanted.contains(word))
                .collect::<HashSet<String>>()
                .len()
        })
        .max()
        .unwrap_or(0)
}

/// The English-language publisher is the edition most libraries hold, then the original one.
fn pick_publisher(publishers: &[Publisher]) -> String {
    for wanted in ["English", "Original"] {
        if let Some(name) = publishers
            .iter()
            .find(|p| {
                p.kind.as_deref() == Some(wanted)
                    && p.name.as_deref().is_some_and(|name| !name.is_empty())
            })
            .and_then(|p| p.name.clone())
        {
            return name;
        }
    }
    publishers
        .first()
        .and_then(|p| p.name.clone())
        .unwrap_or_default()
}

/// "school_life" reads as "School Life".
fn genre_labels(genres: &[String]) -> Vec<String> {
    let labels: Vec<String> = genres
        .iter()
        .map(|genre| {
            genre
                .replace('_', " ")
                .split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().chain(chars).collect())
                        .unwrap_or_default()
                })
                .collect::<Vec<String>>()
                .join(" ")
        })
        .collect();
    unique(&labels)
}

pub fn plain_text(markdown: &str) -> String {
    let text = MARKDOWN_LINK.replace_all(markdown, "$1");
    let text = MARKDOWN_STRONG.replace_all(&text, "$1$2");
    let text = MARKDOWN_EM.replace_all(&text, "$1");
    let text = text.replace("\r\n", "\n");
    let text = HARD_BREAK.replace_all(&text, "\n");
    BLANK_LINES.replace_all(&text, "\n\n").trim().to_string()
}

fn unique(list: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    list.iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty() && seen.insert(value.to_lowercase()))
        .map(str::to_string)
        .collect()
}

/// True when every word of the asked-for author is in one of the series' names, in any order:
/// MangaBaka writes "MIURA Kentaro" where a file says "Kentaro Miura". A name that differs only
/// in its separators matches too: "Chu-Gong" is "Chugong".
pub fn author_matches(s: &Series, author: &str) -> bool {
    let wanted = name_words(author);
    if wanted.is_empty() {
        return true;
    }
    let names = s.authors.iter().flatten().chain(s.artists.iter().flatten());
    names.into_iter().any(|name| {
        let words = name_words(name);
        if words.concat() == wanted.concat() {
            return true;
        }
        let have: HashSet<String> = words.into_iter().collect();
        wanted.iter().all(|word| have.contains(word))
    })
}

fn name_words(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(|c: char| !c.is_alphabetic() && !c.is_ascii_digit())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn load_series(json: &str) -> Series {
        #[derive(serde::Deserialize)]
        struct Body {
            data: Series,
        }
        serde_json::from_str::<Body>(json)
            .expect("fixture parses")
            .data
    }

    const SERIES_1692: &str = include_str!("../testdata/series-1692.json");

    #[test]
    fn splits_a_volume_off_the_query() {
        let cases = [
            ("Berserk Vol. 3", "Berserk", "Vol. 3"),
            ("Berserk, Vol. 03", "Berserk", "Vol. 3"),
            ("Berserk v12", "Berserk", "Vol. 12"),
            ("Berserk volume 41", "Berserk", "Vol. 41"),
            ("Berserk vl 7", "Berserk", "Vol. 7"),
            ("One Piece #5", "One Piece", "Vol. 5"),
            ("Berserk (Vol 2)", "Berserk", "Vol. 2"),
            ("Berserk - Vol. 3 Ch. 7", "Berserk", "Vol. 3 Ch. 7"),
            ("Berserk Vol. 3 ch. 7", "Berserk", "Vol. 3 Ch. 7"),
            ("Berserk - Ch. 7", "Berserk", "Ch. 7"),
            ("Berserk - C# 7", "Berserk", "Ch. 7"),
            ("Berserk chptr 7", "Berserk", "Ch. 7"),
            ("Chainsaw Man Vol. 10.5", "Chainsaw Man", "Vol. 10.5"),
            ("Mob Psycho 100", "Mob Psycho 100", ""),
            ("Revolutionary Girl Utena", "Revolutionary Girl Utena", ""),
            ("Vol. 3", "Vol. 3", ""),
            ("Title Vol 2 Ch 11", "Title", "Vol. 2 Ch. 11"),
            ("Title Ch 11", "Title", "Ch. 11"),
            ("Title Vol 2", "Title", "Vol. 2"),
            (
                "The Anthology of Elder Sisterly Mischieves - Ch.0006 - Kasumi-neechan Amaecha Dame!! - Haseyoshi Eria (en) [anonymous]",
                "The Anthology of Elder Sisterly Mischieves",
                "Ch. 6",
            ),
            ("Foo - Ch.0012.50 - Bar", "Foo", "Ch. 12.5"),
            ("Foo - Vol.01 - Bar (en)", "Foo", "Vol. 1"),
            ("Vol. 3 Berserk", "Berserk", "Vol. 3"),
        ];
        for (query, title, volume) in cases {
            assert_eq!(
                split_volume(query),
                (title.to_string(), volume.to_string()),
                "{query}"
            );
        }
    }

    #[test]
    fn maps_a_series() {
        let book = map_series(&load_series(SERIES_1692), "Vol. 3");
        assert_eq!(
            (
                book.provider_id.as_str(),
                book.title.as_str(),
                book.media_type
            ),
            ("1692v3", "Berserk - Vol. 3", "comic")
        );
        assert_eq!(book.authors, ["Kentaro Miura", "Kouji Mori"]);
        assert_eq!(book.publisher, "Dark Horse Manga");
        assert_eq!(
            (
                book.published_year.as_str(),
                book.published_date.as_str(),
                book.language_code.as_str()
            ),
            ("", "", "en")
        );
        assert_eq!(book.rating, Some(4.57));
        assert_eq!(
            book.series,
            [SeriesEntry {
                series: "Berserk".into(),
                sequence: "3".into()
            }]
        );
        assert!(!book.cover.is_empty());
        assert_eq!((book.cover_width, book.cover_height), (751, 1051));
        assert_eq!(book.genres[..3], ["Action", "Adventure", "Drama"]);
        assert_eq!(
            book.genres.last().map(String::as_str),
            Some("Award Winning")
        );
        assert_eq!(book.url, "https://mangabaka.org/manga/1692/BERSERK");
    }

    #[test]
    fn prefers_the_chapter_in_provider_ids_when_both_are_present() {
        let book = map_series(&load_series(SERIES_1692), "Vol. 2 Ch. 11");
        assert_eq!(book.provider_id, "1692v11");
        assert_eq!(book.title, "Berserk - Vol. 2 Ch. 11");
    }

    #[test]
    fn display_title_is_the_name_the_query_used() {
        let series = Series {
            title: Some("A Gamer Is Going to Live a Harem Life in Another World".into()),
            romanized_title: Some(
                "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu".into(),
            ),
            ..Default::default()
        };
        assert_eq!(
            display_title(
                &series,
                "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu the Comic",
                "Ch. 10"
            ),
            "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu - Ch. 10"
        );
        // An English query already matches the English title: nothing to add.
        assert_eq!(
            display_title(&series, "A Gamer Is Going to Live a Harem Life in Another World", "Ch. 10"),
            ""
        );
    }

    #[test]
    fn search_retries_with_shorter_wordings() {
        assert_eq!(
            search_queries(
                "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu the Comic"
            ),
            [
                "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu the Comic",
                "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu",
                "Gamer ga Isekai Konten shite Harem",
            ]
        );
        assert_eq!(search_queries("Berserk the Comic"), ["Berserk the Comic", "Berserk"]);
        assert_eq!(search_queries("Berserk"), ["Berserk"]);
        assert_eq!(search_queries("Manga"), ["Manga"]);
        assert_eq!(search_queries("ベルセルク"), ["ベルセルク"]);
    }

    #[test]
    fn series_index_is_chapter_else_volume_else_nothing() {
        let series = load_series(SERIES_1692);
        let sequence = |label: &str| map_series(&series, label).series[0].sequence.clone();
        assert_eq!(sequence("Vol. 2 Ch. 8"), "8");
        assert_eq!(sequence("Ch. 8"), "8");
        assert_eq!(sequence("Vol. 2"), "2");
        assert_eq!(sequence("Vol. 10.5"), "10.5");
        assert_eq!(sequence(""), "");
    }

    #[test]
    fn dates_the_first_volume_and_the_series_itself() {
        let series = load_series(SERIES_1692);
        for volume in ["", "Vol. 1"] {
            let book = map_series(&series, volume);
            assert_eq!(
                (book.published_year.as_str(), book.published_date.as_str()),
                ("1989", "1989-08-25"),
                "{volume:?}"
            );
        }
        assert_eq!(map_series(&series, "").title, "Berserk");
        assert_eq!(map_series(&series, "").provider_id, "1692");
    }

    #[test]
    fn only_a_novel_is_a_book() {
        let typed = |kind: &str| {
            map_series(
                &Series {
                    kind: Some(kind.into()),
                    ..Series::default()
                },
                "",
            )
            .media_type
        };
        assert_eq!(
            [typed("manga"), typed("manhwa"), typed("novel")],
            ["comic", "comic", "book"]
        );
    }

    #[test]
    fn prefers_the_official_english_title() {
        let series = Series {
            title: Some("FULL METAL ALCHEMIST".into()),
            titles: Some(vec![
                Title {
                    language: Some("en".into()),
                    title: Some("Alchemist of Steel".into()),
                    traits: Some(vec![]),
                    is_primary: Some(false),
                },
                Title {
                    language: Some("en".into()),
                    title: Some("FULLMETAL ALCHEMIST".into()),
                    traits: Some(vec!["official".into()]),
                    is_primary: Some(true),
                },
            ]),
            ..Series::default()
        };
        assert_eq!(map_series(&series, "").title, "Fullmetal Alchemist");
    }

    #[test]
    fn prefers_a_readable_english_title_even_without_official_marker() {
        let series = Series {
            title: Some("THE FIRST WITCH".into()),
            titles: Some(vec![
                Title {
                    language: Some("en".into()),
                    title: Some("The First Witch".into()),
                    traits: Some(vec![]),
                    is_primary: Some(false),
                },
                Title {
                    language: Some("ja".into()),
                    title: Some("はじまりの魔女".into()),
                    traits: Some(vec![]),
                    is_primary: Some(false),
                },
            ]),
            ..Series::default()
        };
        assert_eq!(map_series(&series, "").title, "The First Witch");
    }

    #[test]
    fn tidies_catalogue_capitals() {
        for (raw, tidy) in [
            ("ONE PIECE", "One Piece"),
            ("SPY×FAMILY", "Spy x Family"),
            ("ATTACK ON TITAN", "Attack on Titan"),
            ("Frieren: Beyond Journey’s End", "Frieren: Beyond Journey’s End"),
            ("GTO", "GTO"),
        ] {
            assert_eq!(tidy_title(raw), tidy, "{raw}");
        }
    }

    #[test]
    fn puts_the_given_name_first() {
        assert_eq!(given_name_first("MIURA Kentaro"), "Kentaro Miura");
        assert_eq!(given_name_first("Eiichirou Oda"), "Eiichirou Oda");
        assert_eq!(given_name_first("CLAMP"), "CLAMP");
        assert_eq!(given_name_first("Chugong"), "Chugong");
    }

    #[test]
    fn keeps_only_series_that_share_a_word() {
        let named = |title: &str| Series {
            title: Some(title.into()),
            ..Series::default()
        };
        assert!(shares_a_word(&named("SPY×FAMILY"), "Spy x Family"));
        assert!(!shares_a_word(&named("June"), "Dune"));
        assert!(!shares_a_word(
            &named("In the SAND DUNES where the black feathers dance"),
            "Dune"
        ));
        assert!(names_exactly(&named("SPY×FAMILY"), "Spy x Family"));
        assert!(!names_exactly(
            &named("Spy x Family: Family Portrait"),
            "Spy x Family"
        ));
    }

    #[test]
    fn turns_markdown_into_plain_text() {
        let got = plain_text("**THE WEAKEST HUNTER!**\n\n\n\nFrom [Tapas](https://tapas.io):  \nThe *prototype* chapter, __really__.");
        assert_eq!(
            got,
            "THE WEAKEST HUNTER!\n\nFrom Tapas:\nThe prototype chapter, really."
        );
    }

    #[test]
    fn labels_genres() {
        let genres = ["school_life", "award_winning", "action", "Action"].map(String::from);
        assert_eq!(
            genre_labels(&genres),
            ["School Life", "Award Winning", "Action"]
        );
    }

    #[test]
    fn matches_authors_in_any_order() {
        let s = load_series(SERIES_1692);
        assert!(author_matches(&s, "Kentaro Miura"));
        assert!(author_matches(&s, "miura"));
        assert!(!author_matches(&s, "Eiichiro Oda"));
        let chugong = Series {
            authors: Some(vec!["Chu-Gong".into()]),
            ..Series::default()
        };
        assert!(author_matches(&chugong, "Chugong"));
    }
}

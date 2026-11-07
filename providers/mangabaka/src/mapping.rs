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
    // "Ch. 8", "chapter8", "c#8", "c008", and a range of chapters with no spaces around the dash:
    // "Ch. 0-6", "c000-006". Group 1 is the chapter, group 2 the end of a range.
    regex(r"(?i)(?:^|[\s\-:(\[_])(?:(?:chap(?:ters?|tr)?|chptr|ch|c#)\.?\s*(?:#\s*)?|c)([0-9]{1,4}(?:\.[0-9]+)?)(?:[-\x{2013}\x{2014}~]([0-9]{1,4}(?:\.[0-9]+)?))?(?:$|[\s\-:,)\](\[_])")
});
static EMPTY_BRACKETS: LazyLock<Regex> = LazyLock::new(|| regex(r"\(\s*\)|\[\s*\]"));
static TRAILING_JUNK: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[\s,.:;_\-\x{2013}\x{2014}(\[]+$"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+"));
// "Oneshot", "One-shot", "one shot": a book with no volume or chapter number to put after its name.
static ONESHOT: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?i)(?:^|[\s\-:(\[_])one[\s_\-]?shots?(?:$|[\s\-:,)\](\[_])")
});
// A series number on its own ("12", "0-6", "10.5"): what BookOrbit has for a book whose file is
// just "chapter 12", as the folder gives the series.
static INDEX_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"^\s*([0-9]{1,4}(?:\.[0-9]+)?)(?:[-\x{2013}\x{2014}~]([0-9]{1,4}(?:\.[0-9]+)?))?\s*$")
});
// A title that is only the word: "Oneshot", "(One-Shot)".
static ONESHOT_ONLY: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^\W*one[\s_\-]?shots?\W*$"));
/// What a oneshot's title carries after the series name, in place of a volume or chapter.
pub const ONESHOT_LABEL: &str = "Oneshot";
// A number left alone after a dash ("Title - 39"): what a title looks like once something has
// stripped the "Ch." from "Title - Ch. 39".
static BARE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\s[-\x{2013}\x{2014}]\s*([0-9]{1,4}(?:\.[0-9]+)?)\s*$"));
// A parenthesized, bracketed or braced part of a file name: "(2010)", "(en)", "[group]".
static BRACKETED: LazyLock<Regex> = LazyLock::new(|| regex(r"\s*[(\[{][^)\]}]*[)\]}]"));
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
    let chapter = chapter_match.as_ref().map(chapter_text).unwrap_or_default();

    // A title an earlier scrape left with only its number ("Title - 39") has lost its marker.
    // Read the number as the chapter, so scraping the same book again keeps it instead of
    // dropping it. A year is not a chapter.
    if volume.is_empty() && chapter.is_empty() {
        // A oneshot has no number, so the word itself is what the title keeps. Like a marker, it
        // ends the series name; a title that starts with it ("One Shot Kill") is just a title.
        if let Some(found) = ONESHOT.find(query) {
            let before = query[..found.start()].trim();
            let cleaned = trim_title_prefix(&strip_title_suffix(before));
            if !cleaned.is_empty() {
                return (cleaned, ONESHOT_LABEL.to_string());
            }
        }
        let bare = BARE_NUMBER.captures(query).and_then(|captures| {
            let start = captures.get(0)?.start();
            let number = normalize_number(captures.get(1)?.as_str());
            let is_year = number.parse::<u32>().is_ok_and(|n| (1900..=2100).contains(&n));
            (!is_year && !query[..start].trim().is_empty()).then(|| (start, number))
        });
        if let Some((start, number)) = bare {
            let cleaned = trim_title_prefix(&strip_title_suffix(query[..start].trim()));
            let label = index_label("", &number);
            return if cleaned.is_empty() { (query.to_string(), label) } else { (cleaned, label) };
        }
    }

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
    let normalized = index_label(&volume, &chapter);

    if cleaned.is_empty() {
        (query.to_string(), normalized)
    } else {
        (cleaned, normalized)
    }
}

/// The chapter a match names: "8", or "0-6" for a range of chapters. A range that goes nowhere
/// ("6-2", "3-3") is only its first chapter.
fn chapter_text(captures: &regex::Captures) -> String {
    let start = captures
        .get(1)
        .map(|found| normalize_number(found.as_str()))
        .unwrap_or_default();
    let end = captures.get(2).map(|found| normalize_number(found.as_str()));
    match end {
        Some(end) if end.parse::<f64>().unwrap_or(0.0) > start.parse::<f64>().unwrap_or(f64::MAX) => {
            format!("{start}-{end}")
        }
        _ => start,
    }
}

/// Which volume, chapter or oneshot of a series a book is, for when BookOrbit says what the series
/// is (from the folder) and the title only has to say which book of it this is: whatever the
/// title names ("chapter 1", "Vol. 2", "Oneshot"), else the book's own series number read as a
/// chapter. Empty when neither says.
pub fn position_label(title: &str, series_index: Option<&str>) -> String {
    let (_, label) = split_volume(title);
    if !label.is_empty() {
        return label;
    }
    if ONESHOT_ONLY.is_match(title) {
        return ONESHOT_LABEL.to_string();
    }
    series_index
        .and_then(|index| INDEX_ONLY.captures(index))
        .map(|captures| index_label("", &chapter_text(&captures)))
        .unwrap_or_default()
}

/// The label a title carries for a volume and/or a chapter: "Vol. 2 Ch. 8", "Ch. 8", "Vol. 2",
/// or nothing.
pub fn index_label(volume: &str, chapter: &str) -> String {
    match (volume.is_empty(), chapter.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("Vol. {volume}"),
        (true, false) => format!("Ch. {chapter}"),
        (false, false) => format!("Vol. {volume} Ch. {chapter}"),
    }
}

/// What follows the series id in a provider id, so the id says which volume or chapter it is and
/// a lookup by id can write the same title again: `v8` is chapter 8 (the form earlier ids have),
/// `vol2` is volume 2, `vol2v8` is both.
fn id_suffix(label: &str) -> String {
    if label == ONESHOT_LABEL {
        return "os".to_string();
    }
    let (volume, chapter) = series_index_numbers(label);
    match (volume.is_empty(), chapter.is_empty()) {
        (true, true) => String::new(),
        (true, false) => format!("v{chapter}"),
        (false, true) => format!("vol{volume}"),
        (false, false) => format!("vol{volume}v{chapter}"),
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
        .map(|captures| chapter_text(&captures))
        .unwrap_or_default();
    (volume, chapter)
}

fn provider_index(series_index: &str) -> String {
    let (volume, chapter) = series_index_numbers(series_index);
    if !chapter.is_empty() {
        // A range of chapters sits in the series at its first one: "0-6" is 0.
        chapter.split('-').next().unwrap_or_default().to_string()
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
    let dated = volume.is_empty() || volume == ONESHOT_LABEL || volume_number == "1";
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
        provider_id: format!("{}{}", s.id, id_suffix(volume)),
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

/// The words BookOrbit leaves out of a query when it checks a result against it.
const BOOKORBIT_STOPWORDS: &[&str] = &[
    "the", "an", "and", "or", "of", "in", "on", "at", "to", "for", "with", "from", "by", "is", "it",
    "as", "be", "are", "was", "were", "this", "that", "these", "those", "into", "over", "under",
    "its", "his", "her", "their", "our", "your", "my",
];

/// Whether BookOrbit's own relevance check would keep a result titled `candidate` for `query`: a
/// result survives when more than half of the query's meaningful words (one-letter words and the
/// stopwords above do not count) are in its title. A little stricter than BookOrbit's 50% so a
/// borderline result gets help instead of being lost.
fn reads_as_match(query: &str, candidate: &str) -> bool {
    // BookOrbit also keeps a result whose whole title sits inside the query as whole words, or
    // the other way round ("[Group] Series (en)" holds "Series").
    let (query_words, candidate_words) = (title_words(query), title_words(candidate));
    if contains_words(&query_words, &candidate_words) || contains_words(&candidate_words, &query_words) {
        return true;
    }
    let wanted: Vec<String> = title_words(query)
        .into_iter()
        .filter(|word| word.chars().count() > 1 && !BOOKORBIT_STOPWORDS.contains(&word.as_str()))
        .collect();
    if wanted.is_empty() {
        return true;
    }
    let have: HashSet<String> = title_words(candidate).into_iter().collect();
    let shared = wanted.iter().filter(|word| have.contains(*word)).count();
    shared * 100 > wanted.len() * 55
}

/// Whether `needle` appears in `haystack` as a run of whole words.
fn contains_words(haystack: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|run| run == needle)
}

/// The part of a query from its first volume/chapter marker on, as written: "Vol.01 Ch.0002 -
/// Chapter name (en) [group]". Empty when there is no marker, or when the query starts with it.
pub fn query_tail(query: &str) -> String {
    let query = query.trim();
    let start = [&VOLUME, &CHAPTER]
        .into_iter()
        .filter_map(|pattern| pattern.find(query))
        .map(|found| found.start())
        .min();
    match start {
        Some(start) if !query[..start].trim().is_empty() => query[start..]
            .trim_start_matches(|c: char| c.is_whitespace() || "-\u{2013}\u{2014}:([_".contains(c))
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// What BookOrbit should compare the search against when the title it would use, the English name
/// with the volume, does not read like the query. BookOrbit drops a result that shares too little
/// with the query, and a file name carries a lot that no series name has: a romanized or native
/// name, a chapter title, a language, a scanlation group. So this is the series name that fits the
/// query best (`head`, the query up to its volume/chapter) followed by the query's own tail, which
/// scores as the query itself. Nothing is sent when the English title already reads as a match, or
/// when no name of the series resembles `head`: BookOrbit's own check then stands. `title` and the
/// series stay English; this is only what gets compared.
pub fn display_title(s: &Series, head: &str, query: &str, volume: &str) -> String {
    let english = tidy_title(&english_title(s));
    let plain = if volume.is_empty() {
        english.clone()
    } else {
        format!("{english} - {volume}")
    };
    if reads_as_match(query, &plain) {
        return String::new();
    }

    let wanted: HashSet<String> = title_words(head)
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
    let mut best = english.clone();
    let mut best_overlap = overlap(&english);
    for name in all_titles(s) {
        let name_overlap = overlap(name);
        if name_overlap > best_overlap {
            best = tidy_title(name);
            best_overlap = name_overlap;
        }
    }
    if best_overlap * 2 < wanted.len() {
        return String::new();
    }

    let tail = query_tail(query);
    let label = if tail.is_empty() { volume } else { tail.as_str() };
    let display = if label.is_empty() {
        best.clone()
    } else {
        format!("{best} - {label}")
    };
    if reads_as_match(query, &display) {
        display
    } else if contains_words(&title_words(query), &title_words(&best)) {
        // Words in front of the title ("[Translator](Author) Series (English) Ch 12") are in the
        // query but in no name of the series: the query itself is what scores as a match.
        query.to_string()
    } else {
        // The query does not name the series at all (a bare "chapter 1" in a series' folder): the
        // name goes in front, and the query is inside it.
        format!("{best} - {query}")
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
        .or_else(|| s.title.clone().filter(|t| t.chars().any(|c| c.is_ascii_alphabetic())))
        .or_else(|| s.romanized_title.clone().filter(|t| !t.trim().is_empty()))
        .or_else(|| s.title.clone().filter(|t| !t.trim().is_empty()))
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

/// The title without its parenthesized, bracketed or braced parts ("Series (2010)" is "Series",
/// "[Group] Series (en)" is "Series"), or the title itself when nothing else would be left.
pub fn without_brackets(title: &str) -> String {
    let stripped = strip_title_suffix(&BRACKETED.replace_all(title, " "));
    if stripped.is_empty() {
        title.trim().to_string()
    } else {
        stripped
    }
}

/// The wordings to search MangaBaka with, most specific first: the title as given, then without a
/// trailing edition word ("... the Comic", "(Light Novel)"), then its first half. MangaBaka's
/// search answers nothing when the query has words none of the series' names have, which a
/// romanized or fan-translated file name often does, so a longer query that finds nothing is
/// retried shorter.
pub fn search_queries(title: &str) -> Vec<String> {
    let title = title.trim();
    // A file name's "(2010)", "(en)" or "[group]" is not part of the series' name and makes
    // MangaBaka answer nothing, so the title is searched without them first. The title as given
    // comes next, for the rare series whose name has a bracket in it.
    let plain = without_brackets(title);
    let bare = strip_title_suffix(&EDITION_SUFFIX.replace(&plain, ""));
    let words: Vec<&str> = bare.split_whitespace().collect();
    let mut queries = vec![plain, title.to_string(), bare.clone()];
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
            ("1692vol3", "Berserk - Vol. 3", "comic")
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
    fn provider_ids_say_which_volume_and_chapter() {
        let series = load_series(SERIES_1692);
        let id = |label: &str| map_series(&series, label).provider_id;
        assert_eq!(id("Ch. 11"), "1692v11");
        assert_eq!(id("Vol. 3"), "1692vol3");
        assert_eq!(id("Vol. 2 Ch. 11"), "1692vol2v11");
        assert_eq!(id(""), "1692");
        // The series index stays the chapter, else the volume.
        assert_eq!(map_series(&series, "Vol. 2 Ch. 11").series[0].sequence, "11");
        assert_eq!(map_series(&series, "Vol. 3").series[0].sequence, "3");
    }

    #[test]
    fn the_position_comes_from_the_title_or_else_the_series_number() {
        assert_eq!(position_label("chapter 1", None), "Ch. 1");
        assert_eq!(position_label("Vol. 3", Some("9")), "Vol. 3");
        assert_eq!(position_label("Oneshot", None), "Oneshot");
        assert_eq!(position_label("Something", Some("12")), "Ch. 12");
        assert_eq!(position_label("Something", Some("0-6")), "Ch. 0-6");
        assert_eq!(position_label("Something", Some("10.50")), "Ch. 10.5");
        assert_eq!(position_label("Something", Some("abc")), "");
        assert_eq!(position_label("Something", None), "");
    }

    #[test]
    fn display_title_names_the_series_when_the_title_does_not() {
        let series = Series {
            title: Some("Series".into()),
            ..Default::default()
        };
        // A file that is only "chapter 1" in the series' folder.
        assert_eq!(
            display_title(&series, "Series", "chapter 1", "Ch. 1"),
            "Series - chapter 1"
        );
    }

    #[test]
    fn a_oneshot_keeps_its_name_in_the_title() {
        let split = |query: &str| split_volume(query);
        let to = |series: &str, label: &str| (series.to_string(), label.to_string());
        assert_eq!(split("Series Oneshot"), to("Series", "Oneshot"));
        assert_eq!(split("Series - One-Shot (en) [Group]"), to("Series", "Oneshot"));
        assert_eq!(split("Series (One Shot)"), to("Series", "Oneshot"));
        assert_eq!(split("Series one_shot"), to("Series", "Oneshot"));
        // A chapter or volume wins, and a title that starts with the words is only a title.
        assert_eq!(split("Series Ch. 3 Oneshot"), to("Series", "Ch. 3"));
        assert_eq!(split("One Shot Kill"), to("One Shot Kill", ""));
        assert_eq!(split("Oneshot"), to("Oneshot", ""));

        // The title, the id and the lookup all keep it, and there is no series number.
        let book = map_series(&load_series(SERIES_1692), "Oneshot");
        assert_eq!(book.title, "Berserk - Oneshot");
        assert_eq!(book.provider_id, "1692os");
        assert_eq!(book.series[0].sequence, "");
    }

    #[test]
    fn a_range_of_chapters_stays_in_the_title() {
        let split = |query: &str| split_volume(query);
        let to = |series: &str, label: &str| (series.to_string(), label.to_string());
        assert_eq!(split("Series c000-006"), to("Series", "Ch. 0-6"));
        assert_eq!(split("Series - Ch. 1-6"), to("Series", "Ch. 1-6"));
        assert_eq!(split("Series Chapters 1\u{2013}6"), to("Series", "Ch. 1-6"));
        assert_eq!(split("Series Vol. 2 Ch. 7-9"), to("Series", "Vol. 2 Ch. 7-9"));
        assert_eq!(split("Series c5"), to("Series", "Ch. 5"));
        // Not ranges: a spaced dash (a chapter title starting with a number), a range that goes
        // backwards or nowhere.
        assert_eq!(split("Series Ch. 12 - 100 Days"), to("Series", "Ch. 12"));
        assert_eq!(split("Series Ch. 6-2"), to("Series", "Ch. 6"));
        assert_eq!(split("Series Ch. 3-3"), to("Series", "Ch. 3"));

        // The range is in the title and the id; the series position is its first chapter.
        let book = map_series(&load_series(SERIES_1692), "Ch. 0-6");
        assert_eq!(book.title, "Berserk - Ch. 0-6");
        assert_eq!(book.provider_id, "1692v0-6");
        assert_eq!(book.series[0].sequence, "0");
    }

    #[test]
    fn display_title_follows_a_range_written_as_a_file_name() {
        let series = Series {
            title: Some("Series".into()),
            ..Default::default()
        };
        assert_eq!(
            display_title(&series, "Series", "Series c000-006", "Ch. 0-6"),
            "Series - c000-006"
        );
    }

    #[test]
    fn a_number_left_after_a_dash_is_read_as_the_chapter() {
        let split = |query: &str| split_volume(query);
        assert_eq!(
            split("Secret Rites with the Holy Maidens - 39"),
            ("Secret Rites with the Holy Maidens".to_string(), "Ch. 39".to_string())
        );
        assert_eq!(split("Foo \u{2013} 10.50"), ("Foo".to_string(), "Ch. 10.5".to_string()));
        // A marker still wins, and a year, a title's own number or a lone number is left alone.
        assert_eq!(split("Foo - Ch. 7"), ("Foo".to_string(), "Ch. 7".to_string()));
        assert_eq!(split("Dune - 2019"), ("Dune - 2019".to_string(), String::new()));
        assert_eq!(split("Mob Psycho 100"), ("Mob Psycho 100".to_string(), String::new()));
        assert_eq!(split("- 5"), ("- 5".to_string(), String::new()));
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
        // A romanized query: the romanized name, then the query's own tail.
        let romanized = "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu the Comic Vol. 2 Ch 10";
        assert_eq!(
            display_title(
                &series,
                "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu the Comic",
                romanized,
                "Vol. 2 Ch. 10"
            ),
            "Gamer ga Isekai Konten shite Harem Jinsei e Continue suru Sou desu - Vol. 2 Ch 10"
        );
        // An English query already reads as a match: nothing to add.
        let english = "A Gamer Is Going to Live a Harem Life in Another World";
        assert_eq!(display_title(&series, english, english, ""), "");
    }

    #[test]
    fn display_title_carries_a_noisy_file_name() {
        let series = Series {
            title: Some("Zetsurin!".into()),
            ..Default::default()
        };
        let file = "Zetsurin! - Vol.01 Ch.0002 - Life Begins to Get Strange (en) [Psylocke Scans, Renzokusei Scans]";
        assert_eq!(display_title(&series, "Zetsurin!", file, "Vol. 1 Ch. 2"), file);
        // A series that looks nothing like the title part of the query is not helped.
        let other = Series {
            title: Some("Something Else Entirely".into()),
            ..Default::default()
        };
        assert_eq!(display_title(&other, "Zetsurin!", file, "Vol. 1 Ch. 2"), "");
        assert_eq!(query_tail(file), "Vol.01 Ch.0002 - Life Begins to Get Strange (en) [Psylocke Scans, Renzokusei Scans]");
        assert_eq!(query_tail("Berserk"), "");
    }

    #[test]
    fn display_title_copes_with_words_in_front_of_the_title() {
        let series = Series {
            title: Some("Series".into()),
            ..Default::default()
        };
        // Noise on both sides of the title: nothing to add when the title still sits inside it.
        let around = "[Translator](Author) Series (English)";
        assert_eq!(display_title(&series, "Series", around, ""), "");
        // Noise between the title and the chapter: the query itself is what scores as a match.
        let split = "[Translator](Author) Series (English) Ch 12";
        assert_eq!(display_title(&series, "Series", split, "Ch. 12"), split);
    }

    #[test]
    fn brackets_are_left_out_of_the_search() {
        assert_eq!(without_brackets("Series (2010)"), "Series");
        assert_eq!(without_brackets("Series (en)"), "Series");
        assert_eq!(without_brackets("[Group] Series Name (en) [x]"), "Series Name");
        assert_eq!(without_brackets("(2010)"), "(2010)");
        assert_eq!(without_brackets("Series"), "Series");
        // Searched without the brackets first, then as given.
        assert_eq!(search_queries("Series (2010)"), ["Series", "Series (2010)"]);
        assert_eq!(search_queries("Series (en)"), ["Series", "Series (en)"]);
        assert_eq!(search_queries("Solo Leveling (Light Novel)"), ["Solo Leveling", "Solo Leveling (Light Novel)"]);
        assert_eq!(search_queries("Series"), ["Series"]);
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

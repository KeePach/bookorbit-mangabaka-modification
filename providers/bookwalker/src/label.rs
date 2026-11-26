//! Reading a file's title the way the MangaBaka provider does: the series name, and the volume,
//! chapter or oneshot it names ("Vol. 9-10", "v04c05", "Ch. 7-9"). Copied from
//! `providers/mangabaka/src/mapping.rs` so both providers label books alike; keep them in step.

use std::sync::LazyLock;

use regex::Regex;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

// Accept common volume and chapter aliases such as "Vol.", "Volume", "V", "VL", "Ch",
// "Chapter", "Chptr", and "C#". Allow common typos like "volum" and "chaptr" too.
// Rust's regex engine does not support look-ahead, so we use a non-lookaround end condition.
static VOLUME: LazyLock<Regex> = LazyLock::new(|| {
    // "Vol. 3", "v03", and a range of volumes with no spaces around the dash: "Vol. 9-10",
    // "Vols. 1-3". Group 1 is the volume, group 2 the end of a range.
    regex(
        r"(?i)(?:^|[\s\-:(\[_.])(?:vol(?:umes?|ums?|s)?|vl|v)\.?\s*(?:#\s*)?([0-9]{1,4}(?:\.[0-9]+)?)(?:[-\x{2013}\x{2014}~]([0-9]{1,4}(?:\.[0-9]+)?))?(?:$|[\s\-:,)\](\[_.])",
    )
});
static CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
    // "Ch. 8", "chapter8", "c#8", "c008", and a range of chapters with no spaces around the dash:
    // "Ch. 0-6", "c000-006". Group 1 is the chapter, group 2 the end of a range.
    regex(
        r"(?i)(?:^|[\s\-:(\[_.])(?:(?:chap(?:ters?|tr)?|chptr|ch|c#)\.?\s*(?:#\s*)?|c)([0-9]{1,4}(?:\.[0-9]+)?)(?:[-\x{2013}\x{2014}~]([0-9]{1,4}(?:\.[0-9]+)?))?(?:$|[\s\-:,)\](\[_.])",
    )
});
// A chapter marker written straight after a volume, with no space or only a dot between them:
// "v04c05", "v4.c5", "Vol.2Ch.8". Group 1 is the volume part, group 2 the start of the chapter.
static GLUED_CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?i)((?:^|[\s\-:(\[_.])(?:vol(?:umes?|ums?|s)?|vl|v)\.?\s*(?:#\s*)?[0-9]{1,4}(?:\.[0-9]+)?(?:[-\x{2013}\x{2014}~][0-9]{1,4}(?:\.[0-9]+)?)?)\.?((?:chap(?:ters?|tr)?|chptr|ch|c#|c)\.?\s*(?:#\s*)?[0-9])",
    )
});
static EMPTY_BRACKETS: LazyLock<Regex> = LazyLock::new(|| regex(r"\(\s*\)|\[\s*\]"));
static TRAILING_JUNK: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[\s,.:;_\-\x{2013}\x{2014}(\[]+$"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+"));
// "Oneshot", "One-shot", "one shot": a book with no volume or chapter number to put after its name.
static ONESHOT: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)(?:^|[\s\-:(\[_])one[\s_\-]?shots?(?:$|[\s\-:,)\](\[_])"));
// A series number on its own ("12", "0-6", "10.5"): what BookOrbit has for a book whose file is
// just "chapter 12", as the folder gives the series.
static INDEX_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"^\s*([0-9]{1,4}(?:\.[0-9]+)?)(?:[-\x{2013}\x{2014}~]([0-9]{1,4}(?:\.[0-9]+)?))?\s*$")
});
// A title that is only the word: "Oneshot", "(One-Shot)".
static ONESHOT_ONLY: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^\W*one[\s_\-]?shots?\W*$"));
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
    regex(
        r"(?i)[\s\-:(\[]+(?:the\s+)?(?:light\s+novel|web\s+novel|comics?|manga|novel|webtoon|manhwa|manhua)[\s)\]]*$",
    )
});

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

/// "0006" reads as "6" and "12.50" as "12.5": leading zeros and meaningless trailing decimal
/// zeros go, so the same chapter always produces the same number.
pub fn normalize_number(raw: &str) -> String {
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
    let query = separate_markers(query.trim());
    let query = query.as_str();
    let volume_match = VOLUME.captures(query);
    let chapter_match = CHAPTER.captures(query);
    let volume = volume_match.as_ref().map(range_text).unwrap_or_default();
    let chapter = chapter_match.as_ref().map(range_text).unwrap_or_default();

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
            let is_year = number
                .parse::<u32>()
                .is_ok_and(|n| (1900..=2100).contains(&n));
            (!is_year && !query[..start].trim().is_empty()).then_some((start, number))
        });
        if let Some((start, number)) = bare {
            let cleaned = trim_title_prefix(&strip_title_suffix(query[..start].trim()));
            let label = index_label("", &number);
            return if cleaned.is_empty() {
                (query.to_string(), label)
            } else {
                (cleaned, label)
            };
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

/// Puts a space between a volume and a chapter written together ("v04c05" is "v04 c05", "v4.c5"
/// is "v4 c5"), so both markers are read.
fn separate_markers(query: &str) -> String {
    GLUED_CHAPTER.replace_all(query, "$1 $2").into_owned()
}

/// The volume or chapter a match names: "8", or "0-6" for a range ("Vol. 9-10", "Ch. 0-6"). A
/// range that goes nowhere ("6-2", "3-3") is only its first number.
fn range_text(captures: &regex::Captures) -> String {
    let start = captures
        .get(1)
        .map(|found| normalize_number(found.as_str()))
        .unwrap_or_default();
    let end = captures
        .get(2)
        .map(|found| normalize_number(found.as_str()));
    match end {
        Some(end)
            if end.parse::<f64>().unwrap_or(0.0) > start.parse::<f64>().unwrap_or(f64::MAX) =>
        {
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
        .map(|captures| index_label("", &range_text(&captures)))
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
/// a lookup by id can write the same title again: `v8` is chapter 8, `vol2` is volume 2, `vol2v8`
/// is both, `os` a oneshot. The same form as the MangaBaka provider's ids.
pub fn id_suffix(label: &str) -> String {
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

pub fn series_index_numbers(series_index: &str) -> (String, String) {
    let volume = VOLUME
        .captures(series_index)
        .map(|captures| range_text(&captures))
        .unwrap_or_default();
    let chapter = CHAPTER
        .captures(series_index)
        .map(|captures| range_text(&captures))
        .unwrap_or_default();
    (volume, chapter)
}

pub fn provider_index(series_index: &str) -> String {
    let (volume, chapter) = series_index_numbers(series_index);
    if !chapter.is_empty() {
        // A range of chapters sits in the series at its first one: "0-6" is 0.
        chapter.split('-').next().unwrap_or_default().to_string()
    } else {
        // So does a range of volumes: "9-10" is 9.
        volume.split('-').next().unwrap_or_default().to_string()
    }
}

/// A title without its "(2010)", "(en)" or "[group]" parts, which are never part of the series'
/// name.
pub fn without_brackets(title: &str) -> String {
    let stripped = strip_title_suffix(&BRACKETED.replace_all(title, " "));
    if stripped.is_empty() {
        title.trim().to_string()
    } else {
        stripped
    }
}

/// A title without a trailing edition word a file name adds but BookWalker's names do not:
/// "... the Comic", "(Light Novel)".
pub fn without_edition(title: &str) -> String {
    let bare = strip_title_suffix(&EDITION_SUFFIX.replace(title, ""));
    if bare.is_empty() {
        title.trim().to_string()
    } else {
        bare
    }
}

/// The words of a title, lowercased: "SPY×FAMILY" is `spy x family`.
pub fn title_words(title: &str) -> Vec<String> {
    title
        .replace('×', " x ")
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

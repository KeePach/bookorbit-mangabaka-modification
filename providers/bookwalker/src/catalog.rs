//! Every series, volume and chapter bookwalker.com lists, read from its sitemap. BookWalker's own
//! search refuses requests that do not come from a browser, so titles are matched here instead.
//!
//! The list is saved to disk with the day it was downloaded. The first search on a later day is
//! answered from the saved list at once and fetches a fresh one behind it; a day without searches
//! fetches nothing.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::bookwalker::{Client, UpstreamError};
use crate::label::normalize_number;

const FILE_HEADER: &str = "bookwalker-catalog 1";
/// After a failed download, the next try waits at least this long.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(300);

static ENTRY_LOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<loc>\s*https://bookwalker\.com/(series|volume|chapter)/([0-9A-Z]{12})/([a-z0-9-]{1,300})\s*</loc>")
        .expect("valid pattern")
});
static CHILD_LOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<loc>\s*([^<\s]+/(?:series|volume|chapter)_[0-9]{1,3}\.xml)\s*</loc>")
        .expect("valid pattern")
});
// What follows a series' slug in its volume or chapter slugs: "vol-5", "vols-9-10",
// "chapter-51", "ep12", "5". Group 1 is the first number.
static ITEM_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:vols?|volumes?|chapters?|ch|episodes?|ep|parts?|books?|omnibus(?:-vols?|-volume)?)-?)?([0-9]{1,4})(?:-|$)")
        .expect("valid pattern")
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Series,
    Volume,
    Chapter,
}

impl Kind {
    fn code(self) -> char {
        match self {
            Self::Series => 's',
            Self::Volume => 'v',
            Self::Chapter => 'c',
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            "s" | "series" => Some(Self::Series),
            "v" | "volume" => Some(Self::Volume),
            "c" | "chapter" => Some(Self::Chapter),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub slug: String,
}

/// A series in a search's answer, with how well its slug fits the asked-for title.
#[derive(Clone, Debug)]
pub struct SeriesMatch<'a> {
    pub entry: &'a Entry,
    pub score: u32,
    pub exact: bool,
}

/// A volume or chapter of a series, by its slug.
#[derive(Clone, Debug)]
pub struct ItemMatch<'a> {
    pub entry: &'a Entry,
    /// The slug says exactly the asked-for number ("vol-5", "chapter-51"), not a range or a
    /// subtitle after it.
    pub plain: bool,
}

pub struct Catalog {
    /// Days since 1970-01-01 (UTC) when it was downloaded.
    pub day: u64,
    pub series: Vec<Entry>,
    pub volumes: Vec<Entry>,
    pub chapters: Vec<Entry>,
    series_slugs: HashSet<String>,
}

impl Catalog {
    pub fn new(day: u64, series: Vec<Entry>, volumes: Vec<Entry>, chapters: Vec<Entry>) -> Self {
        let series_slugs = series.iter().map(|s| s.slug.clone()).collect();
        Self {
            day,
            series,
            volumes,
            chapters,
            series_slugs,
        }
    }

    /// Reads sitemap files. Entries seen twice are kept once.
    pub fn from_sitemaps<'a>(day: u64, files: impl IntoIterator<Item = &'a str>) -> Self {
        let (mut series, mut volumes, mut chapters) = (Vec::new(), Vec::new(), Vec::new());
        let mut seen = HashSet::new();
        for file in files {
            for captures in ENTRY_LOC.captures_iter(file) {
                if !seen.insert(captures[2].to_string()) {
                    continue;
                }
                let entry = Entry {
                    id: captures[2].to_string(),
                    slug: captures[3].to_string(),
                };
                match &captures[1] {
                    "series" => series.push(entry),
                    "volume" => volumes.push(entry),
                    _ => chapters.push(entry),
                }
            }
        }
        Self::new(day, series, volumes, chapters)
    }

    pub fn len(&self) -> usize {
        self.series.len() + self.volumes.len() + self.chapters.len()
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("{FILE_HEADER}\nday {}\n", self.day);
        for (kind, list) in [
            (Kind::Series, &self.series),
            (Kind::Volume, &self.volumes),
            (Kind::Chapter, &self.chapters),
        ] {
            for entry in list {
                out.push_str(&format!("{} {} {}\n", kind.code(), entry.id, entry.slug));
            }
        }
        out
    }

    pub fn from_text(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != FILE_HEADER {
            return None;
        }
        let day = lines.next()?.strip_prefix("day ")?.parse().ok()?;
        let (mut series, mut volumes, mut chapters) = (Vec::new(), Vec::new(), Vec::new());
        for line in lines {
            let mut parts = line.split(' ');
            let (Some(kind), Some(id), Some(slug)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let entry = Entry {
                id: id.to_string(),
                slug: slug.to_string(),
            };
            match Kind::from_code(kind) {
                Some(Kind::Series) => series.push(entry),
                Some(Kind::Volume) => volumes.push(entry),
                Some(Kind::Chapter) => chapters.push(entry),
                None => {}
            }
        }
        let catalog = Self::new(day, series, volumes, chapters);
        (!catalog.series.is_empty()).then_some(catalog)
    }

    pub fn series_slug(&self, id: &str) -> Option<&str> {
        self.series
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.slug.as_str())
    }

    /// The series whose slugs fit `title` best: one named exactly as asked first, then by the
    /// share of the title's words in the slug. A series must hold more than half of the title's
    /// words, so "Booty Royale" never answers "Thigh High".
    pub fn rank_series(&self, title: &str) -> Vec<SeriesMatch<'_>> {
        let wanted = slugify(title);
        let words: Vec<&str> = wanted.split('-').filter(|w| !w.is_empty()).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let wanted_set: HashSet<&str> = words.iter().copied().collect();
        let compact_wanted = compact(&wanted);
        let mut found: Vec<(SeriesMatch, usize)> = self
            .series
            .iter()
            .filter_map(|entry| {
                if entry.slug == wanted || compact(&entry.slug) == compact_wanted {
                    return Some((
                        SeriesMatch {
                            entry,
                            score: 1000,
                            exact: true,
                        },
                        0,
                    ));
                }
                let have: HashSet<&str> = entry.slug.split('-').collect();
                let shared = wanted_set.iter().filter(|w| have.contains(**w)).count();
                if shared * 2 <= wanted_set.len() {
                    return None;
                }
                let extra = have.len().saturating_sub(shared);
                // Shared words count most; words the slug has beyond the title count against it.
                let score = (shared * 100 / wanted_set.len()) as u32 * 4;
                Some((
                    SeriesMatch {
                        entry,
                        score: score.saturating_sub(extra as u32 * 10),
                        exact: false,
                    },
                    extra,
                ))
            })
            .collect();
        found.sort_by(|(a, a_extra), (b, b_extra)| {
            b.score
                .cmp(&a.score)
                .then(a_extra.cmp(b_extra))
                .then(a.entry.slug.len().cmp(&b.entry.slug.len()))
        });
        found.into_iter().map(|(m, _)| m).collect()
    }

    /// The volumes (or chapters) of the series with this slug whose slug starts with `number`:
    /// `blue-exorcist-vol-5` for 5. Slugs drop dots, so "5.5" and a range "5-6" both read "5-…";
    /// the page itself says which it is. Items of a longer series slug that starts the same way
    /// ("goblin-slayer-side-story-…" for "goblin-slayer") are left out.
    ///
    /// `extra` is what the asked-for title has beyond the series' name, as a slug ("part-2" for
    /// "Ascendance of a Bookworm Part 2"): items that carry it after the series' slug come first,
    /// and are read past it ("…-part-2-volume-1" is volume 1).
    pub fn items_of(
        &self,
        chapter: bool,
        series_slug: &str,
        extra: &str,
        number: &str,
    ) -> Vec<ItemMatch<'_>> {
        if !extra.is_empty() {
            let found = self.items_after(
                chapter,
                series_slug,
                &format!("{series_slug}-{extra}-"),
                number,
            );
            if !found.is_empty() {
                return found;
            }
        }
        self.items_after(chapter, series_slug, &format!("{series_slug}-"), number)
    }

    fn items_after(
        &self,
        chapter: bool,
        series_slug: &str,
        prefix: &str,
        number: &str,
    ) -> Vec<ItemMatch<'_>> {
        let first = normalize_number(number.split('-').next().unwrap_or_default());
        let whole = first.split('.').next().unwrap_or_default().to_string();
        let list = if chapter {
            &self.chapters
        } else {
            &self.volumes
        };
        let mut found: Vec<ItemMatch> = list
            .iter()
            .filter_map(|entry| {
                let rest = entry.slug.strip_prefix(prefix)?;
                if self.belongs_to_longer_series(&entry.slug, series_slug.len()) {
                    return None;
                }
                let captures = ITEM_NUMBER.captures(rest)?;
                if normalize_number(&captures[1]) != whole {
                    return None;
                }
                Some(ItemMatch {
                    entry,
                    plain: captures[0].len() == rest.len(),
                })
            })
            .collect();
        found.sort_by_key(|m| !m.plain);
        found
    }

    /// Volumes listed under no series in the sitemap whose slug, without its number, is `title`:
    /// `human-stock-exchange-volume-2` for "Human Stock Exchange".
    pub fn loose_volumes(&self, title: &str, number: &str) -> Vec<ItemMatch<'_>> {
        let wanted = slugify(title);
        if wanted.is_empty() || self.series_slugs.contains(&wanted) {
            return Vec::new();
        }
        self.items_of(
            false,
            &wanted,
            "",
            if number.is_empty() { "1" } else { number },
        )
    }

    fn belongs_to_longer_series(&self, slug: &str, series_len: usize) -> bool {
        slug.char_indices()
            .filter(|&(i, c)| c == '-' && i > series_len)
            .any(|(i, _)| self.series_slugs.contains(&slug[..i]))
    }
}

/// A title as bookwalker.com writes it into a link: punctuation dropped where it stands, spaces
/// and hyphens as `-`. "Didn't I Say to Make My Abilities Average?!" is
/// `didnt-i-say-to-make-my-abilities-average`, "Re:ZERO" is `rezero`, "Guide—Eyes Only" is
/// `guideeyes-only`.
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for c in title.replace('×', " x ").chars().map(fold_accent) {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if (c.is_whitespace() || c == '-' || c == '_')
            && !slug.ends_with('-')
            && !slug.is_empty()
        {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// A slug without its hyphens, to compare names whose punctuation differs: "Re: Zero" and
/// "Re:ZERO".
fn compact(slug: &str) -> String {
    slug.replace('-', "")
}

fn fold_accent(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' => 'a',
        'Á' | 'À' | 'Â' | 'Ä' | 'Ã' | 'Å' | 'Ā' => 'A',
        'é' | 'è' | 'ê' | 'ë' | 'ē' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' | 'Ē' => 'E',
        'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
        'Í' | 'Ì' | 'Î' | 'Ï' | 'Ī' => 'I',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ō' => 'o',
        'Ó' | 'Ò' | 'Ô' | 'Ö' | 'Õ' | 'Ō' => 'O',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' => 'u',
        'Ú' | 'Ù' | 'Û' | 'Ü' | 'Ū' => 'U',
        'ñ' => 'n',
        'Ñ' => 'N',
        'ç' => 'c',
        'Ç' => 'C',
        other => other,
    }
}

/// Days since 1970-01-01, UTC.
pub fn today() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / 86_400)
}

/// The catalog, its saved copy, and when to fetch a new one.
pub struct Store {
    client: Arc<Client>,
    file: Option<PathBuf>,
    clock: fn() -> u64,
    current: RwLock<Option<Arc<Catalog>>>,
    loading: Arc<tokio::sync::Mutex<()>>,
    failed_at: Mutex<Option<Instant>>,
}

impl Store {
    pub fn new(client: Arc<Client>, file: Option<PathBuf>, clock: fn() -> u64) -> Self {
        Self {
            client,
            file,
            clock,
            current: RwLock::new(None),
            loading: Arc::new(tokio::sync::Mutex::new(())),
            failed_at: Mutex::new(None),
        }
    }

    /// The saved catalog from disk, when there is one. Run once at start.
    pub fn load_saved(&self) {
        let Some(file) = &self.file else { return };
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        match Catalog::from_text(&text) {
            Some(catalog) => {
                eprintln!(
                    "[catalog] [load] entries={} day={} - saved catalog loaded",
                    catalog.len(),
                    catalog.day
                );
                self.set(Arc::new(catalog));
            }
            None => eprintln!("[catalog] [load] file={file:?} - saved catalog unreadable, ignored"),
        }
    }

    /// The catalog for a search. A catalog from an earlier day is answered at once, and a fresh
    /// one is fetched behind it; only the very first download makes the caller wait.
    pub async fn get(self: &Arc<Self>) -> Result<Arc<Catalog>, UpstreamError> {
        if let Some(catalog) = self.snapshot() {
            if catalog.day < (self.clock)() && !self.failed_recently() {
                self.refresh_in_background();
            }
            return Ok(catalog);
        }
        let _guard = self.loading.lock().await;
        if let Some(catalog) = self.snapshot() {
            return Ok(catalog);
        }
        if self.failed_recently() {
            return Err(UpstreamError::Failed(
                "catalog download failed recently".into(),
            ));
        }
        self.reload().await
    }

    /// Starts a download unless one is running. True when this call started it.
    pub fn refresh_in_background(self: &Arc<Self>) -> bool {
        let Ok(guard) = self.loading.clone().try_lock_owned() else {
            return false;
        };
        let this = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let _ = this.reload().await;
        });
        true
    }

    pub fn snapshot(&self) -> Option<Arc<Catalog>> {
        self.current.read().expect("catalog lock poisoned").clone()
    }

    fn set(&self, catalog: Arc<Catalog>) {
        *self.current.write().expect("catalog lock poisoned") = Some(catalog);
    }

    fn failed_recently(&self) -> bool {
        self.failed_at
            .lock()
            .expect("catalog lock poisoned")
            .is_some_and(|at| at.elapsed() < RETRY_AFTER_FAILURE)
    }

    async fn reload(&self) -> Result<Arc<Catalog>, UpstreamError> {
        let started = Instant::now();
        match self.download().await {
            Ok(catalog) => {
                let catalog = Arc::new(catalog);
                eprintln!(
                    "[catalog] [end] series={} volumes={} chapters={} durationMs={} - catalog downloaded",
                    catalog.series.len(),
                    catalog.volumes.len(),
                    catalog.chapters.len(),
                    started.elapsed().as_millis()
                );
                self.save(&catalog);
                self.set(catalog.clone());
                *self.failed_at.lock().expect("catalog lock poisoned") = None;
                Ok(catalog)
            }
            Err(err) => {
                eprintln!(
                    "[catalog] [fail] durationMs={} error={:?} - catalog download failed",
                    started.elapsed().as_millis(),
                    err.message()
                );
                *self.failed_at.lock().expect("catalog lock poisoned") = Some(Instant::now());
                Err(err)
            }
        }
    }

    async fn download(&self) -> Result<Catalog, UpstreamError> {
        let index = self.client.sitemap_index().await?;
        let children: Vec<String> = CHILD_LOC
            .captures_iter(&index)
            .map(|captures| captures[1].to_string())
            .collect();
        if children.is_empty() {
            return Err(UpstreamError::Failed(
                "sitemap index lists no catalog files".into(),
            ));
        }
        let mut files = Vec::with_capacity(children.len());
        for child in &children {
            files.push(self.client.sitemap_file(child).await?);
        }
        let catalog = Catalog::from_sitemaps((self.clock)(), files.iter().map(String::as_str));
        if catalog.series.is_empty() {
            return Err(UpstreamError::Failed("sitemap lists no series".into()));
        }
        Ok(catalog)
    }

    fn save(&self, catalog: &Catalog) {
        let Some(file) = &self.file else { return };
        let temporary = file.with_extension("tmp");
        let written = std::fs::write(&temporary, catalog.to_text())
            .and_then(|()| std::fs::rename(&temporary, file));
        if let Err(error) = written {
            eprintln!(
                "[catalog] [save] file={file:?} error={error:?} - catalog kept in memory only"
            );
        }
    }
}

/// How the asked-for number compares with a page's: 2 the same ("9-10" and "9-10"), 1 inside it
/// ("9" in "9-10"), 0 not at all.
pub fn number_fit(wanted: &str, have: &str) -> u8 {
    if wanted.is_empty() || have.is_empty() {
        return 0;
    }
    if wanted == have {
        return 2;
    }
    let bounds = |text: &str| -> Option<(f64, f64)> {
        let mut parts = text.split('-');
        let low: f64 = parts.next()?.parse().ok()?;
        let high: f64 = parts.next().map_or(Some(low), |p| p.parse().ok())?;
        Some((low, high))
    };
    match (bounds(wanted), bounds(have)) {
        (Some((low, _)), Some((have_low, have_high))) if have_low <= low && low <= have_high => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIES: &str = include_str!("../testdata/sitemap-series_1.xml");
    const VOLUMES: &str = include_str!("../testdata/sitemap-volume_1.xml");
    const CHAPTERS: &str = include_str!("../testdata/sitemap-chapter_1.xml");

    fn catalog() -> Catalog {
        Catalog::from_sitemaps(20_000, [SERIES, VOLUMES, CHAPTERS])
    }

    #[test]
    fn reads_the_sitemaps() {
        let c = catalog();
        assert_eq!(
            (c.series.len(), c.volumes.len(), c.chapters.len()),
            (12, 14, 6)
        );
        assert_eq!(c.series_slug("1M9B28BP9B40"), Some("blue-exorcist"));
    }

    #[test]
    fn saves_and_reads_back() {
        let c = catalog();
        let back = Catalog::from_text(&c.to_text()).unwrap();
        assert_eq!(back.day, 20_000);
        assert_eq!(back.series, c.series);
        assert_eq!(back.volumes, c.volumes);
        assert_eq!(back.chapters, c.chapters);
        assert!(Catalog::from_text("something else").is_none());
    }

    #[test]
    fn slugs_titles_like_the_site() {
        assert_eq!(
            slugify("Booty Royale: Never Go Down Without a Fight!"),
            "booty-royale-never-go-down-without-a-fight"
        );
        assert_eq!(
            slugify("Didn't I Say to Make My Abilities Average?!"),
            "didnt-i-say-to-make-my-abilities-average"
        );
        assert_eq!(slugify("SPY×FAMILY"), "spy-x-family");
        assert_eq!(
            slugify("Re:ZERO -Starting Life in Another World-"),
            "rezero-starting-life-in-another-world"
        );
        assert_eq!(
            slugify("Spy x Family: The Official Guide—Eyes Only"),
            "spy-x-family-the-official-guideeyes-only"
        );
        assert_eq!(
            slugify("2.5 Dimensional Seduction"),
            "25-dimensional-seduction"
        );
        assert_eq!(
            slugify("Mistress Kanan is Devilishly Easy (Omnibus)"),
            "mistress-kanan-is-devilishly-easy-omnibus"
        );
        assert_eq!(slugify("Pokémon Adventures"), "pokemon-adventures");
    }

    #[test]
    fn ranks_series_by_name() {
        let c = catalog();
        let ranked = c.rank_series("Booty Royale: Never Go Down Without a Fight!");
        assert!(ranked[0].exact);
        assert_eq!(ranked[0].entry.id, "1KJFRS6F0V2G");
        let ranked = c.rank_series("Booty Royale");
        assert_eq!(ranked[0].entry.id, "1KJFRS6F0V2G");
        // Both series named alike come back, the manga and the novel.
        let ids: Vec<&str> = c
            .rank_series("Goblin Slayer Side Story: Year One")
            .iter()
            .filter(|m| m.exact)
            .map(|m| m.entry.id.as_str())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(c.rank_series("Thigh High Reiwa").is_empty());
        // Punctuation written differently still names the series exactly.
        assert!(c.rank_series("SPY×FAMILY")[0].exact);
        assert!(c.rank_series("SpyxFamily")[0].exact);
        assert!(c.rank_series("").is_empty());
    }

    #[test]
    fn finds_items_by_number() {
        let c = catalog();
        let slugs = |chapter, series, number| -> Vec<String> {
            c.items_of(chapter, series, "", number)
                .iter()
                .map(|m| m.entry.slug.clone())
                .collect()
        };
        assert_eq!(slugs(false, "blue-exorcist", "5"), ["blue-exorcist-vol-5"]);
        assert_eq!(
            slugs(false, "booty-royale-never-go-down-without-a-fight", "9-10"),
            ["booty-royale-never-go-down-without-a-fight-vols-9-10"]
        );
        assert_eq!(
            slugs(true, "goblin-slayer-side-story-year-one", "51"),
            ["goblin-slayer-side-story-year-one-chapter-51"]
        );
        // "chapter-5-1" may be 5.1 or a part; the page decides.
        assert_eq!(slugs(true, "king-of-eden", "5").len(), 2);
        assert!(slugs(false, "blue-exorcist", "7").is_empty());
    }

    #[test]
    fn words_after_the_series_name_pick_the_part() {
        let c = Catalog::new(
            0,
            vec![Entry {
                id: "S".into(),
                slug: "ascendance-of-a-bookworm".into(),
            }],
            [
                "part-1-volume-1",
                "part-2-volume-1",
                "part-2-volume-2",
                "volume-1",
            ]
            .iter()
            .map(|rest| Entry {
                id: (*rest).into(),
                slug: format!("ascendance-of-a-bookworm-{rest}"),
            })
            .collect(),
            Vec::new(),
        );
        let ids = |extra, number| -> Vec<String> {
            c.items_of(false, "ascendance-of-a-bookworm", extra, number)
                .iter()
                .map(|m| m.entry.id.clone())
                .collect()
        };
        assert_eq!(ids("part-2", "1"), ["part-2-volume-1"]);
        assert_eq!(ids("part-2", "2"), ["part-2-volume-2"]);
        // Without a part, a slug that is only the number comes first.
        assert_eq!(ids("", "1"), ["volume-1", "part-1-volume-1"]);
    }

    #[test]
    fn finds_volumes_listed_under_no_series() {
        let c = catalog();
        let found = c.loose_volumes("Human Stock Exchange", "2");
        assert_eq!(found[0].entry.slug, "human-stock-exchange-volume-2");
        assert!(c.loose_volumes("Blue Exorcist", "5").is_empty());
    }

    #[test]
    fn compares_numbers() {
        assert_eq!(number_fit("9-10", "9-10"), 2);
        assert_eq!(number_fit("9", "9-10"), 1);
        assert_eq!(number_fit("10", "9-10"), 1);
        assert_eq!(number_fit("11", "9-10"), 0);
        assert_eq!(number_fit("5", "5"), 2);
        assert_eq!(number_fit("5", "6"), 0);
        assert_eq!(number_fit("", "6"), 0);
    }
}

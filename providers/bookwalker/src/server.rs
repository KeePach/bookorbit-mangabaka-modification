use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;

use crate::bookwalker::{Pages, UpstreamError};
use crate::catalog::{number_fit, slugify, Catalog, Store};
use crate::label::{
    position_label, series_index_numbers, split_volume, title_words, without_brackets,
    without_edition,
};
use crate::mapping::{
    edition_tag, map_item, map_series, other_version, parse_provider_id, tag_edition, BookMetadata,
    Target, MAX_ID_LENGTH,
};
use crate::pages::{Format, ItemKind, ItemPage, SeriesPage};
use crate::{Config, VERSION};

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 20;
/// Series whose pages one search reads: a manga, its novel and a spin-off is the usual set.
const MAX_SERIES_PER_SEARCH: usize = 3;
/// Volume or chapter pages tried for one series before giving up on the asked-for number.
const MAX_ITEM_TRIES: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone)]
struct AppState {
    cfg: Arc<Config>,
    store: Arc<Store>,
    pages: Arc<Pages>,
}

pub fn routes(cfg: Arc<Config>, store: Arc<Store>, pages: Arc<Pages>) -> Router {
    Router::new()
        .route("/provider/info", get(info))
        .route("/search", get(search))
        .route("/book/{id}", get(lookup))
        .route("/open/{id}", get(open))
        .route("/catalog/refresh", post(refresh))
        .with_state(AppState { cfg, store, pages })
}

async fn info(State(state): State<AppState>) -> Response {
    Json(json!({
        "name": "BookWalker",
        "version": VERSION,
        "avatarUrl": "https://bookwalker.com/icons/icon-512x512.png",
        "specVersion": "1",
        "authenticationRequired": state.cfg.auth_token.is_some(),
        // Proposed extensions.
        "key": "bookwalker",
        "mediaTypes": ["book", "comic"],
        "lookup": true,
        "supportedFields": [
            "title", "description", "cover", "authors", "publisher", "publishedYear", "language",
            "isbn", "seriesName", "seriesIndex", "genres", "tags"
        ],
        // `series` and `seriesIndex` are what BookOrbit knows of a book from its folder and file
        // number. A BookOrbit that does not send them is unaffected.
        "searchParams": ["limit", "series", "seriesIndex"],
        "idMaxLength": MAX_ID_LENGTH,
        // An id is not a page address, so the template points at this provider's own `/open/{id}`,
        // which sends the browser on to the volume, chapter or series on bookwalker.com. Only
        // declared when `PUBLIC_URL` says where a browser can reach this provider.
        "bookUrlTemplate": state
            .cfg
            .public_url
            .as_ref()
            .map(|base| format!("{base}/open/{{id}}")),
        "coverShape": "portrait",
        // BookWalker's own accent (`--color-yellow-350`, its button and highlight yellow). Its
        // theme color is black, which cannot be read on a dark page.
        "color": "#edb45e",
        // A search reads up to three series pages and a volume or chapter page.
        "rateLimit": {
            "maxConcurrency": 1,
            "minIntervalMs": 2 * state.pages.client().interval().as_millis() as u64,
        },
    }))
    .into_response()
}

#[derive(Serialize)]
struct SearchAnswer {
    matches: Vec<BookMetadata>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    partial: bool,
}

/// What one search asks for, read from its parameters.
struct Wanted {
    /// Series names to try, in order: BookOrbit's series (from the folder), then the title's.
    names: Vec<String>,
    /// "Vol. 9-10", "Vol. 4 Ch. 5", "Ch. 51", "Oneshot" or nothing.
    label: String,
    volume: String,
    chapter: String,
    author: String,
}

impl Wanted {
    fn read(q: &HashMap<String, String>) -> Self {
        let param = |key: &str| q.get(key).map(|v| v.trim()).unwrap_or_default();
        let query = param("query");
        let series = Some(param("series")).filter(|v| !v.is_empty());
        let (head, title_label) = split_volume(query);
        // With BookOrbit's series, the title only says which book of it this is.
        let label = match series {
            Some(_) => position_label(query, Some(param("seriesIndex")).filter(|v| !v.is_empty())),
            None => title_label.clone(),
        };
        let mut names = Vec::new();
        if let Some(series) = series {
            names.push(series.to_string());
        }
        // A title that is only "chapter 1" names no series.
        if !(head == query && !title_label.is_empty()) {
            names.push(head);
        }
        let mut seen = HashSet::new();
        names = names
            .into_iter()
            .flat_map(|name| {
                let plain = without_brackets(&name);
                [plain.clone(), without_edition(&plain)]
            })
            .filter(|name| !name.is_empty() && seen.insert(name.to_lowercase()))
            .collect();
        let (volume, chapter) = series_index_numbers(&label);
        Self {
            names,
            label,
            volume,
            chapter,
            author: param("author").to_string(),
        }
    }
}

async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(denied) = deny_unauthorized(&state, &headers) {
        return denied;
    }
    let param = |key: &str| q.get(key).map(|v| v.trim()).unwrap_or_default();
    let query = param("query");
    if query.is_empty() && param("isbn").is_empty() {
        return error(StatusCode::BAD_REQUEST, "query is required");
    }
    // No ISBN search without the site's own search, and audiobooks are left out.
    if query.is_empty() || param("mediaType") == "audiobook" {
        return Json(json!({ "matches": [] })).into_response();
    }
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(DEFAULT_LIMIT, |n| n.clamp(1, MAX_LIMIT));
    let wanted = Wanted::read(&q);

    let found = tokio::time::timeout(REQUEST_TIMEOUT, find(&state, &wanted, limit))
        .await
        .unwrap_or_else(|_| Err(UpstreamError::Failed("timed out".into())));
    match found {
        Ok(answer) => Json(answer).into_response(),
        Err(err) => upstream_error("search", err),
    }
}

struct Found {
    book: BookMetadata,
    /// The series page the record came from, to tell a manga that has a novel of its name.
    series: Option<Arc<SeriesPage>>,
    by_author: bool,
    /// 2 the asked-for volume/chapter itself, 1 a volume holding it, 0 the series.
    item_fit: u8,
    exact: bool,
    score: u32,
    rank: usize,
}

async fn find(
    state: &AppState,
    wanted: &Wanted,
    limit: usize,
) -> Result<SearchAnswer, UpstreamError> {
    let catalog = state.store.get().await?;
    let mut found = Vec::new();
    let mut failure = None;
    let mut seen = HashSet::new();

    'names: for name in &wanted.names {
        let ranked = catalog.rank_series(name);
        // Every series named exactly as asked (a manga and its novel share a name), else the best few.
        let exact = ranked.iter().filter(|m| m.exact).count();
        let take = exact
            .max(MAX_SERIES_PER_SEARCH.min(ranked.len()))
            .min(MAX_SERIES_PER_SEARCH + 1);
        for (rank, candidate) in ranked.into_iter().take(take).enumerate() {
            if !seen.insert(candidate.entry.id.clone()) {
                continue;
            }
            let series = match state
                .pages
                .series(&candidate.entry.id, &candidate.entry.slug)
                .await
            {
                Ok(series) => series,
                Err(UpstreamError::NotFound) => continue,
                Err(err) => {
                    failure = Some(err);
                    break 'names;
                }
            };
            if series.format == Format::Audiobook {
                continue;
            }
            let extra = extra_words(name, &series.slug);
            let (book, item_fit) = match pick_item(state, &catalog, &series, wanted, &extra).await {
                Ok(Some((item, fit))) => (map_item(&item, Some(&series), &wanted.label), fit),
                Ok(None) => (map_series(&series, &wanted.label), 0),
                Err(err) => {
                    failure = Some(err);
                    break 'names;
                }
            };
            found.push(Found {
                by_author: author_matches(&book.authors, &wanted.author),
                book,
                series: Some(series.clone()),
                item_fit,
                exact: candidate.exact,
                score: candidate.score,
                rank,
            });
        }
        if !found.is_empty() {
            break;
        }
    }

    // A volume the sitemap lists under no series: matched by its own slug.
    if found.is_empty() && failure.is_none() && !wanted.volume.is_empty() {
        for name in &wanted.names {
            let loose: Vec<String> = catalog
                .loose_volumes(name, &wanted.volume)
                .iter()
                .take(MAX_ITEM_TRIES)
                .map(|m| m.entry.id.clone())
                .collect();
            for id in loose {
                match state.pages.item(ItemKind::Volume, &id).await {
                    Ok(item) => {
                        let fit =
                            number_fit(&wanted.volume, &series_index_numbers(&item.label()).0);
                        if fit > 0 {
                            found.push(Found {
                                book: map_item(&item, None, &wanted.label),
                                series: None,
                                by_author: author_matches(&item.authors, &wanted.author),
                                item_fit: fit,
                                exact: true,
                                score: 0,
                                rank: 0,
                            });
                            break;
                        }
                    }
                    Err(UpstreamError::NotFound) => {}
                    Err(err) => {
                        failure = Some(err);
                        break;
                    }
                }
            }
            if !found.is_empty() || failure.is_some() {
                break;
            }
        }
    }

    if found.is_empty() {
        if let Some(err) = failure {
            return Err(err);
        }
    }
    // A series whose other version (the novel of a manga, the manga of a novel) came up under the
    // same name is tagged "(Manga)" or "(Novel)", so BookOrbit files the two apart.
    let read: Vec<(String, Format)> = found
        .iter()
        .filter_map(|f| f.series.as_ref())
        .map(|s| (slugify(&s.name), s.format))
        .collect();
    for f in &mut found {
        if let Some(series) = &f.series {
            if let Some(tag) = edition_tag(series.format) {
                let name = slugify(&series.name);
                if read
                    .iter()
                    .any(|(other, format)| *other == name && other_version(series.format, *format))
                {
                    tag_edition(&mut f.book, tag);
                }
            }
        }
    }
    // The asked-for author first, then the asked-for volume or chapter itself, then a series named
    // exactly as asked, then the closest name. A manga and its novel of the same name both come
    // back, each with its own media type: BookOrbit picks by the book's file (a CBZ prefers the
    // manga, an EPUB the novel). Its `mediaType` parameter says nothing here, being `book` for
    // every book that is not an audiobook.
    found.sort_by_key(|f| {
        (
            !f.by_author,
            Reverse(f.item_fit),
            !f.exact,
            Reverse(f.score),
            f.rank,
        )
    });
    found.truncate(limit);
    Ok(SearchAnswer {
        matches: found.into_iter().map(|f| f.book).collect(),
        partial: failure.is_some(),
    })
}

/// The volume or chapter page a search asks for, with how well it fits (2 itself, 1 a volume
/// holding it), or `None` when BookWalker does not sell it on its own.
async fn pick_item(
    state: &AppState,
    catalog: &Catalog,
    series: &SeriesPage,
    wanted: &Wanted,
    extra: &str,
) -> Result<Option<(Arc<ItemPage>, u8)>, UpstreamError> {
    // A chapter: by its slug, checked against the chapter page's own title.
    if !wanted.chapter.is_empty() {
        let ids: Vec<String> = catalog
            .items_of(true, &series.slug, extra, &wanted.chapter)
            .iter()
            .take(MAX_ITEM_TRIES)
            .map(|m| m.entry.id.clone())
            .collect();
        for id in ids {
            let item = match state.pages.item(ItemKind::Chapter, &id).await {
                Ok(item) => item,
                Err(UpstreamError::NotFound) => continue,
                Err(err) => return Err(err),
            };
            if !same_series(&item, series) {
                continue;
            }
            let fit = number_fit(&wanted.chapter, &series_index_numbers(&item.label()).1);
            if fit > 0 {
                return Ok(Some((item, fit)));
            }
        }
    }
    if wanted.volume.is_empty() {
        return Ok(None);
    }
    // A volume: as the series page lists it ("Vols. 9-10"), else by its slug. With words after
    // the series' name ("Part 2"), a volume whose slug carries them comes first.
    let marker = format!("-{extra}-");
    let listed = series
        .volumes
        .iter()
        .map(|link| {
            (
                link,
                number_fit(&wanted.volume, &series_index_numbers(&link.label).0),
            )
        })
        .filter(|(_, fit)| *fit > 0)
        .max_by_key(|(link, fit)| (!extra.is_empty() && link.slug.contains(&marker), *fit));
    if let Some((link, fit)) = listed {
        match state.pages.item(ItemKind::Volume, &link.id).await {
            Ok(item) => return Ok(Some((item, fit))),
            Err(UpstreamError::NotFound) => {}
            Err(err) => return Err(err),
        }
    }
    let listed_ids: HashSet<&str> = series
        .volumes
        .iter()
        .filter(|link| !link.label.is_empty())
        .map(|link| link.id.as_str())
        .collect();
    let ids: Vec<String> = catalog
        .items_of(false, &series.slug, extra, &wanted.volume)
        .iter()
        // A volume the series page lists under another number is not this one.
        .filter(|m| !listed_ids.contains(m.entry.id.as_str()))
        .take(MAX_ITEM_TRIES)
        .map(|m| m.entry.id.clone())
        .collect();
    for id in ids {
        let item = match state.pages.item(ItemKind::Volume, &id).await {
            Ok(item) => item,
            Err(UpstreamError::NotFound) => continue,
            Err(err) => return Err(err),
        };
        if !same_series(&item, series) {
            continue;
        }
        let fit = number_fit(&wanted.volume, &series_index_numbers(&item.label()).0);
        if fit > 0 {
            return Ok(Some((item, fit)));
        }
    }
    Ok(None)
}

/// What `name` has beyond the series' name, as a slug: "part-2" for "Ascendance of a Bookworm
/// Part 2" and the series `ascendance-of-a-bookworm`. Empty when it has nothing more.
fn extra_words(name: &str, series_slug: &str) -> String {
    slugify(name)
        .strip_prefix(&format!("{series_slug}-"))
        .unwrap_or_default()
        .to_string()
}

/// A manga and its novel can share a slug, so a page found by slug must name this series.
fn same_series(item: &ItemPage, series: &SeriesPage) -> bool {
    item.series.as_ref().is_none_or(|s| s.id == series.id)
}

/// Whether `author` names one of `authors`, in any order and ignoring punctuation.
fn author_matches(authors: &[String], author: &str) -> bool {
    let wanted: HashSet<String> = title_words(author).into_iter().collect();
    !wanted.is_empty()
        && authors.iter().any(|name| {
            let have: HashSet<String> = title_words(name).into_iter().collect();
            wanted.is_subset(&have) || have.is_subset(&wanted)
        })
}

async fn lookup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(denied) = deny_unauthorized(&state, &headers) {
        return denied;
    }
    let Some(parsed) = parse_provider_id(&id) else {
        return error(StatusCode::NOT_FOUND, "Not found");
    };
    let result = tokio::time::timeout(
        REQUEST_TIMEOUT,
        look_up(&state, &parsed.target, &parsed.label),
    )
    .await
    .unwrap_or_else(|_| Err(UpstreamError::Failed("timed out".into())));
    match result {
        Ok(book) => Json(book).into_response(),
        Err(err) => upstream_error("lookup", err),
    }
}

async fn look_up(
    state: &AppState,
    target: &Target,
    label: &str,
) -> Result<BookMetadata, UpstreamError> {
    match target {
        Target::Item(kind, id) => {
            let item = state.pages.item(*kind, id).await?;
            // The series page adds genres and tags; the record stands without them.
            let series = match &item.series {
                Some(series) => match state.pages.series(&series.id, &series.slug).await {
                    Ok(page) => Some(page),
                    Err(UpstreamError::NotFound) => None,
                    Err(err) => return Err(err),
                },
                None => None,
            };
            let mut book = map_item(&item, series.as_deref(), label);
            if let Some(series) = &series {
                tag_if_other_version(state, series, &mut book).await;
            }
            Ok(book)
        }
        Target::Series(id) => {
            // The series page only answers in full at its address with the slug.
            let catalog = state.store.get().await?;
            let slug = catalog.series_slug(id).ok_or(UpstreamError::NotFound)?;
            let series = state.pages.series(id, slug).await?;
            let mut book = map_series(&series, label);
            tag_if_other_version(state, &series, &mut book).await;
            Ok(book)
        }
    }
}

/// Tags a series "(Manga)" or "(Novel)" when the catalog holds its other version under the same
/// name, as the search does, so a refresh by id keeps the tag. Another series with the same slug is
/// read (its page is cached) to see which it is. A page that cannot be read leaves the record
/// untagged.
async fn tag_if_other_version(state: &AppState, series: &SeriesPage, book: &mut BookMetadata) {
    let Some(tag) = edition_tag(series.format) else {
        return;
    };
    let Ok(catalog) = state.store.get().await else {
        return;
    };
    let namesakes: Vec<(String, String)> = catalog
        .series
        .iter()
        .filter(|e| e.slug == series.slug && e.id != series.id)
        .map(|e| (e.id.clone(), e.slug.clone()))
        .collect();
    for (id, slug) in namesakes {
        if let Ok(page) = state.pages.series(&id, &slug).await {
            if other_version(series.format, page.format) {
                tag_edition(book, tag);
                return;
            }
        }
    }
}

/// Sends a browser from a provider id to its page on bookwalker.com. It is what a provider link
/// opens, so it answers without the auth token. Volumes and chapters need nothing looked up
/// (bookwalker.com adds the slug itself); a series page only renders at its address with the slug,
/// which comes from the catalog.
async fn open(State(state): State<AppState>, Path(raw): Path<String>) -> Response {
    let Some(parsed) = parse_provider_id(&raw) else {
        return error(StatusCode::NOT_FOUND, "Not found");
    };
    let url = match &parsed.target {
        Target::Item(kind, id) => format!("https://bookwalker.com/{}/{id}", kind.path()),
        Target::Series(id) => {
            let catalog = match state.store.snapshot() {
                Some(catalog) => catalog,
                None => match tokio::time::timeout(REQUEST_TIMEOUT, state.store.get()).await {
                    Ok(Ok(catalog)) => catalog,
                    Ok(Err(err)) => return upstream_error("open", err),
                    Err(_) => {
                        return upstream_error("open", UpstreamError::Failed("timed out".into()))
                    }
                },
            };
            match catalog.series_slug(id) {
                Some(slug) => format!("https://bookwalker.com/series/{id}/{slug}"),
                None => return error(StatusCode::NOT_FOUND, "Not found"),
            }
        }
    };
    Redirect::temporary(&url).into_response()
}

/// Fetches a fresh catalog now, behind the answer. Not needed in normal use: the first search of
/// each day does this on its own.
async fn refresh(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(denied) = deny_unauthorized(&state, &headers) {
        return denied;
    }
    let started = state.store.refresh_in_background();
    (StatusCode::ACCEPTED, Json(json!({ "started": started }))).into_response()
}

/// Status codes are part of the contract: a client backs off on 429 and treats 5xx as a failure,
/// never as "no results".
fn upstream_error(op: &str, err: UpstreamError) -> Response {
    match err {
        UpstreamError::RateLimited {
            retry_after_seconds,
        } => {
            let mut res = error(StatusCode::TOO_MANY_REQUESTS, "rate limited");
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_seconds));
            res
        }
        UpstreamError::NotFound => error(StatusCode::NOT_FOUND, "Not found"),
        UpstreamError::Failed(message) => {
            eprintln!("[provider.{op}] [fail] error={message:?} - bookwalker.com request failed");
            error(StatusCode::BAD_GATEWAY, "bookwalker.com unavailable")
        }
    }
}

/// The refusal to send when a token is configured and the request does not carry it.
fn deny_unauthorized(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let token = state.cfg.auth_token.as_ref()?;
    let given = headers
        .get(header::AUTHORIZATION)
        .map(HeaderValue::as_bytes)
        .unwrap_or_default();
    (!constant_time_eq(given, token.as_bytes()))
        .then(|| error(StatusCode::UNAUTHORIZED, "Unauthorized"))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};

    use axum::extract::Request;

    use super::*;
    use crate::bookwalker::Client;

    #[derive(Default)]
    struct Upstream {
        /// An error status every page answers with instead, when not zero.
        status: AtomicU16,
        index: AtomicUsize,
        pages: AtomicUsize,
    }

    /// Answers like bookwalker.com and its static host from the captured fixtures.
    async fn fake_bookwalker(upstream: Arc<Upstream>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let index =
            include_str!("../testdata/sitemap.xml").replace("https://static.bookwalker.com", &base);
        let app = Router::new().fallback(move |request: Request| {
            let upstream = upstream.clone();
            let index = index.clone();
            async move {
                let path = request.uri().path().to_string();
                if path == "/sitemap.xml" {
                    upstream.index.fetch_add(1, Ordering::SeqCst);
                    return (StatusCode::OK, index).into_response();
                }
                let body = match path.rsplit('/').next().unwrap_or_default() {
                    "series_1.xml" => Some(include_str!("../testdata/sitemap-series_1.xml")),
                    "volume_1.xml" => Some(include_str!("../testdata/sitemap-volume_1.xml")),
                    "chapter_1.xml" => Some(include_str!("../testdata/sitemap-chapter_1.xml")),
                    _ => None,
                };
                if let Some(body) = body {
                    return (StatusCode::OK, body).into_response();
                }
                upstream.pages.fetch_add(1, Ordering::SeqCst);
                let code = upstream.status.load(Ordering::SeqCst);
                if code != 0 {
                    return StatusCode::from_u16(code).unwrap().into_response();
                }
                let page = match path.as_str() {
                    "/series/1M9B28BP9B40/blue-exorcist" => {
                        include_str!("../testdata/series-blue-exorcist.html")
                    }
                    "/series/1KJFRS6F0V2G/booty-royale-never-go-down-without-a-fight" => {
                        include_str!("../testdata/series-booty-royale.html")
                    }
                    "/series/0MNCZ95SPE80/goblin-slayer-side-story-year-one" => {
                        include_str!("../testdata/series-goblin-slayer-year-one-manga.html")
                    }
                    "/series/3K8N7ZS3PM40/goblin-slayer-side-story-year-one" => {
                        include_str!("../testdata/series-goblin-slayer-year-one-novel.html")
                    }
                    "/volume/0B08EGZVR4NG" => {
                        include_str!("../testdata/volume-blue-exorcist-5.html")
                    }
                    "/volume/1K4M8FFM0DZG" => {
                        include_str!("../testdata/volume-booty-royale-9-10.html")
                    }
                    "/chapter/3EW93XA7Z120" => {
                        include_str!("../testdata/chapter-goblin-slayer-year-one-51.html")
                    }
                    _ => return StatusCode::NOT_FOUND.into_response(),
                };
                (StatusCode::OK, page).into_response()
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bookwalker-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("catalog.txt");
        let _ = std::fs::remove_file(&file);
        file
    }

    async fn provider_with(
        upstream: Arc<Upstream>,
        base: &str,
        auth_token: Option<&str>,
        file: Option<PathBuf>,
        clock: fn() -> u64,
    ) -> String {
        let client = Arc::new(Client::new(base, base, "test", Duration::ZERO));
        let store = Arc::new(Store::new(client.clone(), file, clock));
        store.load_saved();
        let pages = Arc::new(Pages::new(client, Duration::from_secs(3600)));
        let _ = upstream;
        let cfg = Config {
            auth_token: auth_token.map(str::to_string),
            public_url: Some("http://localhost:9999".to_string()),
        };
        serve(routes(Arc::new(cfg), store, pages)).await
    }

    async fn provider(auth_token: Option<&str>) -> (String, Arc<Upstream>) {
        let upstream = Arc::new(Upstream::default());
        let base = fake_bookwalker(upstream.clone()).await;
        let url = provider_with(
            upstream.clone(),
            &base,
            auth_token,
            None,
            crate::catalog::today,
        )
        .await;
        (url, upstream)
    }

    async fn get_json(url: &str, auth: Option<&str>) -> (u16, serde_json::Value) {
        let mut req = reqwest::Client::new().get(url);
        if let Some(auth) = auth {
            req = req.header("Authorization", auth);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn search(base: &str, query: &[(&str, &str)]) -> (u16, serde_json::Value) {
        let url = reqwest::Url::parse_with_params(&format!("{base}/search"), query).unwrap();
        get_json(url.as_str(), None).await
    }

    #[tokio::test]
    async fn a_range_of_volumes_is_one_omnibus() {
        let (base, _) = provider(None).await;
        let (code, body) = search(
            &base,
            &[
                (
                    "query",
                    "Booty Royale: Never Go Down Without a Fight! - Vol. 9-10",
                ),
                ("mediaType", "comic"),
            ],
        )
        .await;
        assert_eq!(code, 200);
        let first = &body["matches"][0];
        assert_eq!(first["providerId"], "v-1K4M8FFM0DZG_vol9-10");
        assert_eq!(
            first["title"],
            "Booty Royale: Never Go Down Without a Fight! - Vol. 9-10"
        );
        assert_eq!(first["series"][0]["sequence"], "9");
        assert_eq!(first["isbn13"], "9781685799922");
        assert_eq!(first["mediaType"], "comic");
        assert_eq!(first["genres"][0], "Yuri");
        assert!(first.get("cover").is_none(), "a hidden cover is left out");
    }

    #[tokio::test]
    async fn a_chapter_of_a_series_sold_in_volumes_takes_its_volume() {
        let (base, _) = provider(None).await;
        let (_, body) = search(&base, &[("query", "Blue Exorcist v05c38")]).await;
        let first = &body["matches"][0];
        assert_eq!(first["providerId"], "v-0B08EGZVR4NG_vol5v38");
        assert_eq!(first["title"], "Blue Exorcist - Vol. 5 Ch. 38");
        assert_eq!(first["series"][0]["sequence"], "38");
        assert_eq!(first["publisher"], "VIZ Media");
        // Without a volume, the series itself says which chapter it is.
        let (_, body) = search(&base, &[("query", "Blue Exorcist c05")]).await;
        let first = &body["matches"][0];
        assert_eq!(first["providerId"], "s-1M9B28BP9B40_v5");
        assert_eq!(first["title"], "Blue Exorcist - Ch. 5");
        assert_eq!(first["series"][0]["sequence"], "5");
    }

    #[tokio::test]
    async fn a_chapter_named_by_its_folder_finds_the_manga_not_the_novel() {
        let (base, _) = provider(None).await;
        let (code, body) = search(
            &base,
            &[
                ("query", "c051"),
                ("series", "Goblin Slayer Side Story: Year One"),
                ("mediaType", "comic"),
            ],
        )
        .await;
        assert_eq!(code, 200);
        let first = &body["matches"][0];
        assert_eq!(first["providerId"], "c-3EW93XA7Z120_v51");
        // BookWalker also sells the novel of this name, so the manga is told apart by name.
        assert_eq!(
            first["title"],
            "Goblin Slayer Side Story: Year One (Manga) - Ch. 51"
        );
        assert_eq!(
            first["series"][0]["series"],
            "Goblin Slayer Side Story: Year One (Manga)"
        );
        assert_eq!(
            first["displayTitle"],
            "Goblin Slayer Side Story: Year One - Ch. 51"
        );
        assert_eq!(first["mediaType"], "comic");
        assert_eq!(first["authors"][0], "Kumo Kagyu");
        // The novel of the same name comes after, as a series record.
        assert_eq!(body["matches"][1]["providerId"], "s-3K8N7ZS3PM40_v51");
        assert_eq!(body["matches"][1]["mediaType"], "book");
        assert_eq!(
            body["matches"][1]["title"],
            "Goblin Slayer Side Story: Year One (Novel) - Ch. 51"
        );
        assert_eq!(
            body["matches"][1]["series"][0]["series"],
            "Goblin Slayer Side Story: Year One (Novel)"
        );
        assert_eq!(
            body["matches"][1]["displayTitle"],
            "Goblin Slayer Side Story: Year One - Ch. 51"
        );
    }

    #[tokio::test]
    async fn a_manga_and_its_novel_both_come_back_labelled() {
        let (base, _) = provider(None).await;
        let (_, body) = search(
            &base,
            &[
                ("query", "Goblin Slayer Side Story Year One Vol. 1"),
                ("mediaType", "book"),
            ],
        )
        .await;
        let mut kinds: Vec<&str> = body["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["mediaType"].as_str().unwrap())
            .collect();
        kinds.sort_unstable();
        assert_eq!(kinds, ["book", "comic"]);
    }

    #[tokio::test]
    async fn an_unknown_title_answers_no_matches() {
        let (base, _) = provider(None).await;
        let (code, body) = search(&base, &[("query", "Thigh High: Reiwa Hanamaru Academy")]).await;
        assert_eq!((code, body["matches"].as_array().unwrap().len()), (200, 0));
    }

    #[tokio::test]
    async fn lookups_answer_what_the_search_titled() {
        let (base, _) = provider(None).await;
        for (id, answered, title, sequence) in [
            (
                "v-0B08EGZVR4NG_vol5",
                "v-0B08EGZVR4NG_vol5",
                "Blue Exorcist - Vol. 5",
                "5",
            ),
            // An id without a label takes the page's own.
            (
                "v-0B08EGZVR4NG",
                "v-0B08EGZVR4NG_vol5",
                "Blue Exorcist - Vol. 5",
                "5",
            ),
            (
                "v-1K4M8FFM0DZG_vol9-10",
                "v-1K4M8FFM0DZG_vol9-10",
                "Booty Royale: Never Go Down Without a Fight! - Vol. 9-10",
                "9",
            ),
            (
                "c-3EW93XA7Z120_vol4v51",
                "c-3EW93XA7Z120_vol4v51",
                "Goblin Slayer Side Story: Year One (Manga) - Vol. 4 Ch. 51",
                "51",
            ),
            (
                "s-1M9B28BP9B40_v5",
                "s-1M9B28BP9B40_v5",
                "Blue Exorcist - Ch. 5",
                "5",
            ),
        ] {
            let (code, body) = get_json(&format!("{base}/book/{id}"), None).await;
            assert_eq!(code, 200, "{id}");
            assert_eq!(body["providerId"], answered, "{id}");
            assert_eq!(
                (
                    body["title"].as_str(),
                    body["series"][0]["sequence"].as_str()
                ),
                (Some(title), Some(sequence)),
                "{id}"
            );
        }
        for id in [
            "v-ZZZZZZZZZZZZ",
            "s-ZZZZZZZZZZZZ",
            "abc",
            "v-0B08EGZVR4NG_x",
        ] {
            let (code, _) = get_json(&format!("{base}/book/{id}"), None).await;
            assert_eq!(code, 404, "{id}");
        }
    }

    static DAY: AtomicU64 = AtomicU64::new(100);

    fn test_day() -> u64 {
        DAY.load(Ordering::SeqCst)
    }

    #[tokio::test]
    async fn the_catalog_is_saved_and_refreshed_on_the_first_search_of_a_later_day() {
        let upstream = Arc::new(Upstream::default());
        let base = fake_bookwalker(upstream.clone()).await;
        let file = temp_file("refresh");
        let query = [("query", "Blue Exorcist Vol. 5")];

        // Nothing is downloaded at start; the first search waits for the download.
        let first =
            provider_with(upstream.clone(), &base, None, Some(file.clone()), test_day).await;
        assert_eq!(upstream.index.load(Ordering::SeqCst), 0);
        let (code, _) = search(&first, &query).await;
        assert_eq!(code, 200);
        assert_eq!(upstream.index.load(Ordering::SeqCst), 1);
        assert!(std::fs::read_to_string(&file).unwrap().contains("day 100"));

        // A restart the same day reads the saved list and downloads nothing.
        let again =
            provider_with(upstream.clone(), &base, None, Some(file.clone()), test_day).await;
        search(&again, &query).await;
        assert_eq!(upstream.index.load(Ordering::SeqCst), 1);

        // The next day, the search is answered from the saved list and a fresh one comes behind it.
        DAY.store(101, Ordering::SeqCst);
        let (code, body) = search(&again, &query).await;
        assert_eq!(
            (code, body["matches"][0]["title"].as_str()),
            (200, Some("Blue Exorcist - Vol. 5"))
        );
        for _ in 0..50 {
            if std::fs::read_to_string(&file).unwrap().contains("day 101") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(upstream.index.load(Ordering::SeqCst), 2);
        assert!(std::fs::read_to_string(&file).unwrap().contains("day 101"));
        // Once refreshed, further searches that day download nothing.
        search(&again, &query).await;
        assert_eq!(upstream.index.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn refresh_can_be_asked_for() {
        let (base, upstream) = provider(None).await;
        let res = reqwest::Client::new()
            .post(format!("{base}/catalog/refresh"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 202);
        for _ in 0..50 {
            if upstream.index.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(upstream.index.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn info_auth_and_errors() {
        let (base, upstream) = provider(Some("secret")).await;
        let (code, body) = get_json(&format!("{base}/provider/info"), None).await;
        assert_eq!((code, body["key"].as_str()), (200, Some("bookwalker")));
        assert_eq!(body["authenticationRequired"], true);
        assert_eq!(body["color"], "#edb45e");
        let (code, _) = get_json(&format!("{base}/search?query=x"), None).await;
        assert_eq!(code, 401);
        let (code, _) = get_json(&format!("{base}/search"), Some("secret")).await;
        assert_eq!(code, 400);
        let (code, body) = get_json(
            &format!("{base}/search?query=x&mediaType=audiobook"),
            Some("secret"),
        )
        .await;
        assert_eq!(
            (code, body["matches"].as_array().map(Vec::len)),
            (200, Some(0))
        );
        // A blocked or failing site is an error, never "no results".
        upstream.status.store(403, Ordering::SeqCst);
        let (code, _) = get_json(
            &format!("{base}/search?query=Blue%20Exorcist%20v5"),
            Some("secret"),
        )
        .await;
        assert_eq!(code, 502);
        upstream.status.store(429, Ordering::SeqCst);
        let (code, _) = get_json(&format!("{base}/book/v-0B08EGZVR4NG"), Some("secret")).await;
        assert_eq!(code, 429);
    }

    #[tokio::test]
    async fn open_sends_an_id_to_its_page_on_bookwalker() {
        let (base, _) = provider(Some("secret")).await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        // A link opens in a browser, which has no token: /open answers without one.
        for (id, to) in [
            (
                "v-1K4M8FFM0DZG_vol9-10",
                "https://bookwalker.com/volume/1K4M8FFM0DZG",
            ),
            (
                "c-3EW93XA7Z120_v51",
                "https://bookwalker.com/chapter/3EW93XA7Z120",
            ),
            (
                "s-1M9B28BP9B40_v5",
                "https://bookwalker.com/series/1M9B28BP9B40/blue-exorcist",
            ),
        ] {
            let res = client
                .get(format!("{base}/open/{id}"))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status().as_u16(), 307, "open {id}");
            assert_eq!(res.headers()["location"], to, "open {id}");
        }
        for id in ["s-ZZZZZZZZZZZZ", "abc", "v-0B08EGZVR4NG_x"] {
            let res = client
                .get(format!("{base}/open/{id}"))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status().as_u16(), 404, "open {id}");
        }
        let (_, info) = get_json(&format!("{base}/provider/info"), None).await;
        assert_eq!(info["bookUrlTemplate"], "http://localhost:9999/open/{id}");
    }
}

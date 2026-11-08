use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use regex::Regex;
use serde_json::json;

use crate::mangabaka::{Client, Series, UpstreamError};
use crate::mapping::{
    author_matches, display_title, index_label, map_series, names_exactly, position_label,
    search_queries, shared_words, shares_a_word, split_volume, without_brackets, BookMetadata,
    ONESHOT_LABEL,
};
use crate::{Config, VERSION};

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A series id, then `v` and a volume for one volume of it: `1692`, `1692v3`, `1692v10.5`.
const MAX_ID_LENGTH: usize = 20;
static BOOK_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^([0-9]{1,12})(?:(os)|(?:vol([0-9]{1,4}(?:\.[0-9]{1,2})?))?(?:v([0-9]{1,4}(?:\.[0-9]{1,2})?(?:-[0-9]{1,4}(?:\.[0-9]{1,2})?)?))?)$",
    )
    .expect("valid id pattern")
});

#[derive(Clone)]
struct AppState {
    cfg: Arc<Config>,
    mb: Arc<Client>,
}

pub fn routes(cfg: Arc<Config>, mb: Arc<Client>) -> Router {
    Router::new()
        .route("/provider/info", get(info))
        .route("/search", get(search))
        .route("/book/{id}", get(lookup))
        .route("/open/{id}", get(open))
        .with_state(AppState { cfg, mb })
}

async fn info(State(state): State<AppState>) -> Response {
    let link_template = state
        .cfg
        .public_url
        .as_ref()
        .map(|base| format!("{base}/open/{{id}}"));
    Json(json!({
        "name": "MangaBaka",
        "version": VERSION,
        "avatarUrl": "https://mangabaka.org/images/maomao/180x180.png",
        "specVersion": "1",
        "authenticationRequired": state.cfg.auth_token.is_some(),
        // Proposed extensions.
        "key": "mangabaka",
        "mediaTypes": ["book", "comic"],
        "lookup": true,
        "supportedFields": [
            "title", "description", "cover", "authors", "publisher", "publishedYear", "language",
            "seriesName", "seriesIndex", "genres", "tags", "communityRating"
        ],
        // `series` and `seriesIndex` are what BookOrbit knows of a book from its folder and file
        // number. A BookOrbit that does not send them is unaffected.
        "searchParams": ["limit", "series", "seriesIndex"],
        "idMaxLength": MAX_ID_LENGTH,
        // A volume's id is not a page, so the template points at this provider's own `/open/{id}`,
        // which sends the browser on to the series' page. Only declared when `PUBLIC_URL` says
        // where a browser can reach this provider.
        "bookUrlTemplate": link_template,
        "coverShape": "portrait",
        // MangaBaka's own `--primary`.
        "color": "#e30047",
        // Searches are the scarce call upstream: 30 a minute.
        "rateLimit": { "maxConcurrency": 1, "minIntervalMs": state.cfg.search_interval.as_millis() as u64 },
    }))
    .into_response()
}

async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(denied) = deny_unauthorized(&state, &headers) {
        return denied;
    }
    let query = q.get("query").map(|v| v.trim()).unwrap_or_default();
    if query.is_empty() {
        return error(StatusCode::BAD_REQUEST, "query is required");
    }
    if q.get("mediaType").map(String::as_str) == Some("audiobook") {
        return Json(json!({ "matches": [] })).into_response();
    }
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(DEFAULT_LIMIT, |n| n.clamp(1, MAX_LIMIT));
    // When BookOrbit says which series the book is in (from its folder), that is what is searched
    // for, and the title only says which book of it this is: "chapter 1", "Vol. 2", "Oneshot", or
    // else the book's own series number. Without it, the series is read out of the title.
    let series = q.get("series").map(|v| v.trim()).filter(|v| !v.is_empty());
    let (title_head, title_label) = split_volume(query);
    let volume = match series {
        Some(_) => position_label(query, q.get("seriesIndex").map(|v| v.trim())),
        None => title_label.clone(),
    };

    // What is searched, in order: the series when BookOrbit gave one, then the title, which is only
    // tried when the series found nothing. Each is paired with the name a result has to resemble,
    // which has no "(2010)" or "[group]" in it (see `search_queries` for the wordings tried).
    let mut attempts: Vec<(String, String)> = Vec::new();
    if let Some(series) = series {
        let name = without_brackets(series);
        attempts.extend(search_queries(series).into_iter().map(|wording| (wording, name.clone())));
    }
    // A title that is only "chapter 1" names no series, so there is nothing to fall back to.
    let title_names_a_series = !title_head.is_empty() && !(title_head == query && !title_label.is_empty());
    if series.is_none() || title_names_a_series {
        let name = without_brackets(&title_head);
        let wordings = search_queries(&title_head);
        let kept = if series.is_some() { 2 } else { wordings.len() };
        attempts.extend(wordings.into_iter().take(kept).map(|wording| (wording, name.clone())));
    }
    let mut seen = std::collections::HashSet::new();
    attempts.retain(|(wording, _)| seen.insert(wording.to_lowercase()));

    let (found, title) = match tokio::time::timeout(
        REQUEST_TIMEOUT,
        find_series(&state.mb, &attempts, limit),
    )
    .await
    {
        Ok(Ok(found)) => found,
        Ok(Err(err)) => return upstream_error("search", err),
        Err(_) => return upstream_error("search", UpstreamError::Failed("timed out".into())),
    };

    let author = q.get("author").map(|v| v.trim()).unwrap_or_default();
    let mut active: Vec<&Series> = found.iter().collect();
    // The asked-for author's series first, then one named exactly as asked, then the one sharing
    // the most words with the query, otherwise MangaBaka's own order: "Spy x Family" is the
    // series, not the "Spy x Family: Family Portrait" novel.
    active.sort_by_key(|s| {
        (
            !author_matches(s, author),
            !names_exactly(s, &title),
            std::cmp::Reverse(shared_words(s, &title)),
        )
    });
    let matches: Vec<BookMetadata> = active
        .into_iter()
        .map(|s| {
            let mut book = map_series(s, &volume);
            book.display_title = display_title(s, &title, query, &volume);
            book
        })
        .collect();
    Json(json!({ "matches": matches })).into_response()
}

/// Searches MangaBaka with each wording of `title` in turn and returns the active series of the
/// first answer that has any worth keeping. A search can be asked in any language, but one that
/// found nothing is retried shorter, since MangaBaka answers nothing to a query carrying words
/// none of the series' names have.
async fn find_series(
    mb: &Client,
    attempts: &[(String, String)],
    limit: usize,
) -> Result<(Vec<Series>, String), UpstreamError> {
    for (wording, name) in attempts {
        let found = mb.search_series(wording, limit).await?;
        let usable: Vec<Series> = found
            .into_iter()
            .filter(|s| s.state == "active" && shares_a_word(s, name))
            .collect();
        if !usable.is_empty() {
            // The name that found them is what they are ordered by.
            return Ok((usable, name.clone()));
        }
    }
    Ok((Vec::new(), String::new()))
}

async fn lookup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    if let Some(denied) = deny_unauthorized(&state, &headers) {
        return denied;
    }
    let Some(captures) = BOOK_ID.captures(&raw) else {
        return error(StatusCode::NOT_FOUND, "Not found");
    };
    let Ok(id) = captures[1].parse::<u64>() else {
        return error(StatusCode::NOT_FOUND, "Not found");
    };
    // The id says which volume and chapter it is, so the title and the series index come out the
    // same as the search wrote them: "vol2v8" is "Vol. 2 Ch. 8", "v8" is "Ch. 8".
    let volume = if captures.get(2).is_some() {
        ONESHOT_LABEL.to_string()
    } else {
        index_label(
            captures.get(3).map_or("", |m| m.as_str()),
            captures.get(4).map_or("", |m| m.as_str()),
        )
    };

    let result = tokio::time::timeout(REQUEST_TIMEOUT, series_by_id(&state.mb, id))
        .await
        .unwrap_or_else(|_| Err(UpstreamError::Failed("timed out".into())));

    match result {
        Ok(found) if found.state == "active" => Json(map_series(&found, &volume)).into_response(),
        Ok(_) | Err(UpstreamError::NotFound) => error(StatusCode::NOT_FOUND, "Not found"),
        Err(err) => upstream_error("lookup", err),
    }
}

/// A series by id. One merged into another answers with the one it now lives in, once.
async fn series_by_id(mb: &Client, id: u64) -> Result<Series, UpstreamError> {
    let found = mb.series(id).await?;
    match (found.state.as_str(), found.merged_with) {
        ("merged", Some(target)) => mb.series(target).await,
        _ => Ok(found),
    }
}

/// Sends a browser to the MangaBaka page of the series an id belongs to (`1692v3` and `1692` both
/// go to series 1692). It is what a provider link opens, so it answers without the auth token a
/// browser cannot send, and it only ever redirects to mangabaka.org.
async fn open(State(state): State<AppState>, Path(raw): Path<String>) -> Response {
    let Some(captures) = BOOK_ID.captures(&raw) else {
        return error(StatusCode::NOT_FOUND, "Not found");
    };
    let Ok(id) = captures[1].parse::<u64>() else {
        return error(StatusCode::NOT_FOUND, "Not found");
    };

    let result = tokio::time::timeout(REQUEST_TIMEOUT, series_by_id(&state.mb, id))
        .await
        .unwrap_or_else(|_| Err(UpstreamError::Failed("timed out".into())));

    match result {
        Ok(found) if found.state == "active" => {
            match found
                .canonical_url
                .as_deref()
                .filter(|url| url.starts_with("https://mangabaka.org/"))
            {
                Some(url) => Redirect::temporary(url).into_response(),
                None => error(StatusCode::NOT_FOUND, "Not found"),
            }
        }
        Ok(_) | Err(UpstreamError::NotFound) => error(StatusCode::NOT_FOUND, "Not found"),
        Err(err) => upstream_error("open", err),
    }
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
            eprintln!("[provider.{op}] [fail] error={message:?} - mangabaka request failed");
            error(StatusCode::BAD_GATEWAY, "mangabaka unavailable")
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
    use std::sync::atomic::{AtomicU16, Ordering};

    use super::*;

    /// Answers like api.mangabaka.org from the captured fixtures.
    async fn fake_mangabaka(status: Arc<AtomicU16>) -> String {
        let app = Router::new()
            .route(
                "/v1/series/search",
                get({
                    let status = status.clone();
                    move |Query(q): Query<HashMap<String, String>>| async move {
                        fixture(
                            &status,
                            if q.get("q").map(String::as_str) == Some("Berserk") {
                                include_str!("../testdata/search-berserk.json")
                            } else {
                                include_str!("../testdata/search-empty.json")
                            },
                        )
                    }
                }),
            )
            .route(
                "/v1/series/{id}",
                get(move |Path(id): Path<String>| async move {
                    match id.as_str() {
                        "1692" => fixture(&status, include_str!("../testdata/series-1692.json")),
                        "999001" => fixture(
                            &status,
                            include_str!("../testdata/series-999001-merged.json"),
                        ),
                        _ => (
                            StatusCode::NOT_FOUND,
                            include_str!("../testdata/not-found.json"),
                        )
                            .into_response(),
                    }
                }),
            );
        serve(app).await
    }

    fn fixture(status: &AtomicU16, body: &'static str) -> Response {
        match status.load(Ordering::SeqCst) {
            0 => (StatusCode::OK, body).into_response(),
            code => (
                StatusCode::from_u16(code).unwrap(),
                [(header::RETRY_AFTER, "42")],
                "",
            )
                .into_response(),
        }
    }

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    async fn provider(auth_token: Option<&str>) -> (String, Arc<AtomicU16>) {
        let status = Arc::new(AtomicU16::new(0));
        let upstream = fake_mangabaka(status.clone()).await;
        let cfg = Config {
            auth_token: auth_token.map(str::to_string),
            public_url: Some("http://localhost:9999".to_string()),
            search_interval: Duration::from_secs(2),
            lookup_interval: Duration::ZERO,
        };
        let client = Client::new(&upstream, "test", Duration::ZERO, Duration::ZERO);
        (serve(routes(Arc::new(cfg), Arc::new(client))).await, status)
    }

    async fn get_json(url: &str, auth: Option<&str>) -> (u16, serde_json::Value, HeaderMap) {
        let mut req = reqwest::Client::new().get(url);
        if let Some(auth) = auth {
            req = req.header("Authorization", auth);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let headers = res.headers().clone();
        let body =
            serde_json::from_slice(&res.bytes().await.unwrap()).unwrap_or(serde_json::Value::Null);
        (status, body, headers)
    }

    #[tokio::test]
    async fn info_declares_the_contract() {
        let (base, _) = provider(None).await;
        let (code, body, _) = get_json(&format!("{base}/provider/info"), None).await;
        assert_eq!(code, 200);
        assert_eq!(
            (
                body["key"].as_str(),
                body["lookup"].as_bool(),
                body["authenticationRequired"].as_bool()
            ),
            (Some("mangabaka"), Some(true), Some(false))
        );
        assert_eq!(body["rateLimit"]["minIntervalMs"], 2000);
        // BookOrbit lists a provider for a field rule only when it declares the field, so tags
        // have to be here for the Tags rule to use this provider.
        let fields: Vec<&str> = body["supportedFields"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|field| field.as_str())
            .collect();
        assert!(fields.contains(&"genres") && fields.contains(&"tags"));
        assert_eq!(
            (body["avatarUrl"].as_str(), body["color"].as_str()),
            (
                Some("https://mangabaka.org/images/maomao/180x180.png"),
                Some("#e30047")
            )
        );
    }

    #[tokio::test]
    async fn open_sends_a_volume_id_to_its_series_page() {
        let (base, _) = provider(None).await;
        // Keep the redirect itself: following it would leave the test for mangabaka.org.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for id in ["1692v3", "1692", "999001", "1692v0-6", "1692os"] {
            let res = client.get(format!("{base}/open/{id}")).send().await.unwrap();
            assert_eq!(res.status().as_u16(), 307, "open {id}");
            assert_eq!(
                res.headers()["location"],
                "https://mangabaka.org/manga/1692/BERSERK",
                "open {id}"
            );
        }
        for id in ["abc", "1692v", "5"] {
            let res = client.get(format!("{base}/open/{id}")).send().await.unwrap();
            assert_eq!(res.status().as_u16(), 404, "open {id}");
        }
        // The link template BookOrbit stores points at it.
        let (_, info, _) = get_json(&format!("{base}/provider/info"), None).await;
        assert_eq!(info["bookUrlTemplate"], "http://localhost:9999/open/{id}");
    }

    #[tokio::test]
    async fn search_strips_the_volume_and_puts_it_on_the_series() {
        let (base, _) = provider(None).await;
        let (code, body, _) = get_json(
            &format!("{base}/search?query=Berserk+Vol.+3&mediaType=book"),
            None,
        )
        .await;
        assert_eq!(code, 200);
        let matches = body["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 5);
        assert_eq!(matches[0]["providerId"], "1692vol3");
        assert_eq!(matches[0]["series"][0]["sequence"], "3");
    }

    #[tokio::test]
    async fn search_uses_the_series_from_bookorbit_when_the_title_is_bare() {
        let (base, _) = provider(None).await;
        // The file is only "chapter 9" in the folder "Berserk"; the fake answers only to "Berserk".
        let (code, body, _) = get_json(
            &format!("{base}/search?query=chapter+9&series=Berserk&seriesIndex=9&mediaType=comic"),
            None,
        )
        .await;
        assert_eq!(code, 200);
        let matches = body["matches"].as_array().unwrap();
        assert_eq!(matches[0]["title"], "Berserk - Ch. 9");
        assert_eq!(matches[0]["providerId"], "1692v9");
        assert_eq!(matches[0]["series"][0]["sequence"], "9");
        // Shown to BookOrbit's relevance check as a title that holds the query.
        assert_eq!(matches[0]["displayTitle"], "Berserk - chapter 9");

        // A title with no number at all takes it from the series number; a oneshot stays one.
        let (_, body, _) = get_json(
            &format!("{base}/search?query=Extras&series=Berserk&seriesIndex=12&mediaType=comic"),
            None,
        )
        .await;
        assert_eq!(body["matches"][0]["title"], "Berserk - Ch. 12");
        let (_, body, _) = get_json(
            &format!("{base}/search?query=Oneshot&series=Berserk&mediaType=comic"),
            None,
        )
        .await;
        assert_eq!(body["matches"][0]["title"], "Berserk - Oneshot");
    }

    #[tokio::test]
    async fn search_tries_the_series_first_and_the_title_after() {
        let (base, _) = provider(None).await;
        // The series finds nothing at MangaBaka (the fake answers only to "Berserk"), so the
        // title is what is searched, and it still gives its chapter.
        let (_, body, _) = get_json(
            &format!("{base}/search?query=Berserk+Ch.+9&series=Nonexistent&mediaType=comic"),
            None,
        )
        .await;
        assert_eq!(body["matches"][0]["title"], "Berserk - Ch. 9");

        // A title that is only "chapter 9" names no series: there is nothing to fall back to.
        let (code, body, _) = get_json(
            &format!("{base}/search?query=chapter+9&series=Nonexistent&mediaType=comic"),
            None,
        )
        .await;
        assert_eq!(code, 200);
        assert!(body["matches"].as_array().unwrap().is_empty());

        // The series wins over a title that names something else.
        let (_, body, _) = get_json(
            &format!("{base}/search?query=Something+Else+Ch.+4&series=Berserk&mediaType=comic"),
            None,
        )
        .await;
        assert_eq!(body["matches"][0]["title"], "Berserk - Ch. 4");
    }

    #[tokio::test]
    async fn search_keeps_a_oneshot_in_the_title() {
        let (base, _) = provider(None).await;
        for query in ["Berserk+Oneshot", "Berserk+-+One-Shot+(en)"] {
            let (code, body, _) = get_json(
                &format!("{base}/search?query={query}&mediaType=comic"),
                None,
            )
            .await;
            assert_eq!(code, 200, "{query}");
            let matches = body["matches"].as_array().unwrap();
            assert_eq!(matches[0]["title"], "Berserk - Oneshot", "{query}");
            assert_eq!(matches[0]["providerId"], "1692os", "{query}");
            assert!(matches[0]["series"][0].get("sequence").is_none(), "{query}");
        }
    }

    #[tokio::test]
    async fn search_keeps_a_range_of_chapters_in_the_title() {
        let (base, _) = provider(None).await;
        let (code, body, _) = get_json(
            &format!("{base}/search?query=Berserk+c000-006&mediaType=comic"),
            None,
        )
        .await;
        assert_eq!(code, 200);
        let matches = body["matches"].as_array().unwrap();
        assert_eq!(matches[0]["title"], "Berserk - Ch. 0-6");
        assert_eq!(matches[0]["providerId"], "1692v0-6");
        assert_eq!(matches[0]["series"][0]["sequence"], "0");
    }

    #[tokio::test]
    async fn search_leaves_the_brackets_of_a_file_name_out() {
        let (base, _) = provider(None).await;
        // The fake answers only to "Berserk": the year, the language and the group are noise.
        for query in [
            "Berserk+(2010)",
            "Berserk+(en)",
            "Berserk+(2010)+(en)+%5BGroup%5D",
            "Berserk+(2010)+-+Ch.+9",
        ] {
            let (code, body, _) = get_json(
                &format!("{base}/search?query={query}&mediaType=comic"),
                None,
            )
            .await;
            assert_eq!(code, 200, "{query}");
            let matches = body["matches"].as_array().unwrap();
            assert!(!matches.is_empty(), "{query}");
            assert!(matches[0]["title"].as_str().unwrap().starts_with("Berserk"), "{query}");
        }
    }

    #[tokio::test]
    async fn search_retries_shorter_when_the_long_query_finds_nothing() {
        let (base, _) = provider(None).await;
        // The fake answers only to "Berserk", so "Berserk the Comic" has to be retried without
        // its edition word, and the chapter still becomes the series position.
        let (code, body, _) = get_json(
            &format!("{base}/search?query=Berserk+the+Comic+Ch.+9&mediaType=comic"),
            None,
        )
        .await;
        assert_eq!(code, 200);
        let matches = body["matches"].as_array().unwrap();
        assert_eq!(matches[0]["providerId"], "1692v9");
        assert_eq!(matches[0]["series"][0]["sequence"], "9");
    }

    #[tokio::test]
    async fn search_puts_the_asked_for_author_first() {
        let (base, _) = provider(None).await;
        let (_, body, _) = get_json(
            &format!("{base}/search?query=Berserk&author=Ichika+Isshiki"),
            None,
        )
        .await;
        assert_eq!(body["matches"][0]["providerId"], "60246");
    }

    #[tokio::test]
    async fn search_answers_empty_and_refuses_no_query() {
        let (base, _) = provider(None).await;
        let (code, body, _) = get_json(&format!("{base}/search?query=nothing"), None).await;
        assert_eq!(
            (code, body["matches"].as_array().map(Vec::len)),
            (200, Some(0))
        );
        let (code, body, _) = get_json(
            &format!("{base}/search?query=Berserk&mediaType=audiobook"),
            None,
        )
        .await;
        assert_eq!(
            (code, body["matches"].as_array().map(Vec::len)),
            (200, Some(0))
        );
        let (code, _, _) = get_json(&format!("{base}/search"), None).await;
        assert_eq!(code, 400);
    }

    #[tokio::test]
    async fn lookup_follows_a_merge_and_answers_404() {
        let (base, _) = provider(None).await;
        let (code, body, _) = get_json(&format!("{base}/book/999001"), None).await;
        assert_eq!((code, body["providerId"].as_str()), (200, Some("1692")));
        // An id answers the volume or chapter it names, titled as the search titled it.
        for (id, title, sequence) in [
            ("1692vol3", "Berserk - Vol. 3", Some("3")),
            ("1692v3", "Berserk - Ch. 3", Some("3")),
            ("1692vol2v11", "Berserk - Vol. 2 Ch. 11", Some("11")),
            ("1692v0-6", "Berserk - Ch. 0-6", Some("0")),
            ("1692vol2v7-9", "Berserk - Vol. 2 Ch. 7-9", Some("7")),
            ("1692os", "Berserk - Oneshot", None),
        ] {
            let (code, body, _) = get_json(&format!("{base}/book/{id}"), None).await;
            assert_eq!(
                (
                    code,
                    body["title"].as_str(),
                    body["series"][0]["sequence"].as_str()
                ),
                (200, Some(title), sequence),
                "lookup {id}"
            );
        }
        for id in ["5", "abc", "..", &"9".repeat(13), "1692v", "1692v3v4", "1692vol", "1692v3vol2", "1692v-6", "1692v0-", "1692osv3", "1692os3", "1692vol2os"] {
            let (code, _, _) = get_json(&format!("{base}/book/{id}"), None).await;
            assert_eq!(code, 404, "lookup {id}");
        }
    }

    #[tokio::test]
    async fn upstream_throttle_and_failure() {
        let (base, status) = provider(None).await;
        status.store(429, Ordering::SeqCst);
        let (code, _, headers) = get_json(&format!("{base}/search?query=Berserk"), None).await;
        assert_eq!(
            (
                code,
                headers.get("retry-after").and_then(|v| v.to_str().ok())
            ),
            (429, Some("42"))
        );
        status.store(500, Ordering::SeqCst);
        let (code, _, _) = get_json(&format!("{base}/book/1692"), None).await;
        assert_eq!(code, 502);
    }

    #[tokio::test]
    async fn auth_token() {
        let (base, _) = provider(Some("Bearer s3cret")).await;
        assert_eq!(
            get_json(&format!("{base}/search?query=Berserk"), None)
                .await
                .0,
            401
        );
        assert_eq!(
            get_json(
                &format!("{base}/search?query=Berserk"),
                Some("Bearer s3cret")
            )
            .await
            .0,
            200
        );
        assert_eq!(
            get_json(&format!("{base}/provider/info"), None).await.0,
            200
        );
    }

    #[tokio::test]
    async fn pacer_turns_a_long_queue_into_a_429() {
        let pacer = crate::mangabaka::Pacer::new(Duration::from_secs(10));
        assert!(pacer.wait().await.is_ok());
        pacer.push_back(Duration::from_secs(20));
        assert!(matches!(
            pacer.wait().await,
            Err(UpstreamError::RateLimited { .. })
        ));
    }
}

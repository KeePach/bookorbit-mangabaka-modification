//! Talking to bookwalker.com: pages one at a time with a gap between them, and the parsed pages
//! kept for a while so a library of one series' volumes reads its series page once.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::pages::{parse_item, parse_series, ItemKind, ItemPage, SeriesPage};

/// A series page is about 850 KB; a sitemap file a few MB.
const MAX_BODY_BYTES: usize = 24 << 20;
/// Longer than this in the pacer's queue and the caller is told to come back later instead.
const MAX_QUEUE_WAIT: Duration = Duration::from_secs(15);
const MAX_CACHED_SERIES: usize = 64;
const MAX_CACHED_ITEMS: usize = 256;

#[derive(Debug)]
pub enum UpstreamError {
    NotFound,
    RateLimited { retry_after_seconds: u64 },
    Failed(String),
}

impl UpstreamError {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound => "not found".into(),
            Self::RateLimited {
                retry_after_seconds,
            } => format!("rate limited for {retry_after_seconds}s"),
            Self::Failed(message) => message.clone(),
        }
    }
}

pub struct Client {
    base: String,
    /// Where the sitemap index may send the provider: bookwalker.com's own static host.
    static_base: String,
    http: reqwest::Client,
    pacer: Pacer,
}

impl Client {
    pub fn new(base: &str, static_base: &str, user_agent: &str, interval: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(user_agent)
            .build()
            .expect("could not build the HTTP client");
        Self {
            base: base.trim_end_matches('/').to_string(),
            static_base: static_base.trim_end_matches('/').to_string(),
            http,
            pacer: Pacer::new(interval),
        }
    }

    pub fn interval(&self) -> Duration {
        self.pacer.interval
    }

    pub async fn sitemap_index(&self) -> Result<String, UpstreamError> {
        self.pacer.wait().await?;
        self.get(&format!("{}/sitemap.xml", self.base)).await
    }

    /// One file the sitemap index lists. Only bookwalker.com's own hosts are followed.
    pub async fn sitemap_file(&self, url: &str) -> Result<String, UpstreamError> {
        let allowed = [&self.static_base, &self.base]
            .iter()
            .any(|base| url.starts_with(&format!("{base}/")));
        if !allowed {
            return Err(UpstreamError::Failed(format!(
                "sitemap index points outside bookwalker.com: {url}"
            )));
        }
        // The static host is a CDN, not the site, so these go back to back.
        self.get(url).await
    }

    /// `/series/{id}/{slug}`: the slug is part of the address the site answers in full.
    pub async fn series_page(&self, id: &str, slug: &str) -> Result<String, UpstreamError> {
        self.pacer.wait().await?;
        self.get(&format!(
            "{}/series/{}/{}",
            self.base,
            safe(id)?,
            safe(slug)?
        ))
        .await
    }

    /// `/volume/{id}` and `/chapter/{id}` redirect to the address with the slug.
    pub async fn item_page(&self, kind: ItemKind, id: &str) -> Result<String, UpstreamError> {
        self.pacer.wait().await?;
        self.get(&format!("{}/{}/{}", self.base, kind.path(), safe(id)?))
            .await
    }

    async fn get(&self, url: &str) -> Result<String, UpstreamError> {
        let mut res = self
            .http
            .get(url)
            .header("Accept", "text/html,application/xml;q=0.9,*/*;q=0.8")
            .send()
            .await
            .map_err(|error| UpstreamError::Failed(error.to_string()))?;
        match res.status().as_u16() {
            404 | 410 => return Err(UpstreamError::NotFound),
            429 => {
                let retry_after_seconds = res
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .filter(|seconds| *seconds > 0)
                    .unwrap_or(60);
                return Err(UpstreamError::RateLimited {
                    retry_after_seconds,
                });
            }
            200..=299 => {}
            status => {
                return Err(UpstreamError::Failed(format!(
                    "bookwalker.com answered {status} for {url}"
                )))
            }
        }
        let mut body = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|error| UpstreamError::Failed(error.to_string()))?
        {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(UpstreamError::Failed(
                    "bookwalker.com answer too large".into(),
                ));
            }
            body.extend_from_slice(&chunk);
        }
        String::from_utf8(body)
            .map_err(|_| UpstreamError::Failed("bookwalker.com answer is not UTF-8".into()))
    }
}

/// Ids and slugs go into the request path, so only the site's own shapes are let through.
fn safe(part: &str) -> Result<&str, UpstreamError> {
    let ok = !part.is_empty()
        && part.len() <= 300
        && part.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-'
        });
    ok.then_some(part).ok_or(UpstreamError::NotFound)
}

type Cache<K, V> = Mutex<HashMap<K, (Instant, Arc<V>)>>;

/// Parsed pages, kept for `ttl`.
pub struct Pages {
    client: Arc<Client>,
    ttl: Duration,
    series: Cache<String, SeriesPage>,
    items: Cache<(ItemKind, String), ItemPage>,
}

impl Pages {
    pub fn new(client: Arc<Client>, ttl: Duration) -> Self {
        Self {
            client,
            ttl,
            series: Mutex::new(HashMap::new()),
            items: Mutex::new(HashMap::new()),
        }
    }

    pub fn client(&self) -> &Arc<Client> {
        &self.client
    }

    pub async fn series(&self, id: &str, slug: &str) -> Result<Arc<SeriesPage>, UpstreamError> {
        if let Some(page) = cached(&self.series, &id.to_string(), self.ttl) {
            return Ok(page);
        }
        let html = self.client.series_page(id, slug).await?;
        let page = Arc::new(parse_series(id, slug, &html).ok_or(UpstreamError::NotFound)?);
        remember(
            &self.series,
            id.to_string(),
            page.clone(),
            self.ttl,
            MAX_CACHED_SERIES,
        );
        Ok(page)
    }

    pub async fn item(&self, kind: ItemKind, id: &str) -> Result<Arc<ItemPage>, UpstreamError> {
        let key = (kind, id.to_string());
        if let Some(page) = cached(&self.items, &key, self.ttl) {
            return Ok(page);
        }
        let html = self.client.item_page(kind, id).await?;
        let page = Arc::new(parse_item(kind, id, &html).ok_or(UpstreamError::NotFound)?);
        remember(&self.items, key, page.clone(), self.ttl, MAX_CACHED_ITEMS);
        Ok(page)
    }
}

fn cached<K: Eq + Hash, V>(cache: &Cache<K, V>, key: &K, ttl: Duration) -> Option<Arc<V>> {
    let cache = cache.lock().expect("page cache lock poisoned");
    cache
        .get(key)
        .filter(|(at, _)| at.elapsed() < ttl)
        .map(|(_, page)| page.clone())
}

fn remember<K: Eq + Hash + Clone, V>(
    cache: &Cache<K, V>,
    key: K,
    page: Arc<V>,
    ttl: Duration,
    max: usize,
) {
    let mut cache = cache.lock().expect("page cache lock poisoned");
    cache.retain(|_, (at, _)| at.elapsed() < ttl);
    if cache.len() >= max {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(key, (Instant::now(), page));
}

/// Spaces request starts at least `interval` apart, across all callers.
pub struct Pacer {
    interval: Duration,
    next: Mutex<Instant>,
}

impl Pacer {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            next: Mutex::new(Instant::now()),
        }
    }

    pub async fn wait(&self) -> Result<(), UpstreamError> {
        if self.interval.is_zero() {
            return Ok(());
        }
        let start = {
            let mut next = self.next.lock().expect("pacer lock poisoned");
            let now = Instant::now();
            let start = (*next).max(now);
            let wait = start - now;
            if wait > MAX_QUEUE_WAIT {
                return Err(UpstreamError::RateLimited {
                    retry_after_seconds: wait.as_secs_f64().ceil() as u64,
                });
            }
            *next = start + self.interval;
            start
        };
        tokio::time::sleep_until(tokio::time::Instant::from_std(start)).await;
        Ok(())
    }
}

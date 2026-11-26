mod bookwalker;
mod catalog;
mod label;
mod mapping;
mod pages;
mod server;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub const VERSION: &str = "0.1.2";

pub struct Config {
    pub auth_token: Option<String>,
}

#[tokio::main]
async fn main() {
    let port = env("PORT", "8080");
    let base = env("BOOKWALKER_URL", "https://bookwalker.com");
    let static_base = env("BOOKWALKER_STATIC_URL", "https://static.bookwalker.com");
    let user_agent = env(
        "USER_AGENT",
        &format!("bookorbit-bookwalker-provider/{VERSION}"),
    );
    let data_dir = env("DATA_DIR", "/data");
    let cfg = Config {
        auth_token: std::env::var("AUTH_TOKEN")
            .ok()
            .filter(|token| !token.is_empty()),
    };
    // BookWalker publishes no limit; one page a second stays well under what its own site loads
    // while someone browses.
    let client = Arc::new(bookwalker::Client::new(
        &base,
        &static_base,
        &user_agent,
        env_millis("REQUEST_INTERVAL_MS", 1000),
    ));
    let file = (!data_dir.is_empty()).then(|| PathBuf::from(&data_dir).join("catalog.txt"));
    let store = Arc::new(catalog::Store::new(client.clone(), file, catalog::today));
    // Only the saved list is read now: nothing is downloaded until the first search.
    store.load_saved();
    let pages = Arc::new(bookwalker::Pages::new(
        client,
        Duration::from_secs(60 * env_u64("PAGE_CACHE_MINUTES", 60).max(1)),
    ));
    let app = server::routes(Arc::new(cfg), store, pages);

    let listener = tokio::net::TcpListener::bind((
        "0.0.0.0",
        port.parse::<u16>().expect("PORT must be a port number"),
    ))
    .await
    .expect("could not bind the port");
    eprintln!("[provider] [start] port={port} site={base} version={VERSION} - listening");
    axum::serve(listener, app).await.expect("server stopped");
}

fn env(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn env_millis(key: &str, fallback: u64) -> Duration {
    Duration::from_millis(env_u64(key, fallback))
}

//! Shared test helpers: an in-process Pascal stub, temporary databases, a started desk.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use desk::{Config, Running};

/// Repository root.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The 28 Oct tape's market list, as Pascal's envelope (`{status, data, seq, ...}`).
pub fn tape_markets() -> String {
    let text =
        std::fs::read_to_string(repo_root().join("tapes/nba-2026-10-28/markets_selected.json"))
            .expect("tape markets");
    let v: serde_json::Value = serde_json::from_str(&text).expect("tape json");
    if v.get("status").is_some() {
        text
    } else {
        serde_json::json!({"status": "success", "data": v}).to_string()
    }
}

/// Serves `GET /api/v1/markets` from the tape on 127.0.0.1; everything else is 404.
pub async fn pascal_stub() -> String {
    let body = tape_markets();
    let app = Router::new().route(
        "/api/v1/markets",
        get(move || {
            let body = body.clone();
            async move {
                (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    body,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

/// A fresh database path under the system temp dir (removed by [`TempDb`]'s drop).
pub struct TempDb(pub PathBuf);

impl TempDb {
    pub fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "schedina-desk-{tag}-{}-{}-{}.sqlite",
            std::process::id(),
            desk::now_ms(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        TempDb(p)
    }

    pub fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut p = self.0.clone().into_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Starts a desk against `pascal_url` with a fast heartbeat.
pub async fn start_desk(pascal_url: &str, db: &TempDb) -> Running {
    let mut cfg = Config::local(pascal_url, db.path());
    cfg.heartbeat_every = Duration::from_millis(200);
    cfg.shutdown_grace = Duration::from_secs(5);
    desk::start(cfg).await.expect("desk starts")
}

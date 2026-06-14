//! MagnetBox — a personal, self-hosted "debrid"-style gateway.
//!
//! Paste a magnet or .torrent, the embedded BitTorrent engine (librqbit)
//! downloads it, and you get direct HTTP links to download or **stream** each
//! file (with HTTP Range support, so video seeks before the download finishes).
//!
//! Bound to localhost by design — see README for exposing it safely.

mod auth;
mod downloads;
mod engine;
mod http;
mod metrics;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

/// Default port. Change with the MAGNETBOX_PORT env var.
const DEFAULT_PORT: u16 = 8080;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let download_dir = download_dir();
    std::fs::create_dir_all(&download_dir)
        .with_context(|| format!("could not create downloads dir {}", download_dir.display()))?;

    // Secure cookies must be ON in production (behind HTTPS); OFF for local
    // http dev, or the browser won't send the session cookie.
    let secure_cookie = env_flag("MAGNETBOX_HTTPS");
    let users_path = data_dir().join("users.json");
    let auth = auth::Auth::load(users_path.clone(), secure_cookie)
        .context("failed to initialize auth")?;

    let app = engine::App::new(download_dir.clone(), data_dir().join("settings.json"))
        .await
        .context("failed to start the torrent engine")?;

    let direct = downloads::DirectManager::new(download_dir.join("_links"));
    let host_metrics = metrics::Metrics::start(download_dir.clone());

    // Persist per-user usage periodically (cumulative bandwidth survives restarts).
    {
        let auth = auth.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                auth.save_usage();
            }
        });
    }

    // Retention: hourly, delete torrents/downloads older than the configured age.
    {
        let auth = auth.clone();
        let app = app.clone();
        let direct = direct.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                let days = auth.retention_days();
                if days > 0 {
                    let secs = days as u64 * 86_400;
                    let t = app.remove_expired(secs).await;
                    let d = direct.remove_expired(secs);
                    if t + d > 0 {
                        tracing::info!("retention: removed {t} torrents + {d} downloads older than {days}d");
                    }
                }
            }
        });
    }

    let port: u16 = std::env::var("MAGNETBOX_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    // Default to localhost (put a reverse proxy in front for public access).
    let ip: IpAddr = std::env::var("MAGNETBOX_BIND")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| IpAddr::from([127, 0, 0, 1]));
    let addr = SocketAddr::new(ip, port);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("could not bind {addr} (is the port in use?)"))?;

    println!();
    println!("  ┌─ MagnetBox ───────────────────────────────");
    println!("  │  listening http://{addr}");
    println!("  │  login page /login   (secure cookies: {secure_cookie})");
    println!("  │  downloads  {}", download_dir.display());
    println!("  │  users db   {}", users_path.display());
    println!("  └────────────────────────────────────────────");
    println!();

    axum::serve(listener, http::router(app, auth, direct, host_metrics))
        .await
        .context("http server error")?;
    Ok(())
}

fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MAGNETBOX_DATA") {
        return PathBuf::from(dir);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("magnetbox-data")
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn download_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MAGNETBOX_DIR") {
        return PathBuf::from(dir);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("downloads")
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,librqbit=info,tracing::span=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

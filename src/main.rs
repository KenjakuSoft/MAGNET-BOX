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
mod notify;
mod rss;

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
    let auth =
        auth::Auth::load(users_path.clone(), secure_cookie).context("failed to initialize auth")?;

    let app = engine::App::new(download_dir.clone(), data_dir().join("settings.json"))
        .await
        .context("failed to start the torrent engine")?;

    let direct = downloads::DirectManager::new(download_dir.join("_links"));
    let host_metrics = metrics::Metrics::start(download_dir.clone());
    let rss = rss::RssManager::load(data_dir().join("feeds.json"));

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
                        tracing::info!(
                            "retention: removed {t} torrents + {d} downloads older than {days}d"
                        );
                    }
                }
            }
        });
    }

    // Completion notifications (optional): ping a webhook when items finish.
    if let Ok(notify_url) = std::env::var("MAGNETBOX_NOTIFY_URL") {
        let notify_url = notify_url.trim().to_string();
        if !notify_url.is_empty() {
            tokio::spawn(notifier_loop(notify_url, app.clone(), direct.clone()));
        }
    }

    // RSS auto-download: poll subscribed feeds and grab new matching torrents.
    tokio::spawn(rss_loop(rss.clone(), app.clone()));

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

    let url = format!("http://{addr}");
    // First run: send people to the guided setup wizard instead of a login form.
    let landing = if auth.needs_setup() {
        "/setup"
    } else {
        "/login"
    };
    println!();
    println!("  ┌─ MagnetBox ───────────────────────────────");
    if auth.needs_setup() {
        println!("  │  Welcome! Create your account in the browser:");
        println!("  │     {url}/setup");
        if let Some(code) = auth.setup_token() {
            println!("  │  Setup code (paste it once): {code}");
        }
    } else {
        println!("  │  Open this in your browser:");
        println!("  │     {url}/login");
    }
    println!("  │  downloads  {}", download_dir.display());
    println!("  │  secure cookies: {secure_cookie}");
    println!("  └────────────────────────────────────────────");
    println!("  (keep this window open — closing it stops MagnetBox)");
    println!();

    maybe_open_browser(ip, &format!("{url}{landing}"));

    axum::serve(listener, http::router(app, auth, direct, host_metrics, rss))
        .await
        .context("http server error")?;
    Ok(())
}

/// Poll subscribed RSS feeds and auto-add new matching torrents. The first fetch
/// of each feed only seeds (so subscribing doesn't grab the whole backlog).
async fn rss_loop(rss: rss::RssManager, app: engine::App) {
    use librqbit::AddTorrent;
    let client = reqwest::Client::builder()
        .user_agent("MagnetBox/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();
    loop {
        for feed in rss.list() {
            if !feed.enabled
                || !(feed.url.starts_with("http://") || feed.url.starts_with("https://"))
            {
                continue;
            }
            let xml = match client.get(&feed.url).send().await {
                Ok(r) => r.text().await.unwrap_or_default(),
                Err(e) => {
                    tracing::warn!(feed = %feed.url, error = %e, "rss fetch failed");
                    continue;
                }
            };
            for link in rss.new_links(feed.id, &feed.filter, feed.seeded, &xml) {
                tracing::info!("rss: auto-adding from {}", feed.url);
                app.spawn_add(AddTorrent::from_url(link), false, false);
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    }
}

/// Open the default browser at `url` when launched interactively on localhost
/// (e.g. double-clicking the binary), so a non-technical user lands straight on
/// the app. Skipped for headless/server runs and when `MAGNETBOX_NO_OPEN` is set.
fn maybe_open_browser(ip: IpAddr, url: &str) {
    use std::io::IsTerminal;
    if env_flag("MAGNETBOX_NO_OPEN") || !ip.is_loopback() || !std::io::stdout().is_terminal() {
        return;
    }
    if open_url(url).is_ok() {
        println!("  Opening your browser…");
    }
}

fn open_url(url: &str) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn().map(|_| ())
}

/// Watch for newly-finished torrents and downloads and POST a webhook message.
/// Seeds the "already done" set on the first pass, so it never spams on startup.
async fn notifier_loop(url: String, app: engine::App, direct: downloads::DirectManager) {
    use std::collections::HashSet;
    let client = reqwest::Client::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut first = true;
    loop {
        let mut done: Vec<(String, String)> = Vec::new();
        for t in app.list() {
            if t.finished {
                done.push((format!("t:{}", t.id), t.name));
            }
        }
        for d in direct.list() {
            if d.status == "done" {
                done.push((format!("d:{}", d.id), d.filename));
            }
        }
        for (key, name) in done {
            if seen.insert(key) && !first {
                notify::send(
                    &client,
                    &url,
                    &format!("✅ \"{name}\" finished downloading."),
                )
                .await;
            }
        }
        first = false;
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    }
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

//! Direct-link download manager: fetch ordinary `http(s)` URLs to disk with
//! progress, list them, and serve the results. This is a plain download
//! manager — NOT premium-host unlocking.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use url::Url;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Downloading,
    Done,
    Error,
}

struct Item {
    id: usize,
    url: String,
    filename: String,
    path: PathBuf,
    owner: String,
    created: u64,
    total: Mutex<Option<u64>>,
    downloaded: AtomicU64,
    phase: Mutex<Phase>,
    error: Mutex<Option<String>>,
}

/// Persisted record of a finished download, so the list survives restarts.
#[derive(Serialize, Deserialize)]
struct Record {
    id: usize,
    url: String,
    filename: String,
    created: u64,
    #[serde(default)]
    owner: String,
}

/// JSON view of one direct download for the UI.
#[derive(Serialize)]
pub struct DirectView {
    pub id: usize,
    pub url: String,
    pub filename: String,
    pub owner: String,
    pub total: Option<u64>,
    pub downloaded: u64,
    pub status: String,
    pub error: Option<String>,
    pub created: u64,
}

#[derive(Clone)]
pub struct DirectManager {
    dir: PathBuf,
    client: reqwest::Client,
    items: Arc<Mutex<HashMap<usize, Arc<Item>>>>,
    next_id: Arc<AtomicUsize>,
}

impl DirectManager {
    pub fn new(dir: PathBuf) -> Self {
        // SSRF guard: re-validate the target on every redirect hop so a public
        // URL can't bounce us into the private network / cloud metadata.
        let redirect = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 8 {
                return attempt.error("too many redirects");
            }
            match host_is_public(attempt.url()) {
                true => attempt.follow(),
                false => attempt.stop(),
            }
        });
        let client = reqwest::Client::builder()
            .user_agent("MagnetBox/0.1")
            .redirect(redirect)
            .build()
            .unwrap_or_default();
        let mgr = Self {
            dir,
            client,
            items: Default::default(),
            next_id: Arc::new(AtomicUsize::new(0)),
        };
        mgr.load();
        mgr
    }

    /// Restore finished downloads from `index.json` (best effort).
    fn load(&self) {
        let Ok(data) = std::fs::read(self.dir.join("index.json")) else {
            return;
        };
        let Ok(records) = serde_json::from_slice::<Vec<Record>>(&data) else {
            return;
        };
        let mut items = self.items.lock().unwrap();
        let mut max_id = 0;
        for r in records {
            let path = self.dir.join(r.id.to_string()).join(&r.filename);
            if let Ok(meta) = std::fs::metadata(&path) {
                let len = meta.len();
                items.insert(
                    r.id,
                    Arc::new(Item {
                        id: r.id,
                        url: r.url,
                        filename: r.filename,
                        path,
                        owner: r.owner,
                        created: r.created,
                        total: Mutex::new(Some(len)),
                        downloaded: AtomicU64::new(len),
                        phase: Mutex::new(Phase::Done),
                        error: Mutex::new(None),
                    }),
                );
                max_id = max_id.max(r.id + 1);
            }
        }
        drop(items);
        self.next_id.store(max_id, Ordering::Relaxed);
    }

    /// Write finished downloads to `index.json`.
    fn persist(&self) {
        let records: Vec<Record> = self
            .items
            .lock()
            .unwrap()
            .values()
            .filter(|i| *i.phase.lock().unwrap() == Phase::Done)
            .map(|i| Record {
                id: i.id,
                url: i.url.clone(),
                filename: i.filename.clone(),
                created: i.created,
                owner: i.owner.clone(),
            })
            .collect();
        std::fs::create_dir_all(&self.dir).ok();
        if let Ok(bytes) = serde_json::to_vec_pretty(&records) {
            let _ = std::fs::write(self.dir.join("index.json"), bytes);
        }
    }

    /// Number of a user's downloads currently in progress.
    pub fn active_count(&self, owner: &str) -> usize {
        self.items
            .lock()
            .unwrap()
            .values()
            .filter(|i| i.owner == owner && *i.phase.lock().unwrap() == Phase::Downloading)
            .count()
    }

    /// Queue a direct download in the background; returns its id.
    pub fn add(&self, url: String, owner: String) -> Result<usize> {
        let url = url.trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(anyhow!("only http(s) links are supported"));
        }
        let filename = filename_from_url(&url);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let path = self.dir.join(id.to_string()).join(&filename);

        let item = Arc::new(Item {
            id,
            url: url.clone(),
            filename,
            path,
            owner,
            created: now(),
            total: Mutex::new(None),
            downloaded: AtomicU64::new(0),
            phase: Mutex::new(Phase::Downloading),
            error: Mutex::new(None),
        });
        self.items.lock().unwrap().insert(id, item.clone());

        let client = self.client.clone();
        let mgr = self.clone();
        tokio::spawn(async move {
            match download(&client, &item).await {
                Ok(()) => {
                    *item.phase.lock().unwrap() = Phase::Done;
                    mgr.persist();
                }
                Err(e) => {
                    *item.error.lock().unwrap() = Some(format!("{e:#}"));
                    *item.phase.lock().unwrap() = Phase::Error;
                }
            }
        });
        Ok(id)
    }

    pub fn list(&self) -> Vec<DirectView> {
        let mut v: Vec<DirectView> = self
            .items
            .lock()
            .unwrap()
            .values()
            .map(|i| i.view())
            .collect();
        v.sort_by_key(|x| x.id);
        v
    }

    /// (path, filename) for serving a finished/partial download.
    pub fn path_of(&self, id: usize) -> Option<(PathBuf, String)> {
        self.items
            .lock()
            .unwrap()
            .get(&id)
            .map(|i| (i.path.clone(), i.filename.clone()))
    }

    /// Delete downloads created more than `max_age_secs` ago. Returns the count.
    pub fn remove_expired(&self, max_age_secs: u64) -> usize {
        let now = now();
        let expired: Vec<usize> = self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, i)| now.saturating_sub(i.created) > max_age_secs)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            let _ = self.delete(*id, true);
        }
        expired.len()
    }

    /// Remove all finished downloads (and their files). Returns the count.
    pub fn clear_completed(&self) -> usize {
        let done: Vec<usize> = self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, i)| *i.phase.lock().unwrap() == Phase::Done)
            .map(|(id, _)| *id)
            .collect();
        for id in &done {
            let _ = self.delete(*id, true);
        }
        done.len()
    }

    pub fn delete(&self, id: usize, files: bool) -> Result<()> {
        let removed = self.items.lock().unwrap().remove(&id);
        if removed.is_none() {
            return Err(anyhow!("no such download"));
        }
        if files {
            std::fs::remove_dir_all(self.dir.join(id.to_string())).ok();
        }
        self.persist();
        Ok(())
    }
}

impl Item {
    fn view(&self) -> DirectView {
        let phase = *self.phase.lock().unwrap();
        DirectView {
            id: self.id,
            url: self.url.clone(),
            filename: self.filename.clone(),
            owner: self.owner.clone(),
            total: *self.total.lock().unwrap(),
            downloaded: self.downloaded.load(Ordering::Relaxed),
            status: match phase {
                Phase::Downloading => "downloading",
                Phase::Done => "done",
                Phase::Error => "error",
            }
            .to_string(),
            error: self.error.lock().unwrap().clone(),
            created: self.created,
        }
    }
}

async fn download(client: &reqwest::Client, item: &Item) -> Result<()> {
    // SSRF guard: resolve the host and refuse private/loopback/link-local/etc.
    verify_public(&item.url).await?;

    if let Some(parent) = item.path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("creating download folder")?;
    }
    let resp = client
        .get(&item.url)
        .send()
        .await
        .context("request failed")?
        .error_for_status()
        .context("server returned an error status")?;

    if let Some(len) = resp.content_length() {
        *item.total.lock().unwrap() = Some(len);
    }

    let mut file = tokio::fs::File::create(&item.path)
        .await
        .context("creating output file")?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("download stream error")?;
        file.write_all(&chunk).await.context("write error")?;
        item.downloaded
            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    file.flush().await.ok();
    Ok(())
}

/// Async SSRF check: resolve the URL's host and reject if it maps to any
/// private/loopback/link-local/etc. address (blocks localhost, internal hosts,
/// and cloud metadata like 169.254.169.254).
async fn verify_public(raw: &str) -> Result<()> {
    let url = Url::parse(raw).map_err(|_| anyhow!("invalid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("only http(s) URLs are allowed"));
    }
    let host = url.host_str().ok_or_else(|| anyhow!("URL has no host"))?;
    let port = url.port_or_known_default().unwrap_or(443);

    let mut resolved = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| anyhow!("could not resolve host"))?
        .peekable();
    if resolved.peek().is_none() {
        return Err(anyhow!("could not resolve host"));
    }
    if resolved.any(|addr| !is_global_ip(&addr.ip())) {
        return Err(anyhow!(
            "refusing to fetch a private or internal address"
        ));
    }
    Ok(())
}

/// Synchronous variant used inside the redirect policy (reqwest resolves).
fn host_is_public(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let port = url.port_or_known_default().unwrap_or(443);
    match (host, port).to_socket_addrs() {
        Ok(addrs) => {
            let addrs: Vec<_> = addrs.collect();
            !addrs.is_empty() && addrs.iter().all(|a| is_global_ip(&a.ip()))
        }
        Err(_) => false,
    }
}

fn is_global_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_global_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_global_v4(&mapped);
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80) // link-local fe80::/10
        }
    }
}

fn is_global_v4(v4: &Ipv4Addr) -> bool {
    let o = v4.octets();
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local() // 169.254/16 incl. cloud metadata
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // CGNAT 100.64/10
        || o[0] >= 224) // multicast/reserved
}

fn filename_from_url(url: &str) -> String {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    let last = no_query.rsplit('/').next().unwrap_or("");
    let decoded = percent_decode(last);
    let cleaned: String = decoded
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0'))
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "download.bin".to_string()
    } else {
        cleaned
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: usize, age_secs: u64) -> Arc<Item> {
        Arc::new(Item {
            id,
            url: "http://example.com/f".into(),
            filename: "f".into(),
            path: std::env::temp_dir().join(format!("mb_test_{id}")),
            owner: "u".into(),
            created: now().saturating_sub(age_secs),
            total: Mutex::new(Some(1)),
            downloaded: AtomicU64::new(1),
            phase: Mutex::new(Phase::Done),
            error: Mutex::new(None),
        })
    }

    #[test]
    fn remove_expired_drops_only_old_items() {
        let mgr = DirectManager::new(std::env::temp_dir().join("mb_test_links_x"));
        {
            let mut items = mgr.items.lock().unwrap();
            items.insert(1, item(1, 100)); // 100s old
            items.insert(2, item(2, 10_000)); // ~2.7h old
        }
        // Expire anything older than 1 hour: only id 2 should go.
        let removed = mgr.remove_expired(3600);
        assert_eq!(removed, 1);
        let left: Vec<usize> = mgr.items.lock().unwrap().keys().copied().collect();
        assert_eq!(left, vec![1]);
    }
}

//! Thin wrapper around librqbit's `Session`: add torrents, keep a registry of
//! handles, and project their live state into JSON-friendly views for the UI.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use librqbit::{AddTorrent, AddTorrentOptions, ManagedTorrent, Session};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// `librqbit`'s handle alias isn't re-exported at the crate root, so define it.
type ManagedTorrentHandle = Arc<ManagedTorrent>;

/// Reliable public UDP trackers, added only when the user opts in. Never apply
/// these to private-tracker torrents — extra trackers can get you banned there.
const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.tracker.cl:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://explodie.org:6969/announce",
    "udp://tracker.dler.org:6969/announce",
];

/// Application state shared with every HTTP handler. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct App {
    session: Arc<Session>,
    /// torrent id -> handle. Populated on add and from persisted torrents.
    registry: Arc<Mutex<HashMap<usize, ManagedTorrentHandle>>>,
    /// Number of adds currently resolving (magnet metadata fetch can be slow).
    pending: Arc<AtomicUsize>,
    /// Errors from background adds, drained by the UI via `take_status`.
    errors: Arc<Mutex<Vec<String>>>,
    /// Where global speed limits are persisted.
    settings_path: PathBuf,
    /// torrent id -> unix time added (for retention/auto-expiry).
    added: Arc<Mutex<HashMap<usize, u64>>>,
    added_path: PathBuf,
}

/// Persisted app settings (global speed limits, bytes/sec; `None` = unlimited).
#[derive(Serialize, Deserialize, Default)]
struct Settings {
    download_bps: Option<u32>,
    upload_bps: Option<u32>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// One file within a torrent, as shown in the UI.
#[derive(Serialize)]
pub struct FileView {
    pub index: usize,
    pub name: String,
    pub len: u64,
    pub downloaded: u64,
    /// Whether this file is currently selected for download.
    pub selected: bool,
}

/// A torrent's live state, as shown in the UI.
#[derive(Serialize)]
pub struct TorrentView {
    pub id: usize,
    pub name: String,
    pub info_hash: String,
    pub state: String,
    pub finished: bool,
    pub paused: bool,
    pub error: Option<String>,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub download_mbps: f64,
    pub files: Vec<FileView>,
}

impl App {
    pub async fn new(download_dir: PathBuf, settings_path: PathBuf) -> Result<Self> {
        let session = Session::new(download_dir)
            .await
            .context("Session::new failed")?;

        let registry: Arc<Mutex<HashMap<usize, ManagedTorrentHandle>>> = Default::default();

        // Adopt torrents librqbit restored from its own persistence.
        {
            let reg = registry.clone();
            session.with_torrents(|it| {
                let mut g = reg.lock().unwrap();
                for (_id, handle) in it {
                    g.insert(handle.id(), handle.clone());
                }
            });
        }

        let added_path = settings_path
            .parent()
            .map(|p| p.join("torrent_added.json"))
            .unwrap_or_else(|| PathBuf::from("torrent_added.json"));

        let app = Self {
            session,
            registry,
            pending: Arc::new(AtomicUsize::new(0)),
            errors: Default::default(),
            settings_path,
            added: Default::default(),
            added_path,
        };
        app.load_and_apply_limits();
        app.init_added();
        Ok(app)
    }

    // ---- retention / auto-expiry ----

    /// Load persisted add-times; stamp any adopted torrents we don't know yet.
    fn init_added(&self) {
        let loaded: HashMap<usize, u64> = std::fs::read(&self.added_path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let now = now_secs();
        let ids: Vec<usize> = self.registry.lock().unwrap().keys().copied().collect();
        {
            let mut added = self.added.lock().unwrap();
            *added = loaded;
            for id in ids {
                added.entry(id).or_insert(now);
            }
        }
        self.save_added();
    }

    fn save_added(&self) {
        if let Ok(bytes) = serde_json::to_vec(&*self.added.lock().unwrap()) {
            let _ = std::fs::write(&self.added_path, bytes);
        }
    }

    /// Delete torrents added more than `max_age_secs` ago (with their files).
    pub async fn remove_expired(&self, max_age_secs: u64) -> usize {
        let now = now_secs();
        let expired: Vec<usize> = {
            let added = self.added.lock().unwrap();
            self.registry
                .lock()
                .unwrap()
                .keys()
                .filter(|id| {
                    added
                        .get(id)
                        .map_or(false, |t| now.saturating_sub(*t) > max_age_secs)
                })
                .copied()
                .collect()
        };
        let mut n = 0;
        for id in expired {
            if self.delete(id, true).await.is_ok() {
                n += 1;
            }
        }
        n
    }

    // ---- global speed limits (bytes/sec; None = unlimited) ----

    /// Current download/upload limits, read from the persisted settings file
    /// (the source of truth we write on every change).
    pub fn rate_limits(&self) -> (Option<u32>, Option<u32>) {
        std::fs::read(&self.settings_path)
            .ok()
            .and_then(|d| serde_json::from_slice::<Settings>(&d).ok())
            .map(|s| (s.download_bps, s.upload_bps))
            .unwrap_or((None, None))
    }

    /// Apply new limits live and persist them.
    pub fn set_rate_limits(&self, download_bps: Option<u32>, upload_bps: Option<u32>) {
        self.apply_limits(download_bps, upload_bps);
        let settings = Settings {
            download_bps,
            upload_bps,
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&settings) {
            if let Some(dir) = self.settings_path.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            let _ = std::fs::write(&self.settings_path, bytes);
        }
    }

    fn apply_limits(&self, download_bps: Option<u32>, upload_bps: Option<u32>) {
        self.session
            .ratelimits
            .set_download_bps(download_bps.and_then(NonZeroU32::new));
        self.session
            .ratelimits
            .set_upload_bps(upload_bps.and_then(NonZeroU32::new));
    }

    fn load_and_apply_limits(&self) {
        if let Ok(data) = std::fs::read(&self.settings_path) {
            if let Ok(s) = serde_json::from_slice::<Settings>(&data) {
                self.apply_limits(s.download_bps, s.upload_bps);
            }
        }
    }

    /// Add a magnet/URL or raw `.torrent` in the background.
    ///
    /// Magnet metadata resolves over DHT/trackers and can take a while, so we
    /// never block the HTTP request on it — otherwise a slow or firewalled
    /// swarm looks like a frozen "failure". The UI polls `list()` and
    /// `take_status()` to see the torrent appear or an error surface.
    pub fn spawn_add(&self, source: AddTorrent<'static>, paused: bool, add_trackers: bool) {
        let this = self.clone();
        self.pending.fetch_add(1, Ordering::Relaxed);
        let trackers = add_trackers
            .then(|| DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect());
        tokio::spawn(async move {
            let fut = this.session.add_torrent(
                source,
                Some(AddTorrentOptions {
                    overwrite: true,
                    paused,
                    trackers,
                    ..Default::default()
                }),
            );
            // Bound the wait so an unreachable swarm yields a clear message
            // instead of an endless spinner.
            match tokio::time::timeout(std::time::Duration::from_secs(120), fut).await {
                Ok(Ok(resp)) => match resp.into_handle() {
                    Some(handle) => {
                        let id = handle.id();
                        this.registry.lock().unwrap().insert(id, handle);
                        this.added.lock().unwrap().insert(id, now_secs());
                        this.save_added();
                    }
                    None => this.push_error("torrent was list-only; no download handle".into()),
                },
                Ok(Err(e)) => this.push_error(format!("{e:#}")),
                Err(_) => this.push_error(
                    "Timed out after 120s resolving the torrent. BitTorrent traffic may be \
                     blocked by a firewall, antivirus, or VPN — allow magnetbox.exe through, \
                     or try a magnet with active seeders."
                        .into(),
                ),
            }
            this.pending.fetch_sub(1, Ordering::Relaxed);
        });
    }

    fn push_error(&self, msg: String) {
        self.errors.lock().unwrap().push(msg);
    }

    /// Count of in-flight adds, plus any errors since the last call (drained).
    pub fn take_status(&self) -> (usize, Vec<String>) {
        let pending = self.pending.load(Ordering::Relaxed);
        let errors = std::mem::take(&mut *self.errors.lock().unwrap());
        (pending, errors)
    }

    pub fn get(&self, id: usize) -> Option<ManagedTorrentHandle> {
        self.registry.lock().unwrap().get(&id).cloned()
    }

    /// Choose exactly which files to download. An empty set deselects all.
    pub async fn set_files(&self, id: usize, files: Vec<usize>) -> Result<()> {
        let handle = self.get(id).ok_or_else(|| anyhow!("no such torrent"))?;
        let set: HashSet<usize> = files.into_iter().collect();
        self.session
            .update_only_files(&handle, &set)
            .await
            .context("update_only_files failed")
    }

    pub async fn pause(&self, id: usize) -> Result<()> {
        let handle = self.get(id).ok_or_else(|| anyhow!("no such torrent"))?;
        self.session.pause(&handle).await.context("pause failed")
    }

    pub async fn resume(&self, id: usize) -> Result<()> {
        let handle = self.get(id).ok_or_else(|| anyhow!("no such torrent"))?;
        self.session.unpause(&handle).await.context("resume failed")
    }

    /// Aggregate stats for the admin overview.
    pub fn summary(&self) -> serde_json::Value {
        let handles: Vec<ManagedTorrentHandle> =
            self.registry.lock().unwrap().values().cloned().collect();
        let (mut active, mut paused, mut finished) = (0, 0, 0);
        let (mut downloaded, mut uploaded) = (0u64, 0u64);
        let mut download_mbps = 0.0;
        for h in &handles {
            let s = h.stats();
            if s.finished {
                finished += 1;
            } else {
                active += 1;
            }
            if h.is_paused() {
                paused += 1;
            }
            downloaded += s.progress_bytes;
            uploaded += s.uploaded_bytes;
            if let Some(live) = s.live.as_ref() {
                download_mbps += live.download_speed.mbps;
            }
        }
        json!({
            "torrents": handles.len(),
            "active": active,
            "paused": paused,
            "finished": finished,
            "download_mbps": download_mbps,
            "downloaded_bytes": downloaded,
            "uploaded_bytes": uploaded,
        })
    }

    pub async fn pause_all(&self) {
        let handles: Vec<ManagedTorrentHandle> =
            self.registry.lock().unwrap().values().cloned().collect();
        for h in handles {
            let _ = self.session.pause(&h).await;
        }
    }

    pub async fn resume_all(&self) {
        let handles: Vec<ManagedTorrentHandle> =
            self.registry.lock().unwrap().values().cloned().collect();
        for h in handles {
            let _ = self.session.unpause(&h).await;
        }
    }

    /// Remove a torrent. When `delete_files` is true, its downloaded data is
    /// also erased from disk.
    pub async fn delete(&self, id: usize, delete_files: bool) -> Result<()> {
        self.session
            .delete(librqbit::api::TorrentIdOrHash::Id(id), delete_files)
            .await
            .context("delete failed")?;
        self.registry.lock().unwrap().remove(&id);
        self.added.lock().unwrap().remove(&id);
        self.save_added();
        Ok(())
    }

    pub fn list(&self) -> Vec<TorrentView> {
        let handles: Vec<ManagedTorrentHandle> = {
            self.registry.lock().unwrap().values().cloned().collect()
        };
        let mut out: Vec<TorrentView> = handles.iter().map(|h| view(h)).collect();
        out.sort_by_key(|v| v.id);
        out
    }

    /// The on-disk relative name of a file, if metadata is available.
    pub fn file_name(&self, id: usize, file: usize) -> Option<String> {
        let handle = self.get(id)?;
        handle
            .with_metadata(|m| {
                m.file_infos
                    .get(file)
                    .map(|fi| fi.relative_filename.to_string_lossy().into_owned())
            })
            .ok()
            .flatten()
    }
}

fn view(h: &ManagedTorrentHandle) -> TorrentView {
    let stats = h.stats();
    // None means "all files selected"; otherwise only these indices.
    let only = h.only_files();

    let files = h
        .with_metadata(|m| {
            m.file_infos
                .iter()
                .enumerate()
                .map(|(i, fi)| FileView {
                    index: i,
                    name: fi.relative_filename.to_string_lossy().into_owned(),
                    len: fi.len,
                    downloaded: stats.file_progress.get(i).copied().unwrap_or(0),
                    selected: only.as_ref().map_or(true, |v| v.contains(&i)),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    TorrentView {
        id: h.id(),
        name: h
            .name()
            .unwrap_or_else(|| "(resolving metadata…)".to_string()),
        info_hash: h.info_hash().as_string(),
        state: format!("{:?}", stats.state),
        finished: stats.finished,
        paused: h.is_paused(),
        error: stats.error.clone(),
        progress_bytes: stats.progress_bytes,
        total_bytes: stats.total_bytes,
        download_mbps: stats
            .live
            .as_ref()
            .map(|l| l.download_speed.mbps)
            .unwrap_or(0.0),
        files,
    }
}

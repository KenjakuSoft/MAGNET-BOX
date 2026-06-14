//! Host metrics (CPU / RAM / disk) sampled on a dedicated OS thread so the
//! async runtime is never blocked by sysinfo's synchronous refresh + the CPU
//! sampling interval.

use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sysinfo::{Disks, System};

/// A point-in-time snapshot of host resource usage (all byte counts in bytes).
#[derive(Default, Clone, Serialize)]
pub struct Host {
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
}

/// Cheap-to-clone handle over the latest sampled [`Host`] snapshot.
#[derive(Clone)]
pub struct Metrics {
    inner: Arc<Mutex<Host>>,
}

impl Metrics {
    /// Start the background sampler for the filesystem holding `download_dir`.
    pub fn start(download_dir: PathBuf) -> Self {
        let inner = Arc::new(Mutex::new(Host::default()));
        let handle = inner.clone();

        std::thread::Builder::new()
            .name("metrics".into())
            .spawn(move || {
                let mut sys = System::new();
                loop {
                    // CPU usage needs two refreshes spaced by the minimum interval.
                    sys.refresh_cpu_usage();
                    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                    sys.refresh_cpu_usage();
                    sys.refresh_memory();

                    let cpus = sys.cpus();
                    let cpu = if cpus.is_empty() {
                        0.0
                    } else {
                        cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
                    };
                    let (disk_total, disk_avail) = disk_for(&download_dir);

                    if let Ok(mut h) = handle.lock() {
                        h.cpu = cpu;
                        h.mem_used = sys.used_memory();
                        h.mem_total = sys.total_memory();
                        h.disk_total = disk_total;
                        h.disk_used = disk_total.saturating_sub(disk_avail);
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .ok();

        Self { inner }
    }

    pub fn snapshot(&self) -> Host {
        self.inner.lock().map(|h| h.clone()).unwrap_or_default()
    }
}

/// Returns (total, available) bytes for the filesystem containing `dir`.
fn disk_for(dir: &Path) -> (u64, u64) {
    let target = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let target_s = strip_unc(&target.to_string_lossy().to_lowercase());

    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64, u64)> = None; // (mount len, total, avail)
    let mut largest: (u64, u64) = (0, 0); // fallback: biggest disk (total, avail)

    for d in &disks {
        let total = d.total_space();
        let avail = d.available_space();
        if total > largest.0 {
            largest = (total, avail);
        }
        let mount = strip_unc(&d.mount_point().to_string_lossy().to_lowercase());
        if !mount.is_empty() && target_s.starts_with(&mount) {
            let len = mount.len();
            if best.map_or(true, |(l, _, _)| len > l) {
                best = Some((len, total, avail));
            }
        }
    }

    match best {
        Some((_, total, avail)) => (total, avail),
        None => largest,
    }
}

fn strip_unc(s: &str) -> String {
    s.strip_prefix(r"\\?\").unwrap_or(s).to_string()
}

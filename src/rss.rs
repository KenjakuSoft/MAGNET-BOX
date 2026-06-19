//! RSS auto-download: subscribe to feeds and automatically grab new torrents.
//!
//! The parser is intentionally small and dependency-free — it scans `<item>` /
//! `<entry>` blocks for a `magnet:` link or a `.torrent` enclosure, which covers
//! the overwhelming majority of torrent RSS feeds (RSS 2.0). An optional
//! keyword filter restricts what gets auto-added.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// A subscribed feed.
#[derive(Clone, Serialize, Deserialize)]
pub struct Feed {
    pub id: usize,
    pub url: String,
    /// Case-insensitive keyword the title must contain (empty = match all).
    #[serde(default)]
    pub filter: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Set after the first fetch, so subscribing doesn't grab the whole backlog.
    #[serde(default)]
    pub seeded: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    feeds: Vec<Feed>,
    #[serde(default)]
    seen: Vec<String>,
}

/// Manages subscribed feeds and the set of links already grabbed (persisted).
#[derive(Clone)]
pub struct RssManager {
    path: PathBuf,
    feeds: Arc<Mutex<Vec<Feed>>>,
    seen: Arc<Mutex<HashSet<String>>>,
    next_id: Arc<AtomicUsize>,
}

impl RssManager {
    pub fn load(path: PathBuf) -> Self {
        let p: Persisted = std::fs::read(&path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let next = p.feeds.iter().map(|f| f.id + 1).max().unwrap_or(0);
        Self {
            path,
            feeds: Arc::new(Mutex::new(p.feeds)),
            seen: Arc::new(Mutex::new(p.seen.into_iter().collect())),
            next_id: Arc::new(AtomicUsize::new(next)),
        }
    }

    pub fn list(&self) -> Vec<Feed> {
        self.feeds.lock().unwrap().clone()
    }

    pub fn add(&self, url: String, filter: String) -> usize {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.feeds.lock().unwrap().push(Feed {
            id,
            url: url.trim().to_string(),
            filter: filter.trim().to_string(),
            enabled: true,
            seeded: false,
        });
        self.save();
        id
    }

    pub fn remove(&self, id: usize) {
        self.feeds.lock().unwrap().retain(|f| f.id != id);
        self.save();
    }

    fn mark_seeded(&self, id: usize) {
        if let Some(f) = self.feeds.lock().unwrap().iter_mut().find(|f| f.id == id) {
            f.seeded = true;
        }
        self.save();
    }

    fn is_seen(&self, link: &str) -> bool {
        self.seen.lock().unwrap().contains(link)
    }

    fn mark_seen(&self, link: &str) {
        let mut s = self.seen.lock().unwrap();
        s.insert(link.to_string());
        // Keep the persisted set from growing without bound.
        if s.len() > 5000 {
            let drop: Vec<String> = s.iter().take(s.len() - 5000).cloned().collect();
            for d in drop {
                s.remove(&d);
            }
        }
    }

    fn save(&self) {
        let data = Persisted {
            feeds: self.feeds.lock().unwrap().clone(),
            seen: self.seen.lock().unwrap().iter().cloned().collect(),
        };
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&data) {
            let _ = std::fs::write(&self.path, bytes);
        }
    }

    /// New (unseen, filter-matching) torrent links from one feed's XML. Marks
    /// them seen. On a feed's first fetch it only seeds (returns nothing), so
    /// subscribing never floods the box with the whole backlog.
    pub fn new_links(&self, feed_id: usize, filter: &str, seeded: bool, xml: &str) -> Vec<String> {
        let items = extract_items(xml);
        if !seeded {
            for (_, link) in &items {
                self.mark_seen(link);
            }
            self.mark_seeded(feed_id);
            self.save();
            return Vec::new();
        }
        let f = filter.to_lowercase();
        let mut fresh = Vec::new();
        for (title, link) in items {
            let matches = f.is_empty() || title.to_lowercase().contains(&f);
            if matches && !self.is_seen(&link) {
                self.mark_seen(&link);
                fresh.push(link);
            }
        }
        if !fresh.is_empty() {
            self.save();
        }
        fresh
    }
}

/// Extract `(title, torrent-link)` pairs from RSS/Atom XML.
pub fn extract_items(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut blocks = blocks_of(xml, "item");
    blocks.extend(blocks_of(xml, "entry"));
    for b in blocks {
        if let Some(link) = torrent_link(&b) {
            let title = tag_text(&b, "title").unwrap_or_else(|| "(untitled)".to_string());
            out.push((title, link));
        }
    }
    out
}

/// Inner text of every `<tag ...> ... </tag>` block.
fn blocks_of(xml: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let close = format!("</{tag}>");
    let mut rest = xml;
    while let Some(open) = find_open_tag(rest, tag) {
        let after_open = &rest[open..];
        let Some(gt) = after_open.find('>') else {
            break;
        };
        let body_start = open + gt + 1;
        let Some(end) = rest[body_start..].find(&close) else {
            break;
        };
        out.push(rest[body_start..body_start + end].to_string());
        rest = &rest[body_start + end + close.len()..];
    }
    out
}

/// Position of an opening `<tag` (matching whole tag name, ignoring namespaces).
fn find_open_tag(xml: &str, tag: &str) -> Option<usize> {
    let needle = format!("<{tag}");
    let mut from = 0;
    while let Some(i) = xml[from..].find(&needle) {
        let pos = from + i;
        let after = xml.as_bytes().get(pos + needle.len());
        // Next char must end the tag name (space, >, or /), so <item> matches but
        // <itemfoo> doesn't.
        if matches!(
            after,
            Some(b' ') | Some(b'>') | Some(b'/') | Some(b'\t') | Some(b'\n') | Some(b'\r') | None
        ) {
            return Some(pos);
        }
        from = pos + needle.len();
    }
    None
}

/// Find a magnet link, or an http(s) `.torrent` URL, inside an item block.
fn torrent_link(block: &str) -> Option<String> {
    if let Some(start) = block.find("magnet:?") {
        let rest = &block[start..];
        let end = rest
            .find(['"', '\'', '<', '>', ' ', '\n', '\r', '\t'])
            .unwrap_or(rest.len());
        return Some(decode_entities(&rest[..end]));
    }
    // enclosure url="...torrent" / url='...torrent'
    for q in ['"', '\''] {
        let key = format!("url={q}");
        if let Some(i) = block.find(&key) {
            let rest = &block[i + key.len()..];
            if let Some(end) = rest.find(q) {
                let url = decode_entities(&rest[..end]);
                if url.starts_with("http") && url.to_lowercase().contains(".torrent") {
                    return Some(url);
                }
            }
        }
    }
    // <link>http...torrent</link>
    if let Some(l) = tag_text(block, "link") {
        if l.starts_with("http") && l.to_lowercase().contains(".torrent") {
            return Some(l);
        }
    }
    None
}

/// Text inside the first `<tag>...</tag>`, stripped of CDATA and basic entities.
fn tag_text(block: &str, tag: &str) -> Option<String> {
    let inner = blocks_of(block, tag).into_iter().next()?;
    let inner = inner.trim();
    let inner = inner
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(inner);
    Some(decode_entities(inner.trim()))
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_magnet_and_torrent_items() {
        let xml = r#"
        <rss><channel>
          <item><title><![CDATA[Ubuntu 24.04]]></title>
            <link>magnet:?xt=urn:btih:ABC&amp;dn=ubuntu</link></item>
          <item><title>Debian 12</title>
            <enclosure url="https://t.example/d.torrent" type="application/x-bittorrent"/></item>
          <item><title>No link here</title><description>nothing</description></item>
        </channel></rss>"#;
        let items = extract_items(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "Ubuntu 24.04");
        assert_eq!(items[0].1, "magnet:?xt=urn:btih:ABC&dn=ubuntu"); // &amp; decoded
        assert_eq!(items[1].1, "https://t.example/d.torrent");
    }

    #[test]
    fn filter_and_seeding_via_new_links() {
        let dir = std::env::temp_dir().join("mb_rss_test");
        let _ = std::fs::remove_file(dir.join("feeds.json"));
        let mgr = RssManager::load(dir.join("feeds.json"));
        let id = mgr.add("http://feed".into(), "ubuntu".into());
        let xml = r#"<rss><item><title>Ubuntu ISO</title><link>magnet:?xt=A</link></item>
            <item><title>Fedora ISO</title><link>magnet:?xt=B</link></item></rss>"#;
        // First fetch only seeds — nothing is grabbed.
        assert!(mgr.new_links(id, "ubuntu", false, xml).is_empty());
        // A later fetch with a new matching item returns just that one.
        let xml2 = r#"<rss><item><title>Ubuntu 25 ISO</title><link>magnet:?xt=C</link></item>
            <item><title>Fedora ISO</title><link>magnet:?xt=B</link></item></rss>"#;
        let fresh = mgr.new_links(id, "ubuntu", true, xml2);
        assert_eq!(fresh, vec!["magnet:?xt=C".to_string()]);
    }
}

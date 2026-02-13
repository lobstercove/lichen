// Peer ban list persistence

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
struct BanListData {
    entries: Vec<BanEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BanEntry {
    pub address: String,
    pub score: i64,
    pub banned_until: u64,
}

pub struct PeerBanList {
    path: PathBuf,
    entries: HashMap<SocketAddr, BanEntry>,
}

impl PeerBanList {
    pub fn new(path: PathBuf) -> Self {
        let entries = Self::load_from_path(&path);
        PeerBanList { path, entries }
    }

    fn load_from_path(path: &Path) -> HashMap<SocketAddr, BanEntry> {
        let data = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(_) => return HashMap::new(),
        };

        let parsed: BanListData = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => return HashMap::new(),
        };

        parsed
            .entries
            .into_iter()
            .filter_map(|entry| {
                entry
                    .address
                    .parse::<SocketAddr>()
                    .ok()
                    .map(|addr| (addr, entry))
            })
            .collect()
    }

    fn save(&self) {
        let data = BanListData {
            entries: self.entries.values().cloned().collect(),
        };

        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(json) = serde_json::to_string_pretty(&data) {
            let _ = fs::write(&self.path, json);
        }
    }

    pub fn is_banned(&self, addr: &SocketAddr) -> bool {
        if let Some(entry) = self.entries.get(addr) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            return entry.banned_until > now;
        }
        false
    }

    pub fn record_score(&mut self, addr: SocketAddr, score: i64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let banned_until = if score <= -10 { now + 600 } else { 0 };

        self.entries.insert(
            addr,
            BanEntry {
                address: addr.to_string(),
                score,
                banned_until,
            },
        );
        self.save();
    }

    pub fn prune(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.entries.retain(|_, entry| entry.banned_until > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_path(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "moltchain_ban_{}_{}_{}_{}.json",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            n,
        ))
    }

    #[test]
    fn test_new_empty_ban_list() {
        let path = unique_path("empty");
        let ban_list = PeerBanList::new(path.clone());
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        assert!(!ban_list.is_banned(&addr));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_ban_on_low_score() {
        let path = unique_path("lowscore");
        let mut ban_list = PeerBanList::new(path.clone());
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        ban_list.record_score(addr, -10);
        assert!(ban_list.is_banned(&addr));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_no_ban_on_positive_score() {
        let path = unique_path("posscore");
        let mut ban_list = PeerBanList::new(path.clone());
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        ban_list.record_score(addr, 5);
        assert!(!ban_list.is_banned(&addr));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_prune_removes_expired() {
        let path = unique_path("prune");
        let mut ban_list = PeerBanList::new(path.clone());
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        ban_list.entries.insert(
            addr,
            BanEntry {
                address: addr.to_string(),
                score: -10,
                banned_until: 0,
            },
        );
        ban_list.prune();
        assert!(ban_list.entries.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_persistence_roundtrip() {
        let path = unique_path("roundtrip");
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        {
            let mut ban_list = PeerBanList::new(path.clone());
            ban_list.record_score(addr, -15);
            assert!(path.exists(), "Ban list file should exist");
        }
        let ban_list = PeerBanList::new(path.clone());
        assert!(ban_list.is_banned(&addr));
        let _ = std::fs::remove_file(&path);
    }
}

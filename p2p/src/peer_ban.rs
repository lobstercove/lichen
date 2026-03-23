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
    /// AUDIT-FIX L1: Track how many times this peer has been banned.
    /// Each repeated ban doubles the duration (10 min → 20 min → 40 min → …)
    /// capped at 24 hours.
    #[serde(default)]
    pub ban_count: u32,
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

        // AUDIT-FIX L1: Escalating ban durations for repeat offenders.
        // Base = 600s (10 min). Each repeat doubles: 10m → 20m → 40m → 80m → …
        // Capped at 86400s (24 hours).
        const BASE_BAN_SECS: u64 = 600;
        const MAX_BAN_SECS: u64 = 86400;

        let previous_count = self.entries.get(&addr).map(|e| e.ban_count).unwrap_or(0);

        let ban_count = previous_count + 1;
        let ban_duration = if score <= -10 {
            // 600 * 2^(ban_count-1), capped at 24h
            let shift = ban_count.saturating_sub(1).min(16); // cap shift to prevent overflow
            BASE_BAN_SECS
                .saturating_mul(1u64 << shift)
                .min(MAX_BAN_SECS)
        } else {
            0
        };
        let banned_until = if ban_duration > 0 {
            now + ban_duration
        } else {
            0
        };

        self.entries.insert(
            addr,
            BanEntry {
                address: addr.to_string(),
                score,
                banned_until,
                ban_count,
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
            "lichen_ban_{}_{}_{}_{}.json",
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
        // First ban should have ban_count=1
        assert_eq!(ban_list.entries.get(&addr).unwrap().ban_count, 1);
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
                ban_count: 1,
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

    #[test]
    fn test_escalating_ban_duration() {
        // AUDIT-FIX L1: Verify that repeat offenders get longer bans
        let path = unique_path("escalate");
        let mut ban_list = PeerBanList::new(path.clone());
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();

        // First ban: 600s base
        ban_list.record_score(addr, -10);
        let entry1 = ban_list.entries.get(&addr).unwrap().clone();
        assert_eq!(entry1.ban_count, 1);

        // Manually expire the first ban to allow re-banning
        ban_list.entries.get_mut(&addr).unwrap().banned_until = 0;

        // Second ban: 1200s (doubled)
        ban_list.record_score(addr, -10);
        let entry2 = ban_list.entries.get(&addr).unwrap().clone();
        assert_eq!(entry2.ban_count, 2);
        // Duration should be longer: entry2.banned_until - now should be ~1200s
        // We can't check exact times easily, but ban_count tracks escalation
        assert!(entry2.banned_until > 0);

        // Third ban: 2400s (doubled again)
        ban_list.entries.get_mut(&addr).unwrap().banned_until = 0;
        ban_list.record_score(addr, -10);
        let entry3 = ban_list.entries.get(&addr).unwrap().clone();
        assert_eq!(entry3.ban_count, 3);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_ban_duration_capped_at_24h() {
        // Verify ban duration doesn't exceed 24 hours even with many repeated bans
        let path = unique_path("cap");
        let mut ban_list = PeerBanList::new(path.clone());
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();

        // Simulate 20 repeated bans — should cap at 86400s
        for _ in 0..20 {
            ban_list
                .entries
                .entry(addr)
                .and_modify(|e| e.banned_until = 0);
            ban_list.record_score(addr, -10);
        }

        let entry = ban_list.entries.get(&addr).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // banned_until should be at most now + 86400
        assert!(entry.banned_until <= now + 86400 + 1);
        assert_eq!(entry.ban_count, 20);

        let _ = std::fs::remove_file(&path);
    }
}

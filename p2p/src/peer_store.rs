// Durable peer store for bootstrap and restart recovery

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::warn;

#[derive(Debug, Serialize, Deserialize)]
struct PeerStoreData {
    peers: Vec<String>,
}

pub struct PeerStore {
    path: PathBuf,
    peers: Mutex<Vec<SocketAddr>>,
    /// P10-P2P-02: O(1) address lookup index (mirrors `peers` Vec)
    peer_set: Mutex<HashSet<SocketAddr>>,
    max_peers: usize,
}

impl PeerStore {
    pub fn new(path: PathBuf, max_peers: usize) -> Self {
        let peers = Self::load_from_path(&path);
        let peer_set: HashSet<SocketAddr> = peers.iter().copied().collect();
        PeerStore {
            path,
            peers: Mutex::new(peers),
            peer_set: Mutex::new(peer_set),
            max_peers,
        }
    }

    pub fn load_from_path(path: &Path) -> Vec<SocketAddr> {
        let data = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(_) => return Vec::new(),
        };

        let parsed: PeerStoreData = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };

        parsed
            .peers
            .into_iter()
            .filter_map(|peer| peer.parse::<SocketAddr>().ok())
            .collect()
    }

    pub fn record_peer(&self, addr: SocketAddr) {
        // AUDIT-FIX F-4: Acquire both locks in a single scope with consistent ordering.
        // Lock ordering: peers FIRST, then peer_set. This matches get_known_peers()
        // which only acquires peers. No other code path acquires peer_set→peers.
        let data_to_write = {
            let mut peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
            let mut set = self.peer_set.lock().unwrap_or_else(|e| e.into_inner());
            if set.contains(&addr) {
                return;
            }
            set.insert(addr);

            peers.push(addr);
            if peers.len() > self.max_peers {
                let evicted = peers[0];
                peers.rotate_left(1);
                peers.truncate(self.max_peers);
                // Also remove evicted peer from the set
                set.remove(&evicted);
            }

            PeerStoreData {
                peers: peers.iter().map(|peer| peer.to_string()).collect(),
            }
        }; // locks dropped here

        // File I/O outside the lock
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(json) = serde_json::to_string_pretty(&data_to_write) {
            // AUDIT-FIX 3.15: Write + fsync to prevent data loss on power failure
            match fs::File::create(&self.path) {
                Ok(mut file) => {
                    use std::io::Write;
                    if let Err(e) = file.write_all(json.as_bytes()) {
                        warn!("peer_store: write failed: {}", e);
                    } else if let Err(e) = file.sync_all() {
                        warn!("peer_store: fsync failed: {}", e);
                    }
                }
                Err(e) => {
                    warn!("peer_store: create failed: {}", e);
                }
            }
        }
    }

    pub fn peers(&self) -> Vec<SocketAddr> {
        self.peers.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
            "moltchain_ps_{}_{}_{}_{}.json",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            n,
        ))
    }

    #[test]
    fn test_new_empty_store() {
        let path = unique_path("empty");
        let store = PeerStore::new(path.clone(), 100);
        assert!(store.peers().is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_record_peer() {
        let path = unique_path("record");
        let store = PeerStore::new(path.clone(), 100);
        let addr: SocketAddr = "127.0.0.1:9001".parse().unwrap();
        store.record_peer(addr);
        assert_eq!(store.peers().len(), 1);
        assert_eq!(store.peers()[0], addr);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_duplicate_peer_ignored() {
        let path = unique_path("dup");
        let store = PeerStore::new(path.clone(), 100);
        let addr: SocketAddr = "127.0.0.1:9002".parse().unwrap();
        store.record_peer(addr);
        store.record_peer(addr);
        assert_eq!(store.peers().len(), 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_max_peers_enforcement() {
        let path = unique_path("max");
        let store = PeerStore::new(path.clone(), 3);
        for i in 0..5u16 {
            let addr: SocketAddr = format!("127.0.0.1:{}", 9100 + i).parse().unwrap();
            store.record_peer(addr);
        }
        assert_eq!(store.peers().len(), 3);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_persistence_roundtrip() {
        let path = unique_path("roundtrip");
        let addr1: SocketAddr = "127.0.0.1:9201".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.1:9202".parse().unwrap();
        {
            let store = PeerStore::new(path.clone(), 100);
            store.record_peer(addr1);
            store.record_peer(addr2);
            assert!(
                path.exists(),
                "Peer store file should exist after record_peer"
            );
        }
        let store2 = PeerStore::new(path.clone(), 100);
        let loaded = store2.peers();
        assert_eq!(loaded.len(), 2);
        let _ = fs::remove_file(&path);
    }
}

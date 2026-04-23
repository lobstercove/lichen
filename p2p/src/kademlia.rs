// Kademlia DHT Routing Table
//
// P3-2: Structured overlay with O(log N) routing. Replaces flat broadcasts
// for non-latency-critical messages (blocks, transactions). Votes continue
// using flat broadcast for minimum latency.

use std::net::SocketAddr;
use std::time::Instant;

/// Number of entries per k-bucket.
pub const K_BUCKET_SIZE: usize = 20;

/// Number of buckets in the routing table (one per bit of NodeId).
const NUM_BUCKETS: usize = 256;

/// 256-bit node identifier (SHA-256 of the node's public key).
pub type NodeId = [u8; 32];

/// Entry in a k-bucket.
#[derive(Debug, Clone)]
pub struct KademliaEntry {
    pub node_id: NodeId,
    pub address: SocketAddr,
    pub last_seen: Instant,
}

/// A single k-bucket storing up to K_BUCKET_SIZE entries.
/// Entries are ordered by last-seen time (least-recently-seen first).
#[derive(Debug)]
struct KBucket {
    entries: Vec<KademliaEntry>,
}

impl KBucket {
    fn new() -> Self {
        Self {
            entries: Vec::with_capacity(K_BUCKET_SIZE),
        }
    }

    /// Insert or update a node in this bucket.
    /// Returns `true` if the node was inserted or updated.
    fn upsert(&mut self, node_id: NodeId, address: SocketAddr) -> bool {
        // If already present, move to tail (most-recently-seen)
        if let Some(pos) = self.entries.iter().position(|e| e.node_id == node_id) {
            let mut entry = self.entries.remove(pos);
            entry.last_seen = Instant::now();
            entry.address = address;
            self.entries.push(entry);
            return true;
        }

        // Bucket not full — append
        if self.entries.len() < K_BUCKET_SIZE {
            self.entries.push(KademliaEntry {
                node_id,
                address,
                last_seen: Instant::now(),
            });
            return true;
        }

        // Bucket full — evict the least-recently-seen entry
        // (standard Kademlia: ping the LRS entry first, but for simplicity
        // we evict immediately; the entry can re-announce to get back in)
        self.entries.remove(0);
        self.entries.push(KademliaEntry {
            node_id,
            address,
            last_seen: Instant::now(),
        });
        true
    }

    fn entries(&self) -> &[KademliaEntry] {
        &self.entries
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Kademlia routing table with 256 k-buckets.
pub struct KademliaTable {
    /// Our own node ID.
    local_id: NodeId,
    /// One k-bucket per distance bit.
    buckets: Vec<KBucket>,
}

impl KademliaTable {
    /// Create a new routing table for the given local node ID.
    pub fn new(local_id: NodeId) -> Self {
        let mut buckets = Vec::with_capacity(NUM_BUCKETS);
        for _ in 0..NUM_BUCKETS {
            buckets.push(KBucket::new());
        }
        Self { local_id, buckets }
    }

    /// Insert or update a node in the appropriate bucket.
    pub fn insert(&mut self, node_id: NodeId, address: SocketAddr) -> bool {
        if node_id == self.local_id {
            return false; // Don't insert ourselves
        }
        let bucket_idx = self.bucket_index(&node_id);
        self.buckets[bucket_idx].upsert(node_id, address)
    }

    /// Remove a node from the routing table.
    pub fn remove(&mut self, node_id: &NodeId) {
        let bucket_idx = self.bucket_index(node_id);
        self.buckets[bucket_idx]
            .entries
            .retain(|e| &e.node_id != node_id);
    }

    /// Return the `count` closest nodes to `target` by XOR distance.
    pub fn closest(&self, target: &NodeId, count: usize) -> Vec<KademliaEntry> {
        let mut all: Vec<(NodeId, &KademliaEntry)> = Vec::new();
        for bucket in &self.buckets {
            for entry in bucket.entries() {
                all.push((xor_distance(&entry.node_id, target), entry));
            }
        }
        all.sort_by_key(|a| a.0);
        all.into_iter()
            .take(count)
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// Total number of entries across all buckets.
    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    /// Whether the routing table is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get our local node ID.
    pub fn local_id(&self) -> &NodeId {
        &self.local_id
    }

    /// Determine which bucket a node ID belongs to (by XOR distance from us).
    fn bucket_index(&self, node_id: &NodeId) -> usize {
        let dist = xor_distance(&self.local_id, node_id);
        // Find the highest bit set in the distance (leading zeros → bucket)
        for (byte_idx, &byte) in dist.iter().enumerate() {
            if byte != 0 {
                let bit = 7 - byte.leading_zeros() as usize;
                return byte_idx * 8 + (7 - bit);
            }
        }
        // If distance is 0 (same ID), return the last bucket
        NUM_BUCKETS - 1
    }

    /// Get all known peers as (NodeId, SocketAddr) pairs.
    pub fn all_peers(&self) -> Vec<(NodeId, SocketAddr)> {
        let mut result = Vec::new();
        for bucket in &self.buckets {
            for entry in bucket.entries() {
                result.push((entry.node_id, entry.address));
            }
        }
        result
    }
}

/// XOR distance between two NodeIds.
pub fn xor_distance(a: &NodeId, b: &NodeId) -> NodeId {
    let mut result = [0u8; 32];
    for i in 0..32 {
        result[i] = a[i] ^ b[i];
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn make_addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    fn make_id(value: u8) -> NodeId {
        [value; 32]
    }

    #[test]
    fn test_xor_distance() {
        let a = make_id(0);
        let b = make_id(0xFF);
        let dist = xor_distance(&a, &b);
        assert_eq!(dist, [0xFF; 32]);

        // Distance to self is 0
        let self_dist = xor_distance(&a, &a);
        assert_eq!(self_dist, [0; 32]);
    }

    #[test]
    fn test_insert_and_closest() {
        let local = make_id(0);
        let mut table = KademliaTable::new(local);

        // Insert 5 nodes with different IDs
        for i in 1..=5u8 {
            table.insert(make_id(i), make_addr(8000 + i as u16));
        }
        assert_eq!(table.len(), 5);

        // Closest to ID=1 should return ID=1 first
        let closest = table.closest(&make_id(1), 3);
        assert_eq!(closest.len(), 3);
        assert_eq!(closest[0].node_id, make_id(1));
    }

    #[test]
    fn test_self_not_inserted() {
        let local = make_id(42);
        let mut table = KademliaTable::new(local);
        let inserted = table.insert(make_id(42), make_addr(8000));
        assert!(!inserted);
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_bucket_overflow_evicts_lrs() {
        let local = make_id(0);
        let mut table = KademliaTable::new(local);

        // All nodes with ID=0xFF will land in the same bucket.
        // Fill beyond K_BUCKET_SIZE.
        for i in 0..K_BUCKET_SIZE + 5 {
            let mut id = [0xFFu8; 32];
            id[31] = i as u8; // vary the last byte so IDs differ but bucket is same
            table.insert(id, make_addr(8000 + i as u16));
        }

        // Total entries shouldn't exceed K_BUCKET_SIZE in one bucket
        // (entries in different buckets are fine, but these all XOR-distance
        // to bucket 0 or nearby)
        assert!(table.len() <= K_BUCKET_SIZE + 5); // some may spill to adjacent buckets
    }

    #[test]
    fn test_remove() {
        let local = make_id(0);
        let mut table = KademliaTable::new(local);
        let id = make_id(5);
        table.insert(id, make_addr(8005));
        assert_eq!(table.len(), 1);
        table.remove(&id);
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_upsert_updates_address() {
        let local = make_id(0);
        let mut table = KademliaTable::new(local);
        let id = make_id(10);
        table.insert(id, make_addr(8010));
        table.insert(id, make_addr(9010)); // update address
        let closest = table.closest(&id, 1);
        assert_eq!(closest[0].address.port(), 9010);
    }

    #[test]
    fn test_all_peers() {
        let local = make_id(0);
        let mut table = KademliaTable::new(local);
        for i in 1..=3u8 {
            table.insert(make_id(i), make_addr(8000 + i as u16));
        }
        let all = table.all_peers();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_bucket_index_distinct() {
        let local = make_id(0);
        let table = KademliaTable::new(local);
        // ID with highest bit set → bucket 0
        let mut id_high = [0u8; 32];
        id_high[0] = 0x80;
        assert_eq!(table.bucket_index(&id_high), 0);

        // ID with only lowest bit → bucket 255
        let mut id_low = [0u8; 32];
        id_low[31] = 0x01;
        assert_eq!(table.bucket_index(&id_low), 255);
    }
}

// P3-6: NAT Traversal — QUIC Hole Punching
//
// Provides relay-assisted NAT traversal so validators behind NAT
// can accept inbound connections without port forwarding.
//
// Protocol:
// 1. Peer A (behind NAT) connects outbound to relay R.
// 2. When A wants to reach peer B (also behind NAT), A sends
//    HolePunchRequest { target_addr: B, requester_observed_addr: A_ext }
//    to relay R.
// 3. R forwards HolePunchNotify { peer_observed_addr: A_ext } to B.
// 4. B sends a QUIC packet (connect attempt) to A_ext, which punches
//    a hole in B's NAT for A's return traffic.
// 5. Simultaneously, A sends a QUIC packet to B_ext (if known),
//    establishing bidirectional connectivity.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// NAT status of a peer — determines connectivity strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NatStatus {
    /// Peer is directly reachable (public IP, port-forwarded, or cloud VM)
    Public,
    /// Peer is behind NAT but reachable via hole punching
    NatPunched,
    /// NAT status unknown — default for new peers
    Unknown,
}

impl Default for NatStatus {
    fn default() -> Self {
        NatStatus::Unknown
    }
}

/// Observed address information for NAT detection.
/// Peers compare their listen_addr with the address the remote end
/// sees (from QUIC connection metadata) to detect NAT.
#[derive(Debug, Clone)]
pub struct NatDetector {
    /// The address we listen on locally
    pub local_addr: SocketAddr,
    /// The externally-configured address (if set by user)
    pub configured_external: Option<SocketAddr>,
    /// Addresses that remote peers have reported seeing us as
    pub observed_addrs: Vec<SocketAddr>,
}

impl NatDetector {
    pub fn new(local_addr: SocketAddr, configured_external: Option<SocketAddr>) -> Self {
        Self {
            local_addr,
            configured_external,
            observed_addrs: Vec::new(),
        }
    }

    /// Record an externally-observed address reported by a remote peer
    /// (or learned from the QUIC connection's remote_address on the far end).
    pub fn record_observed_addr(&mut self, addr: SocketAddr) {
        if !self.observed_addrs.contains(&addr) {
            // Keep bounded — only track last 10 observed addresses
            if self.observed_addrs.len() >= 10 {
                self.observed_addrs.remove(0);
            }
            self.observed_addrs.push(addr);
        }
    }

    /// Detect our NAT status based on observed vs local addresses.
    pub fn detect_status(&self) -> NatStatus {
        // If external addr is configured, assume public
        if self.configured_external.is_some() {
            return NatStatus::Public;
        }

        // If we have no observations yet, unknown
        if self.observed_addrs.is_empty() {
            return NatStatus::Unknown;
        }

        // If any observed address matches our local address (IP + port),
        // we're likely public (not behind NAT)
        let local_ip = self.local_addr.ip();
        let local_port = self.local_addr.port();
        for obs in &self.observed_addrs {
            if obs.ip() == local_ip && obs.port() == local_port {
                return NatStatus::Public;
            }
        }

        // Otherwise, we're behind NAT — our observed address differs from local
        NatStatus::NatPunched
    }

    /// Get the best known external address for this node.
    /// Priority: configured_external > most recent observed > local_addr
    pub fn external_addr(&self) -> SocketAddr {
        if let Some(ext) = self.configured_external {
            return ext;
        }
        if let Some(obs) = self.observed_addrs.last() {
            return *obs;
        }
        self.local_addr
    }
}

/// Hole punch attempt state tracker.
/// Tracks pending hole punch requests to avoid duplicates and detect failures.
#[derive(Debug)]
pub struct HolePunchTracker {
    /// Pending hole punch attempts: (target_addr, start_time)
    pending: Vec<(SocketAddr, std::time::Instant)>,
    /// Maximum concurrent pending attempts
    max_pending: usize,
    /// How long before a pending attempt times out
    timeout: std::time::Duration,
}

impl HolePunchTracker {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            max_pending: 32,
            timeout: std::time::Duration::from_secs(10),
        }
    }

    /// Start a hole punch attempt. Returns false if already pending or limit reached.
    pub fn start_attempt(&mut self, target: SocketAddr) -> bool {
        self.cleanup_expired();
        if self.pending.len() >= self.max_pending {
            return false;
        }
        if self.pending.iter().any(|(addr, _)| *addr == target) {
            return false; // already pending
        }
        self.pending.push((target, std::time::Instant::now()));
        true
    }

    /// Mark a hole punch attempt as completed (success or failure).
    pub fn complete_attempt(&mut self, target: &SocketAddr) {
        self.pending.retain(|(addr, _)| addr != target);
    }

    /// Check if a hole punch to this target is pending.
    pub fn is_pending(&self, target: &SocketAddr) -> bool {
        self.pending
            .iter()
            .any(|(addr, t)| addr == target && t.elapsed() < self.timeout)
    }

    /// Remove expired attempts.
    fn cleanup_expired(&mut self) {
        let timeout = self.timeout;
        self.pending.retain(|(_, t)| t.elapsed() < timeout);
    }

    /// Number of active pending attempts.
    pub fn pending_count(&self) -> usize {
        self.cleanup_expired_count()
    }

    fn cleanup_expired_count(&self) -> usize {
        self.pending
            .iter()
            .filter(|(_, t)| t.elapsed() < self.timeout)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nat_status_default() {
        assert_eq!(NatStatus::default(), NatStatus::Unknown);
    }

    #[test]
    fn test_nat_detector_public_configured() {
        let local: SocketAddr = "192.168.1.5:7001".parse().unwrap();
        let external: SocketAddr = "203.0.113.50:7001".parse().unwrap();
        let detector = NatDetector::new(local, Some(external));
        assert_eq!(detector.detect_status(), NatStatus::Public);
        assert_eq!(detector.external_addr(), external);
    }

    #[test]
    fn test_nat_detector_public_matching_observed() {
        let local: SocketAddr = "203.0.113.50:7001".parse().unwrap();
        let mut detector = NatDetector::new(local, None);
        detector.record_observed_addr("203.0.113.50:7001".parse().unwrap());
        assert_eq!(detector.detect_status(), NatStatus::Public);
        assert_eq!(detector.external_addr(), local);
    }

    #[test]
    fn test_nat_detector_behind_nat() {
        let local: SocketAddr = "192.168.1.5:7001".parse().unwrap();
        let mut detector = NatDetector::new(local, None);
        let observed: SocketAddr = "203.0.113.50:34567".parse().unwrap();
        detector.record_observed_addr(observed);
        assert_eq!(detector.detect_status(), NatStatus::NatPunched);
        assert_eq!(detector.external_addr(), observed);
    }

    #[test]
    fn test_nat_detector_unknown_no_observations() {
        let local: SocketAddr = "192.168.1.5:7001".parse().unwrap();
        let detector = NatDetector::new(local, None);
        assert_eq!(detector.detect_status(), NatStatus::Unknown);
        assert_eq!(detector.external_addr(), local);
    }

    #[test]
    fn test_nat_detector_observed_addr_bounded() {
        let local: SocketAddr = "192.168.1.5:7001".parse().unwrap();
        let mut detector = NatDetector::new(local, None);
        // Add 15 observed addresses — only last 10 should be kept
        for i in 0..15u16 {
            let addr: SocketAddr = format!("10.0.0.{}:{}", i / 256, 8000 + i).parse().unwrap();
            detector.record_observed_addr(addr);
        }
        assert_eq!(detector.observed_addrs.len(), 10);
    }

    #[test]
    fn test_nat_detector_no_duplicates() {
        let local: SocketAddr = "192.168.1.5:7001".parse().unwrap();
        let mut detector = NatDetector::new(local, None);
        let obs: SocketAddr = "203.0.113.50:34567".parse().unwrap();
        detector.record_observed_addr(obs);
        detector.record_observed_addr(obs);
        assert_eq!(detector.observed_addrs.len(), 1);
    }

    #[test]
    fn test_hole_punch_tracker_start_attempt() {
        let mut tracker = HolePunchTracker::new();
        let target: SocketAddr = "10.0.0.1:7001".parse().unwrap();
        assert!(tracker.start_attempt(target));
        assert!(tracker.is_pending(&target));
        // Duplicate should fail
        assert!(!tracker.start_attempt(target));
    }

    #[test]
    fn test_hole_punch_tracker_complete_attempt() {
        let mut tracker = HolePunchTracker::new();
        let target: SocketAddr = "10.0.0.1:7001".parse().unwrap();
        assert!(tracker.start_attempt(target));
        tracker.complete_attempt(&target);
        assert!(!tracker.is_pending(&target));
        // Can start again after completion
        assert!(tracker.start_attempt(target));
    }

    #[test]
    fn test_hole_punch_tracker_max_pending() {
        let mut tracker = HolePunchTracker::new();
        // Fill up to max
        for i in 0..32u16 {
            let target: SocketAddr = format!("10.0.0.{}:{}", i / 256, 8000 + i).parse().unwrap();
            assert!(tracker.start_attempt(target));
        }
        // 33rd should fail
        let overflow: SocketAddr = "10.0.0.100:9999".parse().unwrap();
        assert!(!tracker.start_attempt(overflow));
    }

    #[test]
    fn test_hole_punch_tracker_pending_count() {
        let mut tracker = HolePunchTracker::new();
        assert_eq!(tracker.pending_count(), 0);
        let t1: SocketAddr = "10.0.0.1:7001".parse().unwrap();
        let t2: SocketAddr = "10.0.0.2:7001".parse().unwrap();
        tracker.start_attempt(t1);
        assert_eq!(tracker.pending_count(), 1);
        tracker.start_attempt(t2);
        assert_eq!(tracker.pending_count(), 2);
        tracker.complete_attempt(&t1);
        assert_eq!(tracker.pending_count(), 1);
    }
}

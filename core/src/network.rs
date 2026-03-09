// MoltChain Network Configuration
// Seed nodes, bootstrap peers, and network discovery

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Network type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkType {
    Testnet,
    Mainnet,
    Devnet,
}

impl NetworkType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "testnet" => Some(NetworkType::Testnet),
            "mainnet" => Some(NetworkType::Mainnet),
            "devnet" => Some(NetworkType::Devnet),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            NetworkType::Testnet => "testnet",
            NetworkType::Mainnet => "mainnet",
            NetworkType::Devnet => "devnet",
        }
    }
}

/// Seed node information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedNode {
    pub id: String,
    pub address: String,
    pub pubkey: String,
    pub region: String,
    pub operator: String,
    pub rpc: String,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub network_id: String,
    pub chain_id: String,
    pub seeds: Vec<SeedNode>,
    pub bootstrap_peers: Vec<String>,
    pub rpc_endpoints: Vec<String>,
    pub explorers: Vec<String>,
    pub faucets: Vec<String>,
}

/// Seeds configuration for all networks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedsConfig {
    pub testnet: NetworkConfig,
    pub mainnet: NetworkConfig,
    pub devnet: NetworkConfig,
}

impl SeedsConfig {
    /// Load seeds configuration from file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let contents =
            fs::read_to_string(path).map_err(|e| format!("Failed to read seeds file: {}", e))?;

        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse seeds file: {}", e))
    }

    /// Get configuration for specific network
    pub fn get_network(&self, network: NetworkType) -> &NetworkConfig {
        match network {
            NetworkType::Testnet => &self.testnet,
            NetworkType::Mainnet => &self.mainnet,
            NetworkType::Devnet => &self.devnet,
        }
    }

    /// Get bootstrap peers for network
    pub fn get_bootstrap_peers(&self, network: NetworkType) -> Vec<String> {
        self.get_network(network).bootstrap_peers.clone()
    }

    /// Get seed addresses for network
    pub fn get_seed_addresses(&self, network: NetworkType) -> Vec<String> {
        self.get_network(network)
            .seeds
            .iter()
            .map(|s| s.address.clone())
            .collect()
    }

    /// Get RPC endpoints for network
    pub fn get_rpc_endpoints(&self, network: NetworkType) -> Vec<String> {
        self.get_network(network).rpc_endpoints.clone()
    }

    /// Get all peer addresses (seeds + bootstrap)
    pub fn get_all_peers(&self, network: NetworkType) -> Vec<String> {
        let mut peers = self.get_seed_addresses(network);
        peers.extend(self.get_bootstrap_peers(network));
        peers
    }

    /// Default embedded configuration.
    ///
    /// AUDIT-FIX CORE-04: These are compile-time fallback seeds used only when
    /// no seeds.json or config file is provided. Validators override these by
    /// placing a seeds.json file in the data directory (see validator SeedsFile).
    /// DNS-based seed addresses resolve dynamically; raw IP bootstrap_peers are
    /// last-resort fallbacks for initial network discovery.
    pub fn default_embedded() -> Self {
        SeedsConfig {
            testnet: NetworkConfig {
                network_id: "moltchain-testnet-1".to_string(),
                chain_id: "moltchain-testnet-1".to_string(),
                seeds: vec![
                    SeedNode {
                        id: "seed-01".to_string(),
                        address: "seed-01.moltchain.network:7001".to_string(),
                        pubkey: String::new(),
                        region: "us-east".to_string(),
                        operator: "MoltChain Foundation".to_string(),
                        rpc: "https://testnet-rpc.moltchain.network".to_string(),
                    },
                    SeedNode {
                        id: "seed-02".to_string(),
                        address: "seed-02.moltchain.network:7001".to_string(),
                        pubkey: String::new(),
                        region: "eu-west".to_string(),
                        operator: "MoltChain Foundation".to_string(),
                        rpc: "https://testnet-rpc.moltchain.network".to_string(),
                    },
                    SeedNode {
                        id: "seed-03".to_string(),
                        address: "seed-03.moltchain.network:7001".to_string(),
                        pubkey: String::new(),
                        region: "ap-southeast".to_string(),
                        operator: "MoltChain Foundation".to_string(),
                        rpc: "https://testnet-rpc.moltchain.network".to_string(),
                    },
                ],
                bootstrap_peers: vec![
                    "seed-01.moltchain.network:7001".to_string(),
                    "seed-02.moltchain.network:7001".to_string(),
                    "seed-03.moltchain.network:7001".to_string(),
                ],
                rpc_endpoints: vec!["https://testnet-rpc.moltchain.network".to_string()],
                explorers: vec!["https://explorer.moltchain.network".to_string()],
                faucets: vec!["https://faucet.moltchain.network".to_string()],
            },
            mainnet: NetworkConfig {
                network_id: "moltchain-mainnet-1".to_string(),
                chain_id: "moltchain-mainnet-1".to_string(),
                seeds: vec![
                    SeedNode {
                        id: "seed-01".to_string(),
                        address: "seed-01.moltchain.network:8001".to_string(),
                        pubkey: String::new(),
                        region: "us-east".to_string(),
                        operator: "MoltChain Foundation".to_string(),
                        rpc: "https://rpc.moltchain.network".to_string(),
                    },
                    SeedNode {
                        id: "seed-02".to_string(),
                        address: "seed-02.moltchain.network:8001".to_string(),
                        pubkey: String::new(),
                        region: "eu-west".to_string(),
                        operator: "MoltChain Foundation".to_string(),
                        rpc: "https://rpc.moltchain.network".to_string(),
                    },
                    SeedNode {
                        id: "seed-03".to_string(),
                        address: "seed-03.moltchain.network:8001".to_string(),
                        pubkey: String::new(),
                        region: "ap-southeast".to_string(),
                        operator: "MoltChain Foundation".to_string(),
                        rpc: "https://rpc.moltchain.network".to_string(),
                    },
                ],
                bootstrap_peers: vec![
                    "seed-01.moltchain.network:8001".to_string(),
                    "seed-02.moltchain.network:8001".to_string(),
                    "seed-03.moltchain.network:8001".to_string(),
                ],
                rpc_endpoints: vec!["https://rpc.moltchain.network".to_string()],
                explorers: vec!["https://explorer.moltchain.network".to_string()],
                faucets: vec![],
            },
            devnet: NetworkConfig {
                network_id: "moltchain-devnet-1".to_string(),
                chain_id: "moltchain-devnet-1".to_string(),
                seeds: vec![],
                bootstrap_peers: vec!["127.0.0.1:7001".to_string()],
                rpc_endpoints: vec!["http://localhost:8899".to_string()],
                explorers: vec![],
                faucets: vec![],
            },
        }
    }
}

/// Peer discovery manager
pub struct PeerDiscovery {
    config: SeedsConfig,
    network: NetworkType,
    discovered_peers: HashMap<String, PeerInfo>,
}

/// Peer information
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub address: String,
    pub last_seen: u64,
    pub latency_ms: u64,
    pub is_seed: bool,
}

impl PeerDiscovery {
    /// Create new peer discovery manager
    pub fn new(network: NetworkType) -> Self {
        let config = SeedsConfig::default_embedded();
        PeerDiscovery {
            config,
            network,
            discovered_peers: HashMap::new(),
        }
    }

    /// Load configuration from file
    pub fn with_config_file<P: AsRef<Path>>(network: NetworkType, path: P) -> Result<Self, String> {
        let config = SeedsConfig::from_file(path)?;
        Ok(PeerDiscovery {
            config,
            network,
            discovered_peers: HashMap::new(),
        })
    }

    /// Get initial bootstrap peers
    pub fn get_bootstrap_peers(&self) -> Vec<String> {
        self.config.get_all_peers(self.network)
    }

    /// Add discovered peer
    pub fn add_peer(&mut self, address: String, is_seed: bool) {
        let peer = PeerInfo {
            address: address.clone(),
            last_seen: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            latency_ms: 0,
            is_seed,
        };
        self.discovered_peers.insert(address, peer);
    }

    /// Get all known peers
    pub fn get_all_peers(&self) -> Vec<String> {
        self.discovered_peers.keys().cloned().collect()
    }

    /// Get healthy peers (seen recently)
    pub fn get_healthy_peers(&self, max_age_secs: u64) -> Vec<String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.discovered_peers
            .iter()
            .filter(|(_, peer)| now - peer.last_seen < max_age_secs)
            .map(|(addr, _)| addr.clone())
            .collect()
    }

    /// Get network configuration
    pub fn get_network_config(&self) -> &NetworkConfig {
        self.config.get_network(self.network)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_type_conversion() {
        assert_eq!(NetworkType::from_str("testnet"), Some(NetworkType::Testnet));
        assert_eq!(NetworkType::from_str("mainnet"), Some(NetworkType::Mainnet));
        assert_eq!(NetworkType::from_str("devnet"), Some(NetworkType::Devnet));
        assert_eq!(NetworkType::from_str("invalid"), None);
    }

    #[test]
    fn test_default_embedded_config() {
        let config = SeedsConfig::default_embedded();

        // Testnet should have seeds
        assert!(!config.testnet.seeds.is_empty());
        assert!(!config.testnet.bootstrap_peers.is_empty());

        // Mainnet should have seeds and bootstrap peers
        assert!(!config.mainnet.seeds.is_empty());
        assert!(!config.mainnet.bootstrap_peers.is_empty());

        // Devnet should have localhost
        assert!(config
            .devnet
            .bootstrap_peers
            .contains(&"127.0.0.1:7001".to_string()));
    }

    #[test]
    fn test_peer_discovery() {
        let mut discovery = PeerDiscovery::new(NetworkType::Testnet);

        // Get bootstrap peers
        let peers = discovery.get_bootstrap_peers();
        assert!(!peers.is_empty());

        // Add discovered peer
        discovery.add_peer("192.168.1.100:8000".to_string(), false);
        assert_eq!(discovery.get_all_peers().len(), 1);

        // Get healthy peers
        let healthy = discovery.get_healthy_peers(300);
        assert_eq!(healthy.len(), 1);
    }

    #[test]
    fn test_get_all_peers() {
        let config = SeedsConfig::default_embedded();
        let peers = config.get_all_peers(NetworkType::Testnet);

        // Should have both seeds and bootstrap peers
        assert!(peers.len() > 3);
    }
}

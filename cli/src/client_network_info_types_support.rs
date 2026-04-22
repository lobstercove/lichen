use serde::Deserialize;

#[derive(Deserialize)]
pub struct NetworkInfo {
    pub network_id: String,
    #[serde(default)]
    pub chain_id: String,
    pub current_slot: u64,
    pub validator_count: usize,
    #[serde(rename = "peer_count")]
    pub _peer_count: usize,
    #[serde(rename = "version", default)]
    pub _version: String,
    #[serde(default)]
    pub tps: f64,
}
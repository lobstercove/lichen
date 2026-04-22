use serde::Deserialize;

#[derive(Deserialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub address: String,
    pub connected: bool,
}
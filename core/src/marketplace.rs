// MoltChain Core - Marketplace activity tracking

use crate::account::Pubkey;
use crate::hash::Hash;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MarketActivityKind {
    Listing,
    Sale,
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketActivity {
    pub slot: u64,
    pub timestamp: u64,
    pub kind: MarketActivityKind,
    pub program: Pubkey,
    pub collection: Option<Pubkey>,
    pub token: Option<Pubkey>,
    pub token_id: Option<u64>,
    pub price: Option<u64>,
    pub seller: Option<Pubkey>,
    pub buyer: Option<Pubkey>,
    pub function: String,
    pub tx_signature: Hash,
}

pub fn encode_market_activity(activity: &MarketActivity) -> Result<Vec<u8>, String> {
    bincode::serialize(activity).map_err(|e| format!("Failed to encode market activity: {}", e))
}

pub fn decode_market_activity(data: &[u8]) -> Result<MarketActivity, String> {
    bincode::deserialize(data).map_err(|e| format!("Failed to decode market activity: {}", e))
}

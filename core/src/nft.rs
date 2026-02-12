// MoltChain Core - NFT primitives (system-level)

use crate::account::Pubkey;
use crate::hash::Hash;
use serde::{Deserialize, Serialize};

pub const NFT_COLLECTION_VERSION: u8 = 1;
pub const NFT_TOKEN_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionState {
    pub version: u8,
    pub name: String,
    pub symbol: String,
    pub creator: Pubkey,
    pub royalty_bps: u16,
    pub max_supply: u64,
    pub minted: u64,
    pub public_mint: bool,
    pub mint_authority: Option<Pubkey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenState {
    pub version: u8,
    pub collection: Pubkey,
    pub token_id: u64,
    pub owner: Pubkey,
    pub metadata_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCollectionData {
    pub name: String,
    pub symbol: String,
    pub royalty_bps: u16,
    pub max_supply: u64,
    pub public_mint: bool,
    pub mint_authority: Option<Pubkey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintNftData {
    pub token_id: u64,
    pub metadata_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NftActivityKind {
    Mint,
    Transfer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NftActivity {
    pub slot: u64,
    pub timestamp: u64,
    pub kind: NftActivityKind,
    pub collection: Pubkey,
    pub token: Pubkey,
    pub from: Option<Pubkey>,
    pub to: Pubkey,
    pub tx_signature: Hash,
}

pub fn encode_collection_state(state: &CollectionState) -> Result<Vec<u8>, String> {
    bincode::serialize(state).map_err(|e| format!("Failed to encode collection: {}", e))
}

pub fn decode_collection_state(data: &[u8]) -> Result<CollectionState, String> {
    bincode::deserialize(data).map_err(|e| format!("Failed to decode collection: {}", e))
}

pub fn encode_token_state(state: &TokenState) -> Result<Vec<u8>, String> {
    bincode::serialize(state).map_err(|e| format!("Failed to encode token: {}", e))
}

pub fn decode_token_state(data: &[u8]) -> Result<TokenState, String> {
    bincode::deserialize(data).map_err(|e| format!("Failed to decode token: {}", e))
}

pub fn decode_create_collection_data(data: &[u8]) -> Result<CreateCollectionData, String> {
    bincode::deserialize(data).map_err(|e| format!("Failed to decode collection data: {}", e))
}

pub fn decode_mint_nft_data(data: &[u8]) -> Result<MintNftData, String> {
    bincode::deserialize(data).map_err(|e| format!("Failed to decode mint data: {}", e))
}

pub fn encode_nft_activity(activity: &NftActivity) -> Result<Vec<u8>, String> {
    bincode::serialize(activity).map_err(|e| format!("Failed to encode NFT activity: {}", e))
}

pub fn decode_nft_activity(data: &[u8]) -> Result<NftActivity, String> {
    bincode::deserialize(data).map_err(|e| format!("Failed to decode NFT activity: {}", e))
}

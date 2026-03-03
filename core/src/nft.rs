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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Pubkey;
    use crate::hash::Hash;

    fn sample_collection() -> CollectionState {
        CollectionState {
            version: NFT_COLLECTION_VERSION,
            name: "LobsterPunks".to_string(),
            symbol: "LPNK".to_string(),
            creator: Pubkey([0xAAu8; 32]),
            royalty_bps: 500,
            max_supply: 10_000,
            minted: 42,
            public_mint: true,
            mint_authority: Some(Pubkey([0xBBu8; 32])),
        }
    }

    fn sample_token() -> TokenState {
        TokenState {
            version: NFT_TOKEN_VERSION,
            collection: Pubkey([0xAAu8; 32]),
            token_id: 7,
            owner: Pubkey([0xCCu8; 32]),
            metadata_uri: "ipfs://QmTest123".to_string(),
        }
    }

    fn sample_activity() -> NftActivity {
        NftActivity {
            slot: 100,
            timestamp: 1700000000,
            kind: NftActivityKind::Mint,
            collection: Pubkey([0xAAu8; 32]),
            token: Pubkey([0xDDu8; 32]),
            from: None,
            to: Pubkey([0xCCu8; 32]),
            tx_signature: Hash::new([0x11u8; 32]),
        }
    }

    // ── CollectionState round-trip ──

    #[test]
    fn collection_encode_decode_roundtrip() {
        let orig = sample_collection();
        let bytes = encode_collection_state(&orig).unwrap();
        let decoded = decode_collection_state(&bytes).unwrap();
        assert_eq!(decoded.name, orig.name);
        assert_eq!(decoded.symbol, orig.symbol);
        assert_eq!(decoded.creator, orig.creator);
        assert_eq!(decoded.royalty_bps, orig.royalty_bps);
        assert_eq!(decoded.max_supply, orig.max_supply);
        assert_eq!(decoded.minted, orig.minted);
        assert_eq!(decoded.public_mint, orig.public_mint);
        assert_eq!(decoded.mint_authority, orig.mint_authority);
        assert_eq!(decoded.version, NFT_COLLECTION_VERSION);
    }

    #[test]
    fn collection_no_mint_authority() {
        let mut col = sample_collection();
        col.mint_authority = None;
        let bytes = encode_collection_state(&col).unwrap();
        let decoded = decode_collection_state(&bytes).unwrap();
        assert!(decoded.mint_authority.is_none());
    }

    #[test]
    fn collection_decode_garbage_fails() {
        let result = decode_collection_state(&[0xFF; 4]);
        assert!(result.is_err());
    }

    #[test]
    fn collection_decode_empty_fails() {
        let result = decode_collection_state(&[]);
        assert!(result.is_err());
    }

    // ── TokenState round-trip ──

    #[test]
    fn token_encode_decode_roundtrip() {
        let orig = sample_token();
        let bytes = encode_token_state(&orig).unwrap();
        let decoded = decode_token_state(&bytes).unwrap();
        assert_eq!(decoded.version, orig.version);
        assert_eq!(decoded.collection, orig.collection);
        assert_eq!(decoded.token_id, orig.token_id);
        assert_eq!(decoded.owner, orig.owner);
        assert_eq!(decoded.metadata_uri, orig.metadata_uri);
    }

    #[test]
    fn token_decode_garbage_fails() {
        assert!(decode_token_state(&[0x00; 3]).is_err());
    }

    // ── CreateCollectionData ──

    #[test]
    fn create_collection_data_roundtrip() {
        let data = CreateCollectionData {
            name: "TestCol".to_string(),
            symbol: "TC".to_string(),
            royalty_bps: 250,
            max_supply: 1000,
            public_mint: false,
            mint_authority: None,
        };
        let bytes = bincode::serialize(&data).unwrap();
        let decoded = decode_create_collection_data(&bytes).unwrap();
        assert_eq!(decoded.name, "TestCol");
        assert_eq!(decoded.royalty_bps, 250);
        assert_eq!(decoded.max_supply, 1000);
        assert!(!decoded.public_mint);
    }

    #[test]
    fn create_collection_data_decode_garbage() {
        assert!(decode_create_collection_data(&[0xFF; 2]).is_err());
    }

    // ── MintNftData ──

    #[test]
    fn mint_nft_data_roundtrip() {
        let data = MintNftData {
            token_id: 42,
            metadata_uri: "https://example.com/meta.json".to_string(),
        };
        let bytes = bincode::serialize(&data).unwrap();
        let decoded = decode_mint_nft_data(&bytes).unwrap();
        assert_eq!(decoded.token_id, 42);
        assert_eq!(decoded.metadata_uri, "https://example.com/meta.json");
    }

    #[test]
    fn mint_nft_data_decode_garbage() {
        assert!(decode_mint_nft_data(&[]).is_err());
    }

    // ── NftActivity encode/decode ──

    #[test]
    fn activity_encode_decode_roundtrip() {
        let orig = sample_activity();
        let bytes = encode_nft_activity(&orig).unwrap();
        let decoded = decode_nft_activity(&bytes).unwrap();
        assert_eq!(decoded.slot, orig.slot);
        assert_eq!(decoded.timestamp, orig.timestamp);
        assert_eq!(decoded.collection, orig.collection);
        assert_eq!(decoded.token, orig.token);
        assert_eq!(decoded.to, orig.to);
        assert!(decoded.from.is_none());
    }

    #[test]
    fn activity_transfer_has_from() {
        let mut act = sample_activity();
        act.kind = NftActivityKind::Transfer;
        act.from = Some(Pubkey([0xEEu8; 32]));
        let bytes = encode_nft_activity(&act).unwrap();
        let decoded = decode_nft_activity(&bytes).unwrap();
        assert!(decoded.from.is_some());
        assert_eq!(decoded.from.unwrap(), Pubkey([0xEEu8; 32]));
    }

    #[test]
    fn activity_decode_garbage_fails() {
        assert!(decode_nft_activity(&[0x00; 5]).is_err());
    }

    // ── Version constants ──

    #[test]
    fn version_constants_nonzero() {
        assert!(NFT_COLLECTION_VERSION > 0);
        assert!(NFT_TOKEN_VERSION > 0);
    }
}

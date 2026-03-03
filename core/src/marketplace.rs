// MoltChain Core - Marketplace activity tracking

use crate::account::Pubkey;
use crate::hash::Hash;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MarketActivityKind {
    Listing,
    Sale,
    Cancel,
    Offer,
    OfferAccepted,
    OfferCancelled,
    PriceUpdate,
    AuctionCreated,
    AuctionBid,
    AuctionSettled,
    AuctionCancelled,
    CollectionOffer,
    CollectionOfferAccepted,
    Transfer,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Pubkey;
    use crate::hash::Hash;

    fn sample_activity(kind: MarketActivityKind) -> MarketActivity {
        MarketActivity {
            slot: 500,
            timestamp: 1700000000,
            kind,
            program: Pubkey([0xAAu8; 32]),
            collection: Some(Pubkey([0xBBu8; 32])),
            token: Some(Pubkey([0xCCu8; 32])),
            token_id: Some(42),
            price: Some(1_500_000_000),
            seller: Some(Pubkey([0xDDu8; 32])),
            buyer: Some(Pubkey([0xEEu8; 32])),
            function: "buy_now".to_string(),
            tx_signature: Hash::new([0x11u8; 32]),
        }
    }

    #[test]
    fn sale_activity_roundtrip() {
        let orig = sample_activity(MarketActivityKind::Sale);
        let bytes = encode_market_activity(&orig).unwrap();
        let decoded = decode_market_activity(&bytes).unwrap();
        assert_eq!(decoded.kind, MarketActivityKind::Sale);
        assert_eq!(decoded.slot, 500);
        assert_eq!(decoded.price, Some(1_500_000_000));
        assert_eq!(decoded.function, "buy_now");
    }

    #[test]
    fn listing_activity_roundtrip() {
        let orig = sample_activity(MarketActivityKind::Listing);
        let bytes = encode_market_activity(&orig).unwrap();
        let decoded = decode_market_activity(&bytes).unwrap();
        assert_eq!(decoded.kind, MarketActivityKind::Listing);
    }

    #[test]
    fn cancel_activity_roundtrip() {
        let orig = sample_activity(MarketActivityKind::Cancel);
        let bytes = encode_market_activity(&orig).unwrap();
        let decoded = decode_market_activity(&bytes).unwrap();
        assert_eq!(decoded.kind, MarketActivityKind::Cancel);
    }

    #[test]
    fn offer_activity_roundtrip() {
        let orig = sample_activity(MarketActivityKind::Offer);
        let bytes = encode_market_activity(&orig).unwrap();
        let decoded = decode_market_activity(&bytes).unwrap();
        assert_eq!(decoded.kind, MarketActivityKind::Offer);
    }

    #[test]
    fn auction_activities_roundtrip() {
        for kind in [
            MarketActivityKind::AuctionCreated,
            MarketActivityKind::AuctionBid,
            MarketActivityKind::AuctionSettled,
            MarketActivityKind::AuctionCancelled,
        ] {
            let orig = sample_activity(kind.clone());
            let bytes = encode_market_activity(&orig).unwrap();
            let decoded = decode_market_activity(&bytes).unwrap();
            assert_eq!(decoded.kind, kind);
        }
    }

    #[test]
    fn collection_offer_activities_roundtrip() {
        for kind in [
            MarketActivityKind::CollectionOffer,
            MarketActivityKind::CollectionOfferAccepted,
        ] {
            let orig = sample_activity(kind.clone());
            let bytes = encode_market_activity(&orig).unwrap();
            let decoded = decode_market_activity(&bytes).unwrap();
            assert_eq!(decoded.kind, kind);
        }
    }

    #[test]
    fn activity_with_optional_none_fields() {
        let mut act = sample_activity(MarketActivityKind::Transfer);
        act.collection = None;
        act.token = None;
        act.token_id = None;
        act.price = None;
        act.seller = None;
        act.buyer = None;
        let bytes = encode_market_activity(&act).unwrap();
        let decoded = decode_market_activity(&bytes).unwrap();
        assert!(decoded.collection.is_none());
        assert!(decoded.token.is_none());
        assert!(decoded.token_id.is_none());
        assert!(decoded.price.is_none());
        assert!(decoded.seller.is_none());
        assert!(decoded.buyer.is_none());
    }

    #[test]
    fn decode_garbage_fails() {
        assert!(decode_market_activity(&[0xFF; 4]).is_err());
    }

    #[test]
    fn decode_empty_fails() {
        assert!(decode_market_activity(&[]).is_err());
    }

    #[test]
    fn all_activity_kinds_distinct() {
        let kinds = vec![
            MarketActivityKind::Listing,
            MarketActivityKind::Sale,
            MarketActivityKind::Cancel,
            MarketActivityKind::Offer,
            MarketActivityKind::OfferAccepted,
            MarketActivityKind::OfferCancelled,
            MarketActivityKind::PriceUpdate,
            MarketActivityKind::AuctionCreated,
            MarketActivityKind::AuctionBid,
            MarketActivityKind::AuctionSettled,
            MarketActivityKind::AuctionCancelled,
            MarketActivityKind::CollectionOffer,
            MarketActivityKind::CollectionOfferAccepted,
            MarketActivityKind::Transfer,
        ];
        // Verify all 14 variants are covered
        assert_eq!(kinds.len(), 14);
        // Each serializes to different bytes
        let mut encoded: Vec<Vec<u8>> = Vec::new();
        for kind in &kinds {
            let act = sample_activity(kind.clone());
            let bytes = encode_market_activity(&act).unwrap();
            encoded.push(bytes);
        }
        // Each pair is different (different kind enum variant)
        for i in 0..encoded.len() {
            for j in (i + 1)..encoded.len() {
                assert_ne!(encoded[i], encoded[j], "Kinds {:?} and {:?} serialize identically", kinds[i], kinds[j]);
            }
        }
    }
}

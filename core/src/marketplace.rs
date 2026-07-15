// Lichen Core - Marketplace activity tracking

use crate::account::Pubkey;
use crate::codec::{deserialize_legacy_bincode, serialize_legacy_bincode};
use crate::hash::Hash;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

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
    serialize_legacy_bincode(activity, "market activity")
}

pub fn decode_market_activity(data: &[u8]) -> Result<MarketActivity, String> {
    deserialize_legacy_bincode(data, "market activity")
}

#[derive(Debug, Clone, Default)]
struct ParsedMarketArgs {
    collection: Option<Pubkey>,
    token: Option<Pubkey>,
    token_id: Option<u64>,
    price: Option<u64>,
    seller: Option<Pubkey>,
    buyer: Option<Pubkey>,
}

fn parse_marketplace_args_for_function(
    function: &str,
    args: &[u8],
    value: u64,
) -> ParsedMarketArgs {
    let mut parsed = ParsedMarketArgs::default();

    if args.is_empty() {
        return parsed;
    }

    let Ok(json) = serde_json::from_slice::<JsonValue>(args) else {
        return parsed;
    };

    let parse_pubkey = |val: &JsonValue| -> Option<Pubkey> {
        let s = val.as_str()?;
        Pubkey::from_base58(s).ok()
    };

    let parse_u64 = |val: &JsonValue| -> Option<u64> {
        if let Some(num) = val.as_u64() {
            return Some(num);
        }
        val.as_str().and_then(|s| s.parse::<u64>().ok())
    };

    if let Some(obj) = json.as_object() {
        if let Some(val) = obj
            .get("collection")
            .or_else(|| obj.get("nft_contract"))
            .or_else(|| obj.get("nftContract"))
        {
            parsed.collection = parse_pubkey(val);
        }

        if let Some(val) = obj.get("token") {
            parsed.token = parse_pubkey(val);
            if parsed.token.is_none() {
                parsed.token_id = parse_u64(val);
            }
        }

        if let Some(val) = obj.get("token_id").or_else(|| obj.get("tokenId")) {
            parsed.token_id = parse_u64(val);
        }

        if let Some(val) = obj.get("price") {
            parsed.price = parse_u64(val);
        }

        if let Some(val) = obj.get("seller") {
            parsed.seller = parse_pubkey(val);
        }

        if let Some(val) = obj.get("buyer") {
            parsed.buyer = parse_pubkey(val);
        }

        return parsed;
    }

    let Some(arr) = json.as_array() else {
        return parsed;
    };

    let pk = |idx: usize| -> Option<Pubkey> { arr.get(idx).and_then(parse_pubkey) };
    let num = |idx: usize| -> Option<u64> { arr.get(idx).and_then(parse_u64) };

    match function {
        "list_nft" | "list_nft_with_royalty" => {
            parsed.seller = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
            parsed.price = num(3);
        }
        "buy_nft" => {
            parsed.buyer = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
            parsed.price = (value > 0).then_some(value);
        }
        "cancel_listing" | "settle_auction" | "cancel_auction" => {
            parsed.seller = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
        }
        "make_offer" | "make_offer_with_expiry" => {
            parsed.buyer = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
            parsed.price = num(3).or_else(|| (value > 0).then_some(value));
        }
        "accept_offer" | "accept_collection_offer" => {
            parsed.seller = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
            parsed.buyer = pk(3);
        }
        "cancel_offer" => {
            parsed.buyer = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
        }
        "update_listing_price" => {
            parsed.seller = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
            parsed.price = num(3);
        }
        "create_auction" => {
            parsed.seller = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
            parsed.price = num(3);
        }
        "place_bid" => {
            parsed.buyer = pk(0);
            parsed.collection = pk(1);
            parsed.token_id = num(2);
            parsed.price = num(3).or_else(|| (value > 0).then_some(value));
        }
        "make_collection_offer" => {
            parsed.buyer = pk(0);
            parsed.collection = pk(1);
            parsed.price = num(2).or_else(|| (value > 0).then_some(value));
        }
        "cancel_collection_offer" => {
            parsed.buyer = pk(0);
            parsed.collection = pk(1);
        }
        _ => {}
    }

    parsed
}

pub fn market_activity_kind_for_contract_function(function: &str) -> Option<MarketActivityKind> {
    match function {
        "list_nft" | "list_nft_with_royalty" => Some(MarketActivityKind::Listing),
        "buy_nft" => Some(MarketActivityKind::Sale),
        "cancel_listing" => Some(MarketActivityKind::Cancel),
        "make_offer" | "make_offer_with_expiry" => Some(MarketActivityKind::Offer),
        "accept_offer" => Some(MarketActivityKind::OfferAccepted),
        "cancel_offer" => Some(MarketActivityKind::OfferCancelled),
        "update_listing_price" => Some(MarketActivityKind::PriceUpdate),
        "create_auction" => Some(MarketActivityKind::AuctionCreated),
        "place_bid" => Some(MarketActivityKind::AuctionBid),
        "settle_auction" => Some(MarketActivityKind::AuctionSettled),
        "cancel_auction" => Some(MarketActivityKind::AuctionCancelled),
        "make_collection_offer" => Some(MarketActivityKind::CollectionOffer),
        "accept_collection_offer" => Some(MarketActivityKind::CollectionOfferAccepted),
        "cancel_collection_offer" => Some(MarketActivityKind::OfferCancelled),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_market_activity_from_contract_call(
    kind: MarketActivityKind,
    function: String,
    program: Pubkey,
    caller: Pubkey,
    args: &[u8],
    call_value: u64,
    slot: u64,
    timestamp: u64,
    tx_signature: Hash,
) -> MarketActivity {
    let parsed = parse_marketplace_args_for_function(&function, args, call_value);

    let (seller, buyer) = match kind {
        MarketActivityKind::Listing | MarketActivityKind::Cancel => {
            (parsed.seller.or(Some(caller)), parsed.buyer)
        }
        MarketActivityKind::Sale => (parsed.seller, parsed.buyer.or(Some(caller))),
        _ => (parsed.seller, parsed.buyer),
    };

    MarketActivity {
        slot,
        timestamp,
        kind,
        program,
        collection: parsed.collection,
        token: parsed.token,
        token_id: parsed.token_id,
        price: parsed.price,
        seller,
        buyer,
        function,
        tx_signature,
    }
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
                assert_ne!(
                    encoded[i], encoded[j],
                    "Kinds {:?} and {:?} serialize identically",
                    kinds[i], kinds[j]
                );
            }
        }
    }
}

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

fn canonical_layout_fields<'a>(args: &'a [u8], widths: &[u8]) -> Option<Vec<&'a [u8]>> {
    if args.first().copied()? != 0xAB
        || args.get(1..1 + widths.len())? != widths
        || args.len()
            != 1usize
                .checked_add(widths.len())?
                .checked_add(widths.iter().map(|width| usize::from(*width)).sum())?
    {
        return None;
    }
    let mut cursor = 1 + widths.len();
    let mut fields = Vec::with_capacity(widths.len());
    for width in widths {
        let end = cursor.checked_add(usize::from(*width))?;
        fields.push(args.get(cursor..end)?);
        cursor = end;
    }
    Some(fields)
}

fn parse_canonical_marketplace_args(function: &str, args: &[u8]) -> Option<ParsedMarketArgs> {
    let mut parsed = ParsedMarketArgs::default();
    let read_pubkey = |field: &[u8]| -> Option<Pubkey> { Some(Pubkey(field.try_into().ok()?)) };
    let read_u64 =
        |field: &[u8]| -> Option<u64> { Some(u64::from_le_bytes(field.try_into().ok()?)) };

    match function {
        "list_nft" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 8, 32])?;
            parsed.seller = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
            parsed.price = read_u64(fields[3]);
            parsed.token = read_pubkey(fields[4]);
        }
        "list_nft_with_royalty" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 8, 32, 32, 4])?;
            parsed.seller = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
            parsed.price = read_u64(fields[3]);
            parsed.token = read_pubkey(fields[4]);
        }
        "buy_nft" | "cancel_listing" | "settle_auction" | "cancel_auction" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8])?;
            if function == "buy_nft" {
                parsed.buyer = read_pubkey(fields[0]);
            } else {
                parsed.seller = read_pubkey(fields[0]);
            }
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
        }
        "make_offer" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 8, 32])?;
            parsed.buyer = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
            parsed.price = read_u64(fields[3]);
            parsed.token = read_pubkey(fields[4]);
        }
        "make_offer_with_expiry" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 8, 32, 8])?;
            parsed.buyer = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
            parsed.price = read_u64(fields[3]);
            parsed.token = read_pubkey(fields[4]);
        }
        "accept_offer" | "accept_collection_offer" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 32])?;
            parsed.seller = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
            parsed.buyer = read_pubkey(fields[3]);
        }
        "cancel_offer" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8])?;
            parsed.buyer = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
        }
        "update_listing_price" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 8])?;
            parsed.seller = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
            parsed.price = read_u64(fields[3]);
        }
        "create_auction" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 8, 8, 8, 32])?;
            parsed.seller = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
            parsed.price = read_u64(fields[3]);
            parsed.token = read_pubkey(fields[6]);
        }
        "place_bid" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 8])?;
            parsed.buyer = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.token_id = read_u64(fields[2]);
            parsed.price = read_u64(fields[3]);
        }
        "make_collection_offer" => {
            let fields = canonical_layout_fields(args, &[32, 32, 8, 32, 8])?;
            parsed.buyer = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
            parsed.price = read_u64(fields[2]);
            parsed.token = read_pubkey(fields[3]);
        }
        "cancel_collection_offer" => {
            let fields = canonical_layout_fields(args, &[32, 32])?;
            parsed.buyer = read_pubkey(fields[0]);
            parsed.collection = read_pubkey(fields[1]);
        }
        _ => return None,
    }
    Some(parsed)
}

fn apply_market_call_value(
    function: &str,
    value: u64,
    mut parsed: ParsedMarketArgs,
) -> ParsedMarketArgs {
    if parsed.price.is_none()
        && value > 0
        && matches!(
            function,
            "buy_nft"
                | "make_offer"
                | "make_offer_with_expiry"
                | "place_bid"
                | "make_collection_offer"
        )
    {
        parsed.price = Some(value);
    }
    parsed
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

    if args.first() == Some(&0xAB) {
        return apply_market_call_value(
            function,
            value,
            parse_canonical_marketplace_args(function, args).unwrap_or_default(),
        );
    }

    let Ok(json) = serde_json::from_slice::<JsonValue>(args) else {
        return apply_market_call_value(function, value, parsed);
    };

    let parse_pubkey = |val: &JsonValue| -> Option<Pubkey> {
        let s = val.as_str()?;
        if s.is_empty() {
            return Some(Pubkey([0u8; 32]));
        }
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
            parsed.token = pk(4);
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
            parsed.token = pk(4);
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
            parsed.token = pk(6);
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
            parsed.token = pk(3);
        }
        "cancel_collection_offer" => {
            parsed.buyer = pk(0);
            parsed.collection = pk(1);
        }
        _ => {}
    }

    apply_market_call_value(function, value, parsed)
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

    fn canonical_args(widths: &[u8], fields: &[&[u8]]) -> Vec<u8> {
        assert_eq!(widths.len(), fields.len());
        let mut args = Vec::new();
        args.push(0xAB);
        args.extend_from_slice(widths);
        for (width, field) in widths.iter().zip(fields) {
            assert_eq!(usize::from(*width), field.len());
            args.extend_from_slice(field);
        }
        args
    }

    #[test]
    fn canonical_listing_parser_matches_named_export_layout() {
        let seller = [0x11u8; 32];
        let collection = [0x22u8; 32];
        let token_id = 77u64.to_le_bytes();
        let price = 9_500u64.to_le_bytes();
        let payment_token = [0x33u8; 32];
        let royalty_recipient = [0x44u8; 32];
        let royalty_bps = 250u32.to_le_bytes();
        let args = canonical_args(
            &[32, 32, 8, 8, 32, 32, 4],
            &[
                &seller,
                &collection,
                &token_id,
                &price,
                &payment_token,
                &royalty_recipient,
                &royalty_bps,
            ],
        );

        let parsed = parse_marketplace_args_for_function("list_nft_with_royalty", &args, 0);
        assert_eq!(parsed.seller, Some(Pubkey(seller)));
        assert_eq!(parsed.collection, Some(Pubkey(collection)));
        assert_eq!(parsed.token_id, Some(77));
        assert_eq!(parsed.price, Some(9_500));
        assert_eq!(parsed.token, Some(Pubkey(payment_token)));
    }

    #[test]
    fn canonical_native_purchase_uses_exact_call_value() {
        let buyer = [0x51u8; 32];
        let collection = [0x52u8; 32];
        let token_id = 8u64.to_le_bytes();
        let args = canonical_args(&[32, 32, 8], &[&buyer, &collection, &token_id]);

        let parsed = parse_marketplace_args_for_function("buy_nft", &args, 42_000);
        assert_eq!(parsed.buyer, Some(Pubkey(buyer)));
        assert_eq!(parsed.collection, Some(Pubkey(collection)));
        assert_eq!(parsed.token_id, Some(8));
        assert_eq!(parsed.price, Some(42_000));
    }

    #[test]
    fn malformed_canonical_layout_fails_closed() {
        let buyer = [0x61u8; 32];
        let collection = [0x62u8; 32];
        let token_id = 9u64.to_le_bytes();
        let mut args = canonical_args(&[32, 32, 8], &[&buyer, &collection, &token_id]);
        args.push(0);

        let parsed = parse_marketplace_args_for_function("buy_nft", &args, 0);
        assert_eq!(parsed.buyer, None);
        assert_eq!(parsed.collection, None);
        assert_eq!(parsed.token_id, None);
        assert_eq!(parsed.price, None);
    }

    #[test]
    fn legacy_json_listing_maps_native_token_sentinel() {
        let seller = Pubkey([0x71u8; 32]);
        let collection = Pubkey([0x72u8; 32]);
        let args = serde_json::to_vec(&serde_json::json!([
            seller.to_base58(),
            collection.to_base58(),
            "12",
            "8000",
            ""
        ]))
        .unwrap();

        let parsed = parse_marketplace_args_for_function("list_nft", &args, 0);
        assert_eq!(parsed.seller, Some(seller));
        assert_eq!(parsed.collection, Some(collection));
        assert_eq!(parsed.token_id, Some(12));
        assert_eq!(parsed.price, Some(8_000));
        assert_eq!(parsed.token, Some(Pubkey([0u8; 32])));
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

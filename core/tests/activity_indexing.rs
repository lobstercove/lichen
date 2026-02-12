use moltchain_core::{
    Hash, MarketActivity, MarketActivityKind, NftActivity, NftActivityKind, ProgramCallActivity,
    Pubkey, StateStore,
};
use tempfile::tempdir;

#[test]
fn records_and_reads_nft_activity() {
    let temp_dir = tempdir().unwrap();
    let state = StateStore::open(temp_dir.path()).unwrap();

    let collection = Pubkey([1u8; 32]);
    let token_a = Pubkey([2u8; 32]);
    let token_b = Pubkey([3u8; 32]);
    let owner = Pubkey([4u8; 32]);

    let mint = NftActivity {
        slot: 10,
        timestamp: 1_700_000_000_000,
        kind: NftActivityKind::Mint,
        collection,
        token: token_a,
        from: None,
        to: owner,
        tx_signature: Hash::hash(b"mint-tx"),
    };

    let transfer = NftActivity {
        slot: 11,
        timestamp: 1_700_000_001_000,
        kind: NftActivityKind::Transfer,
        collection,
        token: token_b,
        from: Some(owner),
        to: Pubkey([5u8; 32]),
        tx_signature: Hash::hash(b"transfer-tx"),
    };

    state.record_nft_activity(&mint, 0).unwrap();
    state.record_nft_activity(&transfer, 1).unwrap();

    let activity = state
        .get_nft_activity_by_collection(&collection, 10)
        .unwrap();
    assert_eq!(activity.len(), 2);
    assert_eq!(activity[0].slot, 11);
    assert_eq!(activity[1].slot, 10);
}

#[test]
fn records_and_reads_program_calls() {
    let temp_dir = tempdir().unwrap();
    let state = StateStore::open(temp_dir.path()).unwrap();

    let program = Pubkey([9u8; 32]);
    let caller = Pubkey([8u8; 32]);

    let first = ProgramCallActivity {
        slot: 20,
        timestamp: 1_700_000_002_000,
        program,
        caller,
        function: "ping".to_string(),
        value: 0,
        tx_signature: Hash::hash(b"call-1"),
    };

    let second = ProgramCallActivity {
        slot: 21,
        timestamp: 1_700_000_003_000,
        program,
        caller,
        function: "pong".to_string(),
        value: 42,
        tx_signature: Hash::hash(b"call-2"),
    };

    state.record_program_call(&first, 0).unwrap();
    state.record_program_call(&second, 1).unwrap();

    let calls = state.get_program_calls(&program, 10).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function, "pong");
    assert_eq!(calls[1].function, "ping");
    assert_eq!(state.count_program_calls(&program).unwrap(), 2);
}

#[test]
fn records_and_reads_market_activity() {
    let temp_dir = tempdir().unwrap();
    let state = StateStore::open(temp_dir.path()).unwrap();

    let program = Pubkey([7u8; 32]);
    let collection = Pubkey([6u8; 32]);
    let token = Pubkey([5u8; 32]);
    let seller = Pubkey([4u8; 32]);
    let buyer = Pubkey([3u8; 32]);

    let listing = MarketActivity {
        slot: 30,
        timestamp: 1_700_000_004_000,
        kind: MarketActivityKind::Listing,
        program,
        collection: Some(collection),
        token: Some(token),
        token_id: Some(42),
        price: Some(1_500_000_000),
        seller: Some(seller),
        buyer: None,
        function: "list_nft".to_string(),
        tx_signature: Hash::hash(b"listing-tx"),
    };

    let sale = MarketActivity {
        slot: 31,
        timestamp: 1_700_000_005_000,
        kind: MarketActivityKind::Sale,
        program,
        collection: Some(collection),
        token: Some(token),
        token_id: Some(42),
        price: Some(2_000_000_000),
        seller: Some(seller),
        buyer: Some(buyer),
        function: "buy_nft".to_string(),
        tx_signature: Hash::hash(b"sale-tx"),
    };

    state.record_market_activity(&listing, 0).unwrap();
    state.record_market_activity(&sale, 1).unwrap();

    let activity = state
        .get_market_activity(Some(&collection), None, 10)
        .unwrap();
    assert_eq!(activity.len(), 2);
    assert_eq!(activity[0].kind, MarketActivityKind::Sale);
    assert_eq!(activity[1].kind, MarketActivityKind::Listing);

    let listings = state
        .get_market_activity(Some(&collection), Some(MarketActivityKind::Listing), 10)
        .unwrap();
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].function, "list_nft");
}

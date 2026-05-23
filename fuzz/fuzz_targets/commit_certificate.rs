#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::{
    compute_bft_timestamp, Block, CommitSignature, Hash, Keypair, Precommit, StakePool,
    ValidatorInfo, ValidatorSet, MIN_VALIDATOR_STAKE,
};

const CHAIN_ID: &str = "lichen-testnet-1";

fuzz_target!(|data: &[u8]| {
    let keys = validator_keys(data);
    let (validator_set, stake_pool) = validator_fixture(&keys, None);
    let mut block = Block::new_with_timestamp(
        read_u64(data, 1).max(1),
        Hash::hash(b"parent"),
        Hash::hash(data),
        keys[0].pubkey().0,
        Vec::new(),
        1_700_000_000u64.saturating_add(read_u64(data, 9) % 60),
    );
    block.commit_round = read_u32(data, 17) % 4;
    block.sign_with_chain_id(&keys[0], CHAIN_ID);
    assert!(block.verify_signature_with_chain_id(CHAIN_ID));

    let valid_commit = vec![
        commit_signature(
            &block,
            &keys[0],
            1_700_000_001,
            CHAIN_ID,
            block.commit_round,
            block.hash(),
        ),
        commit_signature(
            &block,
            &keys[1],
            1_700_000_003,
            CHAIN_ID,
            block.commit_round,
            block.hash(),
        ),
    ];
    let mut valid_block = block.clone();
    valid_block.commit_signatures = valid_commit.clone();
    assert!(valid_block
        .verify_commit_with_chain_id(CHAIN_ID, &validator_set, &stake_pool)
        .is_ok());
    assert_eq!(
        compute_bft_timestamp(
            &valid_commit,
            &validator_set,
            &stake_pool,
            Some(1_700_000_000)
        ),
        Some(1_700_000_003)
    );

    let mut candidate = block.clone();
    match data.first().copied().unwrap_or(0) % 8 {
        0 => {
            candidate.commit_signatures = Vec::new();
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &validator_set, &stake_pool)
                .is_err());
        }
        1 => {
            candidate.commit_signatures = vec![valid_commit[0].clone(), valid_commit[0].clone()];
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &validator_set, &stake_pool)
                .is_err());
        }
        2 => {
            let mut bad_timestamp = valid_commit[0].clone();
            bad_timestamp.timestamp = bad_timestamp.timestamp.saturating_add(1);
            candidate.commit_signatures = vec![bad_timestamp, valid_commit[1].clone()];
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &validator_set, &stake_pool)
                .is_err());
        }
        3 => {
            candidate.commit_signatures = vec![
                commit_signature(
                    &block,
                    &keys[0],
                    1_700_000_001,
                    CHAIN_ID,
                    block.commit_round.saturating_add(1),
                    block.hash(),
                ),
                commit_signature(
                    &block,
                    &keys[1],
                    1_700_000_003,
                    CHAIN_ID,
                    block.commit_round.saturating_add(1),
                    block.hash(),
                ),
            ];
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &validator_set, &stake_pool)
                .is_err());
        }
        4 => {
            let mut unknown = valid_commit[0].clone();
            unknown.validator = [0xEE; 32];
            candidate.commit_signatures = vec![unknown, valid_commit[1].clone()];
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &validator_set, &stake_pool)
                .is_err());
        }
        5 => {
            let (pending_set, pending_pool) = validator_fixture(&keys, Some(2));
            candidate.commit_signatures = vec![valid_commit[0].clone(), valid_commit[1].clone()];
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &pending_set, &pending_pool)
                .is_ok());
            candidate.commit_signatures = vec![commit_signature(
                &block,
                &keys[2],
                1_700_000_005,
                CHAIN_ID,
                block.commit_round,
                block.hash(),
            )];
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &pending_set, &pending_pool)
                .is_err());
        }
        6 => {
            candidate.commit_signatures = vec![
                commit_signature(
                    &block,
                    &keys[0],
                    1_700_000_001,
                    "",
                    block.commit_round,
                    block.hash(),
                ),
                commit_signature(
                    &block,
                    &keys[1],
                    1_700_000_003,
                    "",
                    block.commit_round,
                    block.hash(),
                ),
            ];
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &validator_set, &stake_pool)
                .is_ok());
        }
        _ => {
            candidate.commit_signatures = vec![
                commit_signature(
                    &block,
                    &keys[0],
                    1_700_000_001,
                    CHAIN_ID,
                    block.commit_round,
                    Hash::hash(b"wrong block"),
                ),
                commit_signature(
                    &block,
                    &keys[1],
                    1_700_000_003,
                    CHAIN_ID,
                    block.commit_round,
                    Hash::hash(b"wrong block"),
                ),
            ];
            assert!(candidate
                .verify_commit_with_chain_id(CHAIN_ID, &validator_set, &stake_pool)
                .is_err());
        }
    }

    let _ = compute_bft_timestamp(
        &candidate.commit_signatures,
        &validator_set,
        &stake_pool,
        Some(1_700_000_000),
    );
});

fn validator_keys(data: &[u8]) -> [Keypair; 3] {
    [
        Keypair::from_seed(&seed(data, 1)),
        Keypair::from_seed(&seed(data, 2)),
        Keypair::from_seed(&seed(data, 3)),
    ]
}

fn validator_fixture(
    keys: &[Keypair; 3],
    pending_index: Option<usize>,
) -> (ValidatorSet, StakePool) {
    let mut validator_set = ValidatorSet::new();
    let mut stake_pool = StakePool::new();
    for (idx, key) in keys.iter().enumerate() {
        let pubkey = key.pubkey();
        let stake = MIN_VALIDATOR_STAKE;
        let mut info = ValidatorInfo::new(pubkey, 0);
        info.stake = stake;
        info.pending_activation = pending_index == Some(idx);
        validator_set.add_validator(info);
        stake_pool
            .stake(pubkey, stake, 0)
            .expect("validator stake fixture");
    }
    (validator_set, stake_pool)
}

fn commit_signature(
    block: &Block,
    keypair: &Keypair,
    timestamp: u64,
    chain_id: &str,
    round: u32,
    block_hash: Hash,
) -> CommitSignature {
    let signable = if chain_id.is_empty() {
        Precommit::signable_bytes(block.header.slot, round, &Some(block_hash), timestamp)
    } else {
        Precommit::signing_bytes_for_chain_id(
            chain_id,
            block.header.slot,
            round,
            &Some(block_hash),
            timestamp,
        )
    };
    CommitSignature {
        validator: keypair.pubkey().0,
        signature: keypair.sign(&signable),
        timestamp,
    }
}

fn seed(data: &[u8], domain: u8) -> [u8; 32] {
    let mut seed = [domain; 32];
    for (idx, byte) in data.iter().take(31).enumerate() {
        seed[idx + 1] ^= *byte;
    }
    seed
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    for (idx, byte) in bytes.iter_mut().enumerate() {
        *byte = data.get(offset + idx).copied().unwrap_or(0);
    }
    u64::from_le_bytes(bytes)
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    for (idx, byte) in bytes.iter_mut().enumerate() {
        *byte = data.get(offset + idx).copied().unwrap_or(0);
    }
    u32::from_le_bytes(bytes)
}

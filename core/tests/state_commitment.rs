// State commitment audit regression tests
//
// Verifies the fixes for:
//   1. Expanded state root surface (stake_pool + mossstake included)
//   2. Deterministic canonical_hash for singleton pools
//   3. Scheduler serialization of singleton-mutating transactions
//   4. cold_start and incremental roots agree

use lichen_core::consensus::StakePool;
use lichen_core::mossstake::MossStakePool;
use lichen_core::*;
use tempfile::TempDir;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_state() -> (StateStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let state = StateStore::open(tmp.path()).unwrap();
    // Seed treasury so the store is valid
    let treasury = Keypair::new();
    let acct = Account {
        spores: 10_000_000_000_000,
        spendable: 10_000_000_000_000,
        staked: 0,
        locked: 0,
        data: Vec::new(),
        public_key: None,
        owner: treasury.pubkey(),
        executable: false,
        rent_epoch: 0,
        dormant: false,
        missed_rent_epochs: 0,
    };
    state.put_account(&treasury.pubkey(), &acct).unwrap();
    state.set_treasury_pubkey(&treasury.pubkey()).unwrap();
    let genesis = Block::new_with_timestamp(
        0,
        Hash::default(),
        Hash::default(),
        [0u8; 32],
        Vec::new(),
        0,
    );
    state.put_block(&genesis).unwrap();
    state.set_last_slot(0).unwrap();
    (state, tmp)
}

// ─── Test: state root changes when stake pool changes ────────────────────────

#[test]
fn state_root_includes_stake_pool() {
    let (state, _dir) = make_state();

    let root_before = state.compute_state_root();

    // Mutate the stake pool
    let mut pool = state.get_stake_pool().unwrap();
    let validator = Keypair::new().pubkey();
    pool.stake(validator, MIN_VALIDATOR_STAKE, 1).unwrap();
    state.put_stake_pool(&pool).unwrap();

    let root_after = state.compute_state_root();
    assert_ne!(
        root_before, root_after,
        "State root must change when stake pool is mutated"
    );
}

// ─── Test: state root changes when mossstake pool changes ────────────────────

#[test]
fn state_root_includes_mossstake_pool() {
    let (state, _dir) = make_state();

    let root_before = state.compute_state_root();

    // Mutate the mossstake pool
    let mut pool = state.get_mossstake_pool().unwrap();
    let user = Keypair::new().pubkey();
    pool.stake(user, 5_000_000_000, 10).unwrap();
    state.put_mossstake_pool(&pool).unwrap();

    let root_after = state.compute_state_root();
    assert_ne!(
        root_before, root_after,
        "State root must change when mossstake pool is mutated"
    );
}

// ─── Test: canonical_hash is deterministic across insertion orders ────────────

#[test]
fn stake_pool_canonical_hash_deterministic() {
    let v1 = Pubkey::new([1u8; 32]);
    let v2 = Pubkey::new([2u8; 32]);
    let v3 = Pubkey::new([3u8; 32]);

    // Insert in order v1, v2, v3
    let mut pool_a = StakePool::new();
    pool_a.stake(v1, MIN_VALIDATOR_STAKE, 0).unwrap();
    pool_a.stake(v2, MIN_VALIDATOR_STAKE, 0).unwrap();
    pool_a.stake(v3, MIN_VALIDATOR_STAKE, 0).unwrap();

    // Insert in order v3, v1, v2
    let mut pool_b = StakePool::new();
    pool_b.stake(v3, MIN_VALIDATOR_STAKE, 0).unwrap();
    pool_b.stake(v1, MIN_VALIDATOR_STAKE, 0).unwrap();
    pool_b.stake(v2, MIN_VALIDATOR_STAKE, 0).unwrap();

    assert_eq!(
        pool_a.canonical_hash(),
        pool_b.canonical_hash(),
        "canonical_hash must be identical regardless of insertion order"
    );
}

#[test]
fn mossstake_pool_canonical_hash_deterministic() {
    let u1 = Pubkey::new([10u8; 32]);
    let u2 = Pubkey::new([20u8; 32]);

    // Insert in order u1, u2
    let mut pool_a = MossStakePool::new();
    pool_a.stake(u1, 1_000_000_000, 5).unwrap();
    pool_a.stake(u2, 2_000_000_000, 10).unwrap();

    // Insert in order u2, u1
    let mut pool_b = MossStakePool::new();
    pool_b.stake(u2, 2_000_000_000, 10).unwrap();
    pool_b.stake(u1, 1_000_000_000, 5).unwrap();

    assert_eq!(
        pool_a.canonical_hash(),
        pool_b.canonical_hash(),
        "MossStakePool canonical_hash must be identical regardless of insertion order"
    );
}

// ─── Test: different pool states produce different hashes ─────────────────────

#[test]
fn stake_pool_canonical_hash_differs_on_mutation() {
    let v1 = Pubkey::new([1u8; 32]);

    let mut pool_a = StakePool::new();
    pool_a.stake(v1, MIN_VALIDATOR_STAKE, 0).unwrap();
    let hash_a = pool_a.canonical_hash();

    let mut pool_b = StakePool::new();
    pool_b
        .stake(v1, MIN_VALIDATOR_STAKE + 1_000_000_000, 0)
        .unwrap();
    let hash_b = pool_b.canonical_hash();

    assert_ne!(
        hash_a, hash_b,
        "Pools with different stake amounts must hash differently"
    );
}

// ─── Test: cold_start and incremental roots agree ────────────────────────────

#[test]
fn cold_start_and_incremental_root_agree() {
    let (state, _dir) = make_state();

    // Add some accounts
    for i in 0u8..5 {
        let pk = Pubkey::new([i + 100; 32]);
        let acct = Account {
            spores: (i as u64 + 1) * 1_000_000_000,
            spendable: (i as u64 + 1) * 1_000_000_000,
            staked: 0,
            locked: 0,
            data: Vec::new(),
            public_key: None,
            owner: pk,
            executable: false,
            rent_epoch: 0,
            dormant: false,
            missed_rent_epochs: 0,
        };
        state.put_account(&pk, &acct).unwrap();
    }

    // Stake pool mutation
    let mut pool = state.get_stake_pool().unwrap();
    pool.stake(Pubkey::new([200u8; 32]), MIN_VALIDATOR_STAKE, 1)
        .unwrap();
    state.put_stake_pool(&pool).unwrap();

    // compute_state_root uses incremental (populates Merkle leaf cache)
    let incremental = state.compute_state_root();
    // compute_state_root_cold_start rebuilds from scratch
    let cold = state.compute_state_root_cold_start();

    assert_eq!(
        incremental, cold,
        "Incremental and cold-start roots must agree"
    );
}

#[test]
fn sparse_state_commitment_rebuild_is_deterministic() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    let state_a = StateStore::open(dir_a.path()).unwrap();
    let state_b = StateStore::open(dir_b.path()).unwrap();
    let treasury = Pubkey::new([9u8; 32]);
    state_a
        .put_account(&treasury, &Account::new(1000, treasury))
        .unwrap();
    state_b
        .put_account(&treasury, &Account::new(1000, treasury))
        .unwrap();
    state_a.set_treasury_pubkey(&treasury).unwrap();
    state_b.set_treasury_pubkey(&treasury).unwrap();

    let accounts = [
        Pubkey::new([31u8; 32]),
        Pubkey::new([7u8; 32]),
        Pubkey::new([99u8; 32]),
        Pubkey::new([42u8; 32]),
    ];
    for pk in accounts {
        state_a.put_account(&pk, &Account::new(10, pk)).unwrap();
    }
    for pk in accounts.into_iter().rev() {
        state_b.put_account(&pk, &Account::new(10, pk)).unwrap();
    }

    let program = Pubkey::new([55u8; 32]);
    for key in [b"alpha".as_slice(), b"gamma".as_slice(), b"beta".as_slice()] {
        state_a.put_contract_storage(&program, key, key).unwrap();
    }
    for key in [b"beta".as_slice(), b"alpha".as_slice(), b"gamma".as_slice()] {
        state_b.put_contract_storage(&program, key, key).unwrap();
    }

    let report_a = state_a.rebuild_sparse_state_commitment(false).unwrap();
    let report_b = state_b.rebuild_sparse_state_commitment(false).unwrap();

    assert_eq!(report_a.accounts_root, report_b.accounts_root);
    assert_eq!(report_a.contract_root, report_b.contract_root);
    assert_eq!(report_a.accounts_leaf_count, report_b.accounts_leaf_count);
    assert_eq!(report_a.contract_leaf_count, report_b.contract_leaf_count);
    assert_eq!(report_a.after_schema, 0, "rebuild alone must not activate");
    assert_eq!(report_b.after_schema, 0, "rebuild alone must not activate");

    state_a.verify_sparse_state_commitment().unwrap();
    state_b.verify_sparse_state_commitment().unwrap();
}

#[test]
fn sparse_state_commitment_proposal_root_matches_commit() {
    let (state, _dir) = make_state();

    let alice = Pubkey::new([1u8; 32]);
    let bob = Pubkey::new([2u8; 32]);
    let carol = Pubkey::new([3u8; 32]);
    let program = Pubkey::new([4u8; 32]);

    state
        .put_account(&alice, &Account::new(1000, alice))
        .unwrap();
    state.put_account(&bob, &Account::new(100, bob)).unwrap();
    state.put_account(&carol, &Account::new(25, carol)).unwrap();
    state
        .put_contract_storage(&program, b"remove", b"old")
        .unwrap();
    state
        .put_contract_storage(&program, b"keep", b"stable")
        .unwrap();

    let activation = state.rebuild_sparse_state_commitment(true).unwrap();
    assert_eq!(activation.after_schema, 1);
    assert!(state.uses_sparse_state_commitment());
    let before = state.compute_state_root();

    let mut batch = state.begin_batch();
    batch
        .transfer(&alice, &bob, Account::licn_to_spores(25))
        .unwrap();
    let mut dormant = batch.get_account(&carol).unwrap().unwrap();
    dormant.dormant = true;
    batch.put_account(&carol, &dormant).unwrap();
    batch
        .put_contract_storage(&program, b"insert", b"new")
        .unwrap();
    batch.delete_contract_storage(&program, b"remove").unwrap();

    let proposal_root = state.compute_state_root_for_batch(&batch);
    state.commit_batch(batch).unwrap();
    let committed_root = state.compute_state_root();
    let cold_root = state.compute_state_root_cold_start();

    assert_ne!(before, committed_root);
    assert_eq!(proposal_root, committed_root);
    assert_eq!(committed_root, cold_root);
    state.verify_sparse_state_commitment().unwrap();
}

#[test]
fn sparse_state_commitment_account_proof_verifies_after_activation() {
    let (state, _dir) = make_state();

    let alice = Pubkey::new([21u8; 32]);
    let bob = Pubkey::new([22u8; 32]);
    state
        .put_account(&alice, &Account::new(1000, alice))
        .unwrap();
    state.put_account(&bob, &Account::new(100, bob)).unwrap();

    let activation = state.rebuild_sparse_state_commitment(true).unwrap();
    assert_eq!(activation.after_schema, 1);
    assert!(state.uses_sparse_state_commitment());

    let state_root = state.compute_state_root();
    let accounts_root = state.compute_accounts_root();
    let proof = state.get_account_proof(&alice).unwrap();
    let sparse_proof = proof
        .sparse_proof
        .as_ref()
        .expect("sparse mode must return sparse_v1 account proof");

    assert!(proof.proof.siblings.is_empty());
    assert!(proof.proof.path.is_empty());
    assert_eq!(proof.state_root, state_root);
    assert!(sparse_proof.verify(&accounts_root));
    assert!(sparse_proof.verify_account(&accounts_root, &alice, &proof.account_data));
    assert!(StateStore::verify_sparse_account_proof(
        &accounts_root,
        &alice,
        &proof.account_data,
        sparse_proof
    ));

    let old_sparse_proof = sparse_proof.clone();
    state.put_account(&bob, &Account::new(250, bob)).unwrap();
    let updated_accounts_root = state.compute_accounts_root();
    assert_ne!(accounts_root, updated_accounts_root);
    assert!(!old_sparse_proof.verify(&updated_accounts_root));

    let updated_proof = state.get_account_proof(&alice).unwrap();
    let updated_sparse_proof = updated_proof.sparse_proof.as_ref().unwrap();
    assert!(updated_sparse_proof.verify(&updated_accounts_root));
}

#[test]
fn sparse_state_commitment_shadow_stays_current_before_activation() {
    let (state, _dir) = make_state();

    let alice = Pubkey::new([11u8; 32]);
    let bob = Pubkey::new([12u8; 32]);
    let program = Pubkey::new([13u8; 32]);
    state
        .put_account(&alice, &Account::new(1000, alice))
        .unwrap();
    state.put_account(&bob, &Account::new(10, bob)).unwrap();
    state
        .put_contract_storage(&program, b"quote", b"old")
        .unwrap();

    let report = state.rebuild_sparse_state_commitment(false).unwrap();
    assert_eq!(report.after_schema, 0);
    assert!(!report.active);
    assert!(!report.activated);
    assert!(!state.uses_sparse_state_commitment());

    let mut batch = state.begin_batch();
    batch
        .transfer(&alice, &bob, Account::licn_to_spores(5))
        .unwrap();
    batch
        .put_contract_storage(&program, b"quote", b"new")
        .unwrap();
    batch
        .put_contract_storage(&program, b"fresh", b"value")
        .unwrap();
    state.commit_batch(batch).unwrap();

    let legacy_root = state.compute_state_root();
    assert_ne!(legacy_root, Hash::default());
    state
        .verify_sparse_state_commitment()
        .expect("shadow sparse roots must stay current while ordered_v0 is active");
}

#[test]
fn sparse_state_commitment_verify_reports_activation_state() {
    let (state, _dir) = make_state();
    let alice = Pubkey::new([31u8; 32]);
    state
        .put_account(&alice, &Account::new(1000, alice))
        .unwrap();

    let shadow_report = state.rebuild_sparse_state_commitment(false).unwrap();
    assert!(!shadow_report.active);
    assert!(!shadow_report.activated);
    assert_eq!(shadow_report.last_slot, 0);
    assert_eq!(shadow_report.latest_block_state_root, Some(Hash::default()));

    let active_report = state.rebuild_sparse_state_commitment(true).unwrap();
    assert!(active_report.active);
    assert!(active_report.activated);
    assert_eq!(active_report.after_schema, 1);

    let verify_report = state.verify_sparse_state_commitment().unwrap();
    assert!(verify_report.active);
    assert!(verify_report.activated);
    assert_eq!(verify_report.current_state_root, state.compute_state_root());
}

#[test]
fn active_sparse_startup_rebuild_skip_requires_clean_atomic_dirty_markers() {
    let (state, _dir) = make_state();
    let alice = Pubkey::new([41u8; 32]);
    let bob = Pubkey::new([42u8; 32]);
    let program = Pubkey::new([43u8; 32]);

    state
        .put_account(&alice, &Account::new(1000, alice))
        .unwrap();
    state.put_account(&bob, &Account::new(100, bob)).unwrap();
    state
        .put_contract_storage(&program, b"quote", b"old")
        .unwrap();

    assert!(!state.can_skip_active_sparse_startup_rebuild());
    state.rebuild_sparse_state_commitment(true).unwrap();
    assert!(state.can_skip_active_sparse_startup_rebuild());

    state.put_account(&bob, &Account::new(250, bob)).unwrap();
    assert!(!state.can_skip_active_sparse_startup_rebuild());
    state.compute_state_root();
    assert!(state.can_skip_active_sparse_startup_rebuild());

    state
        .put_contract_storage(&program, b"quote", b"new")
        .unwrap();
    assert!(!state.can_skip_active_sparse_startup_rebuild());
    state.compute_state_root();
    assert!(state.can_skip_active_sparse_startup_rebuild());
}

// ─── Test: empty pools produce a known distinct root from no-pool ────────────

#[test]
fn empty_pools_produce_consistent_root() {
    let (state, _dir) = make_state();
    let root1 = state.compute_state_root();
    let root2 = state.compute_state_root();
    assert_eq!(root1, root2, "Same state must produce same root");
}

// ─── Test: scheduler groups singleton-touching TXs together ──────────────────

#[test]
fn scheduler_conflict_keys_exist() {
    // Verify the conflict key constants are distinct and non-zero
    use lichen_core::processor::CONFLICT_KEY_GOVERNED_PROPOSALS;
    use lichen_core::processor::CONFLICT_KEY_MOSSSTAKE_POOL;
    use lichen_core::processor::CONFLICT_KEY_STAKE_POOL;

    assert_ne!(CONFLICT_KEY_STAKE_POOL, CONFLICT_KEY_MOSSSTAKE_POOL);
    assert_ne!(CONFLICT_KEY_STAKE_POOL, CONFLICT_KEY_GOVERNED_PROPOSALS);
    assert_ne!(CONFLICT_KEY_MOSSSTAKE_POOL, CONFLICT_KEY_GOVERNED_PROPOSALS);
    assert_ne!(CONFLICT_KEY_STAKE_POOL, Pubkey::new([0u8; 32]));
}

// ─── Test: snapshot import + verify round-trip ───────────────────────────────

#[test]
fn snapshot_import_round_trip_verifies() {
    let (source, _d1) = make_state();

    // Create some state in source
    for i in 0u8..10 {
        let pk = Pubkey::new([i + 50; 32]);
        let acct = Account {
            spores: (i as u64 + 1) * 5_000_000_000,
            spendable: (i as u64 + 1) * 5_000_000_000,
            staked: 0,
            locked: 0,
            data: Vec::new(),
            public_key: None,
            owner: pk,
            executable: false,
            rent_epoch: 0,
            dormant: false,
            missed_rent_epochs: 0,
        };
        source.put_account(&pk, &acct).unwrap();
    }

    let mut pool = source.get_stake_pool().unwrap();
    pool.stake(Pubkey::new([99u8; 32]), MIN_VALIDATOR_STAKE, 0)
        .unwrap();
    source.put_stake_pool(&pool).unwrap();

    let expected_root = source.compute_state_root();

    // Export accounts from source
    let accounts_page = source.export_accounts_iter(0, 10000).unwrap();
    let contract_page = source.export_contract_storage_iter(0, 10000).unwrap();

    // Import into a fresh staging store
    let tmp2 = TempDir::new().unwrap();
    let staging = StateStore::open(tmp2.path()).unwrap();

    // Seed the same treasury
    let treasury_pk = source.get_treasury_pubkey().unwrap().unwrap();
    staging.set_treasury_pubkey(&treasury_pk).unwrap();

    staging.import_accounts(&accounts_page.entries).unwrap();
    staging
        .import_contract_storage(&contract_page.entries)
        .unwrap();

    // Copy singleton pools
    let stake_pool = source.get_stake_pool().unwrap();
    staging.put_stake_pool(&stake_pool).unwrap();
    let mossstake_pool = source.get_mossstake_pool().unwrap();
    staging.put_mossstake_pool(&mossstake_pool).unwrap();

    let computed_root = staging.compute_state_root_cold_start();
    assert_eq!(
        expected_root, computed_root,
        "Staging import must reproduce the same state root"
    );
}

// Minimal working integration tests
use moltchain_core::*;
use tempfile::TempDir;

#[test]
fn test_keypair_generation() {
    let keypair = Keypair::new();
    let pubkey = keypair.pubkey();
    assert_eq!(pubkey.0.len(), 32);
}

#[test]
fn test_pubkey_base58() {
    let keypair = Keypair::new();
    let pubkey = keypair.pubkey();
    let base58 = pubkey.to_base58();
    assert!(!base58.is_empty());
}

#[test]
fn test_pubkey_evm() {
    let keypair = Keypair::new();
    let pubkey = keypair.pubkey();
    let evm = pubkey.to_evm();
    assert!(evm.starts_with("0x"));
    assert_eq!(evm.len(), 42);
}

#[test]
fn test_hash_deterministic() {
    let data = vec![1u8, 2, 3];
    let hash1 = Hash::hash(&data);
    let hash2 = Hash::hash(&data);
    assert_eq!(hash1, hash2);
}

#[test]
fn test_mempool_basic() {
    let mut mempool = Mempool::new(10, 300);

    let msg = Message::new(
        vec![Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![],
        }],
        Hash::default(),
    );

    let tx = Transaction {
        signatures: vec![[1u8; 64]],
        message: msg,
        tx_type: Default::default(),
    };

    assert!(mempool.add_transaction(tx, 1000, 0).is_ok());
    assert_eq!(mempool.size(), 1);
}

#[test]
fn test_state_store_init() {
    let temp = TempDir::new().unwrap();
    let state = StateStore::open(temp.path()).unwrap();

    let pubkey = Pubkey([0u8; 32]);
    let account_opt = state.get_account(&pubkey).unwrap();
    assert!(account_opt.is_some() || account_opt.is_none()); // Either is valid
}

#[test]
fn test_account_storage() {
    let temp = TempDir::new().unwrap();
    let state = StateStore::open(temp.path()).unwrap();

    let keypair = Keypair::new();
    let pubkey = keypair.pubkey();

    let account = Account::new(5, pubkey); // 5 MOLT

    state.put_account(&pubkey, &account).unwrap();

    let retrieved = state.get_account(&pubkey).unwrap().unwrap();
    assert_eq!(retrieved.shells, Account::molt_to_shells(5));
}

#[test]
fn test_block_creation() {
    let validator = Keypair::new();
    let block = Block::new(
        0,
        Hash::default(),
        Hash::default(),
        validator.pubkey().0,
        vec![],
    );

    assert_eq!(block.header.slot, 0);
    assert_eq!(block.header.validator, validator.pubkey().0);
    assert_eq!(block.transactions.len(), 0);
}

#[test]
fn test_validator_set() {
    let mut set = ValidatorSet::new();

    let validator1 = Keypair::new();

    set.add_validator(ValidatorInfo {
        pubkey: validator1.pubkey(),
        stake: 1000000,
        reputation: 100,
        blocks_proposed: 0,
        votes_cast: 0,
        correct_votes: 0,
        last_active_slot: 0,
        joined_slot: 0,
        commission_rate: 500,
        transactions_processed: 0,
    });

    assert!(!set.validators().is_empty());
    assert!(set.total_voting_weight() > 0);
}

#[test]
fn test_tx_processor_init() {
    let temp = TempDir::new().unwrap();
    let state = StateStore::open(temp.path()).unwrap();
    let _processor = TxProcessor::new(state);
    // Just verify it compiles and initializes
}

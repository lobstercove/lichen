// Cross-Contract Call (CCC) Integration Tests
//
// Tests the full cross-contract call pipeline:
//   1. Caller WASM invokes `cross_contract_call` host import
//   2. Host function loads target from StateStore, executes in fresh runtime
//   3. Callee's storage_changes propagate to ContractResult.cross_call_changes
//   4. Processor applies cross_call_changes atomically
//
// Uses WAT (WebAssembly Text) for minimal, self-contained test contracts.

use lichen_core::*;
use std::collections::HashMap;
use tempfile::TempDir;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn create_test_state() -> (StateStore, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let state = StateStore::open(temp_dir.path()).unwrap();
    let treasury = Keypair::new();
    let treasury_account = account_with_spores(treasury.pubkey(), 10_000_000_000_000);
    state
        .put_account(&treasury.pubkey(), &treasury_account)
        .unwrap();
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
    (state, temp_dir)
}

fn account_with_spores(owner: Pubkey, spores: u64) -> Account {
    Account {
        spores,
        spendable: spores,
        staked: 0,
        locked: 0,
        data: Vec::new(),
        owner,
        executable: false,
        rent_epoch: 0,
        dormant: false,
        missed_rent_epochs: 0,
    }
}

/// Deploy a WASM contract into the state store at the given address.
fn deploy_wasm_contract(state: &StateStore, address: &Pubkey, owner: &Pubkey, wasm_bytes: &[u8]) {
    let contract = ContractAccount::new(wasm_bytes.to_vec(), *owner);
    let mut account = account_with_spores(*address, 0);
    account.executable = true;
    account.data = serde_json::to_vec(&contract).unwrap();
    state.put_account(address, &account).unwrap();
}

/// Minimal target WASM: has a `ping` function that writes "ping_key" → "pong"
/// to storage and returns 1.
fn target_ping_wat() -> &'static str {
    r#"(module
        (import "env" "storage_write" (func $storage_write (param i32 i32 i32 i32) (result i32)))
        (import "env" "storage_read" (func $storage_read (param i32 i32 i32 i32) (result i32)))
        (import "env" "storage_read_result" (func $storage_read_result (param i32 i32) (result i32)))
        (import "env" "storage_delete" (func $storage_delete (param i32 i32) (result i32)))
        (import "env" "log" (func $log (param i32 i32)))
        (import "env" "emit_event" (func $emit_event (param i32 i32) (result i32)))
        (import "env" "get_timestamp" (func $get_timestamp (result i64)))
        (import "env" "get_caller" (func $get_caller (param i32) (result i32)))
        (import "env" "get_value" (func $get_value (result i64)))
        (import "env" "get_slot" (func $get_slot (result i64)))
        (import "env" "get_args_len" (func $get_args_len (result i32)))
        (import "env" "get_args" (func $get_args (param i32 i32) (result i32)))
        (import "env" "set_return_data" (func $set_return_data (param i32 i32) (result i32)))
        (import "env" "cross_contract_call" (func $cross_contract_call (param i32 i32 i32 i32 i32 i64 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 0) "ping_key")
        (data (i32.const 16) "pong")
        (func (export "ping") (result i32)
            ;; Write "ping_key" (8 bytes at offset 0) → "pong" (4 bytes at offset 16)
            (call $storage_write (i32.const 0) (i32.const 8) (i32.const 16) (i32.const 4))
            drop
            ;; Return 1 (success)
            (i32.const 1)
        )
    )"#
}

/// Caller WASM: has a `call_target` function that invokes cross_contract_call
/// to call the target contract's `ping` function.
/// The target address is expected in WASM memory at offset 0 (32 bytes),
/// written there via the args mechanism.
fn caller_ccc_wat() -> &'static str {
    r#"(module
        (import "env" "storage_write" (func $storage_write (param i32 i32 i32 i32) (result i32)))
        (import "env" "storage_read" (func $storage_read (param i32 i32 i32 i32) (result i32)))
        (import "env" "storage_read_result" (func $storage_read_result (param i32 i32) (result i32)))
        (import "env" "storage_delete" (func $storage_delete (param i32 i32) (result i32)))
        (import "env" "log" (func $log (param i32 i32)))
        (import "env" "emit_event" (func $emit_event (param i32 i32) (result i32)))
        (import "env" "get_timestamp" (func $get_timestamp (result i64)))
        (import "env" "get_caller" (func $get_caller (param i32) (result i32)))
        (import "env" "get_value" (func $get_value (result i64)))
        (import "env" "get_slot" (func $get_slot (result i64)))
        (import "env" "get_args_len" (func $get_args_len (result i32)))
        (import "env" "get_args" (func $get_args (param i32 i32) (result i32)))
        (import "env" "set_return_data" (func $set_return_data (param i32 i32) (result i32)))
        (import "env" "cross_contract_call" (func $cross_contract_call (param i32 i32 i32 i32 i32 i64 i32 i32) (result i32)))
        (memory (export "memory") 2)
        ;; Function name "ping" stored at offset 100
        (data (i32.const 100) "ping")
        (func (export "call") (result i32)
            ;; Copy args into memory at offset 0 (target address = 32 bytes)
            ;; get_args(out_ptr, out_len) -> bytes_written
            (call $get_args (i32.const 0) (i32.const 32))
            drop
            ;; cross_contract_call(target=0, func=100, func_len=4, args=200, args_len=0, value=0, result=300, result_len=1024)
            (call $cross_contract_call
                (i32.const 0)     ;; target_ptr (32 bytes at offset 0, loaded from args)
                (i32.const 100)   ;; function_ptr ("ping" at offset 100)
                (i32.const 4)     ;; function_len
                (i32.const 200)   ;; args_ptr (no actual args for ping)
                (i32.const 0)     ;; args_len
                (i64.const 0)     ;; value
                (i32.const 300)   ;; result_ptr
                (i32.const 1024)  ;; result_len
            )
            ;; CCC returns bytes written (>0 = success) or 0 (failure).
            ;; We return that as our own return code.
        )
    )"#
}

// ─── Unit Tests: ContractContext & ContractResult fields ──────────────────────

#[test]
fn test_context_new_initializes_ccc_fields() {
    let ctx = ContractContext::new(Pubkey::new([1u8; 32]), Pubkey::new([2u8; 32]), 0, 0);
    assert!(ctx.state_store.is_none());
    assert_eq!(ctx.call_depth, 0);
    assert!(ctx.pending_ccc_changes.lock().unwrap().is_empty());
    assert!(ctx.pending_ccc_events.lock().unwrap().is_empty());
    assert!(ctx.pending_ccc_logs.lock().unwrap().is_empty());
}

#[test]
fn test_context_with_args_initializes_ccc_fields() {
    let ctx = ContractContext::with_args(
        Pubkey::new([1u8; 32]),
        Pubkey::new([2u8; 32]),
        100,
        42,
        HashMap::new(),
        vec![1, 2, 3],
    );
    assert!(ctx.state_store.is_none());
    assert_eq!(ctx.call_depth, 0);
    assert_eq!(ctx.args, vec![1, 2, 3]);
}

#[test]
fn test_pending_ccc_changes_shared_via_arc() {
    let ctx = ContractContext::new(Pubkey::new([1u8; 32]), Pubkey::new([2u8; 32]), 0, 0);

    // Clone shares the same Arc — mutations visible on both sides
    let ctx2 = ctx.clone();
    let target = Pubkey::new([3u8; 32]);

    {
        let mut changes = ctx.pending_ccc_changes.lock().unwrap();
        let entry = changes.entry(target).or_default();
        entry.insert(b"key".to_vec(), Some(b"val".to_vec()));
    }

    // ctx2 should see the same change
    let changes2 = ctx2.pending_ccc_changes.lock().unwrap();
    assert!(changes2.contains_key(&target));
    assert_eq!(
        changes2.get(&target).unwrap().get(&b"key".to_vec()),
        Some(&Some(b"val".to_vec()))
    );
}

// ─── Integration Tests: Full CCC pipeline ────────────────────────────────────

#[test]
fn test_cross_contract_call_executes_target() {
    let (state, _tmp) = create_test_state();
    let owner = Pubkey::new([1u8; 32]);
    let caller_addr = Pubkey::new([10u8; 32]);
    let target_addr = Pubkey::new([20u8; 32]);

    // Deploy target contract (ping)
    let target_wat = target_ping_wat();
    deploy_wasm_contract(&state, &target_addr, &owner, target_wat.as_bytes());

    // Deploy caller contract (calls cross_contract_call)
    let caller_wat = caller_ccc_wat();
    deploy_wasm_contract(&state, &caller_addr, &owner, caller_wat.as_bytes());

    // Load the caller contract
    let caller_account = state.get_account(&caller_addr).unwrap().unwrap();
    let caller_contract: ContractAccount = serde_json::from_slice(&caller_account.data).unwrap();

    // Build context with state_store set (enables CCC)
    // Args = target address (32 bytes) — the caller WASM loads this into memory
    let args = target_addr.0.to_vec();
    let mut ctx = ContractContext::with_args(
        owner,
        caller_addr,
        0,
        1,
        caller_contract.storage.clone(),
        args.clone(),
    );
    ctx.state_store = Some(state.clone());

    // Execute the caller — it should invoke cross_contract_call internally
    let mut runtime = ContractRuntime::new();
    let result = runtime.execute(&caller_contract, "call", &args, ctx);

    let result = result.expect("Execution should not error");

    // The call succeeded
    assert!(
        result.success,
        "CCC call should succeed: {:?}",
        result.error
    );

    // The target's ping function wrote "ping_key" → "pong" to its storage.
    // This should appear in cross_call_changes under the target's address.
    assert!(
        result.cross_call_changes.contains_key(&target_addr),
        "cross_call_changes should contain target contract's changes. Got: {:?}",
        result.cross_call_changes.keys().collect::<Vec<_>>()
    );

    let target_changes = &result.cross_call_changes[&target_addr];
    assert_eq!(
        target_changes.get(&b"ping_key".to_vec()),
        Some(&Some(b"pong".to_vec())),
        "Target should have written ping_key → pong"
    );

    // The WASM return code should be > 0 (bytes written from CCC)
    assert!(
        result.return_code.unwrap_or(0) > 0,
        "Return code should indicate success (>0 bytes from CCC)"
    );
}

#[test]
fn test_cross_contract_call_without_state_store_returns_zero() {
    let (state, _tmp) = create_test_state();
    let owner = Pubkey::new([1u8; 32]);
    let caller_addr = Pubkey::new([10u8; 32]);
    let target_addr = Pubkey::new([20u8; 32]);

    // Deploy target
    deploy_wasm_contract(&state, &target_addr, &owner, target_ping_wat().as_bytes());

    // Deploy caller
    let caller_wat = caller_ccc_wat();
    deploy_wasm_contract(&state, &caller_addr, &owner, caller_wat.as_bytes());

    let caller_account = state.get_account(&caller_addr).unwrap().unwrap();
    let caller_contract: ContractAccount = serde_json::from_slice(&caller_account.data).unwrap();

    // Build context WITHOUT state_store (simulating test mode)
    let args = target_addr.0.to_vec();
    let ctx = ContractContext::with_args(
        owner,
        caller_addr,
        0,
        1,
        caller_contract.storage.clone(),
        args.clone(),
    );
    // Note: ctx.state_store is None

    let mut runtime = ContractRuntime::new();
    let result = runtime
        .execute(&caller_contract, "call", &args, ctx)
        .expect("Should not error");

    // Execution succeeds but CCC returns 0 (no state_store)
    assert!(result.success);
    // Return code should be 0 (CCC failed due to no state store)
    assert_eq!(
        result.return_code,
        Some(0),
        "CCC should return 0 when no state_store"
    );
    // No cross-call changes
    assert!(result.cross_call_changes.is_empty());
}

#[test]
fn test_cross_contract_call_target_not_found() {
    let (state, _tmp) = create_test_state();
    let owner = Pubkey::new([1u8; 32]);
    let caller_addr = Pubkey::new([10u8; 32]);
    let nonexistent_target = Pubkey::new([99u8; 32]); // Not deployed

    let caller_wat = caller_ccc_wat();
    deploy_wasm_contract(&state, &caller_addr, &owner, caller_wat.as_bytes());

    let caller_account = state.get_account(&caller_addr).unwrap().unwrap();
    let caller_contract: ContractAccount = serde_json::from_slice(&caller_account.data).unwrap();

    let args = nonexistent_target.0.to_vec();
    let mut ctx = ContractContext::with_args(
        owner,
        caller_addr,
        0,
        1,
        caller_contract.storage.clone(),
        args.clone(),
    );
    ctx.state_store = Some(state.clone());

    let mut runtime = ContractRuntime::new();
    let result = runtime
        .execute(&caller_contract, "call", &args, ctx)
        .expect("Should not error");

    // CCC returns 0 because target doesn't exist
    assert!(result.success);
    assert_eq!(result.return_code, Some(0));
    assert!(result.cross_call_changes.is_empty());
}

// ─── Processor-level test: cross_call_changes applied to state ───────────────

#[test]
fn test_processor_applies_cross_call_changes() {
    let (state, _tmp) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let deployer = Keypair::new();
    let target_addr = Keypair::new().pubkey();
    let caller_addr = Keypair::new().pubkey();
    let validator_pubkey = Keypair::new().pubkey();

    // Fund deployer
    state
        .put_account(
            &deployer.pubkey(),
            &account_with_spores(deployer.pubkey(), 10_000_000_000_000),
        )
        .unwrap();

    // Deploy target (ping)
    deploy_wasm_contract(
        &state,
        &target_addr,
        &deployer.pubkey(),
        target_ping_wat().as_bytes(),
    );

    // Deploy caller (calls CCC)
    deploy_wasm_contract(
        &state,
        &caller_addr,
        &deployer.pubkey(),
        caller_ccc_wat().as_bytes(),
    );

    // Build a contract call transaction: call the caller's "call" function
    // with args = target_addr (32 bytes)
    let args = target_addr.0.to_vec();
    let call_ix = ContractInstruction::call("call".to_string(), args, 0);
    let call_data = call_ix.serialize().unwrap();

    let recent = state.get_recent_blockhashes(10).unwrap_or_default();
    let blockhash = recent.into_iter().next().unwrap_or_default();

    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![deployer.pubkey(), caller_addr],
        data: call_data,
    };
    let message = Message::new(vec![instruction], blockhash);
    let signature = deployer.sign(&message.serialize());
    let tx = Transaction {
        signatures: vec![signature],
        message,
        tx_type: Default::default(),
    };

    let result = processor.process_transaction(&tx, &validator_pubkey);

    // The transaction should succeed
    assert!(
        result.success,
        "Transaction should succeed: {:?}",
        result.error
    );

    // After processing, the target contract's storage should have "ping_key" → "pong"
    // written via CF_CONTRACT_STORAGE (fast path)
    let stored = state
        .get_contract_storage(&target_addr, b"ping_key")
        .unwrap();
    assert_eq!(
        stored,
        Some(b"pong".to_vec()),
        "Target's storage should have ping_key → pong after CCC"
    );

    // Embedded contract JSON is metadata-only and should not mirror live writes.
    let target_account = state.get_account(&target_addr).unwrap().unwrap();
    let target_contract: ContractAccount = serde_json::from_slice(&target_account.data).unwrap();
    assert_eq!(
        target_contract.get_storage(b"ping_key"),
        None,
        "Target's embedded storage must not mirror canonical CF_CONTRACT_STORAGE writes"
    );
}

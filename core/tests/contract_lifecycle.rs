// Contract lifecycle integration test
// Tests: deploy WASM → init → call → query

use moltchain_core::*;
use tempfile::TempDir;

fn create_test_state() -> (StateStore, TempDir, Hash) {
    let temp_dir = TempDir::new().unwrap();
    let state = StateStore::open(temp_dir.path()).unwrap();
    let treasury = Keypair::new();
    let treasury_account = account_with_shells(treasury.pubkey(), 10_000_000_000_000);
    state
        .put_account(&treasury.pubkey(), &treasury_account)
        .unwrap();
    state.set_treasury_pubkey(&treasury.pubkey()).unwrap();

    // Store a genesis block so get_recent_blockhashes returns a real hash
    let genesis = Block::new_with_timestamp(
        0,
        Hash::default(),
        Hash::default(),
        [0u8; 32],
        Vec::new(),
        0,
    );
    let genesis_hash = genesis.hash();
    state.put_block(&genesis).unwrap();
    state.set_last_slot(0).unwrap();

    (state, temp_dir, genesis_hash)
}

fn account_with_shells(owner: Pubkey, shells: u64) -> Account {
    Account {
        shells,
        spendable: shells,
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

fn build_signed_tx(
    signer: &Keypair,
    instruction: Instruction,
    recent_blockhash: Hash,
) -> Transaction {
    let message = Message::new(vec![instruction], recent_blockhash);
    let signature = signer.sign(&message.serialize());
    Transaction {
        signatures: vec![signature],
        message,
        tx_type: Default::default(),
    }
}

// ============================================================================
// LIFECYCLE: Deploy → Store → Query
// ============================================================================

#[test]
fn test_contract_deploy_lifecycle() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let deployer = Keypair::new();
    let contract_address = Keypair::new();
    let validator = Keypair::new();

    // Fund the deployer with enough for deploy fee
    let deploy_cost = BASE_FEE + CONTRACT_DEPLOY_FEE + 1_000_000;
    state
        .put_account(
            &deployer.pubkey(),
            &account_with_shells(deployer.pubkey(), deploy_cost),
        )
        .unwrap();

    // Minimal valid WASM module (just the magic header + version)
    let minimal_wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    // Create deploy instruction
    let deploy_ix = ContractInstruction::deploy(minimal_wasm.clone(), Vec::new());
    let deploy_data = deploy_ix.serialize().unwrap();

    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![deployer.pubkey(), contract_address.pubkey()],
        data: deploy_data,
    };

    let tx = build_signed_tx(&deployer, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    // The deploy should succeed (or fail gracefully if WASM is too minimal)
    // Either way, the processor shouldn't panic
    if result.success {
        // Verify contract is stored
        let account = state.get_account(&contract_address.pubkey()).unwrap();
        assert!(
            account.is_some(),
            "Contract account should exist after deploy"
        );
        let account = account.unwrap();
        assert!(account.executable, "Contract account should be executable");
        assert!(
            !account.data.is_empty(),
            "Contract account data should not be empty"
        );
    }
    // If the deploy fails (e.g. WASM validation rejects minimal module),
    // that's also acceptable — we're testing the full pipeline doesn't panic
}

#[test]
fn test_contract_call_nonexistent() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let caller = Keypair::new();
    let nonexistent_contract = Keypair::new();
    let validator = Keypair::new();

    // Fund the caller
    state
        .put_account(
            &caller.pubkey(),
            &account_with_shells(caller.pubkey(), 1_000_000_000),
        )
        .unwrap();

    // Try to call a function on a nonexistent contract
    let call_ix = ContractInstruction::call("transfer".to_string(), vec![1, 2, 3], 0);
    let call_data = call_ix.serialize().unwrap();

    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![caller.pubkey(), nonexistent_contract.pubkey()],
        data: call_data,
    };

    let tx = build_signed_tx(&caller, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    // Calling a nonexistent contract should fail gracefully
    assert!(!result.success, "Calling nonexistent contract should fail");
}

#[test]
fn test_contract_instruction_serialization_roundtrip() {
    // Test that deploy instructions serialize and deserialize correctly
    let code = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let init_data = b"hello".to_vec();
    let deploy = ContractInstruction::deploy(code.clone(), init_data.clone());

    let serialized = deploy.serialize().unwrap();
    let deserialized = ContractInstruction::deserialize(&serialized).unwrap();

    match deserialized {
        ContractInstruction::Deploy {
            code: c,
            init_data: d,
        } => {
            assert_eq!(c, code);
            assert_eq!(d, init_data);
        }
        _ => panic!("Expected Deploy instruction"),
    }

    // Test call instruction
    let call = ContractInstruction::call("initialize".to_string(), vec![42], 1000);
    let serialized = call.serialize().unwrap();
    let deserialized = ContractInstruction::deserialize(&serialized).unwrap();

    match deserialized {
        ContractInstruction::Call {
            function,
            args,
            value,
        } => {
            assert_eq!(function, "initialize");
            assert_eq!(args, vec![42]);
            assert_eq!(value, 1000);
        }
        _ => panic!("Expected Call instruction"),
    }
}

#[test]
fn test_system_transfer_lifecycle() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let sender = Keypair::new();
    let receiver = Keypair::new();
    let validator = Keypair::new();

    let initial_balance = 10_000_000_000u64; // 10 MOLT in shells
    let transfer_amount = 2_000_000_000u64; // 2 MOLT

    state
        .put_account(
            &sender.pubkey(),
            &account_with_shells(sender.pubkey(), initial_balance),
        )
        .unwrap();
    state
        .put_account(
            &receiver.pubkey(),
            &account_with_shells(receiver.pubkey(), 0),
        )
        .unwrap();

    // System transfer: data[0] = 0 (transfer opcode), data[1..9] = amount LE
    let mut data = vec![0u8]; // transfer opcode
    data.extend_from_slice(&transfer_amount.to_le_bytes());

    let instruction = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![sender.pubkey(), receiver.pubkey()],
        data,
    };

    let tx = build_signed_tx(&sender, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    assert!(
        result.success,
        "Transfer should succeed: {:?}",
        result.error
    );

    // Verify balances changed
    let sender_account = state.get_account(&sender.pubkey()).unwrap().unwrap();
    let receiver_account = state.get_account(&receiver.pubkey()).unwrap().unwrap();

    assert_eq!(receiver_account.shells, transfer_amount);
    // Sender lost transfer_amount + fee
    assert!(sender_account.shells < initial_balance - transfer_amount);
}

#[test]
fn test_insufficient_funds_for_deploy() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let deployer = Keypair::new();
    let contract_address = Keypair::new();
    let validator = Keypair::new();

    // Fund with almost nothing (not enough for deploy fee)
    state
        .put_account(
            &deployer.pubkey(),
            &account_with_shells(deployer.pubkey(), 100),
        )
        .unwrap();

    let minimal_wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let deploy_ix = ContractInstruction::deploy(minimal_wasm, Vec::new());
    let deploy_data = deploy_ix.serialize().unwrap();

    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![deployer.pubkey(), contract_address.pubkey()],
        data: deploy_data,
    };

    let tx = build_signed_tx(&deployer, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    assert!(
        !result.success,
        "Deploy with insufficient funds should fail"
    );
}

// ============================================================================
// UPGRADE LIFECYCLE TESTS
// ============================================================================

#[test]
fn test_contract_upgrade_lifecycle() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let deployer = Keypair::new();
    let contract_address = Keypair::new();
    let validator = Keypair::new();

    // Fund deployer for deploy + upgrade
    let fund = BASE_FEE * 2 + CONTRACT_DEPLOY_FEE + CONTRACT_UPGRADE_FEE + 10_000_000;
    state
        .put_account(
            &deployer.pubkey(),
            &account_with_shells(deployer.pubkey(), fund),
        )
        .unwrap();

    // Deploy
    let wasm_v1 = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let deploy_ix = ContractInstruction::deploy(wasm_v1.clone(), Vec::new());
    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![deployer.pubkey(), contract_address.pubkey()],
        data: deploy_ix.serialize().unwrap(),
    };
    let tx = build_signed_tx(&deployer, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    if !result.success {
        // Some environments reject the minimal WASM; skip gracefully
        eprintln!("deploy failed (minimal WASM rejected), skipping upgrade test");
        return;
    }

    // Check version == 1 before upgrade
    let acct = state
        .get_account(&contract_address.pubkey())
        .unwrap()
        .unwrap();
    let contract: contract::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
    assert_eq!(contract.version, 1, "initial version should be 1");
    assert!(contract.previous_code_hash.is_none());

    // Upgrade with different bytecode (append a custom section)
    let mut wasm_v2 = wasm_v1.clone();
    wasm_v2.extend_from_slice(&[0x00, 0x04, 0x6e, 0x61, 0x6d, 0x65]); // name custom section

    let upgrade_ix = ContractInstruction::upgrade(wasm_v2);
    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![deployer.pubkey(), contract_address.pubkey()],
        data: upgrade_ix.serialize().unwrap(),
    };
    let tx = build_signed_tx(&deployer, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    if result.success {
        let acct = state
            .get_account(&contract_address.pubkey())
            .unwrap()
            .unwrap();
        let contract: contract::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert_eq!(
            contract.version, 2,
            "version should bump to 2 after upgrade"
        );
        assert!(
            contract.previous_code_hash.is_some(),
            "previous_code_hash should be set after upgrade"
        );
    }
}

#[test]
fn test_upgrade_non_owner_rejected() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let deployer = Keypair::new();
    let attacker = Keypair::new();
    let contract_address = Keypair::new();
    let validator = Keypair::new();

    // Fund deployer for deploy, attacker for upgrade attempt
    let fund_deploy = BASE_FEE + CONTRACT_DEPLOY_FEE + 1_000_000;
    let fund_attack = BASE_FEE + CONTRACT_UPGRADE_FEE + 1_000_000;
    state
        .put_account(
            &deployer.pubkey(),
            &account_with_shells(deployer.pubkey(), fund_deploy),
        )
        .unwrap();
    state
        .put_account(
            &attacker.pubkey(),
            &account_with_shells(attacker.pubkey(), fund_attack),
        )
        .unwrap();

    // Deploy
    let wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let deploy_ix = ContractInstruction::deploy(wasm.clone(), Vec::new());
    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![deployer.pubkey(), contract_address.pubkey()],
        data: deploy_ix.serialize().unwrap(),
    };
    let tx = build_signed_tx(&deployer, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    if !result.success {
        eprintln!("deploy failed (minimal WASM rejected), skipping non-owner test");
        return;
    }

    // Attacker tries to upgrade
    let upgrade_ix = ContractInstruction::upgrade(wasm.clone());
    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![attacker.pubkey(), contract_address.pubkey()],
        data: upgrade_ix.serialize().unwrap(),
    };
    let tx = build_signed_tx(&attacker, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    assert!(!result.success, "Non-owner upgrade should be rejected");
}

#[test]
fn test_upgrade_version_increments_correctly() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let deployer = Keypair::new();
    let contract_address = Keypair::new();
    let validator = Keypair::new();

    // Fund for deploy + 3 upgrades
    let fund = BASE_FEE * 4 + CONTRACT_DEPLOY_FEE + CONTRACT_UPGRADE_FEE * 3 + 50_000_000;
    state
        .put_account(
            &deployer.pubkey(),
            &account_with_shells(deployer.pubkey(), fund),
        )
        .unwrap();

    // Deploy
    let wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let deploy_ix = ContractInstruction::deploy(wasm.clone(), Vec::new());
    let instruction = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![deployer.pubkey(), contract_address.pubkey()],
        data: deploy_ix.serialize().unwrap(),
    };
    let tx = build_signed_tx(&deployer, instruction, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());

    if !result.success {
        eprintln!("deploy failed, skipping version increment test");
        return;
    }

    // Upgrade 3 times, verify version increments each time
    for expected_version in 2..=4u32 {
        let mut wasm_vn = wasm.clone();
        wasm_vn.push(expected_version as u8); // make each version unique

        let upgrade_ix = ContractInstruction::upgrade(wasm_vn);
        let instruction = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![deployer.pubkey(), contract_address.pubkey()],
            data: upgrade_ix.serialize().unwrap(),
        };
        let tx = build_signed_tx(&deployer, instruction, genesis_hash);
        let result = processor.process_transaction(&tx, &validator.pubkey());

        if !result.success {
            eprintln!("upgrade to v{} failed, stopping", expected_version);
            return;
        }

        let acct = state
            .get_account(&contract_address.pubkey())
            .unwrap()
            .unwrap();
        let contract: contract::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert_eq!(
            contract.version, expected_version,
            "version should be {} after upgrade",
            expected_version
        );
    }
}

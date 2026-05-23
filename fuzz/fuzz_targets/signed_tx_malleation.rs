#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::transaction::MAX_TRANSACTION_SERIALIZED_SIZE;
use lichen_core::{Hash, Instruction, Keypair, Message, Pubkey, Transaction};

const CHAIN_ID: &str = "lichen-testnet-1";
const WRONG_CHAIN_ID: &str = "lichen-mainnet-1";

fuzz_target!(|data: &[u8]| {
    let (mut tx, kp1, kp2, kp3) = signed_two_signer_transaction(data);

    assert!(tx.validate_structure().is_ok());
    assert!(tx
        .verify_required_signatures_with_chain_id(CHAIN_ID)
        .is_ok());
    assert!(tx
        .verify_required_signatures_with_chain_id(WRONG_CHAIN_ID)
        .is_err());

    let wire = tx.to_wire();
    let parsed =
        Transaction::from_wire(&wire, MAX_TRANSACTION_SERIALIZED_SIZE).expect("valid tx wire");
    assert!(parsed
        .verify_required_signatures_with_chain_id(CHAIN_ID)
        .is_ok());

    match data.first().copied().unwrap_or(0) % 9 {
        0 => {
            let message = tx.message.signing_bytes_for_chain_id(CHAIN_ID);
            tx.signatures.push(kp3.sign(&message));
            assert!(tx
                .verify_required_signatures_with_chain_id(CHAIN_ID)
                .is_err());
        }
        1 => {
            tx.signatures.swap(0, 1);
            assert!(tx
                .verify_required_signatures_with_chain_id(CHAIN_ID)
                .is_err());
        }
        2 => {
            let message = tx.message.signing_bytes_for_chain_id(CHAIN_ID);
            tx.signatures[1] = kp1.sign(&message);
            assert!(tx
                .verify_required_signatures_with_chain_id(CHAIN_ID)
                .is_err());
        }
        3 => {
            tx.message.instructions[0]
                .data
                .push(data.get(1).copied().unwrap_or(1));
            assert!(tx
                .verify_required_signatures_with_chain_id(CHAIN_ID)
                .is_err());
        }
        4 => {
            tx.message.instructions.swap(0, 1);
            assert!(tx
                .verify_required_signatures_with_chain_id(CHAIN_ID)
                .is_err());
        }
        5 => {
            tx.signatures.pop();
            assert!(tx
                .verify_required_signatures_with_chain_id(CHAIN_ID)
                .is_err());
        }
        6 => {
            tx.message.instructions.insert(
                0,
                Instruction {
                    program_id: Pubkey([44u8; 32]),
                    accounts: Vec::new(),
                    data: vec![data.get(1).copied().unwrap_or(0)],
                },
            );
            assert!(tx
                .verify_required_signatures_with_chain_id(CHAIN_ID)
                .is_err());
        }
        7 => {
            let wrong_message = tx.message.signing_bytes_for_chain_id(WRONG_CHAIN_ID);
            tx.signatures = vec![kp1.sign(&wrong_message), kp2.sign(&wrong_message)];
            assert!(tx
                .verify_required_signatures_with_chain_id(CHAIN_ID)
                .is_err());
        }
        _ => {
            let mut trailing = wire.clone();
            trailing.push(data.get(1).copied().unwrap_or(0));
            assert!(Transaction::from_wire(&trailing, MAX_TRANSACTION_SERIALIZED_SIZE).is_err());

            let mut bad_version = wire.clone();
            bad_version[2] = bad_version[2].saturating_add(1);
            assert!(Transaction::from_wire(&bad_version, MAX_TRANSACTION_SERIALIZED_SIZE).is_err());

            let mut bad_type = wire;
            bad_type[3] = 0xFE;
            assert!(Transaction::from_wire(&bad_type, MAX_TRANSACTION_SERIALIZED_SIZE).is_err());
        }
    }
});

fn signed_two_signer_transaction(data: &[u8]) -> (Transaction, Keypair, Keypair, Keypair) {
    let kp1 = Keypair::from_seed(&seed(data, 1));
    let kp2 = Keypair::from_seed(&seed(data, 2));
    let kp3 = Keypair::from_seed(&seed(data, 3));

    let ix1 = Instruction {
        program_id: Pubkey([1u8; 32]),
        accounts: vec![kp1.pubkey(), Pubkey([9u8; 32])],
        data: vec![data.get(4).copied().unwrap_or(1), 0xAA],
    };
    let ix2 = Instruction {
        program_id: Pubkey([2u8; 32]),
        accounts: vec![kp2.pubkey(), Pubkey([8u8; 32])],
        data: vec![data.get(5).copied().unwrap_or(2), 0xBB],
    };
    let mut tx = Transaction::new(Message::new(vec![ix1, ix2], Hash::hash(data)));
    let message = tx.message.signing_bytes_for_chain_id(CHAIN_ID);
    tx.signatures.push(kp1.sign(&message));
    tx.signatures.push(kp2.sign(&message));

    (tx, kp1, kp2, kp3)
}

fn seed(data: &[u8], domain: u8) -> [u8; 32] {
    let mut seed = [domain; 32];
    for (idx, byte) in data.iter().take(31).enumerate() {
        seed[idx + 1] ^= *byte;
    }
    seed
}

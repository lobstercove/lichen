#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::transaction::MAX_TRANSACTION_SERIALIZED_SIZE;
use lichen_core::{codec::deserialize_legacy_bincode, Transaction};

fuzz_target!(|data: &[u8]| {
    let _ = Transaction::from_wire(data, MAX_TRANSACTION_SERIALIZED_SIZE);
    let _ = serde_json::from_slice::<Transaction>(data);
    let _ = deserialize_legacy_bincode::<Transaction>(data, "fuzz transaction");

    if let Ok(tx) = Transaction::from_wire(data, MAX_TRANSACTION_SERIALIZED_SIZE) {
        let _ = tx.validate_structure();
        let _ = tx.required_signers_ordered();
        let _ = tx.verify_required_signatures_with_chain_id("lichen-testnet-1");

        let wire = tx.to_wire();
        let reparsed = Transaction::from_wire(&wire, MAX_TRANSACTION_SERIALIZED_SIZE)
            .expect("transaction wire round-trip must parse");
        assert_eq!(reparsed.to_wire(), wire);

        let mut trailing = wire;
        trailing.push(0);
        assert!(Transaction::from_wire(&trailing, MAX_TRANSACTION_SERIALIZED_SIZE).is_err());
    }
});

#![no_main]
use libfuzzer_sys::fuzz_target;
use moltchain_core::{Mempool, Transaction, Message, Instruction, Pubkey, Hash};

fuzz_target!(|data: &[u8]| {
    if data.len() < 10 {
        return;
    }

    // Use first byte as operation selector, rest as parameters
    let op = data[0] % 4;
    let fee = u64::from_le_bytes({
        let mut buf = [0u8; 8];
        let len = data.len().min(9);
        buf[..len - 1].copy_from_slice(&data[1..len]);
        buf
    });
    let reputation = if data.len() > 9 {
        u64::from_le_bytes({
            let mut buf = [0u8; 8];
            let end = data.len().min(18);
            let start = 9.min(data.len());
            let copy_len = end - start;
            buf[..copy_len].copy_from_slice(&data[start..end]);
            buf
        })
    } else {
        0
    };

    let mut mempool = Mempool::new(100, 300);

    // Create a deterministic test transaction from remaining bytes
    let mut program_id = [0u8; 32];
    for (i, b) in data.iter().enumerate() {
        program_id[i % 32] ^= b;
    }

    let message = Message::new(
        vec![Instruction {
            program_id: Pubkey(program_id),
            accounts: vec![Pubkey([1u8; 32])],
            data: data.to_vec(),
        }],
        Hash::default(),
    );

    let tx = Transaction {
        signatures: vec![[data[0]; 64]],
        message,
    };

    match op {
        0 => { let _ = mempool.add_transaction(tx, fee, reputation); },
        1 => { let _ = mempool.get_top_transactions(fee as usize % 50); },
        2 => { mempool.cleanup_expired(); },
        3 => { mempool.clear(); },
        _ => {},
    }
});

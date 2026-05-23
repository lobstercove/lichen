#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::transaction::{
    MAX_ACCOUNTS_PER_IX, MAX_DEPLOY_INSTRUCTION_DATA, MAX_INSTRUCTIONS_PER_TX,
    MAX_INSTRUCTION_DATA, MAX_SIGNATURES_PER_TX, MAX_TRANSACTION_SERIALIZED_SIZE,
};
use lichen_core::{
    Block, Hash, Instruction, Keypair, Message, Pubkey, Transaction, MAX_COMPUTE_BUDGET,
    MAX_TX_PER_BLOCK,
};
use lichen_p2p::network::validate_message_for_p2p_admission;
use lichen_p2p::{CompactBlock, MessageType};

fuzz_target!(|data: &[u8]| {
    let mut cursor = Cursor::new(data);
    let mut tx = build_transaction(&mut cursor);

    if cursor.byte() & 1 == 1 {
        let signer = Keypair::from_seed(&[91u8; 32]);
        let signature = signer.sign(&tx.message.serialize());
        let signature_count = match cursor.byte() % 4 {
            0 => 0,
            1 => 1,
            2 => MAX_SIGNATURES_PER_TX,
            _ => MAX_SIGNATURES_PER_TX + 1,
        };
        tx.signatures = vec![signature; signature_count];
    }

    let structure = tx.validate_structure();
    let p2p_result = validate_message_for_p2p_admission(&MessageType::Transaction(tx.clone()));

    if structure.is_ok() {
        assert!(p2p_result.is_ok());
        let wire = tx.to_wire();
        assert!(Transaction::from_wire(&wire, MAX_TRANSACTION_SERIALIZED_SIZE).is_ok());
    } else {
        assert!(p2p_result.is_err());
    }

    let block = Block::new_with_timestamp(
        cursor.u64().max(1),
        Hash::hash(b"parent"),
        Hash::hash(b"state"),
        [cursor.byte(); 32],
        vec![tx],
        cursor.u64(),
    );
    let _ = validate_message_for_p2p_admission(&MessageType::Block(block.clone()));

    let mut compact = CompactBlock::from_block(&block);
    if cursor.byte().is_multiple_of(8) {
        compact.short_ids = vec![[cursor.byte(); 12]; MAX_TX_PER_BLOCK + 1];
    }
    let _ = validate_message_for_p2p_admission(&MessageType::CompactBlockMsg(compact));

    if cursor.byte().is_multiple_of(8) {
        let hashes = vec![Hash::hash(data); MAX_TX_PER_BLOCK + 1];
        assert!(
            validate_message_for_p2p_admission(&MessageType::GetBlockTxs {
                slot: cursor.u64(),
                missing_hashes: hashes,
            })
            .is_err()
        );
    }
});

fn build_transaction(cursor: &mut Cursor<'_>) -> Transaction {
    let mode = cursor.byte();
    let instruction_count = match mode % 5 {
        0 => 0,
        1 => 1,
        2 => 1 + (cursor.byte() as usize % 4),
        3 => MAX_INSTRUCTIONS_PER_TX,
        _ => MAX_INSTRUCTIONS_PER_TX + 1,
    };

    let mut instructions = Vec::with_capacity(instruction_count.min(MAX_INSTRUCTIONS_PER_TX + 1));
    for idx in 0..instruction_count {
        let deploy_path = idx == 0 && mode & 0x20 != 0;
        let mut program_id = Pubkey(cursor.bytes32());
        if deploy_path {
            program_id = Pubkey([0u8; 32]);
        }

        let account_count = match cursor.byte() % 5 {
            0 => 0,
            1 => 1,
            2 => 2 + (cursor.byte() as usize % 4),
            3 => MAX_ACCOUNTS_PER_IX,
            _ => MAX_ACCOUNTS_PER_IX + 1,
        };
        let accounts = (0..account_count)
            .map(|_| Pubkey(cursor.bytes32()))
            .collect();

        let data_len = match (mode >> 1) % 6 {
            0 => 0,
            1 => cursor.byte() as usize % 64,
            2 => cursor.byte() as usize % 1024,
            3 if idx == 0 => MAX_INSTRUCTION_DATA + 1,
            4 if idx == 0 => MAX_DEPLOY_INSTRUCTION_DATA + 1,
            _ => cursor.byte() as usize % 256,
        };
        let mut ix_data = vec![cursor.byte(); data_len];
        if deploy_path && !ix_data.is_empty() {
            ix_data[0] = 17;
        }

        instructions.push(Instruction {
            program_id,
            accounts,
            data: ix_data,
        });
    }

    let mut message = Message::new(instructions, Hash::hash(&cursor.bytes32()));
    message.compute_budget = Some(match cursor.byte() % 4 {
        0 => 0,
        1 => MAX_COMPUTE_BUDGET,
        2 => MAX_COMPUTE_BUDGET + 1,
        _ => cursor.u64(),
    });
    message.compute_unit_price = Some(cursor.u64());

    Transaction::new(message)
}

struct Cursor<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn byte(&mut self) -> u8 {
        if self.data.is_empty() {
            return 0;
        }
        let byte = self.data[self.offset % self.data.len()];
        self.offset = self.offset.wrapping_add(1);
        byte
    }

    fn bytes32(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for byte in &mut out {
            *byte = self.byte();
        }
        out
    }

    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.bytes32()[..8].try_into().expect("fixed slice length"))
    }
}

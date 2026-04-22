use super::decode_solana_pubkey;
use crate::SOLANA_SYSTEM_PROGRAM;

pub(crate) struct SolanaMessageHeader {
    pub(crate) num_required_signatures: u8,
    pub(crate) num_readonly_signed: u8,
    pub(crate) num_readonly_unsigned: u8,
}

pub(crate) struct SolanaInstruction {
    pub(crate) program_id_index: u8,
    pub(crate) account_indices: Vec<u8>,
    pub(crate) data: Vec<u8>,
}

pub(crate) fn build_solana_transfer_message(
    from_pubkey: &[u8; 32],
    to_pubkey: &[u8; 32],
    lamports: u64,
    recent_blockhash: &[u8; 32],
) -> Vec<u8> {
    let system_program = decode_solana_pubkey(SOLANA_SYSTEM_PROGRAM).unwrap_or([0u8; 32]);
    let account_keys = vec![*from_pubkey, *to_pubkey, system_program];
    let header = SolanaMessageHeader {
        num_required_signatures: 1,
        num_readonly_signed: 0,
        num_readonly_unsigned: 1,
    };

    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());

    let instruction = SolanaInstruction {
        program_id_index: 2,
        account_indices: vec![0, 1],
        data,
    };

    build_solana_message_with_instructions(header, &account_keys, recent_blockhash, &[instruction])
}

pub(crate) fn build_solana_message_with_instructions(
    header: SolanaMessageHeader,
    account_keys: &[[u8; 32]],
    recent_blockhash: &[u8; 32],
    instructions: &[SolanaInstruction],
) -> Vec<u8> {
    let mut message = Vec::new();
    message.push(header.num_required_signatures);
    message.push(header.num_readonly_signed);
    message.push(header.num_readonly_unsigned);

    encode_shortvec_len(account_keys.len(), &mut message);
    for key in account_keys {
        message.extend_from_slice(key);
    }

    message.extend_from_slice(recent_blockhash);

    encode_shortvec_len(instructions.len(), &mut message);
    for instruction in instructions {
        message.push(instruction.program_id_index);
        encode_shortvec_len(instruction.account_indices.len(), &mut message);
        message.extend_from_slice(&instruction.account_indices);
        encode_shortvec_len(instruction.data.len(), &mut message);
        message.extend_from_slice(&instruction.data);
    }

    message
}

pub(crate) fn build_solana_transaction(signatures: &[[u8; 64]], message: &[u8]) -> Vec<u8> {
    let mut tx = Vec::new();
    encode_shortvec_len(signatures.len(), &mut tx);
    for signature in signatures {
        tx.extend_from_slice(signature);
    }
    tx.extend_from_slice(message);
    tx
}

fn encode_shortvec_len(len: usize, out: &mut Vec<u8>) {
    let mut value = len as u64;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

pub(crate) fn decode_shortvec_u16(bytes: &[u8]) -> Option<(u16, usize)> {
    let mut value: u16 = 0;
    let mut shift = 0u32;
    for (index, &byte) in bytes.iter().enumerate() {
        let lo = (byte & 0x7f) as u16;
        value |= lo.checked_shl(shift)?;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
        if shift >= 16 {
            return None;
        }
    }
    None
}

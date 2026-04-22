use super::super::*;

fn abi_encode_address_word(address: &str) -> Result<[u8; 32], String> {
    let addr = parse_evm_address(address)?;
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(&addr);
    Ok(word)
}

fn abi_encode_u64_word(value: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

fn abi_encode_u128_word(value: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&value.to_be_bytes());
    word
}

fn abi_encode_bytes_tail(bytes: &[u8]) -> Vec<u8> {
    let mut tail = Vec::new();
    tail.extend_from_slice(&abi_encode_u64_word(bytes.len() as u64));
    tail.extend_from_slice(bytes);
    let padding = (32 - (bytes.len() % 32)) % 32;
    tail.extend_from_slice(&vec![0u8; padding]);
    tail
}

pub(crate) fn evm_function_selector(signature: &str) -> [u8; 4] {
    use sha3::{Digest, Keccak256};

    let digest = Keccak256::digest(signature.as_bytes());
    [digest[0], digest[1], digest[2], digest[3]]
}

pub(super) fn build_evm_safe_get_transaction_hash_calldata(
    inner_to: &str,
    inner_value: u128,
    inner_data: &[u8],
    nonce: u64,
) -> Result<Vec<u8>, String> {
    let mut calldata = Vec::new();
    calldata.extend_from_slice(&evm_function_selector(
        "getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)",
    ));
    calldata.extend_from_slice(&abi_encode_address_word(inner_to)?);
    calldata.extend_from_slice(&abi_encode_u128_word(inner_value));
    calldata.extend_from_slice(&abi_encode_u64_word(10 * 32));
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&abi_encode_u64_word(nonce));
    calldata.extend_from_slice(&abi_encode_bytes_tail(inner_data));
    Ok(calldata)
}

pub(crate) fn build_evm_safe_exec_transaction_calldata(
    inner_to: &str,
    inner_value: u128,
    inner_data: &[u8],
    signatures: &[u8],
) -> Result<Vec<u8>, String> {
    let data_offset = 10 * 32;
    let data_tail = abi_encode_bytes_tail(inner_data);
    let sigs_offset = data_offset + data_tail.len();
    let sigs_tail = abi_encode_bytes_tail(signatures);

    let mut calldata = Vec::new();
    calldata.extend_from_slice(&evm_function_selector(
        "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
    ));
    calldata.extend_from_slice(&abi_encode_address_word(inner_to)?);
    calldata.extend_from_slice(&abi_encode_u128_word(inner_value));
    calldata.extend_from_slice(&abi_encode_u64_word(data_offset as u64));
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&abi_encode_u64_word(sigs_offset as u64));
    calldata.extend_from_slice(&data_tail);
    calldata.extend_from_slice(&sigs_tail);
    Ok(calldata)
}

pub(crate) fn normalize_evm_signature(signature: &[u8]) -> Result<Vec<u8>, String> {
    if signature.len() != 65 {
        return Err(format!(
            "invalid EVM signature length: expected 65, got {}",
            signature.len()
        ));
    }
    let mut normalized = signature.to_vec();
    if normalized[64] < 27 {
        normalized[64] = normalized[64].saturating_add(27);
    }
    if normalized[64] != 27 && normalized[64] != 28 {
        return Err(format!(
            "invalid EVM recovery id: expected 27/28, got {}",
            normalized[64]
        ));
    }
    Ok(normalized)
}

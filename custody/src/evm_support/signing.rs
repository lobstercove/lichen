use hmac::{Hmac, Mac};
use k256::ecdsa::SigningKey;
use sha2::Sha256;
use sha3::{Digest, Keccak256};
use zeroize::Zeroize;

pub(crate) fn derive_evm_address(path: &str, master_seed: &str) -> Result<String, String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let mut seed = mac.finalize().into_bytes();
    let key = SigningKey::from_bytes(&seed).map_err(|_| "invalid seed")?;
    seed.as_mut_slice().zeroize();
    let verifying_key = key.verifying_key();
    let encoded = verifying_key.to_encoded_point(false);
    let pubkey = encoded.as_bytes();
    let hash = Keccak256::digest(&pubkey[1..]);
    let addr = &hash[12..];
    Ok(format!("0x{}", hex::encode(addr)))
}

pub(crate) fn derive_evm_signing_key(path: &str, master_seed: &str) -> Result<SigningKey, String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let mut seed = mac.finalize().into_bytes();
    let result = SigningKey::from_bytes(&seed).map_err(|_| "invalid seed".to_string());
    seed.as_mut_slice().zeroize();
    result
}

pub(crate) fn build_evm_signed_transaction(
    signing_key: &SigningKey,
    nonce: u64,
    gas_price: u128,
    gas_limit: u128,
    to_address: &str,
    value: u128,
    chain_id: u64,
) -> Result<Vec<u8>, String> {
    build_evm_signed_transaction_with_data(
        signing_key,
        nonce,
        gas_price,
        gas_limit,
        to_address,
        value,
        &[],
        chain_id,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_evm_signed_transaction_with_data(
    signing_key: &SigningKey,
    nonce: u64,
    gas_price: u128,
    gas_limit: u128,
    to_address: &str,
    value: u128,
    data: &[u8],
    chain_id: u64,
) -> Result<Vec<u8>, String> {
    let to_bytes = parse_evm_address(to_address)?;
    let mut rlp = Vec::new();
    rlp_encode_list(
        &[
            rlp_encode_u64(nonce),
            rlp_encode_u128(gas_price),
            rlp_encode_u128(gas_limit),
            rlp_encode_bytes(&to_bytes),
            rlp_encode_u128(value),
            rlp_encode_bytes(data),
            rlp_encode_u64(chain_id),
            rlp_encode_u64(0),
            rlp_encode_u64(0),
        ],
        &mut rlp,
    );

    let mut digest = Keccak256::new();
    digest.update(&rlp);
    let (signature, recovery_id) = signing_key
        .sign_digest_recoverable(digest)
        .map_err(|_| "failed to recover signature".to_string())?;
    let sig_bytes = signature.to_bytes();
    let v = recovery_id.to_byte() as u64 + 35 + chain_id * 2;

    let mut tx = Vec::new();
    rlp_encode_list(
        &[
            rlp_encode_u64(nonce),
            rlp_encode_u128(gas_price),
            rlp_encode_u128(gas_limit),
            rlp_encode_bytes(&to_bytes),
            rlp_encode_u128(value),
            rlp_encode_bytes(data),
            rlp_encode_u64(v),
            rlp_encode_bytes(&trim_leading_zeros(&sig_bytes[..32])),
            rlp_encode_bytes(&trim_leading_zeros(&sig_bytes[32..64])),
        ],
        &mut tx,
    );

    Ok(tx)
}

pub(crate) fn evm_encode_erc20_transfer(to_address: &str, amount: u128) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&hex::decode("a9059cbb").map_err(|_| "selector".to_string())?);

    let to_bytes = parse_evm_address(to_address)?;
    let mut padded_to = vec![0u8; 12];
    padded_to.extend_from_slice(&to_bytes);
    data.extend_from_slice(&padded_to);

    let mut padded_amount = vec![0u8; 16];
    padded_amount.extend_from_slice(&amount.to_be_bytes());
    data.extend_from_slice(&padded_amount);

    Ok(data)
}

pub(crate) fn parse_evm_address(address: &str) -> Result<Vec<u8>, String> {
    let trimmed = address.trim_start_matches("0x");
    let bytes = hex::decode(trimmed).map_err(|error| format!("address hex: {}", error))?;
    if bytes.len() != 20 {
        return Err("invalid evm address length".to_string());
    }
    Ok(bytes)
}

fn trim_leading_zeros(value: &[u8]) -> Vec<u8> {
    let mut index = 0;
    while index < value.len() && value[index] == 0 {
        index += 1;
    }
    value[index..].to_vec()
}

fn rlp_encode_u64(value: u64) -> Vec<u8> {
    rlp_encode_uint(&value.to_be_bytes())
}

fn rlp_encode_u128(value: u128) -> Vec<u8> {
    rlp_encode_uint(&value.to_be_bytes())
}

fn rlp_encode_uint(bytes: &[u8]) -> Vec<u8> {
    let trimmed = trim_leading_zeros(bytes);
    if trimmed.is_empty() {
        return vec![0x80];
    }
    rlp_encode_bytes(&trimmed)
}

fn rlp_encode_bytes(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() == 1 && bytes[0] < 0x80 {
        return vec![bytes[0]];
    }

    let mut out = Vec::new();
    if bytes.len() <= 55 {
        out.push(0x80 + bytes.len() as u8);
    } else {
        let len_bytes = to_be_bytes(bytes.len() as u64);
        out.push(0xb7 + len_bytes.len() as u8);
        out.extend_from_slice(&len_bytes);
    }
    out.extend_from_slice(bytes);
    out
}

fn rlp_encode_list(items: &[Vec<u8>], out: &mut Vec<u8>) {
    let total_len: usize = items.iter().map(|item| item.len()).sum();
    if total_len <= 55 {
        out.push(0xc0 + total_len as u8);
    } else {
        let len_bytes = to_be_bytes(total_len as u64);
        out.push(0xf7 + len_bytes.len() as u8);
        out.extend_from_slice(&len_bytes);
    }
    for item in items {
        out.extend_from_slice(item);
    }
}

pub(crate) fn to_be_bytes(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    trim_leading_zeros(&bytes)
}

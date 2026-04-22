use anyhow::Result;
use lichen_core::{Hash, Keypair, Pubkey, MAX_CONTRACT_CODE};
use std::path::Path;

use crate::client::RpcClient;

#[derive(Clone, Copy)]
pub(super) struct WasmValidationConfig<'a> {
    pub(super) read_failure_subject: &'a str,
    pub(super) invalid_magic_help: &'a str,
    pub(super) oversize_help: &'a str,
}

pub(super) fn load_wasm_code(path: &Path, config: WasmValidationConfig<'_>) -> Result<Vec<u8>> {
    let wasm_code = std::fs::read(path).map_err(|error| {
        anyhow::anyhow!(
            "Failed to read {} file: {}",
            config.read_failure_subject,
            error
        )
    })?;
    validate_wasm_code(
        path,
        &wasm_code,
        config.invalid_magic_help,
        config.oversize_help,
    )?;
    Ok(wasm_code)
}

fn validate_wasm_code(
    path: &Path,
    wasm_code: &[u8],
    invalid_magic_help: &str,
    oversize_help: &str,
) -> Result<()> {
    const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
    if wasm_code.len() < 8 || wasm_code[..4] != WASM_MAGIC {
        anyhow::bail!(
            "Invalid WASM file: {} does not have valid WASM magic bytes{}",
            path.display(),
            invalid_magic_help
        );
    }

    if wasm_code.len() > MAX_CONTRACT_CODE {
        anyhow::bail!(
            "Contract too large: {} bytes (max {} bytes = 512 KB){}",
            wasm_code.len(),
            MAX_CONTRACT_CODE,
            oversize_help
        );
    }

    Ok(())
}

pub(super) async fn derive_contract_address(
    client: &RpcClient,
    deployer: &Keypair,
    wasm_code: &[u8],
) -> Pubkey {
    let slot_nonce = client.get_slot().await.unwrap_or(0);
    let code_hash = Hash::hash(wasm_code);
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut hasher, deployer.pubkey().0);
    sha2::Digest::update(&mut hasher, code_hash.0);
    sha2::Digest::update(&mut hasher, slot_nonce.to_le_bytes());
    let result = sha2::Digest::finalize(hasher);
    let mut addr_bytes = [0u8; 32];
    addr_bytes.copy_from_slice(&result[..32]);
    Pubkey(addr_bytes)
}

//! Compatibility ABI for Lichen's native shielded protocol module.
//!
//! The native transaction processor is the only authority for proof
//! verification, custody, commitments, nullifiers, and Merkle state:
//! - system opcode 23: Shield
//! - system opcode 24: Unshield
//! - system opcode 25: ShieldedTransfer
//!
//! Canonical reads are served by the shielded RPC endpoints. Direct WASM
//! mutations and reads return [`ERR_NATIVE_ONLY`] and never create parallel
//! contract storage.

#![cfg_attr(target_arch = "wasm32", no_std)]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use lichen_sdk::{get_caller, log_info, set_return_data, storage_get, storage_set};

const OWNER_KEY: &[u8] = b"shielded_compat_owner";
const EXECUTION_MODEL: &[u8] =
    b"native-system-opcodes:23,24,25;queries:canonical-shielded-rpc;wasm-state:none";

/// Direct callers must use the native protocol or canonical RPC surface.
pub const ERR_NATIVE_ONLY: u32 = 40;

fn read_address32(ptr: *const u8) -> Option<[u8; 32]> {
    if ptr.is_null() {
        return None;
    }
    let mut address = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, address.as_mut_ptr(), 32);
    }
    Some(address)
}

/// Initialize the compatibility marker once. This creates no pool state.
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    if storage_get(OWNER_KEY).is_some() {
        return 3;
    }
    let admin = match read_address32(admin_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller().0 != admin {
        return 1;
    }
    if admin.iter().all(|byte| *byte == 0) {
        return 2;
    }
    storage_set(OWNER_KEY, &admin);
    log_info("Shielded compatibility marker initialized; native module is authoritative");
    0
}

/// Native protocol restrictions, not this compatibility contract, control pause state.
#[no_mangle]
pub extern "C" fn pause() -> u32 {
    native_only("Shielded pause is native-governance-only")
}

/// Native protocol restrictions, not this compatibility contract, control pause state.
#[no_mangle]
pub extern "C" fn unpause() -> u32 {
    native_only("Shielded unpause is native-governance-only")
}

/// Use JSON-RPC `getShieldedPoolState`.
#[no_mangle]
pub extern "C" fn get_pool_stats() -> u32 {
    native_only("Use canonical RPC getShieldedPoolState")
}

/// Use JSON-RPC `getShieldedMerkleRoot`.
#[no_mangle]
pub extern "C" fn get_merkle_root() -> u32 {
    native_only("Use canonical RPC getShieldedMerkleRoot")
}

/// Use JSON-RPC `isNullifierSpent`.
#[no_mangle]
pub extern "C" fn check_nullifier(_nullifier_ptr: *const u8) -> u32 {
    native_only("Use canonical RPC isNullifierSpent")
}

/// Use JSON-RPC `getShieldedCommitments`.
#[no_mangle]
pub extern "C" fn get_commitments(_from_index: u64) -> u32 {
    native_only("Use canonical RPC getShieldedCommitments")
}

/// Use native system opcode 23.
#[no_mangle]
pub extern "C" fn shield(_args_ptr: *const u8, _args_len: u32) -> u32 {
    native_only("Use native Shield instruction (system opcode 23)")
}

/// Use native system opcode 24.
#[no_mangle]
pub extern "C" fn unshield(_args_ptr: *const u8, _args_len: u32) -> u32 {
    native_only("Use native Unshield instruction (system opcode 24)")
}

/// Use native system opcode 25.
#[no_mangle]
pub extern "C" fn transfer(_args_ptr: *const u8, _args_len: u32) -> u32 {
    native_only("Use native ShieldedTransfer instruction (system opcode 25)")
}

/// Return a stable machine-readable description of the authoritative path.
#[no_mangle]
pub extern "C" fn get_execution_model() -> u32 {
    set_return_data(EXECUTION_MODEL);
    0
}

fn native_only(message: &str) -> u32 {
    log_info(message);
    ERR_NATIVE_ONLY
}

#[cfg(test)]
mod tests {
    use super::*;
    use lichen_sdk::test_mock;

    #[test]
    fn initialize_is_authenticated_one_time_and_state_free() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller([9u8; 32]);
        assert_eq!(initialize(admin.as_ptr()), 1);
        assert!(storage_get(OWNER_KEY).is_none());

        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(storage_get(OWNER_KEY), Some(admin.to_vec()));
        assert_eq!(initialize(admin.as_ptr()), 3);
        assert!(storage_get(b"pool_state").is_none());
    }

    #[test]
    fn initialize_rejects_null_and_zero_admin() {
        test_mock::reset();
        assert_eq!(initialize(core::ptr::null()), 98);
        let zero = [0u8; 32];
        test_mock::set_caller(zero);
        assert_eq!(initialize(zero.as_ptr()), 2);
        assert!(storage_get(OWNER_KEY).is_none());
    }

    #[test]
    fn every_direct_pool_surface_fails_closed_without_state() {
        test_mock::reset();
        assert_eq!(pause(), ERR_NATIVE_ONLY);
        assert_eq!(unpause(), ERR_NATIVE_ONLY);
        assert_eq!(get_pool_stats(), ERR_NATIVE_ONLY);
        assert_eq!(get_merkle_root(), ERR_NATIVE_ONLY);
        assert_eq!(check_nullifier(core::ptr::null()), ERR_NATIVE_ONLY);
        assert_eq!(get_commitments(0), ERR_NATIVE_ONLY);
        assert_eq!(shield(core::ptr::null(), 0), ERR_NATIVE_ONLY);
        assert_eq!(unshield(core::ptr::null(), 0), ERR_NATIVE_ONLY);
        assert_eq!(transfer(core::ptr::null(), 0), ERR_NATIVE_ONLY);
        assert!(storage_get(b"pool_state").is_none());
        assert!(storage_get(b"sp_paused").is_none());
    }

    #[test]
    fn execution_model_is_discoverable() {
        test_mock::reset();
        assert_eq!(get_execution_model(), 0);
        assert_eq!(test_mock::get_return_data(), EXECUTION_MODEL);
    }
}

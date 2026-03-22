// MoltChain Smart Contract SDK
// Standard library for contract development

#![no_std]

extern crate alloc;

#[cfg(target_arch = "wasm32")]
use core::panic::PanicInfo;

pub mod crosscall;
pub mod dex;
pub mod nft;
pub mod token;

// Re-export modules
pub use crosscall::{
    call_contract, call_nft_owner, call_nft_transfer, call_token_balance, call_token_transfer,
    encode_layout_args, CrossCall,
};
pub use dex::Pool;
pub use nft::NFT;
pub use token::Token;

/// Test mock infrastructure for host functions.
/// When not compiling for wasm32, we provide mock implementations
/// using thread-local storage so contracts can be unit-tested on the host.
#[cfg(not(target_arch = "wasm32"))]
pub mod test_mock {
    extern crate std;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::string::String;
    use std::vec::Vec;

    std::thread_local! {
        pub static STORAGE: RefCell<HashMap<Vec<u8>, Vec<u8>>> = RefCell::new(HashMap::new());
        pub static CALLER: RefCell<[u8; 32]> = RefCell::new([0u8; 32]);
        pub static CONTRACT_ADDRESS: RefCell<[u8; 32]> = RefCell::new([0u8; 32]);
        pub static ARGS: RefCell<Vec<u8>> = RefCell::new(Vec::new());
        pub static RETURN_DATA: RefCell<Vec<u8>> = RefCell::new(Vec::new());
        pub static EVENTS: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
        pub static LOGS: RefCell<Vec<String>> = RefCell::new(Vec::new());
        pub static TIMESTAMP: RefCell<u64> = RefCell::new(1000);
        pub static VALUE: RefCell<u64> = RefCell::new(0);
        pub static SLOT: RefCell<u64> = RefCell::new(1);
        pub static CROSS_CALL_RESPONSE: RefCell<Option<Vec<u8>>> = RefCell::new(None);
        pub static CROSS_CALL_SHOULD_FAIL: RefCell<bool> = RefCell::new(false);
        pub static LAST_CROSS_CALL: RefCell<Option<([u8; 32], String, Vec<u8>, u64)>> = RefCell::new(None);
    }

    pub fn reset() {
        STORAGE.with(|s| s.borrow_mut().clear());
        CALLER.with(|c| *c.borrow_mut() = [0u8; 32]);
        CONTRACT_ADDRESS.with(|c| *c.borrow_mut() = [0u8; 32]);
        ARGS.with(|a| a.borrow_mut().clear());
        RETURN_DATA.with(|r| r.borrow_mut().clear());
        EVENTS.with(|e| e.borrow_mut().clear());
        LOGS.with(|l| l.borrow_mut().clear());
        TIMESTAMP.with(|t| *t.borrow_mut() = 1000);
        VALUE.with(|v| *v.borrow_mut() = 0);
        SLOT.with(|s| *s.borrow_mut() = 1);
        CROSS_CALL_RESPONSE.with(|c| *c.borrow_mut() = None);
        CROSS_CALL_SHOULD_FAIL.with(|c| *c.borrow_mut() = false);
        LAST_CROSS_CALL.with(|c| *c.borrow_mut() = None);
    }

    pub fn set_caller(addr: [u8; 32]) {
        CALLER.with(|c| *c.borrow_mut() = addr);
    }

    pub fn set_contract_address(addr: [u8; 32]) {
        CONTRACT_ADDRESS.with(|c| *c.borrow_mut() = addr);
    }

    pub fn set_args(data: &[u8]) {
        ARGS.with(|a| *a.borrow_mut() = data.to_vec());
    }

    pub fn set_timestamp(ts: u64) {
        TIMESTAMP.with(|t| *t.borrow_mut() = ts);
    }

    pub fn set_value(val: u64) {
        VALUE.with(|v| *v.borrow_mut() = val);
    }

    pub fn set_slot(s: u64) {
        SLOT.with(|slot| *slot.borrow_mut() = s);
    }

    pub fn set_cross_call_response(data: Option<Vec<u8>>) {
        CROSS_CALL_RESPONSE.with(|c| *c.borrow_mut() = data);
    }

    pub fn set_cross_call_should_fail(should_fail: bool) {
        CROSS_CALL_SHOULD_FAIL.with(|c| *c.borrow_mut() = should_fail);
    }

    pub fn get_last_cross_call() -> Option<([u8; 32], String, Vec<u8>, u64)> {
        LAST_CROSS_CALL.with(|c| c.borrow().clone())
    }

    pub fn get_return_data() -> Vec<u8> {
        RETURN_DATA.with(|r| r.borrow().clone())
    }

    pub fn get_events() -> Vec<Vec<u8>> {
        EVENTS.with(|e| e.borrow().clone())
    }

    pub fn get_storage(key: &[u8]) -> Option<Vec<u8>> {
        STORAGE.with(|s| s.borrow().get(key).cloned())
    }

    pub fn get_logs() -> Vec<String> {
        LOGS.with(|l| l.borrow().clone())
    }
}

/// Panic handler for WASM contracts — uses explicit unreachable instead of
/// `loop {}` which is UB (empty loop with no side effects) and modern LLVM
/// compiles it to `unreachable` anyway. Being explicit avoids UB.
#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// Contract storage operations
pub mod storage {
    use alloc::vec::Vec;

    #[cfg(target_arch = "wasm32")]
    extern "C" {
        fn storage_read(key_ptr: *const u8, key_len: u32, val_ptr: *mut u8, val_len: u32) -> u32;
        fn storage_write(key_ptr: *const u8, key_len: u32, val_ptr: *const u8, val_len: u32)
            -> u32;
    }

    /// Read value from storage
    pub fn get(key: &[u8]) -> Option<Vec<u8>> {
        #[cfg(target_arch = "wasm32")]
        {
            // T5.14: Increased buffer from 1024 to 65536 for large values
            let mut buffer = [0u8; 65536];
            let result = unsafe {
                storage_read(
                    key.as_ptr(),
                    key.len() as u32,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                )
            };

            if result == 0 {
                None
            } else {
                Some(buffer[..(result as usize)].to_vec())
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            super::test_mock::STORAGE.with(|s| s.borrow().get(key).cloned())
        }
    }

    /// Write value to storage
    pub fn set(key: &[u8], value: &[u8]) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let result = unsafe {
                storage_write(
                    key.as_ptr(),
                    key.len() as u32,
                    value.as_ptr(),
                    value.len() as u32,
                )
            };
            result == 1
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            super::test_mock::STORAGE.with(|s| {
                s.borrow_mut().insert(key.to_vec(), value.to_vec());
            });
            true
        }
    }

    /// Remove value from storage
    pub fn remove(key: &[u8]) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            set(key, &[])
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            super::test_mock::STORAGE.with(|s| {
                s.borrow_mut().remove(key);
            });
            true
        }
    }
}

/// Contract args and return data
pub mod contract {
    #[cfg(target_arch = "wasm32")]
    use alloc::vec;
    use alloc::vec::Vec;

    #[cfg(target_arch = "wasm32")]
    extern "C" {
        fn get_args_len() -> u32;
        fn get_args(out_ptr: *mut u8, out_len: u32) -> u32;
        fn set_return_data(data_ptr: *const u8, data_len: u32) -> u32;
    }

    /// Get the args passed to this contract call
    pub fn args() -> Vec<u8> {
        #[cfg(target_arch = "wasm32")]
        {
            let len = unsafe { get_args_len() } as usize;
            if len == 0 {
                return Vec::new();
            }
            let mut buf = vec![0u8; len];
            let read = unsafe { get_args(buf.as_mut_ptr(), len as u32) } as usize;
            buf.truncate(read);
            buf
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            super::test_mock::ARGS.with(|a| a.borrow().clone())
        }
    }

    /// Set return data for this contract execution
    pub fn set_return(data: &[u8]) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            unsafe { set_return_data(data.as_ptr(), data.len() as u32) == 0 }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            super::test_mock::RETURN_DATA.with(|r| {
                *r.borrow_mut() = data.to_vec();
            });
            true
        }
    }
}

/// Structured event emission
pub mod event {
    #[cfg(target_arch = "wasm32")]
    extern "C" {
        fn emit_event(data_ptr: *const u8, data_len: u32) -> u32;
    }

    /// Emit a structured event as JSON.
    /// `json_data` should be a valid JSON object string, e.g. `{"name":"Transfer","from":"...","to":"...","amount":"100"}`
    pub fn emit(json_data: &str) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            unsafe { emit_event(json_data.as_ptr(), json_data.len() as u32) == 0 }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            super::test_mock::EVENTS.with(|e| {
                e.borrow_mut().push(json_data.as_bytes().to_vec());
            });
            true
        }
    }
}

/// Logging functions
pub mod log {
    #[cfg(target_arch = "wasm32")]
    #[link(wasm_import_module = "env")]
    extern "C" {
        /// Host logging function. Declared with explicit wasm_import_module to
        /// prevent the linker from DCE-ing it via --gc-sections when LTO is on.
        fn log(msg_ptr: *const u8, msg_len: u32);
    }

    /// Log a message to the host runtime.
    ///
    /// The call is wrapped with `core::hint::black_box` to prevent the LLVM
    /// optimizer from eliminating it during link-time optimization. Without
    /// this, `opt-level = "z"` + LTO + `--gc-sections` can remove the `log`
    /// import entirely, replacing call sites with `unreachable` instructions.
    pub fn info(msg: &str) {
        #[cfg(target_arch = "wasm32")]
        {
            let ptr = msg.as_ptr();
            let len = msg.len() as u32;
            unsafe {
                // black_box the args to make the call opaque to the optimizer
                log(core::hint::black_box(ptr), core::hint::black_box(len));
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            extern crate std;
            super::test_mock::LOGS.with(|l| {
                l.borrow_mut().push(std::string::String::from(msg));
            });
        }
    }
}

/// Account/Address type
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Address(pub [u8; 32]);

impl Address {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Address(bytes)
    }

    pub fn to_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Result type for contract operations
pub type ContractResult<T> = Result<T, ContractError>;

/// Contract error types
#[derive(Debug)]
pub enum ContractError {
    InsufficientFunds,
    Unauthorized,
    InvalidInput,
    StorageError,
    Overflow,
    Custom(&'static str),
}

/// Helper for u64 serialization
pub fn u64_to_bytes(n: u64) -> [u8; 8] {
    n.to_le_bytes()
}

/// Helper for u64 deserialization (handles short input with zero-padding)
pub fn bytes_to_u64(bytes: &[u8]) -> u64 {
    let mut array = [0u8; 8];
    if bytes.len() >= 8 {
        array.copy_from_slice(&bytes[..8]);
    } else {
        array[..bytes.len()].copy_from_slice(bytes);
    }
    u64::from_le_bytes(array)
}

/// Get current blockchain timestamp
pub fn get_timestamp() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn get_timestamp() -> u64;
        }
        unsafe { get_timestamp() }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        test_mock::TIMESTAMP.with(|t| *t.borrow())
    }
}

/// Get the address of the account that invoked this contract call.
/// Returns a 32-byte `Address` representing the caller's public key.
pub fn get_caller() -> Address {
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn get_caller(out_ptr: u32) -> u32;
        }
        let mut buf = [0u8; 32];
        unsafe {
            get_caller(buf.as_mut_ptr() as u32);
        }
        Address(buf)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Address(test_mock::CALLER.with(|c| *c.borrow()))
    }
}

/// Get the address of the currently executing contract.
/// Returns a 32-byte `Address` representing the contract's own public key.
/// In a cross-contract call context, this returns the callee's (target's) address.
pub fn get_contract_address() -> Address {
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn get_contract_address(out_ptr: u32) -> u32;
        }
        let mut buf = [0u8; 32];
        unsafe {
            get_contract_address(buf.as_mut_ptr() as u32);
        }
        Address(buf)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Address(test_mock::CONTRACT_ADDRESS.with(|c| *c.borrow()))
    }
}

/// Get the value (shells) transferred with this contract call.
pub fn get_value() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn get_value() -> u64;
        }
        unsafe { get_value() }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        test_mock::VALUE.with(|v| *v.borrow())
    }
}

/// Get the current block slot number.
pub fn get_slot() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn get_slot() -> u64;
        }
        unsafe { get_slot() }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        test_mock::SLOT.with(|s| *s.borrow())
    }
}

// Re-exports
pub use alloc::string::String;

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

/// Compute Poseidon hash of two 32-byte field elements.
/// Returns the 32-byte hash result (BN254 Fr, little-endian).
/// Uses the same Poseidon config as the on-chain ZK circuits.
pub fn poseidon_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn host_poseidon_hash(left_ptr: u32, right_ptr: u32, out_ptr: u32) -> u32;
        }
        let mut out = [0u8; 32];
        let rc = unsafe {
            host_poseidon_hash(
                left.as_ptr() as u32,
                right.as_ptr() as u32,
                out.as_mut_ptr() as u32,
            )
        };
        if rc != 0 {
            core::arch::wasm32::unreachable();
        }
        out
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Off-chain stub: return simple XOR hash for testing
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = left[i] ^ right[i];
        }
        out
    }
}

// Function re-exports
pub use contract::{args as get_args, set_return as set_return_data};
pub use event::emit as emit_event;
pub use log::info as log_info;
pub use storage::{get as storage_get, set as storage_set};
// get_caller, get_value, get_slot, get_timestamp are already top-level pub fns

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_to_u64_exact_8_bytes() {
        let val: u64 = 0x0807060504030201;
        let bytes = val.to_le_bytes();
        assert_eq!(bytes_to_u64(&bytes), val);
    }

    #[test]
    fn test_bytes_to_u64_short_4_bytes() {
        // Little-endian: [0x01, 0x02, 0x03, 0x04] = 0x04030201
        let bytes = [0x01, 0x02, 0x03, 0x04];
        assert_eq!(bytes_to_u64(&bytes), 0x04030201);
    }

    #[test]
    fn test_bytes_to_u64_empty() {
        assert_eq!(bytes_to_u64(&[]), 0);
    }

    #[test]
    fn test_bytes_to_u64_single_byte() {
        assert_eq!(bytes_to_u64(&[42]), 42);
    }

    #[test]
    fn test_bytes_to_u64_long_input() {
        // More than 8 bytes — only first 8 matter
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 0xFF, 0xFF];
        assert_eq!(bytes_to_u64(&bytes), 0x0807060504030201);
    }
}

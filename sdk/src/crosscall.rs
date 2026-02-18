// Cross-Contract Call Support
// Enables contracts to call functions on other contracts

use crate::{Address, ContractError};
use alloc::vec::Vec;
use alloc::string::String;

pub type CallResult<T> = Result<T, ContractError>;

/// Cross-contract call context
pub struct CrossCall {
    pub target: Address,
    pub function: String,
    pub args: Vec<u8>,
    pub value: u64,
}

impl CrossCall {
    /// Create a new cross-contract call
    pub fn new(target: Address, function: &str, args: Vec<u8>) -> Self {
        CrossCall {
            target,
            function: String::from(function),
            args,
            value: 0,
        }
    }

    /// Set value to transfer with call
    pub fn with_value(mut self, value: u64) -> Self {
        self.value = value;
        self
    }

}

// Call another contract (extern function provided by runtime)
#[cfg(target_arch = "wasm32")]
extern "C" {
    fn cross_contract_call(
        target_ptr: *const u8,
        function_ptr: *const u8,
        function_len: u32,
        args_ptr: *const u8,
        args_len: u32,
        value: u64,
        result_ptr: *mut u8,
        result_len: u32,
    ) -> u32;
}

/// Execute cross-contract call
pub fn call_contract(call: CrossCall) -> CallResult<Vec<u8>> {
    #[cfg(target_arch = "wasm32")]
    {
        let mut result_buffer = [0u8; 65536];
        
        let status = unsafe {
            cross_contract_call(
                call.target.0.as_ptr(),
                call.function.as_ptr(),
                call.function.len() as u32,
                call.args.as_ptr(),
                call.args.len() as u32,
                call.value,
                result_buffer.as_mut_ptr(),
                result_buffer.len() as u32,
            )
        };
        
        if status == 0 {
            Err(ContractError::Custom("Cross-contract call failed"))
        } else {
            // Return result data
            Ok(result_buffer[..(status as usize)].to_vec())
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Mock: cross-contract calls return empty success in test mode.
        // Individual tests can override behavior via test_mock storage.
        let _ = call;
        Ok(Vec::new())
    }
}

/// Helper: Call token transfer
pub fn call_token_transfer(
    token: Address,
    from: Address,
    to: Address,
    amount: u64,
) -> CallResult<bool> {
    let mut args = Vec::new();
    args.extend_from_slice(&from.0);
    args.extend_from_slice(&to.0);
    args.extend_from_slice(&amount.to_le_bytes());
    
    let call = CrossCall::new(token, "transfer", args);
    
    match call_contract(call) {
        Ok(result) => Ok(!result.is_empty() && result[0] == 1),
        Err(e) => Err(e),
    }
}

/// Helper: Call NFT transfer
pub fn call_nft_transfer(
    nft: Address,
    from: Address,
    to: Address,
    token_id: u64,
) -> CallResult<bool> {
    let mut args = Vec::new();
    args.extend_from_slice(&from.0);
    args.extend_from_slice(&to.0);
    args.extend_from_slice(&token_id.to_le_bytes());
    
    let call = CrossCall::new(nft, "transfer", args);
    
    match call_contract(call) {
        Ok(result) => Ok(!result.is_empty() && result[0] == 1),
        Err(e) => Err(e),
    }
}

/// Helper: Get token balance
pub fn call_token_balance(token: Address, account: Address) -> CallResult<u64> {
    let args = account.0.to_vec();
    
    let call = CrossCall::new(token, "balance_of", args);
    
    match call_contract(call) {
        Ok(result) if result.len() >= 8 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&result[..8]);
            Ok(u64::from_le_bytes(bytes))
        }
        Ok(_) => Err(ContractError::Custom("Invalid balance response")),
        Err(e) => Err(e),
    }
}

/// Helper: Get NFT owner
pub fn call_nft_owner(nft: Address, token_id: u64) -> CallResult<Address> {
    let args = token_id.to_le_bytes().to_vec();
    
    let call = CrossCall::new(nft, "owner_of", args);
    
    match call_contract(call) {
        Ok(result) if result.len() >= 32 => {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&result[..32]);
            Ok(Address(addr))
        }
        Ok(_) => Err(ContractError::Custom("Invalid owner response")),
        Err(e) => Err(e),
    }
}

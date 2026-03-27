// Cross-Contract Call Support
// Enables contracts to call functions on other contracts

use crate::{Address, ContractError};
use alloc::string::String;
use alloc::vec::Vec;

pub type CallResult<T> = Result<T, ContractError>;
pub const ABI_LAYOUT_MARKER: u8 = 0xAB;

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

/// Build layout-encoded args for named-export cross-contract calls with mixed
/// pointer- and value-like I32 parameters.
pub fn encode_layout_args(parts: &[&[u8]]) -> CallResult<Vec<u8>> {
    let data_len: usize = parts.iter().map(|part| part.len()).sum();
    let mut args = Vec::with_capacity(1 + parts.len() + data_len);
    args.push(ABI_LAYOUT_MARKER);
    for part in parts {
        if part.len() > u8::MAX as usize {
            return Err(ContractError::Custom("Layout argument exceeds 255 bytes"));
        }
        args.push(part.len() as u8);
    }
    for part in parts {
        args.extend_from_slice(part);
    }
    Ok(args)
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
        // Mock: cross-contract calls return configurable response in test mode.
        use crate::test_mock;
        test_mock::LAST_CROSS_CALL.with(|last| {
            *last.borrow_mut() = Some((
                call.target.0,
                call.function.clone(),
                call.args.clone(),
                call.value,
            ));
        });
        let should_fail = test_mock::CROSS_CALL_SHOULD_FAIL.with(|c| *c.borrow());
        if should_fail {
            return Err(ContractError::Custom("Mocked cross-contract call failed"));
        }
        let response = test_mock::CROSS_CALL_RESPONSE.with(|c| c.borrow().clone());
        if let Some(response) = response {
            Ok(response)
        } else if call.function == "transfer" {
            // Most unit tests do not seed an explicit mock response for token/NFT
            // transfers. Mirror the on-chain token ABI's zero success code so
            // payout paths remain testable by default.
            Ok(0u32.to_le_bytes().to_vec())
        } else {
            Ok(Vec::new())
        }
    }
}

fn decode_success_status(result: &[u8]) -> CallResult<bool> {
    if result.is_empty() {
        return Err(ContractError::Custom("Missing success response"));
    }

    if result.len() >= 4 {
        let mut status = [0u8; 4];
        status.copy_from_slice(&result[..4]);
        let code = u32::from_le_bytes(status);
        if code == 0 || code == 1 {
            return Ok(true);
        }
        return Ok(false);
    }

    Ok(result[0] == 1)
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
        Ok(result) => decode_success_status(&result),
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
        Ok(result) => decode_success_status(&result),
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

/// System program address (all zeros) — used as the target for native LICN operations.
pub const SYSTEM_PROGRAM: Address = Address([0u8; 32]);

/// Transfer native LICN from the calling contract to a user account.
/// Calls the system program (address zero) with the "transfer" function.
/// The contract must hold sufficient native LICN in its account balance.
pub fn transfer_native(to: Address, amount: u64) -> CallResult<bool> {
    let mut args = Vec::with_capacity(40);
    args.extend_from_slice(&to.0);
    args.extend_from_slice(&amount.to_le_bytes());

    let call = CrossCall::new(SYSTEM_PROGRAM, "transfer", args);

    match call_contract(call) {
        Ok(result) => decode_success_status(&result),
        Err(e) => Err(e),
    }
}

/// Check whether an address is the system program / native LICN sentinel.
pub fn is_native_token(addr: &Address) -> bool {
    addr.0 == [0u8; 32]
}

/// Query the native LICN balance of any account via the system program.
/// Returns the balance in spores (1 LICN = 1e9 spores).
pub fn native_balance_of(account: Address) -> CallResult<u64> {
    let args = account.0.to_vec();
    let call = CrossCall::new(SYSTEM_PROGRAM, "balance_of", args);

    match call_contract(call) {
        Ok(result) if result.len() >= 8 => {
            let bytes: [u8; 8] = result[..8].try_into().unwrap_or([0u8; 8]);
            Ok(u64::from_le_bytes(bytes))
        }
        Ok(_) => Ok(0),
        Err(e) => Err(e),
    }
}

/// Universal token transfer: sends tokens from the calling contract to a recipient.
/// If the token is native LICN (zero address), uses system program transfer.
/// Otherwise, uses cross-contract MT-20 `transfer` call.
/// The `from` parameter is used only for MT-20 tokens (must be the calling contract).
pub fn transfer_token_or_native(
    token: Address,
    from: Address,
    to: Address,
    amount: u64,
) -> CallResult<bool> {
    if is_native_token(&token) {
        transfer_native(to, amount)
    } else {
        call_token_transfer(token, from, to, amount)
    }
}

/// Universal token balance query.
/// If the token is native LICN (zero address), queries native account balance.
/// Otherwise, uses cross-contract MT-20 `balance_of` call.
pub fn balance_of_token_or_native(token: Address, account: Address) -> CallResult<u64> {
    if is_native_token(&token) {
        native_balance_of(account)
    } else {
        call_token_balance(token, account)
    }
}

/// Receive/escrow tokens from a user into the calling contract.
/// For native LICN (zero address): verifies that sufficient value was sent with
/// the transaction (value is already credited to the contract's account).
/// For MT-20 tokens: cross-contract transfer (requires prior approval from sender).
/// The `from` and `to` parameters are used only for MT-20 tokens.
pub fn receive_token_or_native(
    token: Address,
    from: Address,
    to: Address,
    amount: u64,
) -> CallResult<bool> {
    if is_native_token(&token) {
        let received = crate::get_value();
        if received >= amount {
            Ok(true)
        } else {
            Err(ContractError::Custom(
                "Insufficient native LICN value sent with transaction",
            ))
        }
    } else {
        call_token_transfer(token, from, to, amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_mock;

    #[test]
    fn test_encode_layout_args_builds_descriptor_and_payload() {
        let address = [7u8; 32];
        let number = 42u64.to_le_bytes();
        let boolean = [1u8];

        let args = encode_layout_args(&[&address, &number, &boolean]).unwrap();

        assert_eq!(args[0], ABI_LAYOUT_MARKER);
        assert_eq!(&args[1..4], &[32, 8, 1]);
        assert_eq!(&args[4..36], &address);
        assert_eq!(&args[36..44], &number);
        assert_eq!(args[44], 1);
    }

    #[test]
    fn test_decode_success_status_accepts_zero_and_one_codes() {
        assert!(decode_success_status(&0u32.to_le_bytes()).unwrap());
        assert!(decode_success_status(&1u32.to_le_bytes()).unwrap());
        assert!(!decode_success_status(&2u32.to_le_bytes()).unwrap());
        assert!(decode_success_status(&[1u8]).unwrap());
        assert!(!decode_success_status(&[0u8]).unwrap());
    }

    #[test]
    fn test_call_token_transfer_accepts_lichencoin_zero_code() {
        test_mock::reset();
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));

        let ok = call_token_transfer(
            Address([1u8; 32]),
            Address([2u8; 32]),
            Address([3u8; 32]),
            55,
        )
        .expect("cross-call should succeed");

        assert!(ok);
    }

    #[test]
    fn test_call_nft_transfer_accepts_one_code() {
        test_mock::reset();
        test_mock::set_cross_call_response(Some(1u32.to_le_bytes().to_vec()));

        let ok = call_nft_transfer(
            Address([4u8; 32]),
            Address([5u8; 32]),
            Address([6u8; 32]),
            77,
        )
        .expect("cross-call should succeed");

        assert!(ok);
    }

    #[test]
    fn test_call_token_transfer_defaults_to_success_for_unconfigured_mock() {
        test_mock::reset();

        let ok = call_token_transfer(
            Address([7u8; 32]),
            Address([8u8; 32]),
            Address([9u8; 32]),
            88,
        )
        .expect("default transfer mock should succeed");

        assert!(ok);
    }
}

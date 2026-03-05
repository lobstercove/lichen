// MoltChain Token Standard (MT-20)
// Unified storage key format matching wrapped-token convention.
// Balance:   {prefix}_bal_{hex64}   → u64 LE
// Allowance: {prefix}_alw_{hex64}_{hex64} → u64 LE
// Supply:    {prefix}_supply        → u64 LE

use crate::{Address, ContractResult, ContractError, storage_get, storage_set, bytes_to_u64, u64_to_bytes};
use alloc::vec::Vec;

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

fn hex_encode_32(bytes: &[u8; 32], out: &mut [u8; 64]) {
    for (i, &b) in bytes.iter().enumerate() {
        out[i * 2] = HEX_CHARS[(b >> 4) as usize];
        out[i * 2 + 1] = HEX_CHARS[(b & 0x0f) as usize];
    }
}

/// MT-20 fungible token with unified storage key format.
pub struct Token {
    pub name: &'static str,
    pub symbol: &'static str,
    pub decimals: u8,
    pub prefix: &'static str,
    pub total_supply: u64,
}

impl Token {
    /// Create new token with storage key prefix.
    /// `prefix` should be lowercase symbol (e.g., "molt", "wbnb").
    pub const fn new(name: &'static str, symbol: &'static str, decimals: u8, prefix: &'static str) -> Self {
        Token {
            name,
            symbol,
            decimals,
            prefix,
            total_supply: 0,
        }
    }

    /// Initialize token with initial supply
    pub fn initialize(&mut self, initial_supply: u64, owner: Address) -> ContractResult<()> {
        self.total_supply = initial_supply;
        storage_set(&self.supply_key(), &u64_to_bytes(initial_supply));
        self.set_balance(owner, initial_supply)?;
        Ok(())
    }

    /// Get balance of account
    pub fn balance_of(&self, account: Address) -> u64 {
        let key = self.balance_key(account);
        match storage_get(&key) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        }
    }

    /// Transfer tokens
    pub fn transfer(&self, from: Address, to: Address, amount: u64) -> ContractResult<()> {
        let from_balance = self.balance_of(from);
        if from_balance < amount {
            return Err(ContractError::InsufficientFunds);
        }
        self.set_balance(from, from_balance - amount)?;
        let to_balance = self.balance_of(to);
        self.set_balance(to, to_balance + amount)?;
        Ok(())
    }

    /// Mint new tokens (only owner)
    pub fn mint(&mut self, to: Address, amount: u64, caller: Address, owner: Address) -> ContractResult<()> {
        if caller != owner {
            return Err(ContractError::Unauthorized);
        }
        let balance = self.balance_of(to);
        self.set_balance(to, balance + amount)?;
        let current_supply = match storage_get(&self.supply_key()) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        let new_supply = current_supply + amount;
        self.total_supply = new_supply;
        storage_set(&self.supply_key(), &u64_to_bytes(new_supply));
        Ok(())
    }

    /// Burn tokens
    pub fn burn(&mut self, from: Address, amount: u64) -> ContractResult<()> {
        let balance = self.balance_of(from);
        if balance < amount {
            return Err(ContractError::InsufficientFunds);
        }
        self.set_balance(from, balance - amount)?;
        let current_supply = match storage_get(&self.supply_key()) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        let new_supply = current_supply - amount;
        self.total_supply = new_supply;
        storage_set(&self.supply_key(), &u64_to_bytes(new_supply));
        Ok(())
    }

    /// Get allowance
    pub fn allowance(&self, owner: Address, spender: Address) -> u64 {
        let key = self.allowance_key(owner, spender);
        match storage_get(&key) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        }
    }

    /// Approve spender
    pub fn approve(&self, owner: Address, spender: Address, amount: u64) -> ContractResult<()> {
        let key = self.allowance_key(owner, spender);
        storage_set(&key, &u64_to_bytes(amount));
        Ok(())
    }

    /// Transfer from (using allowance)
    pub fn transfer_from(&self, caller: Address, from: Address, to: Address, amount: u64) -> ContractResult<()> {
        let allowance = self.allowance(from, caller);
        if allowance < amount {
            return Err(ContractError::Unauthorized);
        }
        let key = self.allowance_key(from, caller);
        storage_set(&key, &u64_to_bytes(allowance - amount));
        self.transfer(from, to, amount)?;
        Ok(())
    }

    /// Get total supply from persistent storage
    pub fn get_total_supply(&self) -> u64 {
        match storage_get(&self.supply_key()) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        }
    }

    // ── Key builders ────────────────────────────────────────────────────

    /// `{prefix}_supply`
    fn supply_key(&self) -> Vec<u8> {
        let mut key = Vec::with_capacity(self.prefix.len() + 7);
        key.extend_from_slice(self.prefix.as_bytes());
        key.extend_from_slice(b"_supply");
        key
    }

    /// `{prefix}_bal_{hex64}`
    fn balance_key(&self, account: Address) -> Vec<u8> {
        let mut hex = [0u8; 64];
        hex_encode_32(account.to_bytes(), &mut hex);
        let mut key = Vec::with_capacity(self.prefix.len() + 5 + 64);
        key.extend_from_slice(self.prefix.as_bytes());
        key.extend_from_slice(b"_bal_");
        key.extend_from_slice(&hex);
        key
    }

    /// `{prefix}_alw_{hex64}_{hex64}`
    fn allowance_key(&self, owner: Address, spender: Address) -> Vec<u8> {
        let mut owner_hex = [0u8; 64];
        let mut spender_hex = [0u8; 64];
        hex_encode_32(owner.to_bytes(), &mut owner_hex);
        hex_encode_32(spender.to_bytes(), &mut spender_hex);
        let mut key = Vec::with_capacity(self.prefix.len() + 5 + 64 + 1 + 64);
        key.extend_from_slice(self.prefix.as_bytes());
        key.extend_from_slice(b"_alw_");
        key.extend_from_slice(&owner_hex);
        key.push(b'_');
        key.extend_from_slice(&spender_hex);
        key
    }

    fn set_balance(&self, account: Address, balance: u64) -> ContractResult<()> {
        let key = self.balance_key(account);
        storage_set(&key, &u64_to_bytes(balance));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_creation() {
        let token = Token::new("MoltCoin", "MOLT", 9, "molt");
        assert_eq!(token.name, "MoltCoin");
        assert_eq!(token.symbol, "MOLT");
        assert_eq!(token.decimals, 9);
        assert_eq!(token.prefix, "molt");
    }

    #[test]
    fn test_hex_encode() {
        let bytes = [0x01, 0xab, 0xff, 0x00, 0x10, 0x20, 0x30, 0x40,
                     0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0,
                     0xd0, 0xe0, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55,
                     0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd];
        let mut hex = [0u8; 64];
        hex_encode_32(&bytes, &mut hex);
        assert_eq!(&hex, b"01abff00102030405060708090a0b0c0d0e0f0112233445566778899aabbccdd");
    }

    #[test]
    fn test_balance_key_format() {
        let addr = Address::new([0u8; 32]);
        let token = Token::new("MoltCoin", "MOLT", 9, "molt");
        let key = token.balance_key(addr);
        // "molt_bal_" + 64 hex zeros
        let expected = b"molt_bal_0000000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(&key, expected);
    }

    #[test]
    fn test_supply_key_format() {
        let token = Token::new("MoltCoin", "MOLT", 9, "molt");
        let key = token.supply_key();
        assert_eq!(&key, b"molt_supply");
    }
}

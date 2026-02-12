// MoltChain Token Standard (MT-20)
// Similar to ERC-20/SPL Token

use crate::{Address, ContractResult, ContractError, storage_get, storage_set, bytes_to_u64, u64_to_bytes};
use alloc::vec::Vec;

/// Token metadata
pub struct Token {
    pub name: &'static str,
    pub symbol: &'static str,
    pub decimals: u8,
    pub total_supply: u64,
}

impl Token {
    /// Create new token
    pub const fn new(name: &'static str, symbol: &'static str, decimals: u8) -> Self {
        Token {
            name,
            symbol,
            decimals,
            total_supply: 0,
        }
    }

    /// Initialize token with initial supply
    pub fn initialize(&mut self, initial_supply: u64, owner: Address) -> ContractResult<()> {
        // Set total supply
        self.total_supply = initial_supply;
        storage_set(b"total_supply", &u64_to_bytes(initial_supply));
        
        // Mint to owner
        self.set_balance(owner, initial_supply)?;
        
        Ok(())
    }

    /// Get balance of account
    pub fn balance_of(&self, account: Address) -> u64 {
        let key = Self::balance_key(account);
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

        // Update balances
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

        // Read current total_supply from storage (struct field may be stale across calls)
        let current_supply = match storage_get(b"total_supply") {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        let new_supply = current_supply + amount;
        self.total_supply = new_supply;
        storage_set(b"total_supply", &u64_to_bytes(new_supply));

        Ok(())
    }

    /// Burn tokens
    pub fn burn(&mut self, from: Address, amount: u64) -> ContractResult<()> {
        let balance = self.balance_of(from);
        
        if balance < amount {
            return Err(ContractError::InsufficientFunds);
        }

        self.set_balance(from, balance - amount)?;

        // Read current total_supply from storage (struct field may be stale across calls)
        let current_supply = match storage_get(b"total_supply") {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        let new_supply = current_supply - amount;
        self.total_supply = new_supply;
        storage_set(b"total_supply", &u64_to_bytes(new_supply));

        Ok(())
    }

    /// Get allowance
    pub fn allowance(&self, owner: Address, spender: Address) -> u64 {
        let key = Self::allowance_key(owner, spender);
        match storage_get(&key) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        }
    }

    /// Approve spender
    pub fn approve(&self, owner: Address, spender: Address, amount: u64) -> ContractResult<()> {
        let key = Self::allowance_key(owner, spender);
        storage_set(&key, &u64_to_bytes(amount));
        Ok(())
    }

    /// Transfer from (using allowance)
    pub fn transfer_from(&self, caller: Address, from: Address, to: Address, amount: u64) -> ContractResult<()> {
        let allowance = self.allowance(from, caller);
        
        if allowance < amount {
            return Err(ContractError::Unauthorized);
        }

        // Update allowance
        let key = Self::allowance_key(from, caller);
        storage_set(&key, &u64_to_bytes(allowance - amount));

        // Transfer
        self.transfer(from, to, amount)?;

        Ok(())
    }

    /// Get total supply from persistent storage
    pub fn get_total_supply(&self) -> u64 {
        match storage_get(b"total_supply") {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        }
    }

    // Helper functions

    fn balance_key(account: Address) -> Vec<u8> {
        let mut key = Vec::new();
        key.extend_from_slice(b"balance:");
        key.extend_from_slice(account.to_bytes());
        key
    }

    fn allowance_key(owner: Address, spender: Address) -> Vec<u8> {
        let mut key = Vec::new();
        key.extend_from_slice(b"allowance:");
        key.extend_from_slice(owner.to_bytes());
        key.extend_from_slice(b":");
        key.extend_from_slice(spender.to_bytes());
        key
    }

    fn set_balance(&self, account: Address, balance: u64) -> ContractResult<()> {
        let key = Self::balance_key(account);
        storage_set(&key, &u64_to_bytes(balance));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_creation() {
        let token = Token::new("MoltCoin", "MOLT", 9);
        assert_eq!(token.name, "MoltCoin");
        assert_eq!(token.symbol, "MOLT");
        assert_eq!(token.decimals, 9);
    }
}

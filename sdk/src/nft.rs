// Lichen NFT Standard (MT-721)
// Similar to ERC-721 / Metaplex NFT Standard

use crate::{Address, ContractError, storage_get, storage_set, bytes_to_u64, u64_to_bytes};
use alloc::vec::Vec;

pub type NftResult<T> = Result<T, ContractError>;

/// NFT metadata
pub struct NFT {
    pub name: &'static str,
    pub symbol: &'static str,
    pub total_minted: u64,
}

impl NFT {
    /// Create new NFT collection
    pub const fn new(name: &'static str, symbol: &'static str) -> Self {
        NFT {
            name,
            symbol,
            total_minted: 0,
        }
    }

    /// Initialize NFT collection
    pub fn initialize(&mut self, minter: Address) -> NftResult<()> {
        // Set minter (can mint new tokens)
        let key = Self::minter_key();
        storage_set(&key, minter.0.as_slice());
        
        // Initialize counter
        storage_set(b"total_minted", &u64_to_bytes(0));
        
        Ok(())
    }

    /// Mint new NFT
    pub fn mint(&mut self, to: Address, token_id: u64, metadata_uri: &[u8]) -> NftResult<()> {
        // Check if token already exists
        if self.exists(token_id) {
            return Err(ContractError::Custom("Token already exists"));
        }

        // Set owner
        let owner_key = Self::owner_key(token_id);
        storage_set(&owner_key, to.0.as_slice());

        // Set metadata URI
        let metadata_key = Self::metadata_key(token_id);
        storage_set(&metadata_key, metadata_uri);

        // Increment total minted (read from storage to handle fresh WASM instances)
        let current_minted = match storage_get(b"total_minted") {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        let new_minted = current_minted + 1;
        self.total_minted = new_minted;
        storage_set(b"total_minted", &u64_to_bytes(new_minted));

        // Increment owner's balance
        self.increment_balance(to)?;

        Ok(())
    }

    /// Transfer NFT
    pub fn transfer(&self, from: Address, to: Address, token_id: u64) -> NftResult<()> {
        // Verify ownership
        let current_owner = self.owner_of(token_id)?;
        if current_owner.0 != from.0 {
            return Err(ContractError::Unauthorized);
        }

        // Update owner
        let owner_key = Self::owner_key(token_id);
        storage_set(&owner_key, to.0.as_slice());

        // Update balances
        self.decrement_balance(from)?;
        self.increment_balance(to)?;

        Ok(())
    }

    /// Get owner of NFT
    pub fn owner_of(&self, token_id: u64) -> NftResult<Address> {
        let key = Self::owner_key(token_id);
        match storage_get(&key) {
            Some(bytes) if bytes.len() == 32 => {
                let mut addr = [0u8; 32];
                addr.copy_from_slice(&bytes);
                Ok(Address(addr))
            }
            _ => Err(ContractError::Custom("Token does not exist")),
        }
    }

    /// Get metadata URI
    pub fn token_uri(&self, token_id: u64) -> Option<Vec<u8>> {
        let key = Self::metadata_key(token_id);
        storage_get(&key)
    }

    /// Check if token exists
    pub fn exists(&self, token_id: u64) -> bool {
        let key = Self::owner_key(token_id);
        storage_get(&key).is_some()
    }

    /// Get balance (number of NFTs owned)
    pub fn balance_of(&self, owner: Address) -> u64 {
        let key = Self::balance_key(owner);
        match storage_get(&key) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        }
    }

    /// Approve spender for specific token
    pub fn approve(&self, owner: Address, spender: Address, token_id: u64) -> NftResult<()> {
        // Verify ownership
        let current_owner = self.owner_of(token_id)?;
        if current_owner.0 != owner.0 {
            return Err(ContractError::Unauthorized);
        }

        // Set approval
        let key = Self::approval_key(token_id);
        storage_set(&key, spender.0.as_slice());

        Ok(())
    }

    /// Get approved spender for token
    pub fn get_approved(&self, token_id: u64) -> Option<Address> {
        let key = Self::approval_key(token_id);
        storage_get(&key).and_then(|bytes| {
            if bytes.len() == 32 {
                let mut addr = [0u8; 32];
                addr.copy_from_slice(&bytes);
                Some(Address(addr))
            } else {
                None
            }
        })
    }

    /// Set approval for all tokens
    pub fn set_approval_for_all(&self, owner: Address, operator: Address, approved: bool) -> NftResult<()> {
        let key = Self::operator_approval_key(owner, operator);
        storage_set(&key, &[if approved { 1 } else { 0 }]);
        Ok(())
    }

    /// Check if operator is approved for all
    pub fn is_approved_for_all(&self, owner: Address, operator: Address) -> bool {
        let key = Self::operator_approval_key(owner, operator);
        match storage_get(&key) {
            Some(bytes) => !bytes.is_empty() && bytes[0] == 1,
            None => false,
        }
    }

    /// Transfer from (with approval)
    pub fn transfer_from(&self, caller: Address, from: Address, to: Address, token_id: u64) -> NftResult<()> {
        // Check ownership
        let owner = self.owner_of(token_id)?;
        if owner.0 != from.0 {
            return Err(ContractError::Unauthorized);
        }

        // Check authorization
        let is_owner = caller.0 == from.0;
        let is_approved = self.get_approved(token_id).map_or(false, |a| a.0 == caller.0);
        let is_operator = self.is_approved_for_all(from, caller);

        if !is_owner && !is_approved && !is_operator {
            return Err(ContractError::Unauthorized);
        }

        // Clear approval
        let approval_key = Self::approval_key(token_id);
        storage_set(&approval_key, &[]);

        // Transfer
        self.transfer(from, to, token_id)?;

        Ok(())
    }

    /// Burn NFT
    pub fn burn(&mut self, owner: Address, token_id: u64) -> NftResult<()> {
        // Verify ownership
        let current_owner = self.owner_of(token_id)?;
        if current_owner.0 != owner.0 {
            return Err(ContractError::Unauthorized);
        }

        // Clear owner
        let owner_key = Self::owner_key(token_id);
        storage_set(&owner_key, &[]);

        // Clear metadata
        let metadata_key = Self::metadata_key(token_id);
        storage_set(&metadata_key, &[]);

        // Clear approvals
        let approval_key = Self::approval_key(token_id);
        storage_set(&approval_key, &[]);

        // Decrement balance
        self.decrement_balance(owner)?;

        Ok(())
    }

    /// Get total minted count from persistent storage
    pub fn get_total_minted(&self) -> u64 {
        match storage_get(b"total_minted") {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        }
    }

    // Storage key helpers

    fn owner_key(token_id: u64) -> Vec<u8> {
        let mut key = b"owner:".to_vec();
        key.extend_from_slice(&u64_to_bytes(token_id));
        key
    }

    fn metadata_key(token_id: u64) -> Vec<u8> {
        let mut key = b"metadata:".to_vec();
        key.extend_from_slice(&u64_to_bytes(token_id));
        key
    }

    fn balance_key(owner: Address) -> Vec<u8> {
        let mut key = b"balance:".to_vec();
        key.extend_from_slice(&owner.0);
        key
    }

    fn approval_key(token_id: u64) -> Vec<u8> {
        let mut key = b"approval:".to_vec();
        key.extend_from_slice(&u64_to_bytes(token_id));
        key
    }

    fn operator_approval_key(owner: Address, operator: Address) -> Vec<u8> {
        let mut key = b"operator:".to_vec();
        key.extend_from_slice(&owner.0);
        key.push(b':');
        key.extend_from_slice(&operator.0);
        key
    }

    fn minter_key() -> Vec<u8> {
        b"minter".to_vec()
    }

    fn increment_balance(&self, owner: Address) -> NftResult<()> {
        let key = Self::balance_key(owner);
        let balance = match storage_get(&key) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        storage_set(&key, &u64_to_bytes(balance + 1));
        Ok(())
    }

    fn decrement_balance(&self, owner: Address) -> NftResult<()> {
        let key = Self::balance_key(owner);
        let balance = match storage_get(&key) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        if balance == 0 {
            return Err(ContractError::InsufficientFunds);
        }
        storage_set(&key, &u64_to_bytes(balance - 1));
        Ok(())
    }
}

// Lichen NFT Standard (MT-721)
// Similar to ERC-721 / Metaplex NFT Standard

use crate::{bytes_to_u64, storage_get, storage_set, u64_to_bytes, Address, ContractError};
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
        if minter.0 == [0u8; 32] {
            return Err(ContractError::Custom("Minter cannot be zero"));
        }
        // Set minter (can mint new tokens)
        let key = Self::minter_key();
        if storage_get(&key).is_some() {
            return Err(ContractError::Custom("NFT already initialized"));
        }
        storage_set(&key, minter.0.as_slice());

        // Initialize counter
        storage_set(b"total_minted", &u64_to_bytes(0));

        Ok(())
    }

    /// Mint new NFT
    pub fn mint(&mut self, to: Address, token_id: u64, metadata_uri: &[u8]) -> NftResult<()> {
        if to.0 == [0u8; 32] {
            return Err(ContractError::Custom("Recipient cannot be zero"));
        }
        // Check if token already exists
        if self.exists(token_id) {
            return Err(ContractError::Custom("Token already exists"));
        }

        // Validate every fallible counter before writing owner or metadata.
        let current_minted = Self::read_u64_exact(b"total_minted")?;
        let new_minted = current_minted
            .checked_add(1)
            .ok_or(ContractError::Custom("Total minted overflow"))?;
        let recipient_balance = Self::read_u64_exact(&Self::balance_key(to))?;
        let new_recipient_balance = recipient_balance
            .checked_add(1)
            .ok_or(ContractError::Custom("Recipient balance overflow"))?;

        // Set owner
        let owner_key = Self::owner_key(token_id);
        storage_set(&owner_key, to.0.as_slice());

        // Set metadata URI
        let metadata_key = Self::metadata_key(token_id);
        storage_set(&metadata_key, metadata_uri);

        // Increment total minted (read from storage to handle fresh WASM instances)
        self.total_minted = new_minted;
        storage_set(b"total_minted", &u64_to_bytes(new_minted));

        // Increment owner's balance
        storage_set(&Self::balance_key(to), &u64_to_bytes(new_recipient_balance));

        Ok(())
    }

    /// Transfer NFT
    pub fn transfer(&self, from: Address, to: Address, token_id: u64) -> NftResult<()> {
        if from.0 == [0u8; 32] || to.0 == [0u8; 32] {
            return Err(ContractError::Custom("Transfer address cannot be zero"));
        }
        // Verify ownership
        let current_owner = self.owner_of(token_id)?;
        if current_owner.0 != from.0 {
            return Err(ContractError::Unauthorized);
        }

        let from_balance = Self::read_u64_exact(&Self::balance_key(from))?;
        if from_balance == 0 {
            return Err(ContractError::InsufficientFunds);
        }
        let to_balance = Self::read_u64_exact(&Self::balance_key(to))?;
        let new_to_balance = if from == to {
            to_balance
        } else {
            to_balance
                .checked_add(1)
                .ok_or(ContractError::Custom("Recipient balance overflow"))?
        };

        // ERC-721 authorization is token-owner specific. It must never follow
        // the token to a new owner, including through the direct transfer path.
        storage_set(&Self::approval_key(token_id), &[]);

        if from == to {
            return Ok(());
        }

        // Update owner
        let owner_key = Self::owner_key(token_id);
        storage_set(&owner_key, to.0.as_slice());

        // Update balances
        storage_set(&Self::balance_key(from), &u64_to_bytes(from_balance - 1));
        storage_set(&Self::balance_key(to), &u64_to_bytes(new_to_balance));

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
        Self::read_u64_exact(&Self::balance_key(owner)).unwrap_or(0)
    }

    /// Approve spender for specific token
    pub fn approve(&self, owner: Address, spender: Address, token_id: u64) -> NftResult<()> {
        // Verify ownership
        let current_owner = self.owner_of(token_id)?;
        if current_owner.0 != owner.0 {
            return Err(ContractError::Unauthorized);
        }
        if spender == owner {
            return Err(ContractError::Custom("Owner cannot approve itself"));
        }

        // Set approval
        let key = Self::approval_key(token_id);
        if spender.0 == [0u8; 32] {
            storage_set(&key, &[]);
        } else {
            storage_set(&key, spender.0.as_slice());
        }

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
    pub fn set_approval_for_all(
        &self,
        owner: Address,
        operator: Address,
        approved: bool,
    ) -> NftResult<()> {
        if owner.0 == [0u8; 32] || operator.0 == [0u8; 32] || owner == operator {
            return Err(ContractError::Custom("Invalid NFT operator approval"));
        }
        let key = Self::operator_approval_key(owner, operator);
        storage_set(&key, &[if approved { 1 } else { 0 }]);
        Ok(())
    }

    /// Check if operator is approved for all
    pub fn is_approved_for_all(&self, owner: Address, operator: Address) -> bool {
        let key = Self::operator_approval_key(owner, operator);
        match storage_get(&key) {
            Some(bytes) => bytes.as_slice() == [1u8],
            None => false,
        }
    }

    /// Transfer from (with approval)
    pub fn transfer_from(
        &self,
        caller: Address,
        from: Address,
        to: Address,
        token_id: u64,
    ) -> NftResult<()> {
        // Check ownership
        let owner = self.owner_of(token_id)?;
        if owner.0 != from.0 {
            return Err(ContractError::Unauthorized);
        }

        // Check authorization
        let is_owner = caller.0 == from.0;
        let is_approved = self.get_approved(token_id).is_some_and(|a| a.0 == caller.0);
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
        let owner_balance = Self::read_u64_exact(&Self::balance_key(owner))?;
        if owner_balance == 0 {
            return Err(ContractError::InsufficientFunds);
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
        storage_set(&Self::balance_key(owner), &u64_to_bytes(owner_balance - 1));

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

    fn read_u64_exact(key: &[u8]) -> NftResult<u64> {
        match storage_get(key) {
            Some(bytes) if bytes.len() == 8 => Ok(bytes_to_u64(&bytes)),
            Some(_) => Err(ContractError::Custom("Malformed NFT counter")),
            None => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_mock;

    fn initialized_nft() -> NFT {
        test_mock::reset();
        let mut nft = NFT::new("Test", "TNFT");
        nft.initialize(Address([1u8; 32])).expect("initialize");
        nft
    }

    #[test]
    fn direct_transfer_clears_stale_token_approval() {
        let mut nft = initialized_nft();
        let owner = Address([2u8; 32]);
        let approved = Address([3u8; 32]);
        let recipient = Address([4u8; 32]);
        nft.mint(owner, 7, b"moss://metadata").expect("mint");
        nft.approve(owner, approved, 7).expect("approve");

        nft.transfer(owner, recipient, 7).expect("owner transfer");
        assert_eq!(nft.owner_of(7).expect("owner"), recipient);
        assert_eq!(nft.get_approved(7), None);
        assert!(nft.transfer_from(approved, recipient, owner, 7).is_err());
        assert_eq!(nft.owner_of(7).expect("owner remains"), recipient);
    }

    #[test]
    fn mint_validates_counters_before_writing_token_state() {
        let mut nft = initialized_nft();
        storage_set(b"total_minted", &[1u8]);
        assert!(nft.mint(Address([2u8; 32]), 1, b"metadata").is_err());
        assert!(!nft.exists(1));

        storage_set(b"total_minted", &u64_to_bytes(0));
        let balance_key = NFT::balance_key(Address([2u8; 32]));
        storage_set(&balance_key, &u64_to_bytes(u64::MAX));
        assert!(nft.mint(Address([2u8; 32]), 2, b"metadata").is_err());
        assert!(!nft.exists(2));
    }

    #[test]
    fn transfer_rejects_malformed_balance_before_owner_change() {
        let mut nft = initialized_nft();
        let owner = Address([2u8; 32]);
        let recipient = Address([3u8; 32]);
        nft.mint(owner, 1, b"metadata").expect("mint");
        storage_set(&NFT::balance_key(owner), &[1u8]);

        assert!(nft.transfer(owner, recipient, 1).is_err());
        assert_eq!(nft.owner_of(1).expect("owner remains"), owner);
    }

    #[test]
    fn zero_approval_revokes_and_operator_self_approval_is_rejected() {
        let mut nft = initialized_nft();
        let owner = Address([2u8; 32]);
        let approved = Address([3u8; 32]);
        nft.mint(owner, 1, b"metadata").expect("mint");
        nft.approve(owner, approved, 1).expect("approve");
        nft.approve(owner, Address([0u8; 32]), 1).expect("revoke");
        assert_eq!(nft.get_approved(1), None);
        assert!(nft.approve(owner, owner, 1).is_err());
        assert!(nft.set_approval_for_all(owner, owner, true).is_err());
    }
}

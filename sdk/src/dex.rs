// MoltSwap - Decentralized Exchange (DEX)
// Automated Market Maker (AMM) using constant product formula: x * y = k

use crate::{Address, ContractError, storage_get, storage_set, bytes_to_u64, u64_to_bytes};
use alloc::vec::Vec;

pub type DexResult<T> = Result<T, ContractError>;

/// Liquidity pool for token pair
pub struct Pool {
    pub token_a: Address,       // First token address
    pub token_b: Address,       // Second token address
    pub reserve_a: u64,         // Reserve of token A
    pub reserve_b: u64,         // Reserve of token B
    pub total_liquidity: u64,   // Total LP tokens
    pub fee_numerator: u64,     // Fee (e.g., 3 for 0.3%)
    pub fee_denominator: u64,   // Fee denominator (e.g., 1000)
}

impl Pool {
    /// Create new pool
    pub const fn new(token_a: Address, token_b: Address) -> Self {
        Pool {
            token_a,
            token_b,
            reserve_a: 0,
            reserve_b: 0,
            total_liquidity: 0,
            fee_numerator: 3,      // 0.3% fee
            fee_denominator: 1000,
        }
    }

    /// Initialize pool
    pub fn initialize(&mut self, token_a: Address, token_b: Address) -> DexResult<()> {
        self.token_a = token_a;
        self.token_b = token_b;
        self.reserve_a = 0;
        self.reserve_b = 0;
        self.total_liquidity = 0;

        // Store pool data
        self.save()?;

        Ok(())
    }

    /// Add liquidity to pool
    pub fn add_liquidity(
        &mut self,
        provider: Address,
        amount_a: u64,
        amount_b: u64,
        min_liquidity: u64,
    ) -> DexResult<u64> {
        if amount_a == 0 || amount_b == 0 {
            return Err(ContractError::InvalidInput);
        }

        let liquidity: u64;

        if self.total_liquidity == 0 {
            // First liquidity provider
            // Liquidity = sqrt(amount_a * amount_b), use u128 to avoid overflow
            liquidity = Self::sqrt((amount_a as u128) * (amount_b as u128));
            
            if liquidity < min_liquidity {
                return Err(ContractError::Custom("Insufficient liquidity minted"));
            }
        } else {
            // Subsequent liquidity providers
            // Calculate liquidity proportional to pool reserves (u128 to avoid overflow)
            let liquidity_a = ((amount_a as u128) * (self.total_liquidity as u128) / (self.reserve_a as u128)) as u64;
            let liquidity_b = ((amount_b as u128) * (self.total_liquidity as u128) / (self.reserve_b as u128)) as u64;
            
            liquidity = if liquidity_a < liquidity_b { liquidity_a } else { liquidity_b };
            
            if liquidity < min_liquidity {
                return Err(ContractError::Custom("Insufficient liquidity minted"));
            }
        }

        // Update reserves
        self.reserve_a += amount_a;
        self.reserve_b += amount_b;
        self.total_liquidity += liquidity;

        // Update provider's liquidity balance
        let current_balance = self.get_liquidity_balance(provider);
        self.set_liquidity_balance(provider, current_balance + liquidity)?;

        // Save pool state
        self.save()?;

        Ok(liquidity)
    }

    /// Remove liquidity from pool
    pub fn remove_liquidity(
        &mut self,
        provider: Address,
        liquidity: u64,
        min_amount_a: u64,
        min_amount_b: u64,
    ) -> DexResult<(u64, u64)> {
        if liquidity == 0 {
            return Err(ContractError::InvalidInput);
        }

        if self.total_liquidity == 0 {
            return Err(ContractError::Custom("Pool has no liquidity"));
        }

        let provider_balance = self.get_liquidity_balance(provider);
        if provider_balance < liquidity {
            return Err(ContractError::InsufficientFunds);
        }

        // Calculate amounts to return (u128 to avoid overflow)
        let amount_a = ((liquidity as u128) * (self.reserve_a as u128) / (self.total_liquidity as u128)) as u64;
        let amount_b = ((liquidity as u128) * (self.reserve_b as u128) / (self.total_liquidity as u128)) as u64;

        if amount_a < min_amount_a || amount_b < min_amount_b {
            return Err(ContractError::Custom("Insufficient output amount"));
        }

        // Update reserves
        self.reserve_a -= amount_a;
        self.reserve_b -= amount_b;
        self.total_liquidity -= liquidity;

        // Update provider's balance
        self.set_liquidity_balance(provider, provider_balance - liquidity)?;

        // Save pool state
        self.save()?;

        Ok((amount_a, amount_b))
    }

    /// Swap token A for token B
    pub fn swap_a_for_b(
        &mut self,
        amount_a_in: u64,
        min_amount_b_out: u64,
    ) -> DexResult<u64> {
        if amount_a_in == 0 {
            return Err(ContractError::InvalidInput);
        }

        // Calculate output amount using constant product formula (u128 to avoid overflow)
        // amount_out = (amount_in * reserve_out * (1 - fee)) / (reserve_in + amount_in * (1 - fee))
        
        let amount_a_with_fee = (amount_a_in as u128) * ((self.fee_denominator - self.fee_numerator) as u128);
        let numerator = amount_a_with_fee * (self.reserve_b as u128);
        let denominator = ((self.reserve_a as u128) * (self.fee_denominator as u128)) + amount_a_with_fee;
        
        let amount_b_out = (numerator / denominator) as u64;

        if amount_b_out < min_amount_b_out {
            return Err(ContractError::Custom("Insufficient output amount"));
        }

        if amount_b_out >= self.reserve_b {
            return Err(ContractError::Custom("Insufficient liquidity"));
        }

        // Update reserves
        self.reserve_a += amount_a_in;
        self.reserve_b -= amount_b_out;

        // Save pool state
        self.save()?;

        Ok(amount_b_out)
    }

    /// Swap token B for token A
    pub fn swap_b_for_a(
        &mut self,
        amount_b_in: u64,
        min_amount_a_out: u64,
    ) -> DexResult<u64> {
        if amount_b_in == 0 {
            return Err(ContractError::InvalidInput);
        }

        let amount_b_with_fee = (amount_b_in as u128) * ((self.fee_denominator - self.fee_numerator) as u128);
        let numerator = amount_b_with_fee * (self.reserve_a as u128);
        let denominator = ((self.reserve_b as u128) * (self.fee_denominator as u128)) + amount_b_with_fee;
        
        let amount_a_out = (numerator / denominator) as u64;

        if amount_a_out < min_amount_a_out {
            return Err(ContractError::Custom("Insufficient output amount"));
        }

        if amount_a_out >= self.reserve_a {
            return Err(ContractError::Custom("Insufficient liquidity"));
        }

        // Update reserves
        self.reserve_b += amount_b_in;
        self.reserve_a -= amount_a_out;

        // Save pool state
        self.save()?;

        Ok(amount_a_out)
    }

    /// Get quote for swap A -> B
    pub fn get_amount_out(&self, amount_in: u64, reserve_in: u64, reserve_out: u64) -> u64 {
        if amount_in == 0 || reserve_in == 0 || reserve_out == 0 {
            return 0;
        }

        let amount_in_with_fee = (amount_in as u128) * ((self.fee_denominator - self.fee_numerator) as u128);
        let numerator = amount_in_with_fee * (reserve_out as u128);
        let denominator = ((reserve_in as u128) * (self.fee_denominator as u128)) + amount_in_with_fee;
        
        (numerator / denominator) as u64
    }

    /// Get liquidity balance of provider
    pub fn get_liquidity_balance(&self, provider: Address) -> u64 {
        let key = Self::liquidity_key(provider);
        match storage_get(&key) {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        }
    }

    /// Set liquidity balance
    fn set_liquidity_balance(&self, provider: Address, balance: u64) -> DexResult<()> {
        let key = Self::liquidity_key(provider);
        storage_set(&key, &u64_to_bytes(balance));
        Ok(())
    }

    /// Save pool state to storage
    pub fn save(&self) -> DexResult<()> {
        storage_set(b"token_a", &self.token_a.0);
        storage_set(b"token_b", &self.token_b.0);
        storage_set(b"reserve_a", &u64_to_bytes(self.reserve_a));
        storage_set(b"reserve_b", &u64_to_bytes(self.reserve_b));
        storage_set(b"total_liquidity", &u64_to_bytes(self.total_liquidity));
        Ok(())
    }

    /// Load pool state from storage
    pub fn load(&mut self) -> DexResult<()> {
        if let Some(bytes) = storage_get(b"token_a") {
            if bytes.len() == 32 {
                let mut addr = [0u8; 32];
                addr.copy_from_slice(&bytes);
                self.token_a = Address(addr);
            }
        }
        if let Some(bytes) = storage_get(b"token_b") {
            if bytes.len() == 32 {
                let mut addr = [0u8; 32];
                addr.copy_from_slice(&bytes);
                self.token_b = Address(addr);
            }
        }
        self.reserve_a = match storage_get(b"reserve_a") {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        self.reserve_b = match storage_get(b"reserve_b") {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        self.total_liquidity = match storage_get(b"total_liquidity") {
            Some(bytes) => bytes_to_u64(&bytes),
            None => 0,
        };
        Ok(())
    }

    // Storage keys

    fn liquidity_key(provider: Address) -> Vec<u8> {
        let mut key = b"liquidity:".to_vec();
        key.extend_from_slice(&provider.0);
        key
    }

    // Integer square root over u128 (for initial liquidity without overflow)
    fn sqrt(x: u128) -> u64 {
        if x == 0 {
            return 0;
        }
        
        let mut z = x;
        let mut y = (x + 1) / 2;
        
        while y < z {
            z = y;
            y = (x / y + y) / 2;
        }
        
        z as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_mock;

    fn addr(byte: u8) -> Address {
        let mut arr = [0u8; 32];
        arr[0] = byte;
        Address(arr)
    }

    #[test]
    fn test_sqrt() {
        assert_eq!(Pool::sqrt(0), 0);
        assert_eq!(Pool::sqrt(1), 1);
        assert_eq!(Pool::sqrt(4), 2);
        assert_eq!(Pool::sqrt(9), 3);
        assert_eq!(Pool::sqrt(100), 10);
        // floor(sqrt(2^64 - 1)) = 4294967295
        assert_eq!(Pool::sqrt(u64::MAX as u128), 4294967295);
    }

    #[test]
    fn test_add_liquidity_large_amounts_no_overflow() {
        test_mock::reset();
        let mut pool = Pool::new(addr(1), addr(2));
        // Values that would overflow u64 if multiplied directly
        let amount_a: u64 = 10_000_000_000_000; // 10 trillion
        let amount_b: u64 = 10_000_000_000_000;
        let result = pool.add_liquidity(addr(3), amount_a, amount_b, 0);
        assert!(result.is_ok());
        assert!(result.unwrap() > 0);
    }

    #[test]
    fn test_remove_liquidity_zero_pool_errors() {
        test_mock::reset();
        let mut pool = Pool::new(addr(1), addr(2));
        // Simulate corrupted state: provider has balance but pool has no liquidity
        let key = Pool::liquidity_key(addr(3));
        crate::storage_set(&key, &crate::u64_to_bytes(100));
        let result = pool.remove_liquidity(addr(3), 50, 0, 0);
        assert!(result.is_err()); // Should error, not panic with div-by-zero
    }

    #[test]
    fn test_swap_large_amounts_no_overflow() {
        test_mock::reset();
        let mut pool = Pool::new(addr(1), addr(2));
        pool.reserve_a = 1_000_000_000_000;
        pool.reserve_b = 1_000_000_000_000;
        pool.total_liquidity = 1_000_000_000_000;
        // Without u128, amount_a_with_fee * reserve_b overflows
        let result = pool.swap_a_for_b(500_000_000_000, 0);
        assert!(result.is_ok());
        let out = result.unwrap();
        assert!(out > 0);
    }
}

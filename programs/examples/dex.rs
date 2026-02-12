// MoltChain DEX - Automated Market Maker (AMM)
// Constant Product Market Maker (x * y = k)

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct Pool {
    pub token_a: [u8; 32],
    pub token_b: [u8; 32],
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub total_liquidity: u64,
    pub fee_percent: u64, // Basis points (e.g., 30 = 0.3%)
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct LiquidityPosition {
    pub owner: [u8; 32],
    pub pool_id: [u8; 32],
    pub liquidity: u64,
}

/// Initialize DEX
#[no_mangle]
pub extern "C" fn initialize() -> Result<(), String> {
    emit_event("Initialize", "DEX initialized");
    Ok(())
}

/// Create liquidity pool
#[no_mangle]
pub extern "C" fn create_pool(
    token_a: [u8; 32],
    token_b: [u8; 32],
    amount_a: u64,
    amount_b: u64,
    fee_percent: u64,
) -> Result<[u8; 32], String> {
    let caller = get_caller();
    
    if token_a == token_b {
        return Err("Tokens must be different".to_string());
    }
    
    if amount_a == 0 || amount_b == 0 {
        return Err("Amounts must be > 0".to_string());
    }
    
    if fee_percent > 10000 {
        return Err("Fee too high (max 100%)".to_string());
    }
    
    // Calculate pool ID
    let pool_id = calculate_pool_id(&token_a, &token_b);
    
    // Check if pool exists
    if get_storage::<Pool>(&pool_key(&pool_id))?.is_some() {
        return Err("Pool already exists".to_string());
    }
    
    // Calculate initial liquidity (geometric mean)
    let liquidity = (amount_a as u128 * amount_b as u128).sqrt() as u64;
    
    if liquidity == 0 {
        return Err("Initial liquidity too small".to_string());
    }
    
    // Create pool
    let pool = Pool {
        token_a,
        token_b,
        reserve_a: amount_a,
        reserve_b: amount_b,
        total_liquidity: liquidity,
        fee_percent,
    };
    
    set_storage(&pool_key(&pool_id), &pool)?;
    
    // Create liquidity position for creator
    let position = LiquidityPosition {
        owner: caller,
        pool_id,
        liquidity,
    };
    
    set_storage(&position_key(&caller, &pool_id), &position)?;
    
    emit_event("CreatePool", &format!("Pool created with {} liquidity", liquidity));
    
    Ok(pool_id)
}

/// Add liquidity to pool
#[no_mangle]
pub extern "C" fn add_liquidity(
    pool_id: [u8; 32],
    amount_a: u64,
    amount_b: u64,
) -> Result<u64, String> {
    let caller = get_caller();
    
    // Get pool
    let mut pool = get_storage::<Pool>(&pool_key(&pool_id))?
        .ok_or("Pool not found")?;
    
    // Calculate optimal amounts (maintain ratio)
    let amount_b_optimal = (amount_a as u128 * pool.reserve_b as u128 / pool.reserve_a as u128) as u64;
    
    let (final_amount_a, final_amount_b) = if amount_b_optimal <= amount_b {
        (amount_a, amount_b_optimal)
    } else {
        let amount_a_optimal = (amount_b as u128 * pool.reserve_a as u128 / pool.reserve_b as u128) as u64;
        (amount_a_optimal, amount_b)
    };
    
    if final_amount_a == 0 || final_amount_b == 0 {
        return Err("Amounts too small".to_string());
    }
    
    // Calculate liquidity to mint
    let liquidity = std::cmp::min(
        (final_amount_a as u128 * pool.total_liquidity as u128 / pool.reserve_a as u128) as u64,
        (final_amount_b as u128 * pool.total_liquidity as u128 / pool.reserve_b as u128) as u64,
    );
    
    if liquidity == 0 {
        return Err("Liquidity too small".to_string());
    }
    
    // Update pool
    pool.reserve_a += final_amount_a;
    pool.reserve_b += final_amount_b;
    pool.total_liquidity += liquidity;
    set_storage(&pool_key(&pool_id), &pool)?;
    
    // Update position
    let mut position = get_storage::<LiquidityPosition>(&position_key(&caller, &pool_id))?
        .unwrap_or(LiquidityPosition {
            owner: caller,
            pool_id,
            liquidity: 0,
        });
    
    position.liquidity += liquidity;
    set_storage(&position_key(&caller, &pool_id), &position)?;
    
    emit_event("AddLiquidity", &format!("Added {} liquidity", liquidity));
    
    Ok(liquidity)
}

/// Remove liquidity from pool
#[no_mangle]
pub extern "C" fn remove_liquidity(
    pool_id: [u8; 32],
    liquidity: u64,
) -> Result<(u64, u64), String> {
    let caller = get_caller();
    
    // Get pool
    let mut pool = get_storage::<Pool>(&pool_key(&pool_id))?
        .ok_or("Pool not found")?;
    
    // Get position
    let mut position = get_storage::<LiquidityPosition>(&position_key(&caller, &pool_id))?
        .ok_or("No liquidity position")?;
    
    if position.liquidity < liquidity {
        return Err("Insufficient liquidity".to_string());
    }
    
    // Calculate amounts to return
    let amount_a = (liquidity as u128 * pool.reserve_a as u128 / pool.total_liquidity as u128) as u64;
    let amount_b = (liquidity as u128 * pool.reserve_b as u128 / pool.total_liquidity as u128) as u64;
    
    if amount_a == 0 || amount_b == 0 {
        return Err("Amounts too small".to_string());
    }
    
    // Update pool
    pool.reserve_a -= amount_a;
    pool.reserve_b -= amount_b;
    pool.total_liquidity -= liquidity;
    set_storage(&pool_key(&pool_id), &pool)?;
    
    // Update position
    position.liquidity -= liquidity;
    set_storage(&position_key(&caller, &pool_id), &position)?;
    
    emit_event("RemoveLiquidity", &format!("Removed {} liquidity", liquidity));
    
    Ok((amount_a, amount_b))
}

/// Swap tokens (A for B or B for A)
#[no_mangle]
pub extern "C" fn swap(
    pool_id: [u8; 32],
    token_in: [u8; 32],
    amount_in: u64,
    min_amount_out: u64,
) -> Result<u64, String> {
    if amount_in == 0 {
        return Err("Amount must be > 0".to_string());
    }
    
    // Get pool
    let mut pool = get_storage::<Pool>(&pool_key(&pool_id))?
        .ok_or("Pool not found")?;
    
    // Determine direction
    let (reserve_in, reserve_out, is_a_to_b) = if token_in == pool.token_a {
        (pool.reserve_a, pool.reserve_b, true)
    } else if token_in == pool.token_b {
        (pool.reserve_b, pool.reserve_a, false)
    } else {
        return Err("Token not in pool".to_string());
    };
    
    // Calculate amount out with fee
    // amount_out = (amount_in * (10000 - fee) * reserve_out) / (reserve_in * 10000 + amount_in * (10000 - fee))
    let amount_in_with_fee = amount_in as u128 * (10000 - pool.fee_percent) as u128;
    let numerator = amount_in_with_fee * reserve_out as u128;
    let denominator = (reserve_in as u128 * 10000) + amount_in_with_fee;
    let amount_out = (numerator / denominator) as u64;
    
    if amount_out < min_amount_out {
        return Err("Slippage too high".to_string());
    }
    
    if amount_out == 0 {
        return Err("Output amount too small".to_string());
    }
    
    // Update reserves
    if is_a_to_b {
        pool.reserve_a += amount_in;
        pool.reserve_b -= amount_out;
    } else {
        pool.reserve_b += amount_in;
        pool.reserve_a -= amount_out;
    }
    
    set_storage(&pool_key(&pool_id), &pool)?;
    
    emit_event("Swap", &format!("Swapped {} in for {} out", amount_in, amount_out));
    
    Ok(amount_out)
}

/// Get pool info
#[no_mangle]
pub extern "C" fn get_pool(pool_id: [u8; 32]) -> Result<Pool, String> {
    get_storage::<Pool>(&pool_key(&pool_id))?
        .ok_or("Pool not found".to_string())
}

/// Get liquidity position
#[no_mangle]
pub extern "C" fn get_position(
    owner: [u8; 32],
    pool_id: [u8; 32],
) -> Result<LiquidityPosition, String> {
    get_storage::<LiquidityPosition>(&position_key(&owner, &pool_id))?
        .ok_or("Position not found".to_string())
}

/// Calculate price impact
#[no_mangle]
pub extern "C" fn get_amount_out(
    pool_id: [u8; 32],
    token_in: [u8; 32],
    amount_in: u64,
) -> Result<u64, String> {
    let pool = get_storage::<Pool>(&pool_key(&pool_id))?
        .ok_or("Pool not found")?;
    
    let (reserve_in, reserve_out) = if token_in == pool.token_a {
        (pool.reserve_a, pool.reserve_b)
    } else if token_in == pool.token_b {
        (pool.reserve_b, pool.reserve_a)
    } else {
        return Err("Token not in pool".to_string());
    };
    
    let amount_in_with_fee = amount_in as u128 * (10000 - pool.fee_percent) as u128;
    let numerator = amount_in_with_fee * reserve_out as u128;
    let denominator = (reserve_in as u128 * 10000) + amount_in_with_fee;
    
    Ok((numerator / denominator) as u64)
}

// Helper functions
fn get_caller() -> [u8; 32] {
    [0u8; 32]
}

fn calculate_pool_id(token_a: &[u8; 32], token_b: &[u8; 32]) -> [u8; 32] {
    // Simple hash for demo
    let mut id = [0u8; 32];
    for i in 0..32 {
        id[i] = token_a[i] ^ token_b[i];
    }
    id
}

fn pool_key(pool_id: &[u8; 32]) -> Vec<u8> {
    let mut key = b"pool:".to_vec();
    key.extend_from_slice(pool_id);
    key
}

fn position_key(owner: &[u8; 32], pool_id: &[u8; 32]) -> Vec<u8> {
    let mut key = b"position:".to_vec();
    key.extend_from_slice(owner);
    key.push(b':');
    key.extend_from_slice(pool_id);
    key
}

fn get_storage<T: BorshDeserialize>(_key: &[u8]) -> Result<Option<T>, String> {
    Ok(None)
}

fn set_storage<T: BorshSerialize>(_key: &[u8], _value: &T) -> Result<(), String> {
    Ok(())
}

fn emit_event(name: &str, data: &str) {
    println!("[Event] {}: {}", name, data);
}

trait Sqrt {
    fn sqrt(self) -> Self;
}

impl Sqrt for u128 {
    fn sqrt(self) -> Self {
        if self < 2 {
            return self;
        }
        let mut x = self;
        let mut y = (x + 1) / 2;
        while y < x {
            x = y;
            y = (x + self / x) / 2;
        }
        x
    }
}

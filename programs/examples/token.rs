// MoltChain ERC-20 Token Implementation
// Full-featured fungible token standard

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct TokenAccount {
    pub owner: [u8; 32],
    pub balance: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct TokenState {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub total_supply: u64,
    pub owner: [u8; 32],
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct Allowance {
    pub owner: [u8; 32],
    pub spender: [u8; 32],
    pub amount: u64,
}

// Storage keys
const STATE_KEY: &[u8] = b"token_state";

/// Initialize token
#[no_mangle]
pub extern "C" fn initialize(
    name: String,
    symbol: String,
    decimals: u8,
    initial_supply: u64,
) -> Result<(), String> {
    let caller = get_caller();
    
    let state = TokenState {
        name,
        symbol,
        decimals,
        total_supply: initial_supply,
        owner: caller,
    };
    
    // Save state
    set_storage(STATE_KEY, &state)?;
    
    // Mint initial supply to owner
    let owner_account = TokenAccount {
        owner: caller,
        balance: initial_supply,
    };
    set_storage(&account_key(&caller), &owner_account)?;
    
    emit_event("Initialize", &format!("Total supply: {}", initial_supply));
    
    Ok(())
}

/// Get token balance
#[no_mangle]
pub extern "C" fn balance_of(account: [u8; 32]) -> Result<u64, String> {
    match get_storage::<TokenAccount>(&account_key(&account))? {
        Some(acc) => Ok(acc.balance),
        None => Ok(0),
    }
}

/// Transfer tokens
#[no_mangle]
pub extern "C" fn transfer(to: [u8; 32], amount: u64) -> Result<(), String> {
    let from = get_caller();
    
    if from == to {
        return Err("Cannot transfer to self".to_string());
    }
    
    if amount == 0 {
        return Err("Amount must be > 0".to_string());
    }
    
    // Get sender balance
    let mut from_account = get_storage::<TokenAccount>(&account_key(&from))?
        .ok_or("Sender account not found")?;
    
    if from_account.balance < amount {
        return Err("Insufficient balance".to_string());
    }
    
    // Get recipient balance
    let mut to_account = get_storage::<TokenAccount>(&account_key(&to))?
        .unwrap_or(TokenAccount {
            owner: to,
            balance: 0,
        });
    
    // Update balances
    from_account.balance -= amount;
    to_account.balance += amount;
    
    // Save accounts
    set_storage(&account_key(&from), &from_account)?;
    set_storage(&account_key(&to), &to_account)?;
    
    emit_event("Transfer", &format!("{} tokens from {:?} to {:?}", amount, from, to));
    
    Ok(())
}

/// Approve spender
#[no_mangle]
pub extern "C" fn approve(spender: [u8; 32], amount: u64) -> Result<(), String> {
    let owner = get_caller();
    
    let allowance = Allowance {
        owner,
        spender,
        amount,
    };
    
    set_storage(&allowance_key(&owner, &spender), &allowance)?;
    
    emit_event("Approve", &format!("Approved {} tokens to {:?}", amount, spender));
    
    Ok(())
}

/// Transfer from (with allowance)
#[no_mangle]
pub extern "C" fn transfer_from(
    from: [u8; 32],
    to: [u8; 32],
    amount: u64,
) -> Result<(), String> {
    let spender = get_caller();
    
    // Check allowance
    let mut allowance = get_storage::<Allowance>(&allowance_key(&from, &spender))?
        .ok_or("No allowance")?;
    
    if allowance.amount < amount {
        return Err("Insufficient allowance".to_string());
    }
    
    // Get balances
    let mut from_account = get_storage::<TokenAccount>(&account_key(&from))?
        .ok_or("From account not found")?;
    
    if from_account.balance < amount {
        return Err("Insufficient balance".to_string());
    }
    
    let mut to_account = get_storage::<TokenAccount>(&account_key(&to))?
        .unwrap_or(TokenAccount {
            owner: to,
            balance: 0,
        });
    
    // Update balances
    from_account.balance -= amount;
    to_account.balance += amount;
    allowance.amount -= amount;
    
    // Save
    set_storage(&account_key(&from), &from_account)?;
    set_storage(&account_key(&to), &to_account)?;
    set_storage(&allowance_key(&from, &spender), &allowance)?;
    
    emit_event("TransferFrom", &format!("{} tokens from {:?} to {:?}", amount, from, to));
    
    Ok(())
}

/// Mint new tokens (owner only)
#[no_mangle]
pub extern "C" fn mint(to: [u8; 32], amount: u64) -> Result<(), String> {
    let caller = get_caller();
    
    // Get state
    let mut state = get_storage::<TokenState>(STATE_KEY)?
        .ok_or("Token not initialized")?;
    
    // Check owner
    if caller != state.owner {
        return Err("Only owner can mint".to_string());
    }
    
    // Update supply
    state.total_supply += amount;
    set_storage(STATE_KEY, &state)?;
    
    // Update recipient balance
    let mut to_account = get_storage::<TokenAccount>(&account_key(&to))?
        .unwrap_or(TokenAccount {
            owner: to,
            balance: 0,
        });
    
    to_account.balance += amount;
    set_storage(&account_key(&to), &to_account)?;
    
    emit_event("Mint", &format!("Minted {} tokens to {:?}", amount, to));
    
    Ok(())
}

/// Burn tokens
#[no_mangle]
pub extern "C" fn burn(amount: u64) -> Result<(), String> {
    let caller = get_caller();
    
    // Get balance
    let mut account = get_storage::<TokenAccount>(&account_key(&caller))?
        .ok_or("Account not found")?;
    
    if account.balance < amount {
        return Err("Insufficient balance to burn".to_string());
    }
    
    // Update balance and supply
    account.balance -= amount;
    set_storage(&account_key(&caller), &account)?;
    
    let mut state = get_storage::<TokenState>(STATE_KEY)?
        .ok_or("Token not initialized")?;
    state.total_supply -= amount;
    set_storage(STATE_KEY, &state)?;
    
    emit_event("Burn", &format!("Burned {} tokens", amount));
    
    Ok(())
}

/// Get total supply
#[no_mangle]
pub extern "C" fn total_supply() -> Result<u64, String> {
    let state = get_storage::<TokenState>(STATE_KEY)?
        .ok_or("Token not initialized")?;
    Ok(state.total_supply)
}

// Helper functions
fn get_caller() -> [u8; 32] {
    // In real implementation, this would get the transaction signer
    [0u8; 32]
}

fn account_key(address: &[u8; 32]) -> Vec<u8> {
    let mut key = b"account:".to_vec();
    key.extend_from_slice(address);
    key
}

fn allowance_key(owner: &[u8; 32], spender: &[u8; 32]) -> Vec<u8> {
    let mut key = b"allowance:".to_vec();
    key.extend_from_slice(owner);
    key.push(b':');
    key.extend_from_slice(spender);
    key
}

fn get_storage<T: BorshDeserialize>(key: &[u8]) -> Result<Option<T>, String> {
    // Mock implementation
    Ok(None)
}

fn set_storage<T: BorshSerialize>(key: &[u8], value: &T) -> Result<(), String> {
    // Mock implementation
    Ok(())
}

fn emit_event(name: &str, data: &str) {
    println!("[Event] {}: {}", name, data);
}

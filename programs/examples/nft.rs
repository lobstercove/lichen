// MoltChain ERC-721 NFT Implementation
// Non-fungible token standard

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct NFTMetadata {
    pub name: String,
    pub description: String,
    pub image: String,
    pub attributes: Vec<(String, String)>,
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct NFTState {
    pub name: String,
    pub symbol: String,
    pub owner: [u8; 32],
    pub total_supply: u64,
    pub next_token_id: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct TokenOwnership {
    pub token_id: u64,
    pub owner: [u8; 32],
    pub approved: Option<[u8; 32]>,
}

/// Initialize NFT collection
#[no_mangle]
pub extern "C" fn initialize(
    name: String,
    symbol: String,
) -> Result<(), String> {
    let caller = get_caller();
    
    let state = NFTState {
        name,
        symbol,
        owner: caller,
        total_supply: 0,
        next_token_id: 1,
    };
    
    set_storage(b"nft_state", &state)?;
    
    emit_event("Initialize", &format!("NFT collection: {}", state.name));
    
    Ok(())
}

/// Mint new NFT
#[no_mangle]
pub extern "C" fn mint(
    to: [u8; 32],
    metadata: NFTMetadata,
) -> Result<u64, String> {
    let caller = get_caller();
    
    let mut state = get_storage::<NFTState>(b"nft_state")?
        .ok_or("NFT not initialized")?;
    
    // Check owner
    if caller != state.owner {
        return Err("Only owner can mint".to_string());
    }
    
    let token_id = state.next_token_id;
    
    // Create token ownership
    let ownership = TokenOwnership {
        token_id,
        owner: to,
        approved: None,
    };
    
    // Save token
    set_storage(&token_key(token_id), &ownership)?;
    set_storage(&metadata_key(token_id), &metadata)?;
    
    // Update state
    state.total_supply += 1;
    state.next_token_id += 1;
    set_storage(b"nft_state", &state)?;
    
    // Update owner's balance
    let mut balance = get_balance(&to);
    balance += 1;
    set_balance(&to, balance)?;
    
    emit_event("Mint", &format!("Minted token #{} to {:?}", token_id, to));
    
    Ok(token_id)
}

/// Transfer NFT
#[no_mangle]
pub extern "C" fn transfer_from(
    from: [u8; 32],
    to: [u8; 32],
    token_id: u64,
) -> Result<(), String> {
    let caller = get_caller();
    
    if from == to {
        return Err("Cannot transfer to self".to_string());
    }
    
    // Get token ownership
    let mut ownership = get_storage::<TokenOwnership>(&token_key(token_id))?
        .ok_or("Token does not exist")?;
    
    // Check authorization
    if caller != ownership.owner && Some(caller) != ownership.approved {
        return Err("Not authorized to transfer".to_string());
    }
    
    if ownership.owner != from {
        return Err("From address is not the owner".to_string());
    }
    
    // Update ownership
    ownership.owner = to;
    ownership.approved = None; // Clear approval
    set_storage(&token_key(token_id), &ownership)?;
    
    // Update balances
    let mut from_balance = get_balance(&from);
    from_balance -= 1;
    set_balance(&from, from_balance)?;
    
    let mut to_balance = get_balance(&to);
    to_balance += 1;
    set_balance(&to, to_balance)?;
    
    emit_event("Transfer", &format!("Token #{} from {:?} to {:?}", token_id, from, to));
    
    Ok(())
}

/// Approve address to transfer NFT
#[no_mangle]
pub extern "C" fn approve(
    to: [u8; 32],
    token_id: u64,
) -> Result<(), String> {
    let caller = get_caller();
    
    // Get token ownership
    let mut ownership = get_storage::<TokenOwnership>(&token_key(token_id))?
        .ok_or("Token does not exist")?;
    
    // Check owner
    if caller != ownership.owner {
        return Err("Only owner can approve".to_string());
    }
    
    // Set approval
    ownership.approved = Some(to);
    set_storage(&token_key(token_id), &ownership)?;
    
    emit_event("Approve", &format!("Approved {:?} for token #{}", to, token_id));
    
    Ok(())
}

/// Get token owner
#[no_mangle]
pub extern "C" fn owner_of(token_id: u64) -> Result<[u8; 32], String> {
    let ownership = get_storage::<TokenOwnership>(&token_key(token_id))?
        .ok_or("Token does not exist")?;
    Ok(ownership.owner)
}

/// Get token metadata
#[no_mangle]
pub extern "C" fn token_uri(token_id: u64) -> Result<NFTMetadata, String> {
    get_storage::<NFTMetadata>(&metadata_key(token_id))?
        .ok_or("Metadata not found".to_string())
}

/// Get balance of address
#[no_mangle]
pub extern "C" fn balance_of(owner: [u8; 32]) -> Result<u64, String> {
    Ok(get_balance(&owner))
}

/// Burn NFT
#[no_mangle]
pub extern "C" fn burn(token_id: u64) -> Result<(), String> {
    let caller = get_caller();
    
    // Get token ownership
    let ownership = get_storage::<TokenOwnership>(&token_key(token_id))?
        .ok_or("Token does not exist")?;
    
    // Check owner
    if caller != ownership.owner {
        return Err("Only owner can burn".to_string());
    }
    
    // Delete token
    delete_storage(&token_key(token_id))?;
    delete_storage(&metadata_key(token_id))?;
    
    // Update balance
    let mut balance = get_balance(&caller);
    balance -= 1;
    set_balance(&caller, balance)?;
    
    // Update total supply
    let mut state = get_storage::<NFTState>(b"nft_state")?
        .ok_or("NFT not initialized")?;
    state.total_supply -= 1;
    set_storage(b"nft_state", &state)?;
    
    emit_event("Burn", &format!("Burned token #{}", token_id));
    
    Ok(())
}

/// Get collection info
#[no_mangle]
pub extern "C" fn get_collection_info() -> Result<NFTState, String> {
    get_storage::<NFTState>(b"nft_state")?
        .ok_or("NFT not initialized".to_string())
}

// Helper functions
fn get_caller() -> [u8; 32] {
    [0u8; 32]
}

fn token_key(token_id: u64) -> Vec<u8> {
    let mut key = b"token:".to_vec();
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn metadata_key(token_id: u64) -> Vec<u8> {
    let mut key = b"metadata:".to_vec();
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn balance_key(address: &[u8; 32]) -> Vec<u8> {
    let mut key = b"balance:".to_vec();
    key.extend_from_slice(address);
    key
}

fn get_balance(address: &[u8; 32]) -> u64 {
    get_storage::<u64>(&balance_key(address))
        .ok()
        .flatten()
        .unwrap_or(0)
}

fn set_balance(address: &[u8; 32], balance: u64) -> Result<(), String> {
    set_storage(&balance_key(address), &balance)
}

fn get_storage<T: BorshDeserialize>(_key: &[u8]) -> Result<Option<T>, String> {
    Ok(None)
}

fn set_storage<T: BorshSerialize>(_key: &[u8], _value: &T) -> Result<(), String> {
    Ok(())
}

fn delete_storage(_key: &[u8]) -> Result<(), String> {
    Ok(())
}

fn emit_event(name: &str, data: &str) {
    println!("[Event] {}: {}", name, data);
}

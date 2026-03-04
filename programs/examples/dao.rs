// MoltChain DAO - Decentralized Autonomous Organization
// Governance with proposals and voting

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub enum ProposalType {
    Transfer { to: [u8; 32], amount: u64 },
    UpdateConfig { key: String, value: String },
    AddMember { member: [u8; 32] },
    RemoveMember { member: [u8; 32] },
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq)]
pub enum ProposalStatus {
    Pending,
    Active,
    Passed,
    Rejected,
    Executed,
    Cancelled,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct Proposal {
    pub id: u64,
    pub proposer: [u8; 32],
    pub proposal_type: ProposalType,
    pub description: String,
    pub votes_for: u64,
    pub votes_against: u64,
    pub status: ProposalStatus,
    pub created_at: u64,
    pub voting_ends_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct DAOConfig {
    pub name: String,
    pub token: [u8; 32],
    pub quorum_percent: u64,
    pub pass_threshold_percent: u64,
    pub voting_period: u64,
    pub proposal_threshold: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct Member {
    pub address: [u8; 32],
    pub voting_power: u64,
    pub joined_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct Vote {
    pub proposal_id: u64,
    pub voter: [u8; 32],
    pub support: bool,
    pub voting_power: u64,
}

/// Initialize DAO
#[no_mangle]
pub extern "C" fn initialize(
    name: String,
    token: [u8; 32],
    quorum_percent: u64,
    pass_threshold_percent: u64,
    voting_period: u64,
    proposal_threshold: u64,
) -> Result<(), String> {
    let caller = get_caller();
    
    if quorum_percent > 100 || pass_threshold_percent > 100 {
        return Err("Percentages must be <= 100".to_string());
    }
    
    let config = DAOConfig {
        name,
        token,
        quorum_percent,
        pass_threshold_percent,
        voting_period,
        proposal_threshold,
    };
    
    set_storage(b"dao_config", &config)?;
    
    // Add founder as first member
    let member = Member {
        address: caller,
        voting_power: 1000,
        joined_at: get_timestamp(),
    };
    
    set_storage(&member_key(&caller), &member)?;
    
    emit_event("Initialize", &format!("DAO created: {}", config.name));
    
    Ok(())
}

/// Create proposal
#[no_mangle]
pub extern "C" fn create_proposal(
    proposal_type: ProposalType,
    description: String,
) -> Result<u64, String> {
    let caller = get_caller();
    
    // Check if caller is a member
    let member = get_storage::<Member>(&member_key(&caller))?
        .ok_or("Not a DAO member")?;
    
    // Check proposal threshold
    let config = get_storage::<DAOConfig>(b"dao_config")?
        .ok_or("DAO not initialized")?;
    
    if member.voting_power < config.proposal_threshold {
        return Err("Insufficient voting power to create proposal".to_string());
    }
    
    // Get next proposal ID
    let proposal_id = get_next_proposal_id()?;
    
    // Create proposal
    let proposal = Proposal {
        id: proposal_id,
        proposer: caller,
        proposal_type,
        description,
        votes_for: 0,
        votes_against: 0,
        status: ProposalStatus::Active,
        created_at: get_timestamp(),
        voting_ends_at: get_timestamp() + config.voting_period,
    };
    
    set_storage(&proposal_key(proposal_id), &proposal)?;
    
    emit_event("CreateProposal", &format!("Proposal #{} created", proposal_id));
    
    Ok(proposal_id)
}

/// Vote on proposal
#[no_mangle]
pub extern "C" fn vote(
    proposal_id: u64,
    support: bool,
) -> Result<(), String> {
    let caller = get_caller();
    
    // Check if already voted
    if get_storage::<Vote>(&vote_key(proposal_id, &caller))?.is_some() {
        return Err("Already voted".to_string());
    }
    
    // Get member
    let member = get_storage::<Member>(&member_key(&caller))?
        .ok_or("Not a DAO member")?;
    
    // Get proposal
    let mut proposal = get_storage::<Proposal>(&proposal_key(proposal_id))?
        .ok_or("Proposal not found")?;
    
    // Check if voting is still active
    if proposal.status != ProposalStatus::Active {
        return Err("Proposal not active".to_string());
    }
    
    if get_timestamp() > proposal.voting_ends_at {
        return Err("Voting period ended".to_string());
    }
    
    // Record vote
    let vote = Vote {
        proposal_id,
        voter: caller,
        support,
        voting_power: member.voting_power,
    };
    
    set_storage(&vote_key(proposal_id, &caller), &vote)?;
    
    // Update proposal votes
    if support {
        proposal.votes_for += member.voting_power;
    } else {
        proposal.votes_against += member.voting_power;
    }
    
    set_storage(&proposal_key(proposal_id), &proposal)?;
    
    emit_event("Vote", &format!("Voted {} on proposal #{}", if support { "FOR" } else { "AGAINST" }, proposal_id));
    
    Ok(())
}

/// Finalize proposal (after voting ends)
#[no_mangle]
pub extern "C" fn finalize_proposal(proposal_id: u64) -> Result<(), String> {
    let mut proposal = get_storage::<Proposal>(&proposal_key(proposal_id))?
        .ok_or("Proposal not found")?;
    
    if proposal.status != ProposalStatus::Active {
        return Err("Proposal not active".to_string());
    }
    
    if get_timestamp() <= proposal.voting_ends_at {
        return Err("Voting period not ended".to_string());
    }
    
    // Get config
    let config = get_storage::<DAOConfig>(b"dao_config")?
        .ok_or("DAO not initialized")?;
    
    // Get total voting power
    let total_votes = proposal.votes_for + proposal.votes_against;
    let total_power = get_total_voting_power()?;
    
    // Check quorum
    let quorum = total_power * config.quorum_percent / 100;
    if total_votes < quorum {
        proposal.status = ProposalStatus::Rejected;
        emit_event("FinalizeProposal", &format!("Proposal #{} rejected (quorum not met)", proposal_id));
    } else {
        // Check pass threshold
        let pass_threshold = total_votes * config.pass_threshold_percent / 100;
        if proposal.votes_for >= pass_threshold {
            proposal.status = ProposalStatus::Passed;
            emit_event("FinalizeProposal", &format!("Proposal #{} passed", proposal_id));
        } else {
            proposal.status = ProposalStatus::Rejected;
            emit_event("FinalizeProposal", &format!("Proposal #{} rejected", proposal_id));
        }
    }
    
    set_storage(&proposal_key(proposal_id), &proposal)?;
    
    Ok(())
}

/// Execute proposal (after it passed)
#[no_mangle]
pub extern "C" fn execute_proposal(proposal_id: u64) -> Result<(), String> {
    let mut proposal = get_storage::<Proposal>(&proposal_key(proposal_id))?
        .ok_or("Proposal not found")?;
    
    if proposal.status != ProposalStatus::Passed {
        return Err("Proposal not passed".to_string());
    }
    
    // Execute based on type
    match &proposal.proposal_type {
        ProposalType::Transfer { to, amount } => {
            // Transfer tokens from treasury
            emit_event("Execute", &format!("Transferred {} to {:?}", amount, to));
        }
        ProposalType::UpdateConfig { key, value } => {
            // Update configuration
            emit_event("Execute", &format!("Updated config: {} = {}", key, value));
        }
        ProposalType::AddMember { member } => {
            // Add new member
            let new_member = Member {
                address: *member,
                voting_power: 100,
                joined_at: get_timestamp(),
            };
            set_storage(&member_key(member), &new_member)?;
            emit_event("Execute", &format!("Added member: {:?}", member));
        }
        ProposalType::RemoveMember { member } => {
            // Remove member
            delete_storage(&member_key(member))?;
            emit_event("Execute", &format!("Removed member: {:?}", member));
        }
    }
    
    proposal.status = ProposalStatus::Executed;
    set_storage(&proposal_key(proposal_id), &proposal)?;
    
    Ok(())
}

/// Get proposal
#[no_mangle]
pub extern "C" fn get_proposal(proposal_id: u64) -> Result<Proposal, String> {
    get_storage::<Proposal>(&proposal_key(proposal_id))?
        .ok_or("Proposal not found".to_string())
}

/// Cancel proposal (proposer only)
#[no_mangle]
pub extern "C" fn cancel_proposal(proposal_id: u64) -> Result<(), String> {
    let caller = get_caller();
    
    let mut proposal = get_storage::<Proposal>(&proposal_key(proposal_id))?
        .ok_or("Proposal not found")?;
    
    if proposal.proposer != caller {
        return Err("Only proposer can cancel".to_string());
    }
    
    if proposal.status != ProposalStatus::Active && proposal.status != ProposalStatus::Pending {
        return Err("Proposal cannot be cancelled".to_string());
    }
    
    proposal.status = ProposalStatus::Cancelled;
    set_storage(&proposal_key(proposal_id), &proposal)?;
    
    emit_event("CancelProposal", &format!("Proposal #{} cancelled", proposal_id));
    
    Ok(())
}

/// Get DAO stats
#[no_mangle]
pub extern "C" fn get_dao_stats() -> Result<DAOConfig, String> {
    get_storage::<DAOConfig>(b"dao_config")?
        .ok_or("DAO not initialized".to_string())
}

// Helper functions
fn get_caller() -> [u8; 32] {
    [0u8; 32]
}

fn get_timestamp() -> u64 {
    // In real implementation, this would return current block timestamp
    1234567890
}

fn get_next_proposal_id() -> Result<u64, String> {
    let current = get_storage::<u64>(b"next_proposal_id")?
        .unwrap_or(1);
    set_storage(b"next_proposal_id", &(current + 1))?;
    Ok(current)
}

fn get_total_voting_power() -> Result<u64, String> {
    // In real implementation, this would sum all members' voting power
    Ok(10000)
}

fn proposal_key(id: u64) -> Vec<u8> {
    let mut key = b"proposal:".to_vec();
    key.extend_from_slice(&id.to_le_bytes());
    key
}

fn member_key(address: &[u8; 32]) -> Vec<u8> {
    let mut key = b"member:".to_vec();
    key.extend_from_slice(address);
    key
}

fn vote_key(proposal_id: u64, voter: &[u8; 32]) -> Vec<u8> {
    let mut key = b"vote:".to_vec();
    key.extend_from_slice(&proposal_id.to_le_bytes());
    key.push(b':');
    key.extend_from_slice(voter);
    key
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

use crate::client::{StakingRewards, StakingStatus};
use crate::output_support::to_licn;

pub(super) fn print_stake_status_details(status: &StakingStatus) {
    println!("👤 Account: {}", status.address);
    println!("💰 Staked: {} LICN", to_licn(status.staked));
    println!(
        "📊 Status: {}",
        if status.is_validator {
            "Active Validator"
        } else {
            "Not Validating"
        }
    );
}

pub(super) fn print_stake_rewards_details(rewards: &StakingRewards) {
    println!("👤 Account: {}", rewards.address);
    println!("💰 Total rewards: {} LICN", to_licn(rewards.total_rewards));
    println!("⏳ Pending rewards: {} LICN", to_licn(rewards.pending_rewards));
}
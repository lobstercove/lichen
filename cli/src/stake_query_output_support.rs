use crate::client::{StakingRewards, StakingStatus};
use crate::output_support::to_licn;

pub(super) fn print_stake_status_details(status: &StakingStatus) {
    println!("💰 Total staked: {} LICN", to_licn(status.total_staked));
    println!(
        "📊 Status: {}",
        if status.is_validator {
            "Active Validator"
        } else {
            "Not Validating"
        }
    );
    println!("🔎 Chain status: {}", status.status);
    if status.bootstrap_debt > 0 || status.earned_amount > 0 || status.total_debt_repaid > 0 {
        println!("🏦 Bootstrap debt: {} LICN", to_licn(status.bootstrap_debt));
        println!("✅ Earned amount: {} LICN", to_licn(status.earned_amount));
        println!(
            "♻️  Debt repaid: {} LICN",
            to_licn(status.total_debt_repaid)
        );
        if !status.vesting_status.is_empty() {
            println!("📌 Vesting status: {}", status.vesting_status);
        }
    }
}

pub(super) fn print_stake_rewards_details(rewards: &StakingRewards) {
    println!("💰 Total rewards: {} LICN", to_licn(rewards.total_rewards));
    println!("⏳ Pending rewards: {} LICN", to_licn(rewards.pending_rewards));
    println!(
        "📈 Projected pending: {} LICN",
        to_licn(rewards.projected_pending)
    );
    println!(
        "🏁 Projected epoch reward: {} LICN",
        to_licn(rewards.projected_epoch_reward)
    );
    println!("✅ Claimed liquid: {} LICN", to_licn(rewards.claimed_rewards));
    println!(
        "📚 Claimed total: {} LICN",
        to_licn(rewards.claimed_total_rewards)
    );
    if rewards.total_debt_repaid > 0 {
        println!("♻️  Debt repaid: {} LICN", to_licn(rewards.total_debt_repaid));
    }
}

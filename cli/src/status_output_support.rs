use crate::client::{ChainStatus, Metrics};
use crate::output_support::to_licn;

pub(super) fn print_chain_status(status: &ChainStatus) {
    println!("⛓️  Chain: {}", status.chain_id);
    println!("🌐 Network: {}", status.network);
    println!();

    println!("📊 Block Production:");
    println!("   Current slot: {}", status.current_slot);
    println!("   Latest block: {}", status.latest_block);
    println!("   Block time: {}ms", status.block_time_ms);
    println!();

    println!("👥 Network:");
    println!("   Validators: {}", status.validator_count);
    println!("   Connected peers: {}", status.peer_count);
    println!();

    println!("📈 Activity:");
    println!("   TPS: {}", status.tps);
    println!("   Total transactions: {}", status.total_transactions);
    println!("   Total blocks: {}", status.total_blocks);
    println!();

    println!("💰 Economics:");
    println!("   Total supply: {} LICN", to_licn(status.total_supply));
    println!("   Total burned: {} LICN", to_licn(status.total_burned));
    println!("   Total staked: {} LICN", to_licn(status.total_staked));
    println!();

    println!("✅ Chain is healthy");
}

pub(super) fn print_chain_metrics(metrics: &Metrics) {
    println!("📊 Performance:");
    println!("   TPS: {}", metrics.tps);
    println!("   Average block time: {}ms", metrics.avg_block_time_ms);
    println!(
        "   Transactions per block: {:.1}",
        metrics.avg_txs_per_block
    );
    println!();

    println!("📈 Totals:");
    println!("   Blocks: {}", metrics.total_blocks);
    println!("   Transactions: {}", metrics.total_transactions);
    println!("   Accounts: {}", metrics.total_accounts);
    println!("   Contracts: {}", metrics.total_contracts);
    println!();

    println!("💰 Economics:");
    println!("   Total supply: {} LICN", to_licn(metrics.total_supply));
    println!(
        "   Circulating: {} LICN",
        to_licn(metrics.circulating_supply)
    );
    let burn_pct = if metrics.total_supply > 0 {
        (metrics.total_burned as f64 / metrics.total_supply as f64) * 100.0
    } else {
        0.0
    };
    let stake_pct = if metrics.total_supply > 0 {
        (metrics.total_staked as f64 / metrics.total_supply as f64) * 100.0
    } else {
        0.0
    };
    println!(
        "   Burned: {} LICN ({:.2}%)",
        to_licn(metrics.total_burned),
        burn_pct
    );
    println!(
        "   Staked: {} LICN ({:.2}%)",
        to_licn(metrics.total_staked),
        stake_pct
    );
}

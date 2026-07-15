use super::*;

mod bitcoin;
mod evm;
mod solana;

use self::bitcoin::process_bitcoin_deposits;
use self::evm::process_evm_deposits_for_chains;
use self::solana::process_solana_deposits;

pub(super) async fn solana_watcher_loop(state: CustodyState, url: String) {
    loop {
        if let Err(err) = process_solana_deposits(&state, &url).await {
            tracing::warn!("solana watcher error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

pub(super) async fn evm_watcher_loop_for_chains(
    state: CustodyState,
    url: String,
    chains: &'static [&'static str],
) {
    loop {
        if let Err(err) = process_evm_deposits_for_chains(&state, &url, chains).await {
            tracing::warn!("evm watcher ({:?}) error: {}", chains, err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

pub(super) async fn bitcoin_watcher_loop(state: CustodyState) {
    loop {
        if let Err(err) = process_bitcoin_deposits(&state).await {
            tracing::warn!("bitcoin watcher error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

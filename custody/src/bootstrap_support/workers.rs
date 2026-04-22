use super::*;

pub(crate) fn spawn_background_workers(state: &CustodyState) {
    {
        let dispatcher_state = state.clone();
        let mut dispatcher_rx = state.event_tx.subscribe();
        tokio::spawn(async move {
            webhook_dispatcher_loop(dispatcher_state, &mut dispatcher_rx).await;
        });
    }

    if let Some(url) = state.config.solana_rpc_url.clone() {
        let watcher_state = state.clone();
        tokio::spawn(async move {
            solana_watcher_loop(watcher_state, url).await;
        });
    }

    // Per-chain EVM watchers: spawn separate watchers for ETH and BNB
    // so each chain polls its own RPC endpoint
    if let Some(url) = state
        .config
        .eth_rpc_url
        .clone()
        .or_else(|| state.config.evm_rpc_url.clone())
    {
        let watcher_state = state.clone();
        tokio::spawn(async move {
            evm_watcher_loop_for_chains(watcher_state, url, &["ethereum", "eth"]).await;
        });
    }
    if let Some(url) = state.config.bnb_rpc_url.clone() {
        let watcher_state = state.clone();
        tokio::spawn(async move {
            evm_watcher_loop_for_chains(watcher_state, url, &["bsc", "bnb"]).await;
        });
    } else if state.config.evm_rpc_url.is_some() && state.config.eth_rpc_url.is_none() {
        // Legacy fallback: single EVM watcher for all chains
        let url = state.config.evm_rpc_url.clone().unwrap();
        let watcher_state = state.clone();
        tokio::spawn(async move {
            evm_watcher_loop(watcher_state, url).await;
        });
    }

    let sweep_state = state.clone();
    tokio::spawn(async move {
        sweep_worker_loop(sweep_state).await;
    });

    let credit_state = state.clone();
    tokio::spawn(async move {
        credit_worker_loop(credit_state).await;
    });

    // Withdrawal: watches Lichen for burn events → sends native assets on source chain
    let withdrawal_state = state.clone();
    tokio::spawn(async move {
        withdrawal_worker_loop(withdrawal_state).await;
    });

    // Reserve rebalance: monitors USDT/USDC ratio and swaps to maintain balance
    let rebalance_state = state.clone();
    tokio::spawn(async move {
        rebalance_worker_loop(rebalance_state).await;
    });

    // Deposit cleanup: prunes expired unfunded deposit addresses
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        deposit_cleanup_loop(cleanup_state).await;
    });
}

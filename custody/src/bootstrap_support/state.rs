use super::*;

pub(crate) async fn build_custody_state(mut config: CustodyConfig) -> CustodyState {
    let db = open_db(&config.db_path).expect("open custody db");

    // AUDIT-FIX M4: On startup, check for stale TX intents from a previous crash
    recover_stale_intents(&db);
    // Backfill secondary event indexes for pre-index data.
    // Safe to run on every boot; missing keys are inserted idempotently.
    if let Err(error) = backfill_audit_event_indexes(&db) {
        tracing::warn!("audit event index backfill failed: {}", error);
    }

    let withdrawal_rate_state = load_withdrawal_rate_state(&db).unwrap_or_else(|error| {
        warn!(
            "failed to restore withdrawal rate limiter state; starting with empty window: {}",
            error
        );
        WithdrawalRateState::new()
    });
    let deposit_rate_state = load_deposit_rate_state(&db).unwrap_or_else(|error| {
        warn!(
            "failed to restore deposit rate limiter state; starting with empty window: {}",
            error
        );
        DepositRateState::new()
    });

    // Webhook/WebSocket event broadcast channel (1024-event buffer)
    let (event_tx, _event_rx) = broadcast::channel::<CustodyWebhookEvent>(1024);

    // Bound concurrent webhook deliveries to avoid runaway task fan-out under bursty events.
    let webhook_max_inflight = std::env::var("CUSTODY_WEBHOOK_MAX_INFLIGHT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(64)
        .min(1024);

    CustodyState {
        db: Arc::new(db),
        next_index_lock: Arc::new(Mutex::new(())),
        bridge_auth_replay_lock: Arc::new(Mutex::new(())),
        // Auto-discover contract addresses from Lichen before creating state.
        // This ensures all workers see the correct contract addresses from genesis.
        config: {
            let discovery_http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("http client for discovery");
            autodiscover_contract_addresses(&mut config, &discovery_http).await;
            config.clone()
        },
        // AUDIT-FIX 1.19: HTTP client with timeouts to prevent hung RPC freezing custody
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client"),
        // AUDIT-FIX 1.20: Withdrawal rate limiter
        withdrawal_rate: Arc::new(Mutex::new(withdrawal_rate_state)),
        // AUDIT-FIX W-H4: Deposit rate limiter
        deposit_rate: Arc::new(Mutex::new(deposit_rate_state)),
        event_tx: event_tx.clone(),
        webhook_delivery_limiter: Arc::new(Semaphore::new(webhook_max_inflight)),
    }
}

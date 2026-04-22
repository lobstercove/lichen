pub(super) const CF_DEPOSITS: &str = "deposits";
pub(super) const CF_INDEXES: &str = "indexes";
pub(super) const CF_ADDRESS_INDEX: &str = "address_index";
pub(super) const CF_DEPOSIT_EVENTS: &str = "deposit_events";
pub(super) const CF_SWEEP_JOBS: &str = "sweep_jobs";
pub(super) const CF_ADDRESS_BALANCES: &str = "address_balances";
pub(super) const CF_TOKEN_BALANCES: &str = "token_balances";
pub(super) const CF_CREDIT_JOBS: &str = "credit_jobs";
pub(super) const CF_WITHDRAWAL_JOBS: &str = "withdrawal_jobs";
pub(super) const CF_AUDIT_EVENTS: &str = "audit_events";
pub(super) const CF_AUDIT_EVENTS_BY_TIME: &str = "audit_events_by_time";
pub(super) const CF_AUDIT_EVENTS_BY_TYPE_TIME: &str = "audit_events_by_type_time";
pub(super) const CF_AUDIT_EVENTS_BY_ENTITY_TIME: &str = "audit_events_by_entity_time";
pub(super) const CF_AUDIT_EVENTS_BY_TX_TIME: &str = "audit_events_by_tx_time";
pub(super) const CF_CURSORS: &str = "cursors";
pub(super) const CF_RESERVE_LEDGER: &str = "reserve_ledger";
pub(super) const CF_REBALANCE_JOBS: &str = "rebalance_jobs";
pub(super) const CF_BRIDGE_AUTH_REPLAY: &str = "bridge_auth_replay";
/// AUDIT-FIX M1: Secondary status index for O(active) queries.
/// Keys: "status:{table}:{status}:{job_id}" -> empty value.
/// Full-table scans replaced with prefix iteration on this CF.
pub(super) const CF_STATUS_INDEX: &str = "status_index";
/// AUDIT-FIX M4: Write-ahead intent log for crash idempotency.
/// Before broadcasting any on-chain TX, record the intent here.
/// On startup, stale intents are reconciled against chain state.
/// Keys: "intent:{type}:{job_id}" -> JSON {chain, tx_type, created_at}
pub(super) const CF_TX_INTENTS: &str = "tx_intents";
/// Webhook registrations - stores registered webhook endpoints.
/// Keys: webhook_id -> JSON WebhookRegistration
pub(super) const CF_WEBHOOKS: &str = "webhooks";

/// Lichen contract runtime program address (all 0xFF bytes)
pub(super) const LICN_CONTRACT_PROGRAM: [u8; 32] = [0xFF; 32];

pub(super) const SOLANA_SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
pub(super) const SOLANA_TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub(super) const SOLANA_ASSOCIATED_TOKEN_PROGRAM: &str =
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
pub(super) const SOLANA_RENT_SYSVAR: &str = "SysvarRent111111111111111111111111111111111";
pub(super) const SOLANA_SWEEP_FEE_LAMPORTS: u64 = 5_000;
pub(super) const DEPOSIT_SEED_SOURCE_TREASURY_ROOT: &str = "treasury_root";
pub(super) const DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT: &str = "deposit_root";

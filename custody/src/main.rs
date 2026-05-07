use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::delete,
    routing::get,
    routing::post,
    routing::put,
    Json, Router,
};
use base64::Engine;
use ed25519_dalek::Signer;
use lichen_core::{
    Hash, Instruction, Keypair, Message, PqSignature, Pubkey, Transaction, SYSTEM_PROGRAM_ID,
};
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, Options, SliceTransform, WriteBatch, DB,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex, Semaphore};
use tokio::time::{sleep, Duration};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{info, warn};
use uuid::Uuid;

mod asset_support;
mod audit_support;
mod auth_replay_support;
mod balance_cache_support;
mod bootstrap_support;
mod chain_config;
mod chain_confirmation_support;
mod credit_execution_support;
mod custody_constants;
mod deposit_api_support;
mod deposit_cleanup_support;
mod deposit_derivation;
mod deposit_event_support;
mod deposit_monitor_support;
mod deposit_persistence;
mod event_api_support;
mod evm_support;
mod hex_support;
mod instruction_support;
mod job_models;
mod job_persistence_support;
mod lichen_rpc_support;
mod pending_burn_support;
mod pending_deposit_support;
mod rate_limits;
mod rebalance_execution_support;
mod rebalance_output_support;
mod rebalance_threshold_support;
mod request_auth_support;
mod request_models;
mod reserve_ledger_support;
mod retry_support;
mod runtime_types;
mod security_guards;
mod seed_support;
mod service_api_support;
mod signer_support;
mod solana_support;
mod storage_support;
mod sweep_execution_support;
#[cfg(test)]
mod test_support;
mod webhook_support;
mod withdrawal_api_support;
mod withdrawal_authorization_support;
mod withdrawal_broadcast_support;
mod withdrawal_controls;
mod withdrawal_request_support;
mod withdrawal_settlement_support;
mod withdrawal_signing_support;

use asset_support::{
    derive_associated_token_address, derive_associated_token_address_from_str,
    ensure_associated_token_account, ensure_associated_token_account_for_str, ensure_solana_config,
    evm_contract_for_asset, is_solana_stablecoin, load_solana_keypair, resolve_token_contract,
    solana_mint_for_asset, source_chain_decimals, spores_to_chain_amount,
};
use audit_support::{
    emit_custody_event, emit_withdrawal_spike_event, emit_withdrawal_velocity_warning_event,
    next_withdrawal_warning_level, record_audit_event,
};
use auth_replay_support::{
    find_existing_bridge_auth_replay, find_existing_withdrawal_auth_replay,
    persist_new_deposit_with_bridge_auth_replay, persist_new_withdrawal_with_auth_replay,
    prune_expired_bridge_auth_replays,
};
use balance_cache_support::{
    get_last_balance, get_last_balance_with_key, set_last_balance, set_last_balance_with_key,
};
use bootstrap_support::{
    build_custody_app, build_custody_state, custody_listen_addr, prepare_custody_config,
    spawn_background_workers,
};
use chain_config::{
    autodiscover_contract_addresses, derive_treasury_addresses_from_seed, load_config,
    rpc_url_for_chain, treasury_for_chain,
};
use chain_confirmation_support::{check_evm_tx_confirmed, check_solana_tx_confirmed};
use credit_execution_support::{
    build_credit_job, count_credit_jobs, credit_worker_loop, store_credit_job,
};
#[cfg(test)]
use credit_execution_support::{list_credit_jobs_by_status, process_credit_jobs};
use custody_constants::*;
#[cfg(test)]
use deposit_api_support::CreateDepositRequest;
use deposit_api_support::{create_deposit, get_deposit, CreateDepositResponse};
use deposit_cleanup_support::deposit_cleanup_loop;
#[cfg(test)]
use deposit_derivation::bip44_coin_type;
use deposit_derivation::{
    active_deposit_seed_source, bip44_derivation_path, default_deposit_seed_source,
    deposit_seed_for_record, deposit_seed_for_source, derive_deposit_address,
    derive_solana_owner_pubkey, get_last_u64_index, get_or_allocate_derivation_account,
    is_evm_chain, next_deposit_index, set_last_u64_index,
};
use deposit_event_support::{
    deposit_event_already_processed, store_deposit_event, update_deposit_status,
};
use deposit_monitor_support::{evm_watcher_loop, evm_watcher_loop_for_chains, solana_watcher_loop};
use deposit_persistence::{fetch_deposit, store_deposit};
use event_api_support::{list_events, ws_events};
#[cfg(test)]
use evm_support::to_be_bytes;
use evm_support::{
    build_evm_signed_transaction, build_evm_signed_transaction_with_data, derive_evm_address,
    derive_evm_signing_key, evm_encode_erc20_transfer, evm_estimate_gas, evm_get_balance,
    evm_get_block_number, evm_get_chain_id, evm_get_gas_price, evm_get_transaction_count,
    evm_get_transaction_receipt, evm_get_transfer_logs, evm_rpc_call, parse_evm_address,
};
use hex_support::{parse_hex_u128, parse_hex_u64};
use job_models::{
    CreditJob, RebalanceJob, ReserveLedgerEntry, SignerSignature, SignerSignatureKind, SweepJob,
    WithdrawalJob,
};
use job_persistence_support::{
    count_sweep_jobs, count_withdrawal_jobs, enqueue_sweep_job, fetch_withdrawal_job,
    list_rebalance_jobs_by_status, list_sweep_jobs_by_status, list_withdrawal_jobs_by_status,
    store_rebalance_job, store_sweep_job, store_withdrawal_job,
};
use lichen_rpc_support::{licn_get_recent_blockhash, licn_rpc_call, licn_send_transaction};
#[cfg(test)]
use pending_burn_support::burn_signature_index_key;
use pending_burn_support::submit_pending_burn_signature;
use pending_deposit_support::list_pending_deposits_for_chains;
use rate_limits::{
    load_deposit_rate_state, load_withdrawal_rate_state, persist_deposit_rate_state,
    persist_withdrawal_rate_state, DepositRateState, WithdrawalRateState,
    WithdrawalVelocityMetrics, WithdrawalWarningLevel,
};
#[cfg(test)]
use rebalance_execution_support::process_rebalance_jobs;
use rebalance_execution_support::rebalance_worker_loop;
use rebalance_output_support::decode_transfer_log;
#[cfg(test)]
use request_auth_support::bridge_access_message;
use request_auth_support::{
    bridge_access_replay_digest, current_unix_secs, parse_bridge_access_auth_json,
    parse_bridge_access_auth_value, parse_bridge_access_signature, verify_api_auth,
    verify_bridge_access_auth, verify_bridge_access_auth_at,
};
use request_models::{
    BridgeAccessAuth, BridgeAuthReplayRecord, DepositEvent, DepositRequest, WithdrawalAccessAuth,
    WithdrawalAuthReplayRecord, WithdrawalRequest, BRIDGE_ACCESS_CLOCK_SKEW_SECS,
    BRIDGE_ACCESS_DOMAIN, BRIDGE_ACCESS_MAX_TTL_SECS, BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT,
    BRIDGE_AUTH_REPLAY_ACTION_CREATE_WITHDRAWAL, BRIDGE_AUTH_REPLAY_PRUNE_BATCH,
    WITHDRAWAL_ACCESS_CLOCK_SKEW_SECS, WITHDRAWAL_ACCESS_DOMAIN, WITHDRAWAL_ACCESS_MAX_TTL_SECS,
};
use reserve_ledger_support::{
    adjust_reserve_balance, adjust_reserve_balance_once, build_reserve_ledger_response,
    get_reserve_balance,
};
use retry_support::{
    is_ready_for_retry, mark_sweep_failed, next_retry_timestamp, MAX_JOB_ATTEMPTS,
};
use runtime_types::{
    CreateWebhookRequest, CustodyConfig, CustodyState, CustodyWebhookEvent, ErrorResponse,
    HealthResponse, StatusCounts, WebhookRegistration,
};
#[cfg(test)]
use security_guards::validate_custody_security_configuration_with_mode;
use security_guards::{
    default_signer_threshold, ensure_credit_restrictions_allow, ensure_deposit_creation_allowed,
    ensure_deposit_restrictions_allow, ensure_withdrawal_restrictions_allow,
    local_rebalance_policy_error, local_sweep_policy_error,
    validate_custody_security_configuration, validate_pq_signer_configuration,
    withdrawal_incident_block_reason,
};
use seed_support::{load_optional_seed_secret, load_required_seed_secret};
use service_api_support::{get_reserves, health, status};
use signer_support::promote_locally_signed_sweep_jobs;
use signer_support::{SignerRequest, SignerResponse};
use solana_support::{
    build_solana_message_with_instructions, build_solana_transaction,
    build_solana_transfer_message, decode_shortvec_u16, decode_solana_pubkey,
    derive_solana_address, derive_solana_keypair, derive_solana_signer, encode_solana_pubkey,
    find_program_address, solana_get_account_exists, solana_get_balance,
    solana_get_latest_blockhash, solana_get_signature_confirmed, solana_get_signature_status,
    solana_get_signatures_for_address, solana_get_token_balance, solana_rpc_call,
    solana_send_transaction, SimpleSolanaKeypair, SolanaInstruction, SolanaMessageHeader,
};
use storage_support::{
    backfill_audit_event_indexes, clear_tx_intent, list_ids_by_status_index, open_db,
    record_tx_intent, recover_stale_intents, set_status_index, update_status_index,
};
#[cfg(test)]
use sweep_execution_support::process_sweep_jobs;
use sweep_execution_support::sweep_worker_loop;
#[cfg(test)]
use webhook_support::{compute_webhook_signature, validate_webhook_destination};
use webhook_support::{create_webhook, delete_webhook, list_webhooks, webhook_dispatcher_loop};
use withdrawal_api_support::{
    confirm_withdrawal_operator, create_withdrawal, submit_burn_signature,
};
#[cfg(test)]
use withdrawal_api_support::{BurnSignaturePayload, WithdrawalOperatorConfirmationPayload};
#[cfg(test)]
use withdrawal_authorization_support::{
    build_evm_safe_exec_transaction_calldata, build_threshold_solana_withdrawal_message,
    evm_function_selector,
};
use withdrawal_authorization_support::{
    build_evm_safe_transaction_plan, build_solana_token_transfer_message,
    collect_pq_withdrawal_approvals, collect_threshold_evm_withdrawal_signatures,
    collect_threshold_solana_withdrawal_signatures, determine_withdrawal_signing_mode,
    evm_executor_derivation_path, finalize_evm_safe_exec_plan, mark_withdrawal_failed,
    normalize_evm_signature, valid_pq_withdrawal_approvers, WithdrawalSigningMode,
};
#[cfg(test)]
use withdrawal_broadcast_support::assemble_signed_evm_tx;
use withdrawal_broadcast_support::broadcast_outbound_withdrawal;
use withdrawal_controls::{
    build_withdrawal_velocity_snapshot, clear_withdrawal_hold, effective_required_signer_threshold,
    evaluate_withdrawal_velocity_gate, load_withdrawal_velocity_policy,
    process_withdrawal_operator_confirmation, update_withdrawal_hold, velocity_delay_secs,
    verify_operator_confirmation_auth, WithdrawalOperatorConfirmation, WithdrawalVelocityGate,
    WithdrawalVelocityPolicy, WithdrawalVelocitySnapshot, WithdrawalVelocityTier,
};
#[cfg(test)]
use withdrawal_controls::{
    default_withdrawal_daily_caps, default_withdrawal_elevated_thresholds,
    default_withdrawal_extraordinary_thresholds, default_withdrawal_tx_caps, next_utc_day_start,
    operator_token_fingerprint,
};
#[cfg(test)]
use withdrawal_request_support::withdrawal_access_message;
use withdrawal_request_support::{
    build_create_withdrawal_response, complete_withdrawal_request, default_preferred_stablecoin,
    enforce_withdrawal_rate_limits, handle_withdrawal_auth_replay,
    prepare_create_withdrawal_request, resolve_withdrawal_preferred_stablecoin,
    validate_withdrawal_request_destination,
};
#[cfg(test)]
use withdrawal_settlement_support::process_withdrawal_jobs;
use withdrawal_settlement_support::withdrawal_worker_loop;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = prepare_custody_config();
    let state = build_custody_state(config).await;
    spawn_background_workers(&state);
    let app = build_custody_app(state);
    let addr = custody_listen_addr();
    info!("custody service listening on {}", addr);

    axum::serve(
        tokio::net::TcpListener::bind(addr).await.expect("bind"),
        app,
    )
    .await
    .expect("serve");
}

// ============================================================================
// WITHDRAWAL — Burn wrapped tokens on Lichen, send native assets to user
// ============================================================================

// ============================================================================
// RESERVE LEDGER — Track stablecoin reserves per chain+asset
// ============================================================================

// ============================================================================
// REBALANCE — Swap USDT↔USDC on external DEXes to maintain reserve balance
// ============================================================================

// ══════════════════════════════════════════════════════════════════════════════
// Webhook & WebSocket Event System
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests;

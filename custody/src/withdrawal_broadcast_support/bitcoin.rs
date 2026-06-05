use super::*;
use crate::bitcoin_support::BitcoinPaymentRequest;

pub(super) async fn broadcast_self_custody_bitcoin_withdrawal(
    state: &CustodyState,
    job: &WithdrawalJob,
) -> Result<String, String> {
    validate_bitcoin_address_for_network(&job.dest_address, &state.config.btc_network)?;
    let treasury = state
        .config
        .treasury_btc_address
        .clone()
        .or_else(|| derive_bitcoin_treasury_address(&state.config).ok())
        .ok_or_else(|| "missing CUSTODY_TREASURY_BTC".to_string())?;
    validate_bitcoin_address_for_network(&treasury, &state.config.btc_network)?;
    let amount_sats = u64::try_from(spores_to_chain_amount(job.amount, &job.dest_chain, "btc")?)
        .map_err(|_| "bitcoin withdrawal amount overflow".to_string())?;
    let utxos = bitcoin_scan_confirmed_utxos(
        &state.http,
        &state.config,
        &treasury,
        state.config.btc_confirmations,
    )
    .await?;
    let (tx_hex, _sent_sats, _fee_sats) = build_bitcoin_payment_tx_hex(BitcoinPaymentRequest {
        derivation_path: bitcoin_treasury_derivation_path(),
        master_seed: &state.config.master_seed,
        available_utxos: &utxos,
        dest_address: &job.dest_address,
        change_address: &treasury,
        amount_sats,
        network: &state.config.btc_network,
        fee_rate_sats_vb: state.config.btc_fee_rate_sats_vb,
    })?;
    bitcoin_send_raw_transaction(&state.http, &state.config, &tx_hex).await
}

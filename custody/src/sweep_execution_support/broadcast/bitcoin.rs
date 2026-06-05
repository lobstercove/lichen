use super::*;

pub(super) async fn broadcast_bitcoin_sweep(
    state: &CustodyState,
    job: &mut SweepJob,
) -> Result<Option<String>, String> {
    let deposit = fetch_deposit(&state.db, &job.deposit_id)?
        .ok_or_else(|| format!("deposit not found for sweep {}", job.job_id))?;
    validate_bitcoin_address_for_network(&deposit.address, &state.config.btc_network)?;
    validate_bitcoin_address_for_network(&job.to_treasury, &state.config.btc_network)?;
    let deposit_seed = deposit_seed_for_record(&state.config, &deposit);
    let utxos = bitcoin_scan_confirmed_utxos(
        &state.http,
        &state.config,
        &deposit.address,
        state.config.btc_confirmations,
    )
    .await?;
    if utxos.is_empty() {
        return Ok(None);
    }
    let (tx_hex, credited_sats) = build_bitcoin_sweep_tx_hex(
        &deposit.derivation_path,
        deposit_seed,
        &utxos,
        &job.to_treasury,
        &state.config.btc_network,
        state.config.btc_fee_rate_sats_vb,
    )?;
    let txid = bitcoin_send_raw_transaction(&state.http, &state.config, &tx_hex).await?;
    job.credited_amount = Some(credited_sats.to_string());
    Ok(Some(txid))
}

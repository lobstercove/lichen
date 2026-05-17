use super::*;

pub(super) async fn broadcast_self_custody_solana_withdrawal(
    state: &CustodyState,
    url: &str,
    job: &WithdrawalJob,
    outbound_asset: &str,
) -> Result<String, String> {
    let treasury_path = "custody/treasury/solana";
    let (signing_key, from_pubkey) =
        derive_solana_signer(treasury_path, &state.config.master_seed)?;

    if outbound_asset == "sol" {
        let to_pubkey = decode_solana_pubkey(&job.dest_address)?;

        let solana_tx_fee: u64 = 5_000;
        if job.amount <= solana_tx_fee {
            return Err("withdrawal amount too small to cover fees".to_string());
        }
        let transfer_amount = job.amount - solana_tx_fee;

        let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
        let message = build_solana_transfer_message(
            &from_pubkey,
            &to_pubkey,
            transfer_amount,
            &recent_blockhash,
        );
        let signature = signing_key.sign(&message).to_bytes();
        let tx = build_solana_transaction(&[signature], &message);
        return solana_send_transaction(&state.http, url, &tx).await;
    }

    if !is_solana_stablecoin(outbound_asset) {
        return Err(format!(
            "unsupported self-custody Solana withdrawal asset: {}",
            outbound_asset
        ));
    }

    let treasury_owner = encode_solana_pubkey(&from_pubkey);
    let mint = solana_mint_for_asset(&state.config, outbound_asset)?;
    let from_token_account = derive_associated_token_address_from_str(&treasury_owner, &mint)?;
    let to_token_account = derive_associated_token_address_from_str(&job.dest_address, &mint)?;
    ensure_associated_token_account_for_str(state, &treasury_owner, &mint, &from_token_account)
        .await?;
    ensure_associated_token_account_for_str(state, &job.dest_address, &mint, &to_token_account)
        .await?;

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let raw_amount = u64::try_from(spores_to_chain_amount(
        job.amount,
        &job.dest_chain,
        outbound_asset,
    )?)
    .map_err(|_| "solana token withdrawal amount overflow".to_string())?;
    let message = build_solana_token_transfer_message(
        &from_pubkey,
        &decode_solana_pubkey(&from_token_account)?,
        &decode_solana_pubkey(&to_token_account)?,
        raw_amount,
        &recent_blockhash,
    )?;
    let signature = signing_key.sign(&message).to_bytes();
    let tx = build_solana_transaction(&[signature], &message);
    solana_send_transaction(&state.http, url, &tx).await
}

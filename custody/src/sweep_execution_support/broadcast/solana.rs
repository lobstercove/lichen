use super::*;

pub(super) async fn broadcast_solana_sweep(
    state: &CustodyState,
    url: &str,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    if is_solana_stablecoin(&job.asset) {
        return broadcast_solana_token_sweep(state, url, job).await;
    }

    let amount = match job.amount.as_ref() {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };
    if amount == 0 {
        return Ok(None);
    }

    let deposit = fetch_deposit(&state.db, &job.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };
    let deposit_seed = deposit_seed_for_record(&state.config, &deposit);

    // AUDIT-FIX C1: Deduct the Solana transaction fee from the sweep amount.
    // The deposit address is the fee payer, so it needs: transfer_amount + fee.
    // Without this, the tx would fail because the account lacks fee funds.
    if amount <= SOLANA_SWEEP_FEE_LAMPORTS {
        // Dust amount — not worth sweeping (would go entirely to fees)
        return Ok(None);
    }
    let transfer_amount = amount - SOLANA_SWEEP_FEE_LAMPORTS;

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let (signing_key, from_pubkey) = derive_solana_signer(&deposit.derivation_path, deposit_seed)?;
    let to_pubkey = decode_solana_pubkey(&job.to_treasury)?;

    let message =
        build_solana_transfer_message(&from_pubkey, &to_pubkey, transfer_amount, &recent_blockhash);
    let signature = signing_key.sign(&message).to_bytes();
    let tx = build_solana_transaction(&[signature], &message);
    let signature = solana_send_transaction(&state.http, url, &tx).await?;
    Ok(Some(signature))
}

async fn broadcast_solana_token_sweep(
    state: &CustodyState,
    url: &str,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    let amount = match job.amount.as_ref() {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };
    if amount == 0 {
        return Ok(None);
    }

    let deposit = fetch_deposit(&state.db, &job.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    let fee_payer = if let Some(ref fee_payer_path) = state.config.solana_fee_payer_keypair_path {
        load_solana_keypair(fee_payer_path)?
    } else {
        derive_solana_keypair("custody/fee-payer/solana", &state.config.master_seed)?
    };

    let owner_keypair = derive_solana_keypair(
        &deposit.derivation_path,
        deposit_seed_for_record(&state.config, &deposit),
    )?;

    let from_account = decode_solana_pubkey(&job.from_address)?;
    let to_account = decode_solana_pubkey(&job.to_treasury)?;
    let token_program = decode_solana_pubkey(SOLANA_TOKEN_PROGRAM)?;

    let account_keys = vec![
        fee_payer.pubkey,
        owner_keypair.pubkey,
        from_account,
        to_account,
        token_program,
    ];

    let header = SolanaMessageHeader {
        num_required_signatures: 2,
        num_readonly_signed: 1,
        num_readonly_unsigned: 1,
    };

    let mut data = Vec::with_capacity(9);
    data.push(3u8);
    data.extend_from_slice(&amount.to_le_bytes());

    let instruction = SolanaInstruction {
        program_id_index: 4,
        account_indices: vec![2, 3, 1],
        data,
    };

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let message = build_solana_message_with_instructions(
        header,
        &account_keys,
        &recent_blockhash,
        &[instruction],
    );
    let fee_sig = fee_payer.sign(&message);
    let owner_sig = owner_keypair.sign(&message);
    let tx = build_solana_transaction(&[fee_sig, owner_sig], &message);

    let signature = solana_send_transaction(&state.http, url, &tx).await?;
    Ok(Some(signature))
}

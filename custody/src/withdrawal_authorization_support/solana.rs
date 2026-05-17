use super::pq::collect_pq_withdrawal_approvals;
use super::*;

fn solana_treasury_owner_address(config: &CustodyConfig) -> Result<String, String> {
    config
        .solana_treasury_owner
        .clone()
        .or_else(|| config.treasury_solana_address.clone())
        .ok_or_else(|| {
            "missing Solana treasury owner (set CUSTODY_SOLANA_TREASURY_OWNER or CUSTODY_TREASURY_SOLANA_ADDRESS)"
                .to_string()
        })
}

pub(crate) fn resolve_solana_token_withdrawal_accounts(
    config: &CustodyConfig,
    asset: &str,
    dest_owner: &str,
) -> Result<(String, String, String, String), String> {
    let treasury_owner = solana_treasury_owner_address(config)?;
    let mint = solana_mint_for_asset(config, asset)?;
    let from_token_account = derive_associated_token_address_from_str(&treasury_owner, &mint)?;
    let to_token_account = derive_associated_token_address_from_str(dest_owner, &mint)?;
    Ok((treasury_owner, mint, from_token_account, to_token_account))
}

pub(crate) fn build_solana_token_transfer_message(
    authority_pubkey: &[u8; 32],
    from_token_account: &[u8; 32],
    to_token_account: &[u8; 32],
    raw_amount: u64,
    recent_blockhash: &[u8; 32],
) -> Result<Vec<u8>, String> {
    let token_program = decode_solana_pubkey(SOLANA_TOKEN_PROGRAM)?;
    let account_keys = vec![
        *authority_pubkey,
        *from_token_account,
        *to_token_account,
        token_program,
    ];

    let header = SolanaMessageHeader {
        num_required_signatures: 1,
        num_readonly_signed: 0,
        num_readonly_unsigned: 1,
    };

    let mut data = Vec::with_capacity(9);
    data.push(3u8);
    data.extend_from_slice(&raw_amount.to_le_bytes());

    let instruction = SolanaInstruction {
        program_id_index: 3,
        account_indices: vec![1, 2, 0],
        data,
    };

    Ok(build_solana_message_with_instructions(
        header,
        &account_keys,
        recent_blockhash,
        &[instruction],
    ))
}

#[cfg(test)]
pub(crate) fn build_threshold_solana_withdrawal_message(
    state: &CustodyState,
    job: &WithdrawalJob,
    outbound_asset: &str,
    recent_blockhash: &[u8; 32],
) -> Result<Vec<u8>, String> {
    if outbound_asset == "sol" {
        let solana_tx_fee: u64 = 5_000;
        if job.amount <= solana_tx_fee {
            return Err("withdrawal amount too small to cover fees".to_string());
        }

        let treasury_address = state
            .config
            .treasury_solana_address
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_TREASURY_SOLANA_ADDRESS".to_string())?;
        let from_pubkey = decode_solana_pubkey(treasury_address)?;
        let to_pubkey = decode_solana_pubkey(&job.dest_address)?;
        let transfer_amount = job.amount - solana_tx_fee;

        return Ok(build_solana_transfer_message(
            &from_pubkey,
            &to_pubkey,
            transfer_amount,
            recent_blockhash,
        ));
    }

    if !is_solana_stablecoin(outbound_asset) {
        return Err(format!(
            "unsupported threshold Solana withdrawal asset: {}",
            outbound_asset
        ));
    }

    let (treasury_owner, _, from_token_account, to_token_account) =
        resolve_solana_token_withdrawal_accounts(&state.config, outbound_asset, &job.dest_address)?;
    let authority_pubkey = decode_solana_pubkey(&treasury_owner)?;
    let from_token_pubkey = decode_solana_pubkey(&from_token_account)?;
    let to_token_pubkey = decode_solana_pubkey(&to_token_account)?;
    let raw_amount = u64::try_from(spores_to_chain_amount(
        job.amount,
        &job.dest_chain,
        outbound_asset,
    )?)
    .map_err(|_| "solana token withdrawal amount overflow".to_string())?;

    build_solana_token_transfer_message(
        &authority_pubkey,
        &from_token_pubkey,
        &to_token_pubkey,
        raw_amount,
        recent_blockhash,
    )
}

pub(crate) async fn collect_threshold_solana_withdrawal_signatures(
    state: &CustodyState,
    job: &mut WithdrawalJob,
    outbound_asset: &str,
    required_threshold: usize,
) -> Result<usize, String> {
    if is_solana_stablecoin(outbound_asset) {
        let (treasury_owner, mint, from_token_account, to_token_account) =
            resolve_solana_token_withdrawal_accounts(
                &state.config,
                outbound_asset,
                &job.dest_address,
            )?;
        ensure_associated_token_account_for_str(state, &treasury_owner, &mint, &from_token_account)
            .await?;
        ensure_associated_token_account_for_str(state, &job.dest_address, &mint, &to_token_account)
            .await?;
    }
    collect_pq_withdrawal_approvals(state, job, outbound_asset, required_threshold).await
}

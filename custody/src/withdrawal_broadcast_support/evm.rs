use super::*;

async fn checked_evm_chain_id(
    state: &CustodyState,
    url: &str,
    dest_chain: &str,
) -> Result<u64, String> {
    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let expected = evm_route_for_chain(&state.config, dest_chain)
        .map(|route| route.chain_id)
        .ok_or_else(|| format!("unsupported EVM destination chain: {}", dest_chain))?;
    if chain_id != expected {
        return Err(format!(
            "RPC chain ID mismatch for {}: expected {}, got {}",
            dest_chain, expected, chain_id
        ));
    }
    Ok(chain_id)
}

pub(super) async fn broadcast_self_custody_evm_withdrawal(
    state: &CustodyState,
    url: &str,
    job: &WithdrawalJob,
    outbound_asset: &str,
) -> Result<String, String> {
    let treasury_chain = evm_treasury_derivation_path(&job.dest_chain)
        .ok_or_else(|| format!("unsupported EVM destination chain: {}", job.dest_chain))?;
    let signing_key = derive_evm_signing_key(treasury_chain, &state.config.master_seed)?;
    let from_address = derive_evm_address(treasury_chain, &state.config.master_seed)?;
    let to_address = &job.dest_address;

    let nonce = evm_get_transaction_count(&state.http, url, &from_address).await?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let chain_id = checked_evm_chain_id(state, url, &job.dest_chain).await?;

    if outbound_asset == "eth" || outbound_asset == "bnb" || outbound_asset == "gas" {
        let chain_amount = spores_to_chain_amount(job.amount, &job.dest_chain, outbound_asset)?;
        let gas_limit = evm_estimate_gas(
            &state.http,
            url,
            &from_address,
            to_address,
            chain_amount,
            None,
            21_000,
        )
        .await;

        let raw_tx = build_evm_signed_transaction(
            &signing_key,
            nonce,
            gas_price,
            gas_limit,
            to_address,
            chain_amount,
            chain_id,
        )?;
        let tx_hex = format!("0x{}", hex::encode(raw_tx));
        let result =
            evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
        result
            .as_str()
            .map(|value| value.to_string())
            .ok_or_else(|| "no tx hash returned".to_string())
    } else {
        let contract = evm_contract_for_asset(&state.config, outbound_asset)?;
        let chain_amount = spores_to_chain_amount(job.amount, &job.dest_chain, outbound_asset)?;
        let transfer_data = evm_encode_erc20_transfer(to_address, chain_amount)?;
        let gas_limit = evm_estimate_gas(
            &state.http,
            url,
            &from_address,
            &contract,
            0,
            Some(&transfer_data),
            100_000,
        )
        .await;

        let raw_tx = build_evm_signed_transaction_with_data(
            &signing_key,
            nonce,
            gas_price,
            gas_limit,
            &contract,
            0,
            &transfer_data,
            chain_id,
        )?;
        let tx_hex = format!("0x{}", hex::encode(raw_tx));
        let result =
            evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
        result
            .as_str()
            .map(|value| value.to_string())
            .ok_or_else(|| "no tx hash returned".to_string())
    }
}

pub(super) async fn assemble_signed_evm_tx(
    state: &CustodyState,
    job: &WithdrawalJob,
    asset: &str,
) -> Result<Vec<u8>, String> {
    let required_signer_threshold = effective_required_signer_threshold(job, &state.config);
    if job.signatures.is_empty() {
        return Err("no signatures available".to_string());
    }

    if state.config.signer_threshold <= 1 || state.config.signer_endpoints.len() <= 1 {
        let first_sig = &job.signatures[0];
        if first_sig.kind != SignerSignatureKind::EvmEcdsa {
            return Err("expected isolated EVM ECDSA signature entry".to_string());
        }
        return hex::decode(&first_sig.signature)
            .map_err(|error| format!("decode signature: {}", error));
    }

    let mut signer_signatures: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen_signer_addrs = std::collections::HashSet::new();

    for sig_entry in &job.signatures {
        if sig_entry.kind != SignerSignatureKind::EvmEcdsa {
            return Err("EVM Safe path received a non-ECDSA signer entry".to_string());
        }
        let sig_bytes = normalize_evm_signature(
            &hex::decode(&sig_entry.signature)
                .map_err(|error| format!("decode EVM signature: {}", error))?,
        )?;

        let signer_addr = sig_entry
            .signer_pubkey
            .trim_start_matches("0x")
            .to_lowercase();
        if !seen_signer_addrs.insert(signer_addr.clone()) {
            return Err("duplicate EVM signer address in signature set".to_string());
        }
        signer_signatures.push((signer_addr, sig_bytes));
    }

    signer_signatures.sort_by(|left, right| left.0.cmp(&right.0));

    if signer_signatures.len() < required_signer_threshold {
        return Err(format!(
            "insufficient EVM signatures: have {}, need {}",
            signer_signatures.len(),
            required_signer_threshold
        ));
    }

    let packed_sigs: Vec<u8> = signer_signatures
        .iter()
        .take(required_signer_threshold)
        .flat_map(|(_, signature)| signature.clone())
        .collect();

    let url = rpc_url_for_chain(&state.config, &job.dest_chain)
        .ok_or_else(|| format!("missing RPC URL for chain {}", job.dest_chain))?;
    let plan = build_evm_safe_transaction_plan(state, &url, job, asset).await?;
    let expected_hash = hex::encode(plan.safe_tx_hash);
    if job
        .signatures
        .iter()
        .any(|sig| !sig.message_hash.is_empty() && sig.message_hash != expected_hash)
    {
        return Err(
            "EVM signature set does not match the pinned Safe transaction hash".to_string(),
        );
    }

    let exec_plan = finalize_evm_safe_exec_plan(plan, &packed_sigs)?;
    let executor_path = evm_executor_derivation_path(&job.dest_chain);
    let executor_address = derive_evm_address(executor_path, &state.config.master_seed)?;
    let executor_key = derive_evm_signing_key(executor_path, &state.config.master_seed)?;
    let nonce = evm_get_transaction_count(&state.http, &url, &executor_address).await?;
    let gas_price = evm_get_gas_price(&state.http, &url).await?;
    let chain_id = checked_evm_chain_id(state, &url, &job.dest_chain).await?;
    let gas_limit = evm_estimate_gas(
        &state.http,
        &url,
        &executor_address,
        &exec_plan.safe_address,
        0,
        Some(&exec_plan.exec_calldata),
        350_000,
    )
    .await;
    build_evm_signed_transaction_with_data(
        &executor_key,
        nonce,
        gas_price,
        gas_limit,
        &exec_plan.safe_address,
        0,
        &exec_plan.exec_calldata,
        chain_id,
    )
}

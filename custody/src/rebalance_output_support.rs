use super::*;

pub(super) fn decode_transfer_log(log: &Value) -> Option<(String, u128, String)> {
    let topics = log.get("topics")?.as_array()?;
    if topics.len() < 3 {
        return None;
    }
    let to_topic = topics.get(2)?.as_str()?;
    let to_trimmed = to_topic.trim_start_matches("0x");
    if to_trimmed.len() < 40 {
        return None;
    }
    let to = format!("0x{}", &to_trimmed[to_trimmed.len() - 40..]);

    let data = log.get("data")?.as_str()?;
    let amount = parse_hex_u128(data).ok()?;

    let tx_hash = log.get("transactionHash")?.as_str()?.to_string();
    Some((to, amount, tx_hash))
}

pub(super) async fn parse_solana_swap_output(
    client: &reqwest::Client,
    url: &str,
    signature: &str,
    treasury_addr: &str,
    to_mint: &str,
) -> Result<Option<u64>, String> {
    let params = json!([
        signature,
        { "encoding": "jsonParsed", "maxSupportedTransactionVersion": 0 }
    ]);
    let result = solana_rpc_call(client, url, "getTransaction", params).await?;
    if result.is_null() {
        return Ok(None);
    }

    let meta = match result.get("meta") {
        Some(meta) if !meta.is_null() => meta,
        _ => return Ok(None),
    };

    if !meta.get("err").is_none_or(|error| error.is_null()) {
        return Err("Solana swap transaction failed on-chain".to_string());
    }

    let pre_balances = meta
        .get("preTokenBalances")
        .and_then(|value| value.as_array());
    let post_balances = meta
        .get("postTokenBalances")
        .and_then(|value| value.as_array());

    let (pre_balances, post_balances) = match (pre_balances, post_balances) {
        (Some(pre), Some(post)) => (pre, post),
        _ => return Ok(None),
    };

    let extract_amount = |entries: &[Value]| -> Option<u64> {
        for entry in entries {
            let mint = entry
                .get("mint")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let owner = entry
                .get("owner")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if mint == to_mint && owner == treasury_addr {
                return entry
                    .get("uiTokenAmount")
                    .and_then(|value| value.get("amount"))
                    .and_then(|value| value.as_str())
                    .and_then(|value| value.parse::<u64>().ok());
            }
        }
        None
    };

    let pre_amount = extract_amount(pre_balances).unwrap_or(0);
    let post_amount = extract_amount(post_balances).unwrap_or(0);

    if post_amount > pre_amount {
        Ok(Some(post_amount - pre_amount))
    } else {
        Ok(None)
    }
}

pub(super) async fn parse_evm_swap_output(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
    treasury_addr: &str,
    to_token_contract: &str,
) -> Result<Option<u64>, String> {
    let receipt = evm_get_transaction_receipt(client, url, tx_hash).await?;
    let receipt = match receipt {
        Some(receipt) => receipt,
        None => return Ok(None),
    };

    let status = receipt
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("0x0");
    if status != "0x1" {
        return Err("EVM swap transaction reverted".to_string());
    }

    let logs = match receipt.get("logs").and_then(|value| value.as_array()) {
        Some(logs) => logs,
        None => return Ok(None),
    };

    let treasury_lower = treasury_addr.to_lowercase();
    let contract_lower = to_token_contract.to_lowercase();
    let transfer_topic = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

    let mut total_output: u128 = 0;

    for log in logs {
        let log_address = log
            .get("address")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if log_address.to_lowercase() != contract_lower {
            continue;
        }

        let topics = match log.get("topics").and_then(|value| value.as_array()) {
            Some(topics) if topics.len() >= 3 => topics,
            _ => continue,
        };

        let event_topic = topics[0].as_str().unwrap_or("");
        if event_topic != transfer_topic {
            continue;
        }

        let to_topic = topics[2].as_str().unwrap_or("").trim_start_matches("0x");
        if to_topic.len() < 40 {
            continue;
        }
        let to_addr = format!("0x{}", &to_topic[to_topic.len() - 40..]);
        if to_addr.to_lowercase() != treasury_lower {
            continue;
        }

        let data = log
            .get("data")
            .and_then(|value| value.as_str())
            .unwrap_or("0x0");
        if let Ok(amount) = parse_hex_u128(data) {
            total_output = total_output.saturating_add(amount);
        }
    }

    if total_output > 0 {
        if total_output > u64::MAX as u128 {
            return Err(format!(
                "Swap output {} exceeds u64::MAX — cannot safely represent",
                total_output
            ));
        }
        Ok(Some(total_output as u64))
    } else {
        Ok(None)
    }
}

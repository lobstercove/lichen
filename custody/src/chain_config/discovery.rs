use super::*;

/// Auto-discover wrapped token contract addresses from Lichen's symbol registry.
/// This eliminates the need to hardcode contract addresses — they are read from
/// whatever was deployed during genesis (or later). Falls back to env vars if RPC fails.
pub(crate) async fn autodiscover_contract_addresses(
    config: &mut CustodyConfig,
    http: &reqwest::Client,
) {
    let Some(rpc_url) = config.licn_rpc_url.as_ref() else {
        tracing::warn!("CUSTODY_LICHEN_RPC_URL not set — skipping contract auto-discovery");
        return;
    };

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getAllSymbolRegistry",
        "params": [],
    });

    let response = match http.post(rpc_url).json(&payload).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("contract auto-discovery RPC failed: {} — using env vars", e);
            return;
        }
    };

    let value: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "contract auto-discovery JSON parse failed: {} — using env vars",
                e
            );
            return;
        }
    };

    let Some(result) = value.get("result") else {
        tracing::warn!("contract auto-discovery: no result field — using env vars");
        return;
    };

    let entries = result
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if entries.is_empty() {
        tracing::warn!("contract auto-discovery: empty entries — using env vars");
        return;
    }

    let mut addr_by_symbol: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for entry in &entries {
        if let (Some(sym), Some(addr)) = (
            entry.get("symbol").and_then(|v| v.as_str()),
            entry
                .get("program")
                .or_else(|| entry.get("address"))
                .or_else(|| entry.get("program_id"))
                .and_then(|v| v.as_str()),
        ) {
            addr_by_symbol.insert(sym.to_string(), addr.to_string());
        }
    }

    info!(
        "contract auto-discovery: found {} entries in registry",
        addr_by_symbol.len()
    );

    let symbol_map: &[(&str, &str)] = &[
        ("LUSD", "musd"),
        ("WSOL", "wsol"),
        ("WETH", "weth"),
        ("WBNB", "wbnb"),
    ];

    for (symbol, field_name) in symbol_map {
        if let Some(addr) = addr_by_symbol.get(*symbol) {
            match *field_name {
                "musd" if config.musd_contract_addr.is_none() => {
                    info!("auto-discovered {} contract: {}", symbol, addr);
                    config.musd_contract_addr = Some(addr.clone());
                }
                "wsol" if config.wsol_contract_addr.is_none() => {
                    info!("auto-discovered {} contract: {}", symbol, addr);
                    config.wsol_contract_addr = Some(addr.clone());
                }
                "weth" if config.weth_contract_addr.is_none() => {
                    info!("auto-discovered {} contract: {}", symbol, addr);
                    config.weth_contract_addr = Some(addr.clone());
                }
                "wbnb" if config.wbnb_contract_addr.is_none() => {
                    info!("auto-discovered {} contract: {}", symbol, addr);
                    config.wbnb_contract_addr = Some(addr.clone());
                }
                _ => {}
            }
        }
    }

    let discovered = [
        ("LUSD", &config.musd_contract_addr),
        ("WSOL", &config.wsol_contract_addr),
        ("WETH", &config.weth_contract_addr),
        ("WBNB", &config.wbnb_contract_addr),
    ];
    for (name, addr) in &discovered {
        match addr {
            Some(a) => info!("  ✅ {} contract: {}", name, a),
            None => tracing::warn!("  ❌ {} contract: NOT CONFIGURED", name),
        }
    }
}

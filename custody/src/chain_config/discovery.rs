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

    apply_discovered_contract_addresses(config, &addr_by_symbol);

    let discovered = [
        ("LUSD", &config.musd_contract_addr),
        ("WSOL", &config.wsol_contract_addr),
        ("WETH", &config.weth_contract_addr),
        ("WBNB", &config.wbnb_contract_addr),
        ("WGAS", &config.wgas_contract_addr),
        ("WNEO", &config.wneo_contract_addr),
        ("WBTC", &config.wbtc_contract_addr),
    ];
    for (name, addr) in &discovered {
        match addr {
            Some(a) => info!("  ✅ {} contract: {}", name, a),
            None => tracing::warn!("  ❌ {} contract: NOT CONFIGURED", name),
        }
    }
}

fn apply_discovered_contract_addresses(
    config: &mut CustodyConfig,
    addr_by_symbol: &std::collections::HashMap<String, String>,
) {
    let symbol_map: &[(&str, &str)] = &[
        ("LUSD", "musd"),
        ("WSOL", "wsol"),
        ("WETH", "weth"),
        ("WBNB", "wbnb"),
        ("WGAS", "wgas"),
        ("WNEO", "wneo"),
        ("WBTC", "wbtc"),
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
                "wgas" if config.wgas_contract_addr.is_none() => {
                    info!("auto-discovered {} contract: {}", symbol, addr);
                    config.wgas_contract_addr = Some(addr.clone());
                }
                "wneo" if config.wneo_contract_addr.is_none() => {
                    info!("auto-discovered {} contract: {}", symbol, addr);
                    config.wneo_contract_addr = Some(addr.clone());
                }
                "wbtc" if config.wbtc_contract_addr.is_none() => {
                    info!("auto-discovered {} contract: {}", symbol, addr);
                    config.wbtc_contract_addr = Some(addr.clone());
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_applies_wbtc_and_preserves_explicit_pins() {
        let mut config = crate::test_support::test_config();
        config.wbtc_contract_addr = Some("PINNED_WBTC".to_string());

        let addr_by_symbol = std::collections::HashMap::from([
            ("LUSD".to_string(), "DISCOVERED_LUSD".to_string()),
            ("WSOL".to_string(), "DISCOVERED_WSOL".to_string()),
            ("WETH".to_string(), "DISCOVERED_WETH".to_string()),
            ("WBNB".to_string(), "DISCOVERED_WBNB".to_string()),
            ("WGAS".to_string(), "DISCOVERED_WGAS".to_string()),
            ("WNEO".to_string(), "DISCOVERED_WNEO".to_string()),
            ("WBTC".to_string(), "DISCOVERED_WBTC".to_string()),
        ]);

        apply_discovered_contract_addresses(&mut config, &addr_by_symbol);

        assert_eq!(
            config.musd_contract_addr.as_deref(),
            Some("DISCOVERED_LUSD")
        );
        assert_eq!(
            config.wsol_contract_addr.as_deref(),
            Some("DISCOVERED_WSOL")
        );
        assert_eq!(
            config.weth_contract_addr.as_deref(),
            Some("DISCOVERED_WETH")
        );
        assert_eq!(
            config.wbnb_contract_addr.as_deref(),
            Some("DISCOVERED_WBNB")
        );
        assert_eq!(
            config.wgas_contract_addr.as_deref(),
            Some("DISCOVERED_WGAS")
        );
        assert_eq!(
            config.wneo_contract_addr.as_deref(),
            Some("DISCOVERED_WNEO")
        );
        assert_eq!(config.wbtc_contract_addr.as_deref(), Some("PINNED_WBTC"));

        config.wbtc_contract_addr = None;
        apply_discovered_contract_addresses(&mut config, &addr_by_symbol);
        assert_eq!(
            config.wbtc_contract_addr.as_deref(),
            Some("DISCOVERED_WBTC")
        );
    }
}

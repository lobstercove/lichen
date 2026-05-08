use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RestrictionOutputKind {
    Get,
    List { active_only: bool },
    Status { label: String },
    Movement { label: String },
    Builder,
}

pub(super) fn render_restriction_output(kind: &RestrictionOutputKind, value: &Value) -> String {
    match kind {
        RestrictionOutputKind::Get => render_get_restriction(value),
        RestrictionOutputKind::List { active_only } => {
            render_restriction_list(value, *active_only)
        }
        RestrictionOutputKind::Status { label } => render_restriction_status(label, value),
        RestrictionOutputKind::Movement { label } => render_movement_status(label, value),
        RestrictionOutputKind::Builder => render_builder_response(value),
    }
}

fn render_get_restriction(value: &Value) -> String {
    let id = value_u64(value, "id")
        .map(|id| id.to_string())
        .unwrap_or_else(|| "?".to_string());
    let slot = value_u64(value, "slot")
        .map(|slot| slot.to_string())
        .unwrap_or_else(|| "?".to_string());
    let found = value_bool(value, "found").unwrap_or(false);

    let mut out = String::new();
    push_line(&mut out, "Restriction");
    push_line(&mut out, "-----------");
    push_line(&mut out, format!("ID: {}", id));
    push_line(&mut out, format!("Slot: {}", slot));

    if !found {
        push_line(&mut out, "Status: not found");
        return out;
    }

    if let Some(record) = value.get("restriction") {
        render_record_details(&mut out, record);
    }
    out
}

fn render_restriction_list(value: &Value, active_only: bool) -> String {
    let mut out = String::new();
    push_line(
        &mut out,
        if active_only {
            "Active restrictions"
        } else {
            "Restrictions"
        },
    );
    push_line(&mut out, "-------------------");
    push_line(
        &mut out,
        format!(
            "Slot: {}",
            value_u64(value, "slot")
                .map(|slot| slot.to_string())
                .unwrap_or_else(|| "?".to_string())
        ),
    );
    push_line(
        &mut out,
        format!("Count: {}", value_u64(value, "count").unwrap_or(0)),
    );
    push_line(
        &mut out,
        format!(
            "Has more: {}",
            yes_no(value_bool(value, "has_more").unwrap_or(false))
        ),
    );
    if let Some(cursor) = value_string(value, "next_cursor") {
        push_line(&mut out, format!("Next cursor: {}", cursor));
    }
    push_line(&mut out, "");

    match value.get("restrictions").and_then(Value::as_array) {
        Some(records) if !records.is_empty() => {
            for record in records {
                push_line(&mut out, render_record_summary(record));
            }
        }
        _ => push_line(&mut out, "No restrictions found"),
    }

    out
}

fn render_restriction_status(label: &str, value: &Value) -> String {
    let mut out = String::new();
    push_line(&mut out, format!("Restriction status: {}", label));
    push_line(&mut out, "-------------------");
    push_line(
        &mut out,
        format!(
            "Slot: {}",
            value_u64(value, "slot")
                .map(|slot| slot.to_string())
                .unwrap_or_else(|| "?".to_string())
        ),
    );
    push_optional_string(&mut out, "Target type", value, "target_type");
    push_optional_string(&mut out, "Target", value, "target");
    push_optional_string(&mut out, "Contract", value, "contract");
    push_optional_string(&mut out, "Code hash", value, "code_hash");
    push_optional_string(&mut out, "Chain", value, "chain_id");
    push_optional_string(&mut out, "Asset", value, "asset");
    push_optional_string(&mut out, "Lifecycle", value, "lifecycle_status");

    let active = value_bool(value, "active")
        .or_else(|| value_bool(value, "restricted"))
        .or_else(|| value_bool(value, "blocked"))
        .or_else(|| value_bool(value, "paused"))
        .unwrap_or(false);
    push_line(&mut out, format!("Active: {}", yes_no(active)));

    let ids = value
        .get("active_restriction_ids")
        .map(render_id_list)
        .unwrap_or_else(|| "-".to_string());
    push_line(&mut out, format!("Active IDs: {}", ids));

    if let Some(records) = value.get("active_restrictions").and_then(Value::as_array) {
        if !records.is_empty() {
            push_line(&mut out, "");
            push_line(&mut out, "Active records:");
            for record in records {
                push_line(&mut out, format!("  {}", render_record_summary(record)));
            }
        }
    }

    out
}

fn render_movement_status(label: &str, value: &Value) -> String {
    let mut out = String::new();
    push_line(&mut out, format!("Restriction preflight: {}", label));
    push_line(&mut out, "----------------------");
    push_line(
        &mut out,
        format!(
            "Allowed: {}",
            yes_no(value_bool(value, "allowed").unwrap_or(false))
        ),
    );
    push_line(
        &mut out,
        format!(
            "Blocked: {}",
            yes_no(value_bool(value, "blocked").unwrap_or(false))
        ),
    );
    push_optional_string(&mut out, "Operation", value, "operation");
    push_optional_string(&mut out, "Account", value, "account");
    push_optional_string(&mut out, "From", value, "from");
    push_optional_string(&mut out, "To", value, "to");
    push_optional_string(&mut out, "Asset", value, "asset");
    push_optional_u64(&mut out, "Amount", value, "amount");
    push_optional_u64(&mut out, "Spendable", value, "spendable");
    push_optional_u64(&mut out, "Source spendable", value, "source_spendable");
    push_optional_u64(
        &mut out,
        "Recipient spendable",
        value,
        "recipient_spendable",
    );
    push_optional_u64(&mut out, "Slot", value, "slot");

    if value.get("source_restriction_ids").is_some()
        || value.get("recipient_restriction_ids").is_some()
    {
        push_line(
            &mut out,
            format!(
                "Source IDs: {}",
                value
                    .get("source_restriction_ids")
                    .map(render_id_list)
                    .unwrap_or_else(|| "-".to_string())
            ),
        );
        push_line(
            &mut out,
            format!(
                "Recipient IDs: {}",
                value
                    .get("recipient_restriction_ids")
                    .map(render_id_list)
                    .unwrap_or_else(|| "-".to_string())
            ),
        );
    }

    push_line(
        &mut out,
        format!(
            "Active IDs: {}",
            value
                .get("active_restriction_ids")
                .map(render_id_list)
                .unwrap_or_else(|| "-".to_string())
        ),
    );

    out
}

fn render_builder_response(value: &Value) -> String {
    let mut out = String::new();
    push_line(&mut out, "Unsigned restriction governance transaction");
    push_line(&mut out, "-------------------------------------------");
    push_optional_string(&mut out, "Method", value, "method");
    push_optional_string(&mut out, "Action", value, "action_label");
    push_optional_string(&mut out, "Proposer", value, "proposer");
    push_optional_string(&mut out, "Governance authority", value, "governance_authority");
    push_optional_string(&mut out, "Recent blockhash", value, "recent_blockhash");
    push_optional_string(&mut out, "Message hash", value, "message_hash");
    push_optional_u64(&mut out, "Wire size", value, "wire_size");
    push_optional_u64(&mut out, "Signature count", value, "signature_count");

    if let Some(action) = value.get("action") {
        push_line(&mut out, "");
        push_line(&mut out, "Action payload:");
        push_line(&mut out, compact_json(action));
    }

    if let Some(tx) = value_string(value, "transaction_base64")
        .or_else(|| value_string(value, "transaction"))
    {
        push_line(&mut out, "");
        push_line(&mut out, "Transaction base64:");
        push_line(&mut out, tx);
    }

    out
}

fn render_record_details(out: &mut String, record: &Value) {
    push_optional_string(out, "Effective status", record, "effective_status");
    push_optional_string(out, "Stored status", record, "status");
    push_optional_string(out, "Target type", record, "target_type");
    push_optional_string(out, "Target", record, "target");
    push_optional_string(out, "Mode", record, "mode");
    push_optional_u64(out, "Frozen amount", record, "frozen_amount");
    push_optional_string(out, "Reason", record, "reason");
    push_optional_string(out, "Proposer", record, "proposer");
    push_optional_string(out, "Authority", record, "authority");
    push_optional_u64(out, "Created slot", record, "created_slot");
    push_optional_u64(out, "Created epoch", record, "created_epoch");
    push_optional_u64(out, "Expires at slot", record, "expires_at_slot");
    push_optional_u64(out, "Supersedes", record, "supersedes");
    push_optional_string(out, "Lift reason", record, "lift_reason");
    push_optional_u64(out, "Lifted slot", record, "lifted_slot");
}

fn render_record_summary(record: &Value) -> String {
    let id = value_u64(record, "id")
        .map(|id| format!("#{}", id))
        .unwrap_or_else(|| "#?".to_string());
    let status = value_string(record, "effective_status")
        .or_else(|| value_string(record, "status"))
        .unwrap_or_else(|| "unknown".to_string());
    let target_type = value_string(record, "target_type").unwrap_or_else(|| "target".to_string());
    let target = value_string(record, "target").unwrap_or_else(|| "?".to_string());
    let mode = value_string(record, "mode").unwrap_or_else(|| "unknown".to_string());
    let reason = value_string(record, "reason").unwrap_or_else(|| "unknown".to_string());

    format!(
        "{} [{}] {} {} mode={} reason={}",
        id, status, target_type, target, mode, reason
    )
}

fn push_optional_string(out: &mut String, label: &str, value: &Value, key: &str) {
    if let Some(text) = value_string(value, key) {
        push_line(out, format!("{}: {}", label, text));
    }
}

fn push_optional_u64(out: &mut String, label: &str, value: &Value, key: &str) {
    if let Some(number) = value_u64(value, key) {
        push_line(out, format!("{}: {}", label, number));
    }
}

fn render_id_list(value: &Value) -> String {
    value
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(|value| {
                    value
                        .as_u64()
                        .map(|id| id.to_string())
                        .or_else(|| value.as_str().map(str::to_string))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "-".to_string())
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        Value::Null => None,
        other => Some(compact_json(other)),
    }
}

fn value_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

fn value_bool(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn push_line(out: &mut String, line: impl AsRef<str>) {
    out.push_str(line.as_ref());
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_get_restriction_human_output() {
        let value = serde_json::json!({
            "id": 7,
            "slot": 44,
            "found": true,
            "restriction": {
                "id": 7,
                "effective_status": "active",
                "status": "active",
                "target_type": "account",
                "target": "acct",
                "mode": "outgoing_only",
                "reason": "testnet_drill",
                "proposer": "prop",
                "authority": "auth",
                "created_slot": 40,
                "created_epoch": 2
            }
        });

        let rendered = render_restriction_output(&RestrictionOutputKind::Get, &value);

        assert!(rendered.contains("ID: 7"));
        assert!(rendered.contains("Effective status: active"));
        assert!(rendered.contains("Target type: account"));
        assert!(rendered.contains("Mode: outgoing_only"));
    }

    #[test]
    fn renders_list_restriction_human_output() {
        let value = serde_json::json!({
            "slot": 50,
            "count": 1,
            "has_more": true,
            "next_cursor": "7",
            "restrictions": [{
                "id": 7,
                "effective_status": "active",
                "target_type": "asset",
                "target": "native",
                "mode": "asset_paused",
                "reason": "testnet_drill"
            }]
        });

        let rendered = render_restriction_output(
            &RestrictionOutputKind::List { active_only: true },
            &value,
        );

        assert!(rendered.contains("Active restrictions"));
        assert!(rendered.contains("Next cursor: 7"));
        assert!(rendered.contains("#7 [active] asset native"));
    }

    #[test]
    fn renders_builder_human_output() {
        let value = serde_json::json!({
            "method": "buildRestrictAccountTx",
            "action_label": "restrict",
            "proposer": "prop",
            "governance_authority": "auth",
            "recent_blockhash": "abcd",
            "message_hash": "ef01",
            "wire_size": 123,
            "signature_count": 0,
            "action": {"kind": "restrict"},
            "transaction_base64": "AAAA"
        });

        let rendered = render_restriction_output(&RestrictionOutputKind::Builder, &value);

        assert!(rendered.contains("Unsigned restriction governance transaction"));
        assert!(rendered.contains("Method: buildRestrictAccountTx"));
        assert!(rendered.contains("Transaction base64:"));
        assert!(rendered.contains("AAAA"));
    }
}

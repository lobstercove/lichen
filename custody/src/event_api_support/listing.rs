use super::super::*;

pub(crate) async fn list_events(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let limit: usize = params
        .get("limit")
        .and_then(|value| value.parse().ok())
        .unwrap_or(50)
        .min(500);
    let event_type_filter = params.get("event_type").cloned();
    let entity_id_filter = params.get("entity_id").cloned();
    let tx_hash_filter = params.get("tx_hash").cloned();
    let after_cursor = params.get("after").cloned();

    let events_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events cf")))?;
    let index_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS_BY_TIME)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events_by_time cf")))?;
    let type_index_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS_BY_TYPE_TIME)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events_by_type_time cf")))?;
    let entity_index_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS_BY_ENTITY_TIME)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events_by_entity_time cf")))?;
    let tx_index_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS_BY_TX_TIME)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events_by_tx_time cf")))?;

    let mut events = Vec::new();
    let mut next_cursor: Option<String> = None;
    let mut use_filter_index = false;
    let mut filter_prefix = String::new();
    let filter_kind = if tx_hash_filter.is_some() {
        "tx"
    } else if entity_id_filter.is_some() {
        "entity"
    } else if event_type_filter.is_some() {
        "type"
    } else {
        "global"
    };
    let resolved_after = if let Some(after) = after_cursor.as_deref() {
        if filter_kind != "global" {
            use_filter_index = true;
            filter_prefix = match filter_kind {
                "tx" => format!("tx:{}:", tx_hash_filter.as_deref().unwrap_or("")),
                "entity" => {
                    format!("entity:{}:", entity_id_filter.as_deref().unwrap_or(""))
                }
                _ => format!("type:{}:", event_type_filter.as_deref().unwrap_or("")),
            };

            if after.starts_with("type:")
                || after.starts_with("entity:")
                || after.starts_with("tx:")
            {
                Some(after.to_string())
            } else {
                match state.db.get_cf(events_cf, after.as_bytes()) {
                    Ok(Some(bytes)) => {
                        let event: Option<Value> = serde_json::from_slice::<Value>(&bytes).ok();
                        let ts_ms = event
                            .as_ref()
                            .and_then(|value| value.get("timestamp_ms"))
                            .and_then(|value| value.as_i64())
                            .or_else(|| {
                                event
                                    .as_ref()
                                    .and_then(|value| value.get("timestamp"))
                                    .and_then(|value| value.as_i64())
                                    .map(|seconds| seconds.saturating_mul(1000))
                            })
                            .unwrap_or(0)
                            .max(0);

                        let prefix = match filter_kind {
                            "tx" => format!("tx:{}:", tx_hash_filter.as_deref().unwrap_or("")),
                            "entity" => {
                                format!("entity:{}:", entity_id_filter.as_deref().unwrap_or(""))
                            }
                            _ => {
                                format!("type:{}:", event_type_filter.as_deref().unwrap_or(""))
                            }
                        };
                        Some(format!("{}{:020}:{}", prefix, ts_ms, after))
                    }
                    _ => None,
                }
            }
        } else if after.contains(':') {
            Some(after.to_string())
        } else {
            match state.db.get_cf(events_cf, after.as_bytes()) {
                Ok(Some(bytes)) => {
                    let event: Option<Value> = serde_json::from_slice::<Value>(&bytes).ok();
                    let ts_ms = event
                        .as_ref()
                        .and_then(|value| value.get("timestamp_ms"))
                        .and_then(|value| value.as_i64())
                        .or_else(|| {
                            event
                                .as_ref()
                                .and_then(|value| value.get("timestamp"))
                                .and_then(|value| value.as_i64())
                                .map(|seconds| seconds.saturating_mul(1000))
                        })
                        .unwrap_or(0)
                        .max(0);
                    Some(format!("{:020}:{}", ts_ms, after))
                }
                _ => None,
            }
        }
    } else {
        if filter_kind != "global" {
            filter_prefix = match filter_kind {
                "tx" => format!("tx:{}:", tx_hash_filter.as_deref().unwrap_or("")),
                "entity" => {
                    format!("entity:{}:", entity_id_filter.as_deref().unwrap_or(""))
                }
                _ => format!("type:{}:", event_type_filter.as_deref().unwrap_or("")),
            };
            use_filter_index = true;
        }
        None
    };

    let upper_bound = if resolved_after.is_none() && use_filter_index {
        let mut bytes = filter_prefix.as_bytes().to_vec();
        bytes.push(0xFF);
        Some(bytes)
    } else {
        None
    };
    let iter_mode = if let Some(cursor_key) = resolved_after.as_ref() {
        rocksdb::IteratorMode::From(cursor_key.as_bytes(), rocksdb::Direction::Reverse)
    } else if let Some(ref upper) = upper_bound {
        rocksdb::IteratorMode::From(upper, rocksdb::Direction::Reverse)
    } else {
        rocksdb::IteratorMode::End
    };

    let mut skipped_cursor = false;
    let filter_prefix_bytes = filter_prefix.as_bytes();
    let source_cf = if use_filter_index {
        match filter_kind {
            "tx" => tx_index_cf,
            "entity" => entity_index_cf,
            "type" => type_index_cf,
            _ => type_index_cf,
        }
    } else {
        index_cf
    };
    for item in state.db.iterator_cf(source_cf, iter_mode) {
        if events.len() >= limit {
            break;
        }
        let (index_key, value) =
            item.map_err(|error| Json(ErrorResponse::db(&format!("iter: {}", error))))?;

        if use_filter_index && !index_key.starts_with(filter_prefix_bytes) {
            break;
        }

        if let Some(cursor_key) = resolved_after.as_ref() {
            if !skipped_cursor && index_key.as_ref() == cursor_key.as_bytes() {
                skipped_cursor = true;
                continue;
            }
        }

        let event_id = match std::str::from_utf8(&value) {
            Ok(id) if !id.is_empty() => id,
            _ => continue,
        };

        let event_value = match state.db.get_cf(events_cf, event_id.as_bytes()) {
            Ok(Some(value)) => value,
            _ => continue,
        };

        let event = match serde_json::from_slice::<Value>(&event_value) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if let Some(ref filter) = event_type_filter {
            if filter_kind != "type"
                && event.get("event_type").and_then(|value| value.as_str()) != Some(filter.as_str())
            {
                continue;
            }
        }
        if let Some(ref filter) = entity_id_filter {
            if filter_kind != "entity"
                && event.get("entity_id").and_then(|value| value.as_str()) != Some(filter.as_str())
            {
                continue;
            }
        }
        if let Some(ref filter) = tx_hash_filter {
            if filter_kind != "tx"
                && event.get("tx_hash").and_then(|value| value.as_str()) != Some(filter.as_str())
            {
                continue;
            }
        }

        next_cursor = Some(String::from_utf8_lossy(&index_key).to_string());
        events.push(event);
    }

    if events.is_empty() {
        let mut past_cursor = after_cursor.is_none();
        for item in state.db.iterator_cf(events_cf, rocksdb::IteratorMode::End) {
            if events.len() >= limit {
                break;
            }
            let (key, value) =
                item.map_err(|error| Json(ErrorResponse::db(&format!("iter: {}", error))))?;
            let event = match serde_json::from_slice::<Value>(&value) {
                Ok(value) => value,
                Err(_) => continue,
            };

            if !past_cursor {
                let key_str = std::str::from_utf8(&key).unwrap_or("");
                let event_id = event
                    .get("event_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if key_str == after_cursor.as_deref().unwrap_or("")
                    || event_id == after_cursor.as_deref().unwrap_or("")
                {
                    past_cursor = true;
                }
                continue;
            }

            if let Some(ref filter) = event_type_filter {
                if event.get("event_type").and_then(|value| value.as_str()) != Some(filter.as_str())
                {
                    continue;
                }
            }
            if let Some(ref filter) = entity_id_filter {
                if event.get("entity_id").and_then(|value| value.as_str()) != Some(filter.as_str())
                {
                    continue;
                }
            }
            if let Some(ref filter) = tx_hash_filter {
                if event.get("tx_hash").and_then(|value| value.as_str()) != Some(filter.as_str()) {
                    continue;
                }
            }

            events.push(event);
        }
    }

    Ok(Json(json!({
        "events": events,
        "count": events.len(),
        "next_cursor": next_cursor,
    })))
}

use super::super::*;

pub(crate) async fn ws_events(
    State(state): State<CustodyState>,
    ws: WebSocketUpgrade,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let auth_ok = if let Some(token) = params.get("token") {
        if let Some(expected) = state.config.api_auth_token.as_deref() {
            use subtle::ConstantTimeEq;
            let matches: bool = token.as_bytes().ct_eq(expected.as_bytes()).into();
            matches
        } else {
            false
        }
    } else {
        false
    };

    if !auth_ok {
        return axum::response::Response::builder()
            .status(401)
            .body(axum::body::Body::from(
                "Unauthorized: provide ?token=<api_auth_token>",
            ))
            .unwrap_or_default();
    }

    let event_filter: Vec<String> = params
        .get("filter")
        .map(|filter| {
            filter
                .split(',')
                .map(|entry| entry.trim().to_string())
                .filter(|entry| !entry.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let event_rx = state.event_tx.subscribe();

    ws.on_upgrade(move |socket| handle_ws_events(socket, event_rx, event_filter))
}

async fn handle_ws_events(
    mut socket: WebSocket,
    mut event_rx: broadcast::Receiver<CustodyWebhookEvent>,
    event_filter: Vec<String>,
) {
    info!(
        "WebSocket event subscriber connected (filter: {:?})",
        event_filter
    );

    loop {
        tokio::select! {
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        if !event_filter.is_empty() && !event_filter.contains(&event.event_type) {
                            continue;
                        }
                        let payload = match serde_json::to_string(&event) {
                            Ok(payload) => payload,
                            Err(_) => continue,
                        };
                        if socket.send(WsMessage::Text(payload)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        tracing::warn!("WebSocket subscriber lagged, dropped {} events", dropped);
                        let warning = json!({
                            "warning": "lagged",
                            "dropped_events": dropped,
                        });
                        drop(socket.send(WsMessage::Text(warning.to_string())).await);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(WsMessage::Ping(data))) => {
                        drop(socket.send(WsMessage::Pong(data)).await);
                    }
                    _ => {}
                }
            }
        }
    }

    info!("WebSocket event subscriber disconnected");
}

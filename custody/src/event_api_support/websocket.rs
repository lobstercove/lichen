use super::super::*;

const WS_EVENTS_TICKET_TTL_SECS: i64 = 60;
const WS_EVENTS_TICKET_BYTES: usize = 32;
const WS_EVENTS_TICKET_MAX_FILTERS: usize = 64;
const WS_EVENTS_TICKET_MAX_FILTER_LEN: usize = 128;
const WS_EVENTS_TICKET_COOKIE: &str = "lichen_ws_ticket";
const WS_EVENTS_TICKET_DOMAIN: &[u8] = b"LICHEN_CUSTODY_WS_EVENTS_TICKET_V1";
const WS_EVENTS_LEGACY_QUERY_TOKEN_ENV: &str = "CUSTODY_WS_EVENTS_ALLOW_QUERY_TOKEN";

pub(crate) async fn create_ws_events_ticket(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let event_filter =
        parse_event_filter(&params).map_err(|error| Json(ErrorResponse::invalid(&error)))?;
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + WS_EVENTS_TICKET_TTL_SECS;
    let ticket = generate_ws_event_ticket();
    let ticket_hash = ws_event_ticket_hash(&ticket);
    {
        let mut tickets = state.ws_event_tickets.lock().await;
        prune_ws_event_tickets(&mut tickets, now);
        tickets.insert(
            ticket_hash,
            WsEventTicket {
                event_filter: event_filter.clone(),
                expires_at,
            },
        );
    }

    let cookie = format!(
        "{}={}; Max-Age={}; Path=/ws/events; HttpOnly; Secure; SameSite=Strict",
        WS_EVENTS_TICKET_COOKIE, ticket, WS_EVENTS_TICKET_TTL_SECS
    );
    let mut response = Json(json!({
        "ticket": ticket,
        "ticket_type": "ws_events",
        "expires_in_secs": WS_EVENTS_TICKET_TTL_SECS,
        "issued_at": now,
        "expires_at": expires_at,
        "event_filter": event_filter,
    }))
    .into_response();
    response.headers_mut().insert(
        axum::http::header::SET_COOKIE,
        axum::http::HeaderValue::from_str(&cookie).map_err(|_| {
            Json(ErrorResponse::invalid(
                "failed to encode websocket ticket cookie",
            ))
        })?,
    );
    Ok(response)
}

pub(crate) async fn ws_events(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let requested_filter = match parse_event_filter(&params) {
        Ok(filter) => filter,
        Err(error) => return text_response(StatusCode::BAD_REQUEST, error),
    };

    let event_filter = match authorize_ws_events(&state, &headers, &params, requested_filter).await
    {
        Ok(filter) => filter,
        Err((status, message)) => return text_response(status, message),
    };

    let event_rx = state.event_tx.subscribe();

    ws.on_upgrade(move |socket| handle_ws_events(socket, event_rx, event_filter))
}

fn text_response(status: StatusCode, message: impl Into<String>) -> Response {
    axum::response::Response::builder()
        .status(status)
        .body(axum::body::Body::from(message.into()))
        .unwrap_or_default()
}

async fn authorize_ws_events(
    state: &CustodyState,
    headers: &axum::http::HeaderMap,
    params: &std::collections::HashMap<String, String>,
    requested_filter: Vec<String>,
) -> Result<Vec<String>, (StatusCode, String)> {
    if let Some(ticket) = ws_event_ticket_from_request(headers, params) {
        return consume_ws_event_ticket(state, &ticket, requested_filter).await;
    }

    if legacy_query_token_allowed() && verify_legacy_query_token(&state.config, params) {
        return Ok(requested_filter);
    }

    if params.contains_key("token") {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: mint a short-lived ticket with POST /ws/events/tickets; legacy token query auth is disabled"
                .to_string(),
        ));
    }

    Err((
        StatusCode::UNAUTHORIZED,
        "Unauthorized: mint a short-lived ticket with POST /ws/events/tickets".to_string(),
    ))
}

fn parse_event_filter(
    params: &std::collections::HashMap<String, String>,
) -> Result<Vec<String>, String> {
    let Some(filter) = params.get("filter") else {
        return Ok(Vec::new());
    };
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::new();
    for raw in filter.split(',') {
        let entry = raw.trim();
        if entry.is_empty() {
            continue;
        }
        if entry.len() > WS_EVENTS_TICKET_MAX_FILTER_LEN
            || entry.contains(|ch: char| ch.is_control() || ch.is_whitespace())
        {
            return Err("event filter entries must be compact non-control strings".to_string());
        }
        if !seen.insert(entry.to_string()) {
            continue;
        }
        parsed.push(entry.to_string());
        if parsed.len() > WS_EVENTS_TICKET_MAX_FILTERS {
            return Err(format!(
                "event filter supports at most {} entries",
                WS_EVENTS_TICKET_MAX_FILTERS
            ));
        }
    }
    Ok(parsed)
}

fn generate_ws_event_ticket() -> String {
    use base64::Engine as _;
    use rand::RngCore;

    let mut bytes = [0u8; WS_EVENTS_TICKET_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn ws_event_ticket_hash(ticket: &str) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    hasher.update(WS_EVENTS_TICKET_DOMAIN);
    hasher.update([0u8]);
    hasher.update(ticket.as_bytes());
    hex::encode(hasher.finalize())
}

fn prune_ws_event_tickets(tickets: &mut BTreeMap<String, WsEventTicket>, now: i64) {
    tickets.retain(|_, ticket| ticket.expires_at >= now);
}

fn ws_event_ticket_from_request(
    headers: &axum::http::HeaderMap,
    params: &std::collections::HashMap<String, String>,
) -> Option<String> {
    params
        .get("ticket")
        .map(|ticket| ticket.trim().to_string())
        .filter(|ticket| !ticket.is_empty())
        .or_else(|| ws_event_ticket_from_authorization(headers))
        .or_else(|| ws_event_ticket_from_cookie(headers))
}

fn ws_event_ticket_from_authorization(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|ticket| !ticket.is_empty())
        .map(ToOwned::to_owned)
}

fn ws_event_ticket_from_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == WS_EVENTS_TICKET_COOKIE && !value.trim().is_empty())
                    .then(|| value.trim().to_string())
            })
        })
}

async fn consume_ws_event_ticket(
    state: &CustodyState,
    ticket: &str,
    requested_filter: Vec<String>,
) -> Result<Vec<String>, (StatusCode, String)> {
    if ticket.len() > 256 || ticket.contains(|ch: char| ch.is_control() || ch.is_whitespace()) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: invalid websocket ticket".to_string(),
        ));
    }

    let now = chrono::Utc::now().timestamp();
    let hash = ws_event_ticket_hash(ticket);
    let mut tickets = state.ws_event_tickets.lock().await;
    prune_ws_event_tickets(&mut tickets, now);
    let Some(ticket_record) = tickets.get(&hash).cloned() else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: invalid websocket ticket".to_string(),
        ));
    };
    if ticket_record.expires_at < now {
        tickets.remove(&hash);
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: expired websocket ticket".to_string(),
        ));
    }
    if !requested_filter.is_empty()
        && !ticket_record.event_filter.is_empty()
        && !requested_filter
            .iter()
            .all(|entry| ticket_record.event_filter.contains(entry))
    {
        return Err((
            StatusCode::FORBIDDEN,
            "Forbidden: requested event filter is outside ticket scope".to_string(),
        ));
    }

    tickets.remove(&hash);
    if requested_filter.is_empty() {
        Ok(ticket_record.event_filter)
    } else {
        Ok(requested_filter)
    }
}

fn legacy_query_token_allowed() -> bool {
    std::env::var(WS_EVENTS_LEGACY_QUERY_TOKEN_ENV)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn verify_legacy_query_token(
    config: &CustodyConfig,
    params: &std::collections::HashMap<String, String>,
) -> bool {
    let Some(token) = params.get("token") else {
        return false;
    };
    let Some(expected) = config
        .api_auth_token
        .as_deref()
        .filter(|token| !token.is_empty())
    else {
        return false;
    };
    use subtle::ConstantTimeEq;
    token.as_bytes().ct_eq(expected.as_bytes()).into()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{test_auth_headers, test_state};

    fn filter_params(filter: &str) -> std::collections::HashMap<String, String> {
        let mut params = std::collections::HashMap::new();
        params.insert("filter".to_string(), filter.to_string());
        params
    }

    async fn scoped_ticket(
        state: &CustodyState,
        ticket: &str,
        event_filter: Vec<String>,
        ttl_secs: i64,
    ) {
        let now = chrono::Utc::now().timestamp();
        let hash = ws_event_ticket_hash(ticket);
        state.ws_event_tickets.lock().await.insert(
            hash,
            WsEventTicket {
                event_filter,
                expires_at: now + ttl_secs,
            },
        );
    }

    #[tokio::test]
    async fn test_create_ws_events_ticket_requires_bearer_auth() {
        let state = test_state();
        let err = create_ws_events_ticket(
            State(state),
            axum::http::HeaderMap::new(),
            axum::extract::Query(filter_params("deposit.confirmed")),
        )
        .await
        .expect_err("missing bearer auth should fail");

        assert_eq!(err.0.code, "unauthorized");
    }

    #[tokio::test]
    async fn test_create_ws_events_ticket_stores_hashed_ticket_and_cookie() {
        let state = test_state();
        let response = create_ws_events_ticket(
            State(state.clone()),
            test_auth_headers(),
            axum::extract::Query(filter_params("deposit.confirmed,withdrawal.confirmed")),
        )
        .await
        .expect("ticket issuance should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(axum::http::header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("ticket response should set cookie");
        assert!(cookie.contains("lichen_ws_ticket="));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("SameSite=Strict"));

        let tickets = state.ws_event_tickets.lock().await;
        assert_eq!(tickets.len(), 1);
        let stored = tickets.values().next().expect("stored ticket");
        assert_eq!(
            stored.event_filter,
            vec![
                "deposit.confirmed".to_string(),
                "withdrawal.confirmed".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn test_ws_events_ticket_is_one_use_and_scope_bound() {
        let state = test_state();
        scoped_ticket(
            &state,
            "ticket-one",
            vec![
                "deposit.confirmed".to_string(),
                "withdrawal.confirmed".to_string(),
            ],
            60,
        )
        .await;

        let accepted =
            consume_ws_event_ticket(&state, "ticket-one", vec!["deposit.confirmed".to_string()])
                .await
                .expect("scoped ticket should allow subset filter");
        assert_eq!(accepted, vec!["deposit.confirmed".to_string()]);

        let reused =
            consume_ws_event_ticket(&state, "ticket-one", vec!["deposit.confirmed".to_string()])
                .await
                .expect_err("ticket must be one-use");
        assert_eq!(reused.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_ws_events_ticket_rejects_wrong_filter_without_consuming() {
        let state = test_state();
        scoped_ticket(
            &state,
            "ticket-two",
            vec!["deposit.confirmed".to_string()],
            60,
        )
        .await;

        let wrong_scope = consume_ws_event_ticket(
            &state,
            "ticket-two",
            vec!["withdrawal.confirmed".to_string()],
        )
        .await
        .expect_err("wrong filter should fail");
        assert_eq!(wrong_scope.0, StatusCode::FORBIDDEN);

        let accepted = consume_ws_event_ticket(&state, "ticket-two", Vec::new())
            .await
            .expect("wrong-filter attempt should not consume the ticket");
        assert_eq!(accepted, vec!["deposit.confirmed".to_string()]);
    }

    #[tokio::test]
    async fn test_ws_events_ticket_rejects_expired_ticket() {
        let state = test_state();
        scoped_ticket(&state, "ticket-three", Vec::new(), -1).await;

        let expired = consume_ws_event_ticket(&state, "ticket-three", Vec::new())
            .await
            .expect_err("expired ticket should fail");
        assert_eq!(expired.0, StatusCode::UNAUTHORIZED);
        assert!(state.ws_event_tickets.lock().await.is_empty());
    }
}

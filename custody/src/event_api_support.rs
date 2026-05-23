use super::*;

mod listing;
mod websocket;

pub(super) async fn list_events(
    state: State<CustodyState>,
    headers: axum::http::HeaderMap,
    params: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    listing::list_events(state, headers, params).await
}

pub(super) async fn ws_events(
    state: State<CustodyState>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
    params: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    websocket::ws_events(state, headers, ws, params).await
}

pub(super) async fn create_ws_events_ticket(
    state: State<CustodyState>,
    headers: axum::http::HeaderMap,
    params: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, Json<ErrorResponse>> {
    websocket::create_ws_events_ticket(state, headers, params).await
}

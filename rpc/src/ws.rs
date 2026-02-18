// MoltChain WebSocket Server
// Real-time event subscriptions for blocks, transactions, accounts, and logs

use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
    routing::get,
    Router,
};
use moltchain_core::{Block, MarketActivity, Pubkey, StateStore, Transaction};
use crate::dex_ws::{DexChannel, DexEventBroadcaster};
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Prediction Market WS types (lightweight — no separate file needed)
// ─────────────────────────────────────────────────────────────────────────────

/// Prediction market events pushed over WebSocket
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum PredictionEvent {
    MarketCreated { market_id: u64, question: String, slot: u64 },
    TradeExecuted { market_id: u64, outcome: String, shares: u64, price: f64, slot: u64 },
    MarketResolved { market_id: u64, winning_outcome: String, slot: u64 },
    PriceUpdate { market_id: u64, outcomes: Vec<OutcomePrice>, slot: u64 },
}

#[derive(Clone, Debug, Serialize)]
pub struct OutcomePrice {
    pub outcome: String,
    pub price: f64,
}

/// Channel filter for prediction WS subscriptions
#[derive(Debug, Clone)]
pub enum PredictionChannel {
    AllMarkets,
    Market(u64),
}

impl PredictionChannel {
    pub fn parse(s: &str) -> Option<Self> {
        if s == "all" || s == "markets" {
            Some(PredictionChannel::AllMarkets)
        } else if let Some(id_str) = s.strip_prefix("market:") {
            id_str.parse::<u64>().ok().map(PredictionChannel::Market)
        } else if let Ok(id) = s.parse::<u64>() {
            Some(PredictionChannel::Market(id))
        } else {
            None
        }
    }

    pub fn matches(&self, event: &PredictionEvent) -> bool {
        match self {
            PredictionChannel::AllMarkets => true,
            PredictionChannel::Market(id) => {
                let event_id = match event {
                    PredictionEvent::MarketCreated { market_id, .. } => *market_id,
                    PredictionEvent::TradeExecuted { market_id, .. } => *market_id,
                    PredictionEvent::MarketResolved { market_id, .. } => *market_id,
                    PredictionEvent::PriceUpdate { market_id, .. } => *market_id,
                };
                *id == event_id
            }
        }
    }
}

/// Prediction event broadcaster (same pattern as DexEventBroadcaster)
pub struct PredictionEventBroadcaster {
    sender: broadcast::Sender<PredictionEvent>,
}

impl PredictionEventBroadcaster {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        PredictionEventBroadcaster { sender }
    }

    pub fn broadcast(&self, event: PredictionEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PredictionEvent> {
        self.sender.subscribe()
    }

    pub fn emit_market_created(&self, market_id: u64, question: &str, slot: u64) {
        self.broadcast(PredictionEvent::MarketCreated {
            market_id,
            question: question.to_string(),
            slot,
        });
    }

    pub fn emit_trade(&self, market_id: u64, outcome: &str, shares: u64, price: f64, slot: u64) {
        self.broadcast(PredictionEvent::TradeExecuted {
            market_id,
            outcome: outcome.to_string(),
            shares,
            price,
            slot,
        });
    }

    pub fn emit_market_resolved(&self, market_id: u64, winning_outcome: &str, slot: u64) {
        self.broadcast(PredictionEvent::MarketResolved {
            market_id,
            winning_outcome: winning_outcome.to_string(),
            slot,
        });
    }

    pub fn emit_price_update(&self, market_id: u64, outcomes: Vec<OutcomePrice>, slot: u64) {
        self.broadcast(PredictionEvent::PriceUpdate {
            market_id,
            outcomes,
            slot,
        });
    }
}
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

/// Per-IP connection limit
const MAX_CONNECTIONS_PER_IP: u32 = 10;

static IP_CONNECTIONS: std::sync::LazyLock<Mutex<HashMap<IpAddr, u32>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// T8.5: Maximum subscriptions allowed per WebSocket connection
const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 100;
/// DDoS protection: max concurrent WebSocket connections
const MAX_WS_CONNECTIONS: usize = 500;

/// WebSocket subscription request
#[derive(Debug, Deserialize)]
struct SubscriptionRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: serde_json::Value,
    method: String,
    params: Option<serde_json::Value>,
}

/// WebSocket subscription response
#[derive(Debug, Serialize)]
struct SubscriptionResponse {
    jsonrpc: String,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<WsError>,
}

/// WebSocket notification
#[derive(Debug, Serialize, Clone)]
struct Notification {
    jsonrpc: String,
    method: String,
    params: NotificationParams,
}

/// Notification parameters
#[derive(Debug, Serialize, Clone)]
struct NotificationParams {
    subscription: u64,
    result: serde_json::Value,
}

/// WebSocket error
#[derive(Debug, Serialize)]
struct WsError {
    code: i32,
    message: String,
}

/// Event types that can be subscribed to
#[derive(Debug, Clone)]
pub enum Event {
    Slot(u64),
    Block(Block),
    Transaction(Transaction),
    AccountChange {
        pubkey: Pubkey,
        balance: u64,
    },
    Log {
        contract: Pubkey,
        message: String,
    },
    ProgramUpdate {
        program: Pubkey,
        kind: String,
    },
    ProgramCall {
        program: Pubkey,
    },
    NftMint {
        collection: Pubkey,
    },
    NftTransfer {
        collection: Pubkey,
    },
    MarketListing {
        activity: MarketActivity,
    },
    MarketSale {
        activity: MarketActivity,
    },
    BridgeLock {
        chain: String,
        asset: String,
        amount: u64,
        sender: String,
        recipient: Pubkey,
    },
    BridgeMint {
        chain: String,
        asset: String,
        amount: u64,
        recipient: Pubkey,
        tx_hash: String,
    },
    // ─── New subscription events ───
    SignatureStatus {
        signature: String,
        status: String, // "processed" | "confirmed" | "finalized"
        slot: u64,
        err: Option<String>,
    },
    ValidatorUpdate {
        pubkey: Pubkey,
        event_kind: String, // "joined" | "left" | "delinquent" | "stakeChanged"
        stake: u64,
        slot: u64,
    },
    TokenBalanceChange {
        owner: Pubkey,
        mint: Pubkey,
        old_balance: u64,
        new_balance: u64,
        slot: u64,
    },
    EpochChange {
        epoch: u64,
        slot: u64,
        total_stake: u64,
        validator_count: u32,
    },
    GovernanceEvent {
        proposal_id: u64,
        event_kind: String, // "created" | "voted" | "executed" | "cancelled"
        voter: Option<Pubkey>,
        vote_weight: Option<u64>,
        slot: u64,
    },
}

/// Subscription manager
#[derive(Clone)]
struct SubscriptionManager {
    next_id: Arc<RwLock<u64>>,
    subscriptions: Arc<RwLock<HashMap<u64, SubscriptionType>>>,
}

#[derive(Debug, Clone)]
enum SubscriptionType {
    Slots,
    Blocks,
    Transactions,
    Account(Pubkey),
    Logs(Option<Pubkey>), // None = all contracts
    ProgramUpdates,
    ProgramCalls(Option<Pubkey>),
    NftMints(Option<Pubkey>),
    NftTransfers(Option<Pubkey>),
    MarketListings,
    MarketSales,
    BridgeLocks,
    BridgeMints,
    // ─── New subscription types ───
    SignatureStatus(String),     // track one tx signature hex
    Validators,                  // validator set changes
    TokenBalance { owner: Pubkey, mint: Option<Pubkey> },
    Epochs,                      // epoch boundary notifications
    Governance,                  // on-chain governance events
    Dex(DexChannel),             // DEX real-time channels (orderbook, trades, ticker, candles, orders, positions)
    Prediction(PredictionChannel), // Prediction market real-time channels
}

impl SubscriptionManager {
    fn new() -> Self {
        Self {
            next_id: Arc::new(RwLock::new(1)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn subscribe(&self, sub_type: SubscriptionType) -> Result<u64, WsError> {
        let subs = self.subscriptions.read().await;
        if subs.len() >= MAX_SUBSCRIPTIONS_PER_CONNECTION {
            return Err(WsError {
                code: -32005,
                message: format!(
                    "Subscription limit reached: max {} per connection",
                    MAX_SUBSCRIPTIONS_PER_CONNECTION
                ),
            });
        }
        drop(subs);

        let mut next_id = self.next_id.write().await;
        let id = *next_id;
        *next_id += 1;

        let mut subs = self.subscriptions.write().await;
        subs.insert(id, sub_type);

        Ok(id)
    }

    async fn unsubscribe(&self, id: u64) -> bool {
        let mut subs = self.subscriptions.write().await;
        subs.remove(&id).is_some()
    }
}

/// WebSocket state
#[derive(Clone)]
pub struct WsState {
    #[allow(dead_code)]
    state: StateStore,
    event_tx: broadcast::Sender<Event>,
    /// DEX real-time event broadcaster
    dex_broadcaster: Arc<DexEventBroadcaster>,
    /// Prediction market real-time event broadcaster
    prediction_broadcaster: Arc<PredictionEventBroadcaster>,
    /// DDoS protection: active connection counter
    active_connections: Arc<AtomicUsize>,
}

impl WsState {
    pub fn new(state: StateStore) -> (Self, broadcast::Sender<Event>, Arc<DexEventBroadcaster>, Arc<PredictionEventBroadcaster>) {
        let (event_tx, _) = broadcast::channel(1000);
        let dex_broadcaster = Arc::new(DexEventBroadcaster::new(2048));
        let prediction_broadcaster = Arc::new(PredictionEventBroadcaster::new(1024));
        let ws_state = Self {
            state,
            event_tx: event_tx.clone(),
            dex_broadcaster: dex_broadcaster.clone(),
            prediction_broadcaster: prediction_broadcaster.clone(),
            active_connections: Arc::new(AtomicUsize::new(0)),
        };
        (ws_state, event_tx, dex_broadcaster, prediction_broadcaster)
    }
}

/// Start WebSocket server
pub async fn start_ws_server(
    state: StateStore,
    port: u16,
) -> Result<(broadcast::Sender<Event>, Arc<DexEventBroadcaster>, Arc<PredictionEventBroadcaster>, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let (ws_state, event_tx, dex_broadcaster, prediction_broadcaster) = WsState::new(state);

    let app = Router::new()
        .route("/", get(ws_handler))
        .route("/ws", get(ws_handler))
        .with_state(ws_state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("🦞 WebSocket server listening on {}", addr);

    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await {
            error!("WebSocket server error: {}", e);
        }
    });

    Ok((event_tx, dex_broadcaster, prediction_broadcaster, handle))
}

/// WebSocket handler with connection limit enforcement
async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    State(state): State<WsState>,
) -> Response {
    let current = state.active_connections.load(Ordering::SeqCst);
    if current >= MAX_WS_CONNECTIONS {
        warn!(
            "WebSocket connection limit reached ({}/{}), rejecting",
            current, MAX_WS_CONNECTIONS
        );
        return Response::builder()
            .status(503)
            .body(axum::body::Body::from("Too many WebSocket connections"))
            .unwrap();
    }

    // Per-IP connection limit
    let ip = addr.ip();
    {
        let conns = IP_CONNECTIONS.lock().unwrap();
        let count = conns.get(&ip).copied().unwrap_or(0);
        if count >= MAX_CONNECTIONS_PER_IP {
            warn!("Per-IP connection limit reached for {}: {}", ip, count);
            return Response::builder()
                .status(429)
                .body(axum::body::Body::from("Too many connections from this IP"))
                .unwrap();
        }
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state, ip))
}

/// Handle WebSocket connection
async fn handle_socket(socket: WebSocket, state: WsState, ip: IpAddr) {
    state.active_connections.fetch_add(1, Ordering::SeqCst);
    let conn_guard = state.active_connections.clone();

    // Track per-IP connections
    {
        let mut conns = IP_CONNECTIONS.lock().unwrap();
        *conns.entry(ip).or_insert(0) += 1;
    }

    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel::<String>(100);

    // Subscribe to broadcast events
    let mut event_rx = state.event_tx.subscribe();
    let subscription_manager = SubscriptionManager::new();

    // Subscribe to DEX-specific events
    let mut dex_event_rx = state.dex_broadcaster.subscribe();

    // Subscribe to prediction market events
    let mut prediction_event_rx = state.prediction_broadcaster.subscribe();

    // Task to forward notifications to the client
    let send_task = tokio::spawn(async move {
        // Send periodic pings to keep the connection alive
        let mut ping_interval = tokio::time::interval(tokio::time::Duration::from_secs(15));
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(text) => {
                            if sender.send(Message::Text(text)).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if sender.send(Message::Ping(vec![b'k'])).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Task to broadcast events to subscribed clients
    let tx_clone = tx.clone();
    let event_subscription_manager = subscription_manager.clone();
    let event_task = tokio::spawn(async move {
        loop {
            let event = match event_rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("WebSocket subscriber lagged, skipped {} events", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            // Get all subscriptions
            let subs = event_subscription_manager.subscriptions.read().await;

            for (sub_id, sub_type) in subs.iter() {
                let should_send = match (&event, sub_type) {
                    (Event::Slot(_), SubscriptionType::Slots) => true,
                    (Event::Block(_), SubscriptionType::Blocks) => true,
                    (Event::Transaction(_), SubscriptionType::Transactions) => true,
                    (
                        Event::AccountChange { pubkey, .. },
                        SubscriptionType::Account(sub_pubkey),
                    ) => pubkey == sub_pubkey,
                    (Event::Log { .. }, SubscriptionType::Logs(None)) => true,
                    (Event::Log { contract, .. }, SubscriptionType::Logs(Some(sub_contract))) => {
                        contract == sub_contract
                    }
                    (Event::ProgramUpdate { .. }, SubscriptionType::ProgramUpdates) => true,
                    (Event::ProgramCall { .. }, SubscriptionType::ProgramCalls(None)) => true,
                    (
                        Event::ProgramCall { program },
                        SubscriptionType::ProgramCalls(Some(sub_program)),
                    ) => program == sub_program,
                    (Event::NftMint { .. }, SubscriptionType::NftMints(None)) => true,
                    (
                        Event::NftMint { collection },
                        SubscriptionType::NftMints(Some(sub_collection)),
                    ) => collection == sub_collection,
                    (Event::NftTransfer { .. }, SubscriptionType::NftTransfers(None)) => true,
                    (
                        Event::NftTransfer { collection },
                        SubscriptionType::NftTransfers(Some(sub_collection)),
                    ) => collection == sub_collection,
                    (Event::MarketListing { .. }, SubscriptionType::MarketListings) => true,
                    (Event::MarketSale { .. }, SubscriptionType::MarketSales) => true,
                    (Event::BridgeLock { .. }, SubscriptionType::BridgeLocks) => true,
                    (Event::BridgeMint { .. }, SubscriptionType::BridgeMints) => true,
                    // ─── New subscription matching ───
                    (Event::SignatureStatus { ref signature, .. }, SubscriptionType::SignatureStatus(ref sub_sig)) => signature == sub_sig,
                    (Event::ValidatorUpdate { .. }, SubscriptionType::Validators) => true,
                    (Event::TokenBalanceChange { ref owner, ref mint, .. }, SubscriptionType::TokenBalance { owner: ref sub_owner, mint: ref sub_mint }) => {
                        owner == sub_owner && sub_mint.as_ref().map_or(true, |m| m == mint)
                    },
                    (Event::EpochChange { .. }, SubscriptionType::Epochs) => true,
                    (Event::GovernanceEvent { .. }, SubscriptionType::Governance) => true,
                    _ => false,
                };

                if should_send {
                    let notification = create_notification(*sub_id, &event);
                    if let Ok(json) = serde_json::to_string(&notification) {
                        let _ = tx_clone.send(json).await;
                    }
                }
            }
        }
    });

    // Task to forward DEX-specific events to subscribed clients
    let tx_dex = tx.clone();
    let dex_subscription_manager = subscription_manager.clone();
    let dex_event_task = tokio::spawn(async move {
        loop {
            let dex_event = match dex_event_rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("DEX WebSocket subscriber lagged, skipped {} events", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            let subs = dex_subscription_manager.subscriptions.read().await;
            for (sub_id, sub_type) in subs.iter() {
                if let SubscriptionType::Dex(ref channel) = sub_type {
                    if channel.matches(&dex_event) {
                        let notification = Notification {
                            jsonrpc: "2.0".to_string(),
                            method: "notification".to_string(),
                            params: NotificationParams {
                                subscription: *sub_id,
                                result: serde_json::to_value(&dex_event).unwrap_or_default(),
                            },
                        };
                        if let Ok(json) = serde_json::to_string(&notification) {
                            let _ = tx_dex.send(json).await;
                        }
                    }
                }
            }
        }
    });

    // Task to forward prediction market events to subscribed clients
    let tx_pred = tx.clone();
    let pred_subscription_manager = subscription_manager.clone();
    let prediction_event_task = tokio::spawn(async move {
        loop {
            let pred_event = match prediction_event_rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Prediction WebSocket subscriber lagged, skipped {} events", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            let subs = pred_subscription_manager.subscriptions.read().await;
            for (sub_id, sub_type) in subs.iter() {
                if let SubscriptionType::Prediction(ref channel) = sub_type {
                    if channel.matches(&pred_event) {
                        let notification = Notification {
                            jsonrpc: "2.0".to_string(),
                            method: "notification".to_string(),
                            params: NotificationParams {
                                subscription: *sub_id,
                                result: serde_json::to_value(&pred_event).unwrap_or_default(),
                            },
                        };
                        if let Ok(json) = serde_json::to_string(&notification) {
                            let _ = tx_pred.send(json).await;
                        }
                    }
                }
            }
        }
    });

    // Handle incoming messages
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            if let Ok(req) = serde_json::from_str::<SubscriptionRequest>(&text) {
                let response = handle_subscription_request(req, &subscription_manager).await;
                if let Ok(json) = serde_json::to_string(&response) {
                    let _ = tx.send(json).await;
                }
            }
        } else if let Message::Pong(_) = msg {
            // Keepalive pong received — connection is alive
        } else if let Message::Close(_) = msg {
            break;
        }
    }

    // Clean up
    send_task.abort();
    event_task.abort();
    dex_event_task.abort();
    prediction_event_task.abort();
    // DDoS protection: decrement active connection counter
    conn_guard.fetch_sub(1, Ordering::SeqCst);

    // Decrement per-IP connection count
    {
        let mut conns = IP_CONNECTIONS.lock().unwrap();
        if let Some(count) = conns.get_mut(&ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                conns.remove(&ip);
            }
        }
    }
}

/// Handle subscription request
async fn handle_subscription_request(
    req: SubscriptionRequest,
    subscription_manager: &SubscriptionManager,
) -> SubscriptionResponse {
    let result = match req.method.as_str() {
        "subscribeSlots" | "slotSubscribe" => subscription_manager
            .subscribe(SubscriptionType::Slots)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeSlots" | "slotUnsubscribe" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeBlocks" => subscription_manager
            .subscribe(SubscriptionType::Blocks)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeBlocks" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeTransactions" => subscription_manager
            .subscribe(SubscriptionType::Transactions)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeTransactions" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeAccount" => {
            if let Some(params) = req.params {
                if let Some(pubkey_str) = params.as_str() {
                    match Pubkey::from_base58(pubkey_str) {
                        Ok(pubkey) => subscription_manager
                            .subscribe(SubscriptionType::Account(pubkey))
                            .await
                            .map(|sub_id| serde_json::json!(sub_id)),
                        Err(_) => Err(WsError {
                            code: -32602,
                            message: "Invalid pubkey format".to_string(),
                        }),
                    }
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected pubkey string".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "unsubscribeAccount" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeLogs" => {
            let contract = if let Some(params) = req.params {
                if let Some(contract_str) = params.as_str() {
                    match Pubkey::from_base58(contract_str) {
                        Ok(pubkey) => Some(pubkey),
                        Err(_) => {
                            return SubscriptionResponse {
                                jsonrpc: "2.0".to_string(),
                                id: req.id,
                                result: None,
                                error: Some(WsError {
                                    code: -32602,
                                    message: "Invalid contract pubkey format".to_string(),
                                }),
                            };
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let sub_id = subscription_manager
                .subscribe(SubscriptionType::Logs(contract))
                .await;
            sub_id.map(|id| serde_json::json!(id))
        }
        "unsubscribeLogs" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeProgramUpdates" => subscription_manager
            .subscribe(SubscriptionType::ProgramUpdates)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeProgramUpdates" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeProgramCalls" => {
            let program = if let Some(params) = req.params {
                if let Some(program_str) = params.as_str() {
                    match Pubkey::from_base58(program_str) {
                        Ok(pubkey) => Some(pubkey),
                        Err(_) => {
                            return SubscriptionResponse {
                                jsonrpc: "2.0".to_string(),
                                id: req.id,
                                result: None,
                                error: Some(WsError {
                                    code: -32602,
                                    message: "Invalid program pubkey format".to_string(),
                                }),
                            };
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let sub_id = subscription_manager
                .subscribe(SubscriptionType::ProgramCalls(program))
                .await;
            sub_id.map(|id| serde_json::json!(id))
        }
        "unsubscribeProgramCalls" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeNftMints" => {
            let collection = if let Some(params) = req.params {
                if let Some(collection_str) = params.as_str() {
                    match Pubkey::from_base58(collection_str) {
                        Ok(pubkey) => Some(pubkey),
                        Err(_) => {
                            return SubscriptionResponse {
                                jsonrpc: "2.0".to_string(),
                                id: req.id,
                                result: None,
                                error: Some(WsError {
                                    code: -32602,
                                    message: "Invalid collection pubkey format".to_string(),
                                }),
                            };
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let sub_id = subscription_manager
                .subscribe(SubscriptionType::NftMints(collection))
                .await;
            sub_id.map(|id| serde_json::json!(id))
        }
        "unsubscribeNftMints" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeNftTransfers" => {
            let collection = if let Some(params) = req.params {
                if let Some(collection_str) = params.as_str() {
                    match Pubkey::from_base58(collection_str) {
                        Ok(pubkey) => Some(pubkey),
                        Err(_) => {
                            return SubscriptionResponse {
                                jsonrpc: "2.0".to_string(),
                                id: req.id,
                                result: None,
                                error: Some(WsError {
                                    code: -32602,
                                    message: "Invalid collection pubkey format".to_string(),
                                }),
                            };
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let sub_id = subscription_manager
                .subscribe(SubscriptionType::NftTransfers(collection))
                .await;
            sub_id.map(|id| serde_json::json!(id))
        }
        "unsubscribeNftTransfers" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeMarketListings" => subscription_manager
            .subscribe(SubscriptionType::MarketListings)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeMarketListings" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeMarketSales" => subscription_manager
            .subscribe(SubscriptionType::MarketSales)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeMarketSales" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeBridgeLocks" => subscription_manager
            .subscribe(SubscriptionType::BridgeLocks)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeBridgeLocks" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }
        "subscribeBridgeMints" => subscription_manager
            .subscribe(SubscriptionType::BridgeMints)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeBridgeMints" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    let success = subscription_manager.unsubscribe(sub_id).await;
                    Ok(serde_json::json!(success))
                } else {
                    Err(WsError {
                        code: -32602,
                        message: "Invalid params: expected subscription ID".to_string(),
                    })
                }
            } else {
                Err(WsError {
                    code: -32602,
                    message: "Missing params".to_string(),
                })
            }
        }

        // ─── New subscriptions ───
        "subscribeSignatureStatus" | "signatureSubscribe" => {
            if let Some(params) = req.params {
                if let Some(sig) = params.as_str() {
                    subscription_manager
                        .subscribe(SubscriptionType::SignatureStatus(sig.to_string()))
                        .await
                        .map(|sub_id| serde_json::json!(sub_id))
                } else {
                    Err(WsError { code: -32602, message: "Expected signature string".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params: signature string".to_string() })
            }
        }
        "unsubscribeSignatureStatus" | "signatureUnsubscribe" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else {
                    Err(WsError { code: -32602, message: "Invalid params: expected subscription ID".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params".to_string() })
            }
        }

        "subscribeValidators" | "validatorSubscribe" => subscription_manager
            .subscribe(SubscriptionType::Validators)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeValidators" | "validatorUnsubscribe" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else {
                    Err(WsError { code: -32602, message: "Invalid params".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params".to_string() })
            }
        }

        "subscribeTokenBalance" | "tokenBalanceSubscribe" => {
            if let Some(params) = req.params {
                if let Some(obj) = params.as_object() {
                    let owner_str = obj.get("owner").and_then(|v| v.as_str()).unwrap_or("");
                    let mint_str = obj.get("mint").and_then(|v| v.as_str());
                    match Pubkey::from_base58(owner_str) {
                        Ok(owner) => {
                            let mint = mint_str.and_then(|m| Pubkey::from_base58(m).ok());
                            subscription_manager
                                .subscribe(SubscriptionType::TokenBalance { owner, mint })
                                .await
                                .map(|sub_id| serde_json::json!(sub_id))
                        }
                        Err(_) => Err(WsError { code: -32602, message: "Invalid owner pubkey".to_string() }),
                    }
                } else {
                    Err(WsError { code: -32602, message: "Expected object with 'owner' field".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params".to_string() })
            }
        }
        "unsubscribeTokenBalance" | "tokenBalanceUnsubscribe" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else {
                    Err(WsError { code: -32602, message: "Invalid params".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params".to_string() })
            }
        }

        "subscribeEpochs" | "epochSubscribe" => subscription_manager
            .subscribe(SubscriptionType::Epochs)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeEpochs" | "epochUnsubscribe" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else {
                    Err(WsError { code: -32602, message: "Invalid params".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params".to_string() })
            }
        }

        "subscribeGovernance" | "governanceSubscribe" => subscription_manager
            .subscribe(SubscriptionType::Governance)
            .await
            .map(|sub_id| serde_json::json!(sub_id)),
        "unsubscribeGovernance" | "governanceUnsubscribe" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else {
                    Err(WsError { code: -32602, message: "Invalid params".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params".to_string() })
            }
        }

        // ─── DEX real-time channels ───
        "subscribeDex" => {
            if let Some(params) = req.params {
                let channel_str = params.get("channel")
                    .and_then(|v| v.as_str())
                    .or_else(|| params.as_str());
                if let Some(ch) = channel_str {
                    if let Some(dex_channel) = DexChannel::parse(ch) {
                        subscription_manager.subscribe(SubscriptionType::Dex(dex_channel))
                            .await
                            .map(|sub_id| serde_json::json!(sub_id))
                    } else {
                        Err(WsError { code: -32602, message: format!("Invalid DEX channel: {}", ch) })
                    }
                } else {
                    Err(WsError { code: -32602, message: "Missing 'channel' param for subscribeDex".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params for subscribeDex".to_string() })
            }
        }
        "unsubscribeDex" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else if let Some(sub_id) = params.get("subscription").and_then(|v| v.as_u64()) {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else {
                    Err(WsError { code: -32602, message: "Invalid params".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params".to_string() })
            }
        }

        // ─── Prediction market real-time channels ───
        "subscribePrediction" | "subscribePredictionMarket" => {
            if let Some(params) = req.params {
                let channel_str = params.get("channel")
                    .and_then(|v| v.as_str())
                    .or_else(|| params.as_str())
                    .unwrap_or("all");
                if let Some(pred_channel) = PredictionChannel::parse(channel_str) {
                    subscription_manager.subscribe(SubscriptionType::Prediction(pred_channel))
                        .await
                        .map(|sub_id| serde_json::json!(sub_id))
                } else {
                    Err(WsError { code: -32602, message: format!("Invalid prediction channel: {}", channel_str) })
                }
            } else {
                // No params → subscribe to all markets
                subscription_manager.subscribe(SubscriptionType::Prediction(PredictionChannel::AllMarkets))
                    .await
                    .map(|sub_id| serde_json::json!(sub_id))
            }
        }
        "unsubscribePrediction" | "unsubscribePredictionMarket" => {
            if let Some(params) = req.params {
                if let Some(sub_id) = params.as_u64() {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else if let Some(sub_id) = params.get("subscription").and_then(|v| v.as_u64()) {
                    Ok(serde_json::json!(subscription_manager.unsubscribe(sub_id).await))
                } else {
                    Err(WsError { code: -32602, message: "Invalid params".to_string() })
                }
            } else {
                Err(WsError { code: -32602, message: "Missing params".to_string() })
            }
        }

        _ => Err(WsError {
            code: -32601,
            message: format!("Method not found: {}", req.method),
        }),
    };

    match result {
        Ok(result) => SubscriptionResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(result),
            error: None,
        },
        Err(error) => SubscriptionResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(error),
        },
    }
}

/// Create notification from event
fn market_activity_json(activity: &MarketActivity, event: &str) -> serde_json::Value {
    serde_json::json!({
        "event": event,
        "slot": activity.slot,
        "timestamp": activity.timestamp,
        "kind": format!("{:?}", activity.kind).to_lowercase(),
        "program": activity.program.to_base58(),
        "collection": activity.collection.as_ref().map(|p| p.to_base58()),
        "token": activity.token.as_ref().map(|p| p.to_base58()),
        "token_id": activity.token_id,
        "price": activity.price,
        "price_molt": activity.price.map(|val| val as f64 / 1_000_000_000.0),
        "seller": activity.seller.as_ref().map(|p| p.to_base58()),
        "buyer": activity.buyer.as_ref().map(|p| p.to_base58()),
        "function": activity.function.clone(),
        "tx_signature": activity.tx_signature.to_hex(),
    })
}

fn create_notification(sub_id: u64, event: &Event) -> Notification {
    let result = match event {
        Event::Slot(slot) => serde_json::json!({
            "slot": slot,
        }),
        Event::Block(block) => serde_json::json!({
            "slot": block.header.slot,
            "hash": block.hash().to_hex(),
            "parent_hash": block.header.parent_hash.to_hex(),
            "transactions": block.transactions.len(),
            "timestamp": block.header.timestamp,
        }),
        Event::Transaction(tx) => serde_json::json!({
            "signatures": tx.signatures.iter()
                .map(hex::encode)
                .collect::<Vec<_>>(),
            "instructions": tx.message.instructions.len(),
            "recent_blockhash": tx.message.recent_blockhash.to_hex(),
        }),
        Event::AccountChange { pubkey, balance } => serde_json::json!({
            "pubkey": pubkey.to_base58(),
            "balance": balance,
            "molt": balance / 1_000_000_000,
        }),
        Event::Log { contract, message } => serde_json::json!({
            "contract": contract.to_base58(),
            "message": message,
        }),
        Event::ProgramUpdate { program, kind } => serde_json::json!({
            "program": program.to_base58(),
            "kind": kind,
        }),
        Event::ProgramCall { program } => serde_json::json!({
            "program": program.to_base58(),
        }),
        Event::NftMint { collection } => serde_json::json!({
            "collection": collection.to_base58(),
        }),
        Event::NftTransfer { collection } => serde_json::json!({
            "collection": collection.to_base58(),
        }),
        Event::MarketListing { activity } => market_activity_json(activity, "MarketListing"),
        Event::MarketSale { activity } => market_activity_json(activity, "MarketSale"),
        Event::BridgeLock {
            chain,
            asset,
            amount,
            sender,
            recipient,
        } => serde_json::json!({
            "event": "BridgeLock",
            "chain": chain,
            "asset": asset,
            "amount": amount,
            "amount_display": *amount as f64 / 1_000_000.0,
            "sender": sender,
            "recipient": recipient.to_base58(),
        }),
        Event::BridgeMint {
            chain,
            asset,
            amount,
            recipient,
            tx_hash,
        } => serde_json::json!({
            "event": "BridgeMint",
            "chain": chain,
            "asset": asset,
            "amount": amount,
            "amount_display": *amount as f64 / 1_000_000.0,
            "recipient": recipient.to_base58(),
            "tx_hash": tx_hash,
        }),
        // ─── New event notifications ───
        Event::SignatureStatus { signature, status, slot, err } => serde_json::json!({
            "event": "SignatureStatus",
            "signature": signature,
            "status": status,
            "slot": slot,
            "error": err,
        }),
        Event::ValidatorUpdate { pubkey, event_kind, stake, slot } => serde_json::json!({
            "event": "ValidatorUpdate",
            "pubkey": pubkey.to_base58(),
            "kind": event_kind,
            "stake": stake,
            "stake_display": *stake as f64 / 1_000_000_000.0,
            "slot": slot,
        }),
        Event::TokenBalanceChange { owner, mint, old_balance, new_balance, slot } => serde_json::json!({
            "event": "TokenBalanceChange",
            "owner": owner.to_base58(),
            "mint": mint.to_base58(),
            "old_balance": old_balance,
            "new_balance": new_balance,
            "delta": (*new_balance as i128 - *old_balance as i128),
            "slot": slot,
        }),
        Event::EpochChange { epoch, slot, total_stake, validator_count } => serde_json::json!({
            "event": "EpochChange",
            "epoch": epoch,
            "slot": slot,
            "total_stake": total_stake,
            "total_stake_display": *total_stake as f64 / 1_000_000_000.0,
            "validator_count": validator_count,
        }),
        Event::GovernanceEvent { proposal_id, event_kind, voter, vote_weight, slot } => serde_json::json!({
            "event": "GovernanceEvent",
            "proposal_id": proposal_id,
            "kind": event_kind,
            "voter": voter.as_ref().map(|p| p.to_base58()),
            "vote_weight": vote_weight,
            "slot": slot,
        }),
    };

    Notification {
        jsonrpc: "2.0".to_string(),
        method: "subscription".to_string(),
        params: NotificationParams {
            subscription: sub_id,
            result,
        },
    }
}

// Re-export for use in other modules
use futures_util::stream::StreamExt;
use futures_util::SinkExt;

#[cfg(test)]
mod tests {
    #[test]
    fn test_subscription_manager() {
        // Just ensure the module compiles
    }
}

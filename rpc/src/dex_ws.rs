// ═══════════════════════════════════════════════════════════════════════════════
// MoltChain RPC — DEX WebSocket Feeds
// Real-time market data: orderbook, trades, ticker, candles, user events
//
// Extends the existing ws.rs subscription system with DEX-specific channels:
//   orderbook:<pair_id>     — L2 order book snapshots
//   trades:<pair_id>        — Trade stream
//   ticker:<pair_id>        — 1s price ticker
//   candles:<pair_id>:<tf>  — Candle updates
//   orders:<trader_addr>    — User order status updates
//   positions:<trader_addr> — Margin position updates
// ═══════════════════════════════════════════════════════════════════════════════

use serde::Serialize;
use tokio::sync::broadcast;

// ─────────────────────────────────────────────────────────────────────────────
// DEX Event Types
// ─────────────────────────────────────────────────────────────────────────────

/// DEX-specific events broadcast to WebSocket subscribers
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum DexEvent {
    /// Order book snapshot for a pair
    OrderBookUpdate {
        pair_id: u64,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        slot: u64,
    },
    /// New trade executed
    TradeExecution {
        trade_id: u64,
        pair_id: u64,
        price: f64,
        quantity: u64,
        side: String,
        slot: u64,
    },
    /// Ticker update (1s interval)
    TickerUpdate {
        pair_id: u64,
        last_price: f64,
        bid: f64,
        ask: f64,
        volume_24h: u64,
        change_24h: f64,
    },
    /// Candle update
    CandleUpdate {
        pair_id: u64,
        interval: u64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: u64,
        slot: u64,
    },
    /// User order status change
    OrderUpdate {
        order_id: u64,
        trader: String,
        status: String,
        filled: u64,
        remaining: u64,
        slot: u64,
    },
    /// Margin position update
    PositionUpdate {
        position_id: u64,
        trader: String,
        status: String,
        unrealized_pnl: i64,
        margin_ratio: f64,
        slot: u64,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct PriceLevel {
    pub price: f64,
    pub quantity: u64,
    pub orders: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// DEX Subscription Types
// ─────────────────────────────────────────────────────────────────────────────

/// Parsed DEX subscription channel
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DexChannel {
    OrderBook(u64),        // orderbook:<pair_id>
    Trades(u64),           // trades:<pair_id>
    Ticker(u64),           // ticker:<pair_id>
    Candles(u64, u64),     // candles:<pair_id>:<interval>
    UserOrders(String),    // orders:<trader_addr>
    UserPositions(String), // positions:<trader_addr>
}

impl DexChannel {
    /// Parse a channel string into a DexChannel
    pub fn parse(channel: &str) -> Option<DexChannel> {
        let parts: Vec<&str> = channel.split(':').collect();
        match parts.first().copied()? {
            "orderbook" => {
                let pair_id = parts.get(1)?.parse().ok()?;
                Some(DexChannel::OrderBook(pair_id))
            }
            "trades" => {
                let pair_id = parts.get(1)?.parse().ok()?;
                Some(DexChannel::Trades(pair_id))
            }
            "ticker" => {
                let pair_id = parts.get(1)?.parse().ok()?;
                Some(DexChannel::Ticker(pair_id))
            }
            "candles" => {
                let pair_id = parts.get(1)?.parse().ok()?;
                let interval = parts.get(2)?.parse().ok()?;
                Some(DexChannel::Candles(pair_id, interval))
            }
            "orders" => {
                let addr = parts.get(1)?.to_string();
                Some(DexChannel::UserOrders(addr))
            }
            "positions" => {
                let addr = parts.get(1)?.to_string();
                Some(DexChannel::UserPositions(addr))
            }
            _ => None,
        }
    }

    /// Convert back to channel string
    pub fn channel_string(&self) -> String {
        match self {
            DexChannel::OrderBook(p) => format!("orderbook:{}", p),
            DexChannel::Trades(p) => format!("trades:{}", p),
            DexChannel::Ticker(p) => format!("ticker:{}", p),
            DexChannel::Candles(p, i) => format!("candles:{}:{}", p, i),
            DexChannel::UserOrders(a) => format!("orders:{}", a),
            DexChannel::UserPositions(a) => format!("positions:{}", a),
        }
    }

    /// Check if a DexEvent matches this channel
    pub fn matches(&self, event: &DexEvent) -> bool {
        match (self, event) {
            (DexChannel::OrderBook(p), DexEvent::OrderBookUpdate { pair_id, .. }) => p == pair_id,
            (DexChannel::Trades(p), DexEvent::TradeExecution { pair_id, .. }) => p == pair_id,
            (DexChannel::Ticker(p), DexEvent::TickerUpdate { pair_id, .. }) => p == pair_id,
            (
                DexChannel::Candles(p, i),
                DexEvent::CandleUpdate {
                    pair_id, interval, ..
                },
            ) => p == pair_id && i == interval,
            (DexChannel::UserOrders(a), DexEvent::OrderUpdate { trader, .. }) => a == trader,
            (DexChannel::UserPositions(a), DexEvent::PositionUpdate { trader, .. }) => a == trader,
            _ => false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DEX Event Broadcasting
// ─────────────────────────────────────────────────────────────────────────────

/// DEX event broadcaster. Created once and shared across the RPC server.
/// Contract event hooks push DexEvents into this broadcaster.
pub struct DexEventBroadcaster {
    sender: broadcast::Sender<DexEvent>,
}

impl DexEventBroadcaster {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        DexEventBroadcaster { sender }
    }

    /// Broadcast a DEX event to all subscribers
    pub fn broadcast(&self, event: DexEvent) {
        let _ = self.sender.send(event);
    }

    /// Create a new receiver for DEX events
    pub fn subscribe(&self) -> broadcast::Receiver<DexEvent> {
        self.sender.subscribe()
    }

    // Convenience methods for common events

    /// Broadcast a trade execution
    pub fn emit_trade(
        &self,
        trade_id: u64,
        pair_id: u64,
        price: f64,
        quantity: u64,
        side: &str,
        slot: u64,
    ) {
        self.broadcast(DexEvent::TradeExecution {
            trade_id,
            pair_id,
            price,
            quantity,
            side: side.to_string(),
            slot,
        });
    }

    /// Broadcast an order book update
    pub fn emit_orderbook(
        &self,
        pair_id: u64,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        slot: u64,
    ) {
        self.broadcast(DexEvent::OrderBookUpdate {
            pair_id,
            bids,
            asks,
            slot,
        });
    }

    /// Broadcast a ticker update
    pub fn emit_ticker(
        &self,
        pair_id: u64,
        last_price: f64,
        bid: f64,
        ask: f64,
        volume_24h: u64,
        change_24h: f64,
    ) {
        self.broadcast(DexEvent::TickerUpdate {
            pair_id,
            last_price,
            bid,
            ask,
            volume_24h,
            change_24h,
        });
    }

    /// Broadcast a candle update
    #[allow(clippy::too_many_arguments)]
    pub fn emit_candle(
        &self,
        pair_id: u64,
        interval: u64,
        o: f64,
        h: f64,
        l: f64,
        c: f64,
        v: u64,
        slot: u64,
    ) {
        self.broadcast(DexEvent::CandleUpdate {
            pair_id,
            interval,
            open: o,
            high: h,
            low: l,
            close: c,
            volume: v,
            slot,
        });
    }

    /// Broadcast an order status update
    pub fn emit_order_update(
        &self,
        order_id: u64,
        trader: &str,
        status: &str,
        filled: u64,
        remaining: u64,
        slot: u64,
    ) {
        self.broadcast(DexEvent::OrderUpdate {
            order_id,
            trader: trader.to_string(),
            status: status.to_string(),
            filled,
            remaining,
            slot,
        });
    }

    /// Broadcast a margin position update
    pub fn emit_position_update(
        &self,
        position_id: u64,
        trader: &str,
        status: &str,
        unrealized_pnl: i64,
        margin_ratio: f64,
        slot: u64,
    ) {
        self.broadcast(DexEvent::PositionUpdate {
            position_id,
            trader: trader.to_string(),
            status: status.to_string(),
            unrealized_pnl,
            margin_ratio,
            slot,
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WebSocket Message Handling
// ─────────────────────────────────────────────────────────────────────────────

/// Process a DEX WebSocket subscription request.
/// Returns the channel string if valid, None otherwise.
pub fn handle_dex_subscribe(channel: &str) -> Option<String> {
    DexChannel::parse(channel).map(|c| c.channel_string())
}

/// Format a DexEvent into a JSON notification message
pub fn format_dex_notification(channel: &str, event: &DexEvent) -> serde_json::Value {
    serde_json::json!({
        "channel": channel,
        "data": event,
    })
}

/// Check if a channel string is a DEX channel
pub fn is_dex_channel(channel: &str) -> bool {
    DexChannel::parse(channel).is_some()
}

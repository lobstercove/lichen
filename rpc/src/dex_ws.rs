// ═══════════════════════════════════════════════════════════════════════════════
// Lichen RPC — DEX WebSocket Feeds
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
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── DexChannel parsing ──

    #[test]
    fn parse_orderbook() {
        assert_eq!(
            DexChannel::parse("orderbook:1"),
            Some(DexChannel::OrderBook(1))
        );
        assert_eq!(
            DexChannel::parse("orderbook:0"),
            Some(DexChannel::OrderBook(0))
        );
        assert_eq!(
            DexChannel::parse("orderbook:999"),
            Some(DexChannel::OrderBook(999))
        );
    }

    #[test]
    fn parse_trades() {
        assert_eq!(DexChannel::parse("trades:42"), Some(DexChannel::Trades(42)));
    }

    #[test]
    fn parse_ticker() {
        assert_eq!(DexChannel::parse("ticker:7"), Some(DexChannel::Ticker(7)));
    }

    #[test]
    fn parse_candles() {
        assert_eq!(
            DexChannel::parse("candles:1:60"),
            Some(DexChannel::Candles(1, 60))
        );
        assert_eq!(
            DexChannel::parse("candles:5:3600"),
            Some(DexChannel::Candles(5, 3600))
        );
    }

    #[test]
    fn parse_user_orders() {
        assert_eq!(
            DexChannel::parse("orders:ABC123"),
            Some(DexChannel::UserOrders("ABC123".to_string()))
        );
    }

    #[test]
    fn parse_user_positions() {
        assert_eq!(
            DexChannel::parse("positions:XYZ"),
            Some(DexChannel::UserPositions("XYZ".to_string()))
        );
    }

    #[test]
    fn parse_invalid_channels() {
        assert!(DexChannel::parse("").is_none());
        assert!(DexChannel::parse("unknown:1").is_none());
        assert!(DexChannel::parse("orderbook").is_none());
        assert!(DexChannel::parse("orderbook:abc").is_none());
        assert!(DexChannel::parse("candles:1").is_none()); // missing interval
        assert!(DexChannel::parse("candles:abc:60").is_none());
    }

    // ── DexChannel::channel_string round-trip ──

    #[test]
    fn channel_string_roundtrip() {
        let channels = vec![
            DexChannel::OrderBook(1),
            DexChannel::Trades(2),
            DexChannel::Ticker(3),
            DexChannel::Candles(4, 60),
            DexChannel::UserOrders("addr1".to_string()),
            DexChannel::UserPositions("addr2".to_string()),
        ];
        for ch in channels {
            let s = ch.channel_string();
            let parsed = DexChannel::parse(&s).unwrap_or_else(|| panic!("Failed to parse '{}'", s));
            assert_eq!(parsed, ch);
        }
    }

    // ── DexChannel::matches ──

    #[test]
    fn matches_orderbook() {
        let ch = DexChannel::OrderBook(1);
        assert!(ch.matches(&DexEvent::OrderBookUpdate {
            pair_id: 1,
            bids: vec![],
            asks: vec![],
            slot: 0
        }));
        assert!(!ch.matches(&DexEvent::OrderBookUpdate {
            pair_id: 2,
            bids: vec![],
            asks: vec![],
            slot: 0
        }));
    }

    #[test]
    fn matches_trades() {
        let ch = DexChannel::Trades(5);
        assert!(ch.matches(&DexEvent::TradeExecution {
            trade_id: 1,
            pair_id: 5,
            price: 1.0,
            quantity: 100,
            side: "buy".into(),
            slot: 0
        }));
        assert!(!ch.matches(&DexEvent::TradeExecution {
            trade_id: 1,
            pair_id: 6,
            price: 1.0,
            quantity: 100,
            side: "buy".into(),
            slot: 0
        }));
    }

    #[test]
    fn matches_ticker() {
        let ch = DexChannel::Ticker(3);
        assert!(ch.matches(&DexEvent::TickerUpdate {
            pair_id: 3,
            last_price: 1.0,
            bid: 0.99,
            ask: 1.01,
            volume_24h: 1000,
            change_24h: 0.05
        }));
        assert!(!ch.matches(&DexEvent::TickerUpdate {
            pair_id: 4,
            last_price: 1.0,
            bid: 0.99,
            ask: 1.01,
            volume_24h: 1000,
            change_24h: 0.05
        }));
    }

    #[test]
    fn matches_candles_both_pair_and_interval() {
        let ch = DexChannel::Candles(1, 60);
        assert!(ch.matches(&DexEvent::CandleUpdate {
            pair_id: 1,
            interval: 60,
            open: 1.0,
            high: 1.1,
            low: 0.9,
            close: 1.05,
            volume: 100,
            slot: 0
        }));
        assert!(!ch.matches(&DexEvent::CandleUpdate {
            pair_id: 1,
            interval: 3600,
            open: 1.0,
            high: 1.1,
            low: 0.9,
            close: 1.05,
            volume: 100,
            slot: 0
        }));
        assert!(!ch.matches(&DexEvent::CandleUpdate {
            pair_id: 2,
            interval: 60,
            open: 1.0,
            high: 1.1,
            low: 0.9,
            close: 1.05,
            volume: 100,
            slot: 0
        }));
    }

    #[test]
    fn matches_user_orders() {
        let ch = DexChannel::UserOrders("alice".to_string());
        assert!(ch.matches(&DexEvent::OrderUpdate {
            order_id: 1,
            trader: "alice".into(),
            status: "filled".into(),
            filled: 100,
            remaining: 0,
            slot: 0
        }));
        assert!(!ch.matches(&DexEvent::OrderUpdate {
            order_id: 1,
            trader: "bob".into(),
            status: "filled".into(),
            filled: 100,
            remaining: 0,
            slot: 0
        }));
    }

    #[test]
    fn matches_user_positions() {
        let ch = DexChannel::UserPositions("alice".to_string());
        assert!(ch.matches(&DexEvent::PositionUpdate {
            position_id: 1,
            trader: "alice".into(),
            status: "open".into(),
            unrealized_pnl: 500,
            margin_ratio: 2.0,
            slot: 0
        }));
        assert!(!ch.matches(&DexEvent::PositionUpdate {
            position_id: 1,
            trader: "bob".into(),
            status: "open".into(),
            unrealized_pnl: 500,
            margin_ratio: 2.0,
            slot: 0
        }));
    }

    #[test]
    fn cross_type_never_matches() {
        let ch = DexChannel::OrderBook(1);
        assert!(!ch.matches(&DexEvent::TradeExecution {
            trade_id: 1,
            pair_id: 1,
            price: 1.0,
            quantity: 100,
            side: "buy".into(),
            slot: 0
        }));
        assert!(!ch.matches(&DexEvent::TickerUpdate {
            pair_id: 1,
            last_price: 1.0,
            bid: 0.99,
            ask: 1.01,
            volume_24h: 1000,
            change_24h: 0.05
        }));
    }

    // ── DexEventBroadcaster ──

    #[test]
    fn broadcaster_new_subscribe() {
        let bc = DexEventBroadcaster::new(16);
        let _rx = bc.subscribe();
    }

    #[tokio::test]
    async fn broadcaster_emit_trade() {
        let bc = DexEventBroadcaster::new(16);
        let mut rx = bc.subscribe();
        bc.emit_trade(1, 2, 1.5, 100, "buy", 10);
        let event = rx.recv().await.unwrap();
        match event {
            DexEvent::TradeExecution {
                trade_id,
                pair_id,
                price,
                quantity,
                side,
                slot,
            } => {
                assert_eq!(trade_id, 1);
                assert_eq!(pair_id, 2);
                assert!((price - 1.5).abs() < 1e-9);
                assert_eq!(quantity, 100);
                assert_eq!(side, "buy");
                assert_eq!(slot, 10);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn broadcaster_emit_orderbook() {
        let bc = DexEventBroadcaster::new(16);
        let mut rx = bc.subscribe();
        bc.emit_orderbook(
            1,
            vec![PriceLevel {
                price: 100.0,
                quantity: 5,
                orders: 2,
            }],
            vec![],
            50,
        );
        let event = rx.recv().await.unwrap();
        match event {
            DexEvent::OrderBookUpdate {
                pair_id,
                bids,
                asks,
                slot,
            } => {
                assert_eq!(pair_id, 1);
                assert_eq!(bids.len(), 1);
                assert_eq!(asks.len(), 0);
                assert_eq!(slot, 50);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn broadcaster_emit_ticker() {
        let bc = DexEventBroadcaster::new(16);
        let mut rx = bc.subscribe();
        bc.emit_ticker(1, 100.0, 99.5, 100.5, 50000, 2.5);
        let event = rx.recv().await.unwrap();
        match event {
            DexEvent::TickerUpdate {
                pair_id,
                last_price,
                bid: _,
                ask: _,
                volume_24h,
                change_24h: _,
            } => {
                assert_eq!(pair_id, 1);
                assert!((last_price - 100.0).abs() < 1e-9);
                assert_eq!(volume_24h, 50000);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn broadcaster_emit_candle() {
        let bc = DexEventBroadcaster::new(16);
        let mut rx = bc.subscribe();
        bc.emit_candle(1, 60, 1.0, 1.1, 0.9, 1.05, 500, 100);
        let event = rx.recv().await.unwrap();
        match event {
            DexEvent::CandleUpdate {
                pair_id,
                interval,
                open: _,
                high: _,
                low: _,
                close: _,
                volume,
                slot,
            } => {
                assert_eq!(pair_id, 1);
                assert_eq!(interval, 60);
                assert_eq!(volume, 500);
                assert_eq!(slot, 100);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn broadcaster_emit_order_update() {
        let bc = DexEventBroadcaster::new(16);
        let mut rx = bc.subscribe();
        bc.emit_order_update(1, "alice", "filled", 100, 0, 200);
        let event = rx.recv().await.unwrap();
        match event {
            DexEvent::OrderUpdate {
                order_id,
                trader,
                status,
                filled,
                remaining,
                slot: _,
            } => {
                assert_eq!(order_id, 1);
                assert_eq!(trader, "alice");
                assert_eq!(status, "filled");
                assert_eq!(filled, 100);
                assert_eq!(remaining, 0);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn broadcaster_emit_position_update() {
        let bc = DexEventBroadcaster::new(16);
        let mut rx = bc.subscribe();
        bc.emit_position_update(1, "bob", "liquidated", -500, 0.5, 300);
        let event = rx.recv().await.unwrap();
        match event {
            DexEvent::PositionUpdate {
                position_id,
                trader,
                status,
                unrealized_pnl,
                margin_ratio: _,
                slot: _,
            } => {
                assert_eq!(position_id, 1);
                assert_eq!(trader, "bob");
                assert_eq!(status, "liquidated");
                assert_eq!(unrealized_pnl, -500);
            }
            _ => panic!("Wrong event type"),
        }
    }

    // ── Helper functions ──

    #[test]
    fn is_dex_channel_valid() {
        assert!(is_dex_channel("orderbook:1"));
        assert!(is_dex_channel("trades:5"));
        assert!(is_dex_channel("ticker:3"));
        assert!(is_dex_channel("candles:1:60"));
        assert!(is_dex_channel("orders:alice"));
        assert!(is_dex_channel("positions:bob"));
    }

    #[test]
    fn is_dex_channel_invalid() {
        assert!(!is_dex_channel(""));
        assert!(!is_dex_channel("blocks"));
        assert!(!is_dex_channel("unknown:1"));
    }

    #[test]
    fn handle_dex_subscribe_valid() {
        assert_eq!(
            handle_dex_subscribe("orderbook:1"),
            Some("orderbook:1".to_string())
        );
        assert_eq!(
            handle_dex_subscribe("trades:5"),
            Some("trades:5".to_string())
        );
    }

    #[test]
    fn handle_dex_subscribe_invalid() {
        assert!(handle_dex_subscribe("invalid").is_none());
    }

    #[test]
    fn format_dex_notification_structure() {
        let event = DexEvent::TradeExecution {
            trade_id: 1,
            pair_id: 2,
            price: 100.0,
            quantity: 50,
            side: "sell".to_string(),
            slot: 10,
        };
        let json = format_dex_notification("trades:2", &event);
        assert_eq!(json["channel"], "trades:2");
        assert!(json["data"].is_object());
        assert_eq!(json["data"]["type"], "tradeExecution");
    }

    #[test]
    fn ticker_field_names_check() {
        let event = DexEvent::TickerUpdate {
            pair_id: 7,
            last_price: 6274.0,
            bid: 6273.0,
            ask: 6275.0,
            volume_24h: 500,
            change_24h: 1.5,
        };
        let json = serde_json::to_value(&event).unwrap();
        // Fields must be camelCase to match the JS client expectations
        assert!(
            json.get("lastPrice").is_some(),
            "lastPrice field must be camelCase"
        );
        assert!(
            json.get("pairId").is_some(),
            "pairId field must be camelCase"
        );
        assert!(
            json.get("volume24h").is_some(),
            "volume24h field must be camelCase"
        );
        assert!(
            json.get("change24h").is_some(),
            "change24h field must be camelCase"
        );
        // Variant tag must also be camelCase
        assert_eq!(json["type"], "tickerUpdate");
    }
}

# MoltyDEX REST API Reference

**Base URL**: `http://localhost:8899/api/v1`

All responses follow the format:
```json
{ "data": <result>, "error": null }
// or
{ "data": null, "error": "description" }
```

---

## Trading Pairs

### GET /pairs
List all trading pairs.

**Response**: `TradingPair[]`
```json
{
  "data": [
    {
      "id": 0,
      "baseName": "MOLT",
      "quoteName": "mUSD",
      "tickSize": "0.001",
      "lotSize": "0.0001",
      "minOrderSize": "1.0",
      "status": "active",
      "createdAt": 12345
    }
  ]
}
```

### GET /pairs/:id
Get a specific trading pair.

### POST /pairs
Create a new trading pair (admin only).

**Body**:
```json
{
  "baseName": "NEWTOKEN",
  "quoteName": "mUSD",
  "tickSize": 1000000,
  "lotSize": 100,
  "minOrderSize": 1000000
}
```

---

## Order Book

### GET /pairs/:id/orderbook?depth=20
Get order book for a pair.

**Query Parameters**:
| Param | Type | Default | Description |
|-------|------|---------|-------------|
| depth | number | 20 | Price levels per side |

**Response**:
```json
{
  "data": {
    "bids": [{ "price": "1.050", "quantity": "5000.0", "orders": 3 }],
    "asks": [{ "price": "1.055", "quantity": "3000.0", "orders": 2 }],
    "timestamp": 1706900000,
    "pairId": 0
  }
}
```

---

## Orders

### POST /orders
Place a new order.

**Body**:
```json
{
  "pairId": 0,
  "side": "buy",
  "orderType": "limit",
  "price": 1050000000,
  "quantity": 5000000000,
  "sender": "0xabc..."
}
```

**Response**:
```json
{
  "data": { "orderId": 42, "status": "open" }
}
```

### DELETE /orders/:id
Cancel an order.

### GET /orders?trader=0xabc...
Get orders for a trader.

**Query Parameters**:
| Param | Type | Description |
|-------|------|-------------|
| trader | string | Trader address (hex) |
| status | string | Filter: "open", "filled", "cancelled" |

### GET /orders/:id
Get a specific order.

**Response**:
```json
{
  "data": {
    "id": 42,
    "pairId": 0,
    "trader": "0xabc...",
    "side": "buy",
    "orderType": "limit",
    "price": "1.050",
    "quantity": "5000.0",
    "filled": "2000.0",
    "status": "partial",
    "timestamp": 1706900000
  }
}
```

---

## Trades

### GET /pairs/:id/trades?limit=50
Get recent trades for a pair.

**Query Parameters**:
| Param | Type | Default | Description |
|-------|------|---------|-------------|
| limit | number | 50 | Max trades to return |

**Response**:
```json
{
  "data": [
    {
      "id": 100,
      "pairId": 0,
      "price": "1.052",
      "quantity": "1000.0",
      "maker": "0xabc...",
      "taker": "0xdef...",
      "side": "buy",
      "timestamp": 1706900500
    }
  ]
}
```

---

## Ticker

### GET /pairs/:id/ticker
24h ticker for a pair.

**Response**:
```json
{
  "data": {
    "pairId": 0,
    "lastPrice": "1.052",
    "high24h": "1.100",
    "low24h": "0.980",
    "volume24h": "1500000.0",
    "priceChange24h": "0.032",
    "priceChangePct24h": "3.14"
  }
}
```

### GET /tickers
All pair tickers.

---

## Candles (OHLCV)

### GET /pairs/:id/candles?interval=3600&limit=100
Get candlestick data.

**Query Parameters**:
| Param | Type | Default | Description |
|-------|------|---------|-------------|
| interval | number | 3600 | Seconds per candle (60, 300, 900, 3600, 86400) |
| limit | number | 100 | Max candles |

**Response**:
```json
{
  "data": [
    {
      "timestamp": 1706900000,
      "open": "1.050",
      "high": "1.060",
      "low": "1.045",
      "close": "1.055",
      "volume": "50000.0"
    }
  ]
}
```

---

## AMM Pools

### GET /pools
List all liquidity pools.

### GET /pools/:id
Get pool details.

**Response**:
```json
{
  "data": {
    "id": 0,
    "pairId": 0,
    "sqrtPrice": "4294967296",
    "liquidity": "1000000000",
    "currentTick": 0,
    "feeRate": 30,
    "protocolFees": "500000",
    "totalLpShares": "100000000",
    "createdAt": 12345
  }
}
```

### GET /pools/:id/positions?owner=0xabc...
Get LP positions in a pool.

---

## Smart Router

### POST /router/swap
Execute a smart-routed swap.

**Body**:
```json
{
  "tokenIn": "MOLT",
  "tokenOut": "mUSD",
  "amountIn": 10000000000,
  "minAmountOut": 9500000000,
  "sender": "0xabc..."
}
```

### POST /router/quote
Get a swap quote (read-only).

**Body**: Same as `/router/swap` but no execution.

### GET /routes
List available routes.

---

## Margin Trading

### POST /margin/open
Open a leveraged position.

**Body**:
```json
{
  "pairId": 0,
  "side": "long",
  "collateral": 1000000000,
  "leverage": 5,
  "sender": "0xabc..."
}
```

### POST /margin/close
Close a margin position.

**Body**:
```json
{
  "positionId": 7,
  "sender": "0xabc..."
}
```

### POST /margin/add
Add collateral to a position.

**Body**:
```json
{
  "positionId": 7,
  "amount": 500000000,
  "sender": "0xabc..."
}
```

### GET /margin/positions?trader=0xabc...
Get all margin positions for a trader.

### GET /margin/positions/:id
Get a specific margin position.

### GET /margin/info
Get global margin info (insurance fund, etc).

---

## Analytics

### GET /leaderboard?limit=50
Top traders by PnL.

### GET /stats/:address
Trader statistics.

### GET /rewards/:address
Reward info for an address.

---

## Governance

### GET /proposals
List governance proposals.

### GET /proposals/:id
Get proposal details.

---

## WebSocket API

**URL**: `ws://localhost:8900/ws`

### Subscribe
```json
{
  "method": "subscribe",
  "params": { "channel": "trades:0" }
}
```

### Channels
| Channel | Format | Description |
|---------|--------|-------------|
| `orderbook:<pairId>` | `orderbook:0` | Order book updates |
| `trades:<pairId>` | `trades:0` | New trades |
| `ticker:<pairId>` | `ticker:0` | 24h ticker updates |
| `candles:<pairId>:<interval>` | `candles:0:3600` | OHLCV updates |
| `orders:<address>` | `orders:0xabc...` | User order updates |
| `positions:<address>` | `positions:0xabc...` | User position updates |

### Events
```json
{
  "channel": "trades:0",
  "event": "trade",
  "data": {
    "id": 100,
    "price": "1.052",
    "quantity": "1000.0",
    "side": "buy",
    "timestamp": 1706900500
  }
}
```

---

## Error Codes
| Code | Description |
|------|-------------|
| 400 | Bad request (invalid parameters) |
| 404 | Resource not found |
| 409 | Conflict (e.g., insufficient balance) |
| 429 | Rate limited |
| 500 | Internal server error |

## Rate Limits
| Endpoint | Limit |
|----------|-------|
| Read (GET) | 100 req/s per IP |
| Write (POST/DELETE) | 20 req/s per IP |
| WebSocket | 10 subscriptions per connection |

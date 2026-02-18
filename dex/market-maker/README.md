# MoltyDEX Market Maker Bot

Automated market making for MoltyDEX with two strategies: **Spread** (symmetric quoting) and **Grid** (range-bound).

## Quick Start

```bash
cd dex/market-maker
npm install
npm start              # spread strategy (default)
npm run start:grid     # grid strategy
```

## Strategies

### Spread Strategy
Places symmetric bid/ask orders around a reference price (from ticker). Features:
- Configurable half-spread and number of levels
- Skew adjustment — shifts quotes to reduce accumulated inventory
- Auto-cancel + refresh on configurable interval
- WebSocket-driven price updates for low latency

### Grid Strategy
Places buy orders below and sell orders above current price at fixed intervals. Features:
- Configurable price range and grid density
- Automatic order flipping on fills (buy fill → place sell, and vice versa)
- Minimum distance filter to avoid fills at current price

## Configuration

All config via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `DEX_ENDPOINT` | `http://localhost:8899` | DEX API URL |
| `DEX_WS_ENDPOINT` | `ws://localhost:8900` | WebSocket URL |
| `MM_PAIR_ID` | `0` | Trading pair ID |
| `MM_STRATEGY` | `spread` | Strategy: `spread` or `grid` |
| `MM_DRY_RUN` | `false` | Log only, no real orders |
| `MM_LOG_LEVEL` | `info` | Log verbosity |
| **Spread** | | |
| `MM_HALF_SPREAD_BPS` | `15` | Half-spread in basis points |
| `MM_LEVELS` | `5` | Price levels per side |
| `MM_SIZE_PER_LEVEL` | `1000` | Order size per level |
| `MM_LEVEL_STEP_BPS` | `5` | Step between levels (bps) |
| `MM_REFRESH_MS` | `2000` | Quote refresh interval |
| `MM_MAX_SKEW` | `10000` | Max position before full skew |
| **Grid** | | |
| `MM_GRID_LOW` | `0.80` | Lower price bound |
| `MM_GRID_HIGH` | `1.20` | Upper price bound |
| `MM_GRID_LEVELS` | `20` | Number of grid levels |
| `MM_GRID_SIZE` | `500` | Size per grid order |
| `MM_GRID_REFRESH_MS` | `5000` | Refresh interval |

## Example: Dry Run

```bash
MM_DRY_RUN=true MM_PAIR_ID=0 npm start
```

Output:
```
[Spread][DRY] BID 1.048500 x 1000 | ASK 1.051500 x 1000
[Spread][DRY] BID 1.048000 x 1000 | ASK 1.052000 x 1000
...
```

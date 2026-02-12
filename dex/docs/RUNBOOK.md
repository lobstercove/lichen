# MoltyDEX Operations Runbook

## Table of Contents
1. [Deployment](#deployment)
2. [Health Checks](#health-checks)
3. [Incident Response](#incident-response)
4. [Common Operations](#common-operations)
5. [Monitoring & Alerts](#monitoring--alerts)
6. [Disaster Recovery](#disaster-recovery)

---

## Deployment

### First-time Testnet Deploy
```bash
# Build all 26 contracts
./scripts/build-all-contracts.sh

# Deploy to testnet + create initial pairs/pools
./scripts/testnet-deploy.sh --seed-pairs --seed-pools

# Seed insurance fund (100K MOLT)
./scripts/seed-insurance-fund.sh --amount 100000
```

### Rolling Upgrade
```bash
# 1. Build new contract WASM
cd contracts/<contract_name>
cargo build --target wasm32-unknown-unknown --release

# 2. Deploy upgraded contract (state preserved)
./scripts/testnet-deploy.sh --skip-build

# 3. Verify state continuity
curl -s http://localhost:8000/api/v1/pairs | jq '.data | length'
```

### RPC Server Restart
```bash
# Graceful shutdown (finishes in-flight requests)
kill -SIGTERM $(pgrep -f "moltchain-rpc")

# Wait for drain
sleep 5

# Restart
cd rpc && cargo run --release &

# Verify
curl -s http://localhost:8000/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"health"}'
```

---

## Health Checks

### Quick Health
```bash
# RPC health
curl -s http://localhost:8000/ -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"health"}' | jq .

# DEX API health
curl -s http://localhost:8000/api/v1/pairs | jq '.data | length'

# Active WebSocket connections
curl -s http://localhost:8000/metrics | grep ws_connections
```

### Deep Health Check
```bash
#!/bin/bash
echo "=== MoltyDEX Health Check ==="

# 1. RPC responding
echo -n "RPC: "
curl -sf http://localhost:8000/ -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"health"}' > /dev/null && echo "OK" || echo "FAIL"

# 2. DEX pairs exist
PAIRS=$(curl -sf http://localhost:8000/api/v1/pairs | python3 -c "import json,sys; print(len(json.load(sys.stdin).get('data',[])))") 
echo "Pairs: $PAIRS"

# 3. Block height advancing
SLOT1=$(curl -sf http://localhost:8000/ -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}' | python3 -c "import json,sys; print(json.load(sys.stdin).get('result',0))")
sleep 2
SLOT2=$(curl -sf http://localhost:8000/ -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}' | python3 -c "import json,sys; print(json.load(sys.stdin).get('result',0))")
echo "Slots: $SLOT1 → $SLOT2 (advancing: $([ $SLOT2 -gt $SLOT1 ] && echo YES || echo NO))"

# 4. Orderbook has depth
DEPTH=$(curl -sf "http://localhost:8000/api/v1/pairs/0/orderbook?depth=5" | python3 -c "
import json,sys
d = json.load(sys.stdin).get('data',{})
print(f\"bids={len(d.get('bids',[]))} asks={len(d.get('asks',[]))}\")")
echo "Orderbook: $DEPTH"

# 5. Insurance fund
FUND=$(curl -sf http://localhost:8000/api/v1/margin/info | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('insurance_fund',0))")
echo "Insurance fund: $FUND"
```

---

## Incident Response

### Market Emergency — Halt Trading
If anomalous activity is detected (oracle manipulation, flash loan attack, etc):

```bash
# 1. Pause all DEX contracts via governance
# This sets the `paused` flag in each contract's storage
for contract in dex_core dex_amm dex_router dex_margin; do
  curl -s http://localhost:8000/ -X POST -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"sendTransaction\",\"params\":{\"to\":\"$contract\",\"data\":\"0xFF\"}}"
done

# 2. Alert the team
echo "DEX HALTED at $(date)" >> /var/log/moltchain/incidents.log

# 3. Investigate — check recent transactions
curl -s http://localhost:8000/ -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getRecentTransactions","params":{"limit":100}}' | jq .
```

### Liquidation Cascade
If multiple margin positions are being liquidated simultaneously:

1. **Check insurance fund balance** — if depleted, halt new margin opens
2. **Review liquidation prices** — confirm oracle is not stale
3. **Increase maintenance margin** if needed via governance proposal

### High Latency
```bash
# Check RPC server resource usage
top -l 1 -s 0 | grep moltchain

# Check pending transaction pool
curl -s http://localhost:8000/ -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getPendingTransactionCount"}' | jq .

# If backlogged, consider temporarily rejecting new orders
```

---

## Common Operations

### Add New Trading Pair
```bash
# Deploy pair via dex_core::create_pair
# Args: base_name, quote_name, tick_size, lot_size, min_order_size
# Encoded as: opcode 0x01 + args

curl -s http://localhost:8000/api/v1/pairs \
  -X POST -H "Content-Type: application/json" \
  -d '{
    "baseName": "NEWTOKEN",
    "quoteName": "mUSD",
    "tickSize": 1000000,
    "lotSize": 100,
    "minOrderSize": 1000000
  }'
```

### Create AMM Pool for Pair
```bash
curl -s http://localhost:8000/api/v1/pools \
  -X POST -H "Content-Type: application/json" \
  -d '{
    "pairId": 4,
    "feeTier": 30,
    "initialSqrtPrice": 4294967296
  }'
```

### Top Up Insurance Fund
```bash
./scripts/seed-insurance-fund.sh --amount 500000
```

### Export Trade History
```bash
# Get last 1000 trades for a pair
curl -s "http://localhost:8000/api/v1/pairs/0/trades?limit=1000" | jq '.data' > trades-export.json
```

---

## Monitoring & Alerts

### Key Metrics to Track
| Metric | Warning | Critical | Check Command |
|--------|---------|----------|---------------|
| RPC latency p99 | >200ms | >500ms | Prometheus `rpc_latency_p99` |
| Order throughput | <50 rps | <10 rps | `dex_orders_per_second` |
| Insurance fund | <50K MOLT | <10K MOLT | `/api/v1/margin/info` |
| Slot height | stale >10s | stale >30s | `chain_slot_height` |
| WS connections | >5000 | >10000 | `ws_active_connections` |
| Pending txs | >1000 | >5000 | `pending_tx_count` |

### Prometheus Alert Rules
```yaml
groups:
  - name: moltydex
    rules:
      - alert: HighLatency
        expr: rpc_request_duration_seconds{quantile="0.99"} > 0.5
        for: 5m
        labels:
          severity: critical
      - alert: InsuranceFundLow
        expr: dex_insurance_fund_balance < 10000
        for: 1m
        labels:
          severity: critical
      - alert: SlotStale
        expr: time() - chain_last_slot_time > 30
        for: 1m
        labels:
          severity: critical
```

---

## Disaster Recovery

### Full State Backup
```bash
# Backup chain state
cp -r data/state-8000 data/state-backup-$(date +%Y%m%d)

# Backup configuration
cp openclaw.json openclaw.json.backup-$(date +%Y%m%d)
```

### Restore from Backup
```bash
# Stop node
kill $(pgrep -f "moltchain")

# Restore state
cp -r data/state-backup-YYYYMMDD data/state-8000

# Restart
./scripts/testnet-deploy.sh --skip-build
```

### Cross-chain Bridge Recovery
If custody bridge is stalled:
1. Check `custody/` logs for errors
2. Verify `CUSTODY_JUPITER_API_URL` and `CUSTODY_UNISWAP_ROUTER` env vars
3. Restart custody service: `cd custody && cargo run --release`

# MoltChain — Test Deployment Guide

How to reset, start, and verify a local 3-validator testnet from scratch.

---

## Quick Start (One Command)

```bash
cd /path/to/moltchain
./reset-blockchain.sh --restart
```

This does everything:
1. Kills all running moltchain processes
2. Wipes all state dirs (`data/state-8000`, `data/state-8001`, `data/state-8002`)
3. Clears validator keypairs, peer stores, genesis files
4. Starts V1 (creates genesis) → waits 8s → starts V2 → starts V3 → waits 10s
5. Logs to `/tmp/moltchain-v1.log`, `/tmp/moltchain-v2.log`, `/tmp/moltchain-v3.log`

---

## Step-by-Step (Manual)

### 1. Reset

```bash
./reset-blockchain.sh
```

Output:
```
[1/6] Killing all MoltChain processes...
[2/6] Removing blockchain state directories...
[3/6] Removing validator keypairs...
[4/6] Cleaning signer data, peer stores, genesis files...
[5/6] Verifying clean state...
Reset complete. Ready for fresh genesis.
```

### 2. Start Validator 1 (Genesis Creator)

```bash
./skills/validator/run-validator.sh testnet 1
```

Wait ~5 seconds for genesis creation. V1 will:
- Generate 2/3 multi-sig genesis wallet
- Create **6 whitepaper distribution accounts** (1B MOLT total):
  - `validator_rewards` (15%) — 150M MOLT — **TREASURY** (block rewards, fees, bootstraps)
  - `community_treasury` (40%) — 400M MOLT — governance-controlled
  - `builder_grants` (25%) — 250M MOLT — released as agents ship programs
  - `founding_moltys` (10%) — 100M MOLT — 6-month cliff + 18-month vest
  - `ecosystem_partnerships` (5%) — 50M MOLT — strategic partnerships
  - `reserve_pool` (5%) — 50M MOLT — reserve
- Save keypairs to `data/state-8000/genesis-keys/`
- Auto-deploy genesis contracts (MOLT, MUSD, WSOL)
- Start producing blocks

### 3. Start Validator 2

```bash
./skills/validator/run-validator.sh testnet 2
```

Bootstraps from V1 (`127.0.0.1:8000`), syncs genesis block.

### 4. Start Validator 3

```bash
./skills/validator/run-validator.sh testnet 3
```

Bootstraps from V1, syncs genesis block.

---

## Port Assignments

| Service | V1 | V2 | V3 |
|---------|-----|-----|-----|
| P2P | 8000 | 8001 | 8002 |
| RPC | 8899 | 8901 | 8903 |
| WebSocket | 8900 | 8902 | 8904 |
| Signer | 9201 | 9202 | 9203 |

| Service | Port |
|---------|------|
| Faucet | 9100 |
| Custody | 9105 |

---

## Verify Cluster

```bash
# Check cluster info
curl -s http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getClusterInfo"}' | python3 -m json.tool

# Expected: 3 validators, 2 peers, advancing slot number
```

```bash
# Check treasury/genesis info
curl -s http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTreasuryInfo"}' | python3 -m json.tool
```

---

## Start Faucet (Optional)

```bash
cd /path/to/moltchain
FAUCET_KEYPAIR=data/state-8000/genesis-keys/treasury-moltchain-testnet-1.json \
  RUST_LOG=info \
  ./target/release/moltchain-faucet
```

Serves on `http://localhost:9100`. Uses the treasury keypair to sign faucet transactions.

---

## Genesis Keypair Files

After genesis, these files are created in `data/state-8000/genesis-keys/`:

| File | Purpose |
|------|---------|
| `genesis-primary-moltchain-testnet-1.json` | Genesis signer 1 (primary) |
| `genesis-signer-1-moltchain-testnet-1.json` | Genesis signer 2 |
| `genesis-signer-2-moltchain-testnet-1.json` | Genesis signer 3 |
| `validator_rewards-moltchain-testnet-1.json` | Validator rewards wallet (150M) |
| `community_treasury-moltchain-testnet-1.json` | Community treasury wallet (400M) |
| `builder_grants-moltchain-testnet-1.json` | Builder grants wallet (250M) |
| `founding_moltys-moltchain-testnet-1.json` | Founding moltys wallet (100M) |
| `ecosystem_partnerships-moltchain-testnet-1.json` | Ecosystem partnerships wallet (50M) |
| `reserve_pool-moltchain-testnet-1.json` | Reserve pool wallet (50M) |
| `treasury-moltchain-testnet-1.json` | Treasury alias (= validator_rewards, for faucet compat) |

---

## Scripts Reference

| Script | Purpose |
|--------|---------|
| `./reset-blockchain.sh` | Reset all state (wrapper) |
| `./reset-blockchain.sh --restart` | Reset + auto-start 3 validators |
| `./skills/validator/run-validator.sh testnet <1\|2\|3>` | Start a specific validator |
| `./skills/validator/reset-blockchain.sh` | Actual reset implementation |

---

## Troubleshooting

**"Genesis state already exists"** — The state dirs weren't cleaned. Run `./reset-blockchain.sh` first.

**Faucet exits with code 101** — Missing `FAUCET_KEYPAIR` env var. Point it at the treasury keypair.

**V2/V3 can't sync** — V1 must be running and producing blocks before V2/V3 start. The `--restart` flag handles timing automatically.

**Reset script hangs** — If `find` commands hang, the script may be scanning large directories. Fixed in commit `c46ccf2`.

# MoltChain RPC API Reference

**Complete API Surface**: 24 Endpoints  
**Version**: 0.1.0  
**Protocol**: JSON-RPC 2.0  
**Default Port**: 8899  
**Status**: ✅ Production Ready

---

## 🔌 Connection

```bash
# Default endpoint
http://localhost:8899/

# Custom endpoint (CLI)
molt --rpc-url http://custom-host:8899 <command>
```

## 🔁 Compatibility Adapters

MoltChain exposes canonical RPC methods on the root endpoint and provides
compatibility adapters on dedicated routes for external tooling.

- Canonical MoltChain RPC: `http://localhost:8899/`
- Solana compatibility: `http://localhost:8899/solana`
- EVM compatibility: `http://localhost:8899/evm`

## ✅ Release-Verified Operator Baseline (Feb 24, 2026)

This baseline is kept consistent with `skills/validator/SKILL.md`,
`developers/rpc-reference.html`, and `developers/ws-reference.html`.

- Canonical JSON-RPC endpoint: `http://localhost:8899`
- Canonical WebSocket endpoint: `ws://localhost:8900`

### Core RPC baseline

`health`, `getSlot`, `getValidators`, `getChainStatus`, `getNetworkInfo`

### Staking/economics RPC baseline

`getStakingStatus`, `getStakingRewards`, `getTreasuryInfo`, `getGenesisAccounts`, `getTotalBurned`, `getReefStakePoolInfo`

### Core WebSocket baseline

`subscribeSlots`, `subscribeBlocks`, `subscribeTransactions`, `subscribeAccount`, `subscribeLogs`, `subscribeValidators`, `subscribeDex`, `subscribePrediction`

---

## 🧰 Custody Service (REST)

The custody service runs separately from the validator RPC server and exposes
REST endpoints on port 9105 by default.

Base URL: `http://localhost:9105`

### `GET /health`
Basic health check.

**Returns**:
```json
{
  "status": "ok"
}
```

### `GET /status`
Operational status for sweeps and credits.

**Returns**:
```json
{
  "signers": {
    "configured": 3,
    "threshold": 2
  },
  "sweeps": {
    "total": 4,
    "by_status": {
      "queued": 1,
      "signed": 1,
      "sweep_submitted": 2
    }
  },
  "credits": {
    "total": 2,
    "by_status": {
      "queued": 1,
      "submitted": 1
    }
  }
}
```

### `POST /deposits`
Issue a one-time deposit address.

**Body**:
```json
{
  "user_id": "<molt_address>",
  "chain": "solana",
  "asset": "sol"
}
```

**Returns**:
```json
{
  "deposit_id": "uuid",
  "address": "<chain_address>"
}
```

### `GET /deposits/:deposit_id`
Fetch a deposit request.

---

## 📚 All Endpoints

### Basic Queries (11 endpoints)

#### `getBalance`
Get account balance in shells and MOLT.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "shells": 1000000000,
  "molt": 1
}
```

#### `getAccount`
Get complete account information.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "shells": 1000000000,
  "owner": "System...",
  "executable": false,
  "data": "0x..."
}
```

#### `getBlock`
Get block by slot number.

**Params**: `[slot: number]`  
**Returns**: Block data with transactions

#### `getLatestBlock`
Get most recent block.

**Params**: None  
**Returns**: Latest block data

#### `getSlot`
Get current slot number.

**Params**: None  
**Returns**: `{ "slot": 12345 }`

#### `getTransaction`
Get transaction by signature.

**Params**: `[signature: string]`  
**Returns**: Transaction data

#### `sendTransaction`
Submit transaction to mempool.

**Params**: `[transaction: base58]`  
**Returns**: `{ "signature": "..." }`

#### `getTotalBurned`
Get total MOLT burned.

**Params**: None  
**Returns**: `{ "shells": 1000000, "molt": 0.001 }`

#### `getValidators`
List all validators.

**Params**: None  
**Returns**:
```json
{
  "validators": [
    {
      "pubkey": "...",
      "stake": 100000000000,
      "reputation": 985,
      "blocks_proposed": 150,
      "votes_cast": 1200,
      "correct_votes": 1185,
      "last_active_slot": 12340
    }
  ],
  "count": 5
}
```

#### `getMetrics`
Get performance metrics.

**Params**: None  
**Returns**:
```json
{
  "tps": 1250.5,
  "total_transactions": 1500000,
  "total_blocks": 12345,
  "average_block_time": 0.4
}
```

#### `health`
Health check endpoint.

**Params**: None  
**Returns**: `{ "status": "ok" }`

---

### Network Endpoints (2 endpoints)

#### `getPeers`
List connected P2P peers.

**Params**: None  
**Returns**:
```json
{
  "peers": [
    {
      "peer_id": "12D3KooW...",
      "address": "/ip4/127.0.0.1/tcp/8001",
      "connected_since": 1706882400,
      "last_seen": 1706885600
    }
  ],
  "count": 1
}
```

#### `getNetworkInfo`
Get network metadata.

**Params**: None  
**Returns**:
```json
{
  "chain_id": "moltchain-mainnet",
  "network_id": "molt-1",
  "version": "0.1.0",
  "current_slot": 12345,
  "validator_count": 5,
  "peer_count": 1
}
```

---

### Validator Endpoints (3 endpoints)

#### `getValidatorInfo`
Get detailed validator information.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "pubkey": "...",
  "stake": 100000000000,
  "reputation": 985,
  "blocks_proposed": 150,
  "votes_cast": 1200,
  "correct_votes": 1185,
  "last_active_slot": 12340,
  "commission_rate": 0,
  "is_active": true
}
```

#### `getValidatorPerformance`
Get validator performance metrics.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "pubkey": "...",
  "blocks_proposed": 150,
  "votes_cast": 1200,
  "correct_votes": 1185,
  "vote_accuracy": 98.75,
  "reputation": 985,
  "uptime": 99.5
}
```

#### `getChainStatus`
Get comprehensive chain status.

**Params**: None  
**Returns**:
```json
{
  "current_slot": 12345,
  "validator_count": 5,
  "total_stake": 500000000000,
  "tps": 1250.5,
  "total_transactions": 1500000,
  "total_blocks": 12345,
  "average_block_time": 0.4,
  "is_healthy": true
}
```

---

### Staking Endpoints (4 endpoints)

#### `stake`
Create stake transaction.

**Params**: `[from: string, validator: string, amount: number]`  
**Returns**:
```json
{
  "signature": "StakeTx...",
  "amount": 100000000000,
  "validator": "...",
  "status": "pending"
}
```

#### `unstake`
Create unstake transaction.

**Params**: `[from: string, validator: string, amount: number]`  
**Returns**:
```json
{
  "signature": "UnstakeTx...",
  "amount": 100000000000,
  "validator": "...",
  "status": "pending",
  "unlock_epoch": 1706972400
}
```

#### `getStakingStatus`
Get account staking status.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "is_validator": true,
  "total_staked": 100000000000,
  "delegations": [],
  "status": "active"
}
```

#### `getStakingRewards`
Get staking rewards.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "total_rewards": 0,
  "pending_rewards": 0,
  "claimed_rewards": 0,
  "reward_rate": 5.0
}
```

#### `getReefStakePoolInfo`
Get ReefStake pool-level metrics.

**Params**: `[]`  
**Returns**:
```json
{
  "total_supply_st_molt": 0,
  "total_molt_staked": 0,
  "exchange_rate": 1.0,
  "total_validators": 3,
  "average_apr": 0.0
}
```

#### `getTreasuryInfo`
Get treasury address and treasury balances.

**Params**: `[]`  
**Returns**:
```json
{
  "treasury_pubkey": "...",
  "treasury_balance": 0,
  "treasury_balance_molt": 0.0
}
```

#### `getGenesisAccounts`
List genesis accounts and allocation metadata.

**Params**: `[]`  
**Returns**:
```json
{
  "accounts": [
    {
      "role": "genesis",
      "pubkey": "...",
      "amount_molt": 0
    }
  ]
}
```

---

### Account Endpoints (2 endpoints)

#### `getAccountInfo`
Get enhanced account information.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "pubkey": "...",
  "balance": 1000000000,
  "molt": 1,
  "exists": true,
  "is_validator": false,
  "is_executable": false
}
```

#### `getTransactionHistory`
Get account transaction history.

**Params**: `[pubkey: string, limit?: number]`  
**Returns**:
```json
{
  "transactions": [],
  "count": 0,
  "limit": 10
}
```

---

### Contract Endpoints (3 endpoints)

#### `getContractInfo`
Get contract metadata.

**Params**: `[contract_id: string]`  
**Returns**:
```json
{
  "contract_id": "...",
  "owner": "...",
  "code_size": 4096,
  "is_executable": true,
  "deployed_at": 0
}
```

#### `getContractLogs`
Get contract execution logs.

**Params**: `[contract_id: string]`  
**Returns**:
```json
{
  "logs": [],
  "count": 0
}
```

#### `getAllContracts`
List all deployed contracts.

**Params**: None  
**Returns**:
```json
{
  "contracts": [],
  "count": 0
}
```

---

## 💡 Usage Examples

### cURL
```bash
# Health check
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'

# Get network info
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getNetworkInfo","params":[]}'

# Get balance
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["YOUR_PUBKEY"]}'
```

### CLI
```bash
# Network operations
molt network status
molt network peers
molt network info

# Validator operations
molt validator info <pubkey>
molt validator performance <pubkey>
molt validator list

# Staking operations
molt stake add <from> <validator> <amount>
molt stake remove <from> <validator> <amount>
molt stake status <pubkey>
molt stake rewards <pubkey>

# Account operations
molt account info <pubkey>
molt account history <pubkey> 10

# Contract operations
molt contract info <contract_id>
molt contract logs <contract_id>
molt contract list
```

### JavaScript (using fetch)
```javascript
async function rpcCall(method, params = []) {
  const response = await fetch('http://localhost:8899/', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method,
      params
    })
  });
  
  const data = await response.json();
  return data.result;
}

// Get network info
const networkInfo = await rpcCall('getNetworkInfo');
console.log(networkInfo);

// Get balance
const balance = await rpcCall('getBalance', ['YOUR_PUBKEY']);
console.log(balance);
```

---

## 🔒 Error Codes

Standard JSON-RPC 2.0 error codes:

| Code | Message | Description |
|------|---------|-------------|
| -32700 | Parse error | Invalid JSON |
| -32600 | Invalid request | Not JSON-RPC 2.0 |
| -32601 | Method not found | Unknown method |
| -32602 | Invalid params | Invalid parameters |
| -32000 | Database error | State store error |
| -32001 | Not found | Resource not found |

**Error Response Format**:
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32602,
    "message": "Invalid params: expected [pubkey]"
  }
}
```

---

## 🚀 Next: WebSocket API

Coming in Phase 5 Track 2:
- Real-time subscriptions
- Event streaming
- Block notifications
- Transaction confirmations

---

**Documentation Version**: 1.0  
**Last Updated**: February 6, 2026  
**Status**: Complete ✅

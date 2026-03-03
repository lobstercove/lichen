# Cross-Margin Mode — Design Document

> **Status:** Design only — not yet implemented. Isolated margin is the production MVP.

## 1. Overview

Cross-margin mode allows all open positions to share a single margin pool.  
Unlike isolated margin (where each position has its own dedicated collateral),
cross-margin positions draw from and contribute to a shared account balance.

### Key Differences from Isolated Margin

| Aspect | Isolated Margin (Current) | Cross-Margin (Proposed) |
|--------|--------------------------|------------------------|
| Collateral | Per-position locked margin | Shared margin pool |
| Liquidation scope | Individual position | All positions at risk when pool depleted |
| Capital efficiency | Lower — idle margin in each position | Higher — margin shared across positions |
| Risk | Contained to one position | Correlated — losses cascade |
| Max leverage | Up to 100x (tiered) | Up to 3x (`MAX_LEVERAGE_CROSS`) |

## 2. Account Structure

### 2.1 Cross-Margin Account

Each user has one cross-margin account per trading pair group:

```
Storage key: mrg_xacc_{hex_addr}
Layout (48 bytes):
  [0..8]   total_margin     — total collateral deposited (u64)
  [8..16]  used_margin      — margin currently backing open positions (u64)
  [16..24] unrealized_pnl   — sum of all open position PnL (i64, biased)
  [24..32] position_count   — number of open cross-margin positions (u64)
  [32..40] last_updated     — last slot updated (u64)
  [40..48] reserved         — padding
```

### 2.2 Position Record Extension

Cross-margin positions use the existing position record (128 bytes) with an
additional flag:

```
Byte 122: margin_mode — 0 = isolated (default, current), 1 = cross
```

This uses byte 122 from the current padding region (122..128).

## 3. Contract Changes

### 3.1 New Opcodes

| Opcode | Name | Args | Description |
|--------|------|------|-------------|
| 26 | `open_cross_position` | `trader[32], pair_id[8], side[1], size[8], leverage[8]` | Open position using cross-margin pool |
| 27 | `deposit_cross_margin` | `caller[32], amount[8]` | Deposit to cross-margin pool |
| 28 | `withdraw_cross_margin` | `caller[32], amount[8]` | Withdraw available margin from pool |
| 29 | `get_cross_account` | `addr[32]` | Return cross-margin account info |

### 3.2 open_cross_position Logic

1. Verify leverage ≤ `MAX_LEVERAGE_CROSS` (3x)
2. Calculate required margin = size × mark_price / (leverage × 1e9)
3. Check available margin: `total_margin - used_margin ≥ required_margin`
4. Create position record with `margin_mode = 1`
5. Increment `used_margin` by `required_margin`
6. Do NOT call host `lock` — balance is already in the cross-margin pool

### 3.3 Close Cross Position Logic

1. Calculate PnL at current mark price
2. If profit: `total_margin += pnl`
3. If loss: `total_margin -= pnl` (saturating)
4. Reduce `used_margin` by the position's required margin
5. Set position status to closed
6. Do NOT call host `unlock` — funds stay in pool

### 3.4 Liquidation (Cross-Margin)

Cross-margin liquidation triggers when:

```
account_margin_ratio = total_margin / used_margin × 10000  (bps)
```

When `account_margin_ratio < maintenance_margin_bps` for the position's tier:
- ALL open cross-margin positions are liquidated simultaneously
- Remaining margin goes to insurance fund (minus liquidator reward)
- All positions marked as `POS_LIQUIDATED`

### 3.5 Margin Ratio Calculation

For cross-margin accounts, the effective margin includes unrealized P&L
from all open positions:

```
effective_margin = total_margin + sum(unrealized_pnl_i for each open position)
account_ratio = effective_margin * 10000 / sum(notional_i)
```

## 4. Frontend Changes

### 4.1 Mode Toggle

The `#marginInline` panel already has a margin type button group:
```html
<button class="margin-inline-type" data-mtype="isolated">Isolated</button>
```

Add a second button:
```html
<button class="margin-inline-type" data-mtype="cross">Cross</button>
```

### 4.2 Cross-Margin Info Display

When cross-margin mode is active, replace the leverage slider with:
- Pool Balance display (total_margin)
- Used / Available display
- Deposit / Withdraw buttons
- Fixed leverage display (up to 3x)

### 4.3 Position List

Cross-margin positions show a "Cross" badge instead of leverage.
Liquidation warning applies to the entire account, not individual positions.

## 5. Risk Considerations

### 5.1 Cascading Liquidation

In cross-margin, one bad position can drain the entire pool, causing all
other positions to be liquidated. This is the primary risk trade-off.

Mitigation: Implement position-level stop-losses (already done in Phase 2)
as a safety net before pool-level liquidation kicks in.

### 5.2 Leverage Cap

Cross-margin is restricted to 3x maximum (`MAX_LEVERAGE_CROSS = 3`) to
reduce cascading liquidation risk. This is intentionally conservative.

### 5.3 Pair Restrictions

Initially, cross-margin should only be available for major pairs (e.g.,
MOLT/mUSD) with deep liquidity and reliable oracle prices.

## 6. Migration Path

- Phase 1 (Current): Isolated margin only — fully implemented and hardened
- Phase 2 (Future): Add cross-margin opcodes to contract
- Phase 3 (Future): Enable UI toggle, require explicit opt-in
- Phase 4 (Future): Auto-liquidation engine for cross-margin accounts

## 7. Dependencies

- Reliable mark price oracle (existing: `set_mark_price` + staleness check)
- Funding rate implementation (currently constants-only)
- Insurance fund (existing: funded by liquidation penalties)

## 8. Open Questions

1. Should cross-margin accounts be per-pair or universal (all pairs)?
   - Recommendation: Per-pair initially for risk isolation
2. Should cross-margin positions support SL/TP? Yes — reuse Phase 2 logic
3. Should partial close work with cross-margin? Yes — proportional margin
   release back to pool instead of host unlock
4. Should there be a minimum cross-margin pool deposit? Suggest 10 mUSD

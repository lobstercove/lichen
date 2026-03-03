# Cross-Margin Design Document

> **Status:** Design only — not yet implemented. This document is a specification
> for the upcoming cross-margin mode on the MoltChain DEX.

## Overview

Cross-Margin mode allows traders to share collateral across all open positions,
enabling higher capital efficiency and simpler margin management. This contrasts
with the existing **Isolated Margin** mode where each position has its own
independent margin balance.

## Cross-Margin Account Structure

Each user has a single **Cross-Margin Account** identified by their wallet public
key. The account stores:

| Field               | Type    | Description                              |
|---------------------|---------|------------------------------------------|
| `owner`             | Pubkey  | 32-byte owner public key                 |
| `total_deposit`     | u64     | Total collateral deposited               |
| `unrealized_pnl`    | i64     | Aggregate unrealized PnL (biased u64)    |
| `position_count`    | u32     | Number of open cross-margin positions    |
| `margin_mode`       | u8      | Byte 122 — 0 = Isolated, 1 = Cross      |
| `margin_ratio`      | u64     | Current account-level margin ratio       |

### Position Record Extension

Each `MarginPosition` record already reserves **Byte 122** for the `margin_mode`
field (V2 layout, 128 bytes total). When `margin_mode = 1`, the position is
treated as a cross-margin position and its margin is computed from the shared
Cross-Margin Account instead of per-position collateral.

## Isolated Margin vs Cross-Margin Comparison

| Feature              | Isolated Margin                 | Cross-Margin                     |
|----------------------|---------------------------------|----------------------------------|
| Collateral scope     | Per-position                    | Shared across positions          |
| Liquidation risk     | Position-level                  | Account-level (Cascading Liquidation risk) |
| `MAX_LEVERAGE`       | 5x (current)                    | MAX_LEVERAGE_CROSS = 3           |
| Capital efficiency   | Lower — idles in individual pos | Higher — unused margin is shared |
| Implementation       | Live                            | Design only                      |

## New Opcodes

The following contract opcodes will be added:

- **`open_cross_position`** — Opens a new cross-margin position, debiting from
  the shared Cross-Margin Account rather than requiring per-position collateral.
- **`deposit_cross_margin`** — Deposits MOLT into the user's Cross-Margin Account.
- **`withdraw_cross_margin`** — Withdraws excess collateral from the account
  (must maintain minimum margin ratio).
- **`close_cross_position`** — Closes a cross-margin position and credits PnL to
  the Cross-Margin Account.

## Cascading Liquidation Risk

When one cross-margin position moves against the trader, it reduces the available
margin for all other positions. If the account-level margin ratio drops below the
`margin_maintenance` threshold (currently 10%), **all positions in the account**
are subject to cascading liquidation, starting with the largest loss position.

Mitigation: `MAX_LEVERAGE_CROSS` is set to **3** (compared to 5x for isolated)
to reduce cascade probability.

## Frontend: Mode Toggle

The DEX UI includes a margin mode toggle (attribute: `data-mtype`) that switches
between "Isolated" and "Cross" modes. When cross mode is selected:

1. The collateral input shows the user's Cross-Margin Account balance
2. Position cards display a "Cross" badge
3. The liquidation price is computed account-wide, not per-position

## Migration Path

Existing isolated margin positions will NOT be automatically converted.
Users can opt-in to cross-margin mode via the Mode Toggle. New positions opened
in cross mode will use the new Cross-Margin Account. Users can maintain both
isolated and cross positions simultaneously during the migration period.

---

*This document covers the design specification only. Implementation will follow
in a future release after the isolated margin system is battle-tested on mainnet.*

# Permissions Justification

## Core Extension Permissions

### `storage`
Used to persist encrypted wallets, user settings, approved dapp origins, and runtime state.

### `alarms`
Used for auto-lock scheduling.

### `notifications`
Used to notify the user about submitted transactions, bridge updates, and extension events.

### `tabs`
Used to open the full-page wallet and approval routes from the popup and dapp approval flows.

## Host Permissions

### Lichen RPC endpoints
Used for balance reads, activity, staking, identity, bridge, and transaction submission.

Restriction-governance safety checks also use the trusted Lichen RPC endpoints for read-only status and preflight calls, including `getRestrictionStatus`, `canTransfer`, and `getContractLifecycleStatus`. These calls let the wallet show account, asset, contract lifecycle, incident, and transfer-blocking state before a user signs. They do not grant dapps authority to mutate restrictions or bypass wallet warnings.

### Lichen custody endpoints
Used for authenticated bridge deposit flows.

### Lichen WebSocket endpoints
Used for account and provider state refresh.

## Content Script Access

The content script is required to inject the Lichen provider bridge into supported web pages so dapps can request accounts, sign messages, and request transactions through the extension approval flow.

The injected `window.licnwallet` provider also exposes read-only restriction preflight helpers for dapps: `lichen_getRestrictionStatus`, `lichen_canTransfer`, and `lichen_getContractLifecycleStatus`. These methods are query-only. Signing and transaction submission still require the extension approval flow, and the wallet runs its own restriction preflight before key decryption so a dapp cannot suppress or replace wallet-side restriction warnings.

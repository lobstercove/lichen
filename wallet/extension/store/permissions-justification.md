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

### Lichen custody endpoints
Used for authenticated bridge deposit flows.

### Lichen WebSocket endpoints
Used for account and provider state refresh.

## Content Script Access

The content script is required to inject the Lichen provider bridge into supported web pages so dapps can request accounts, sign messages, and request transactions through the extension approval flow.
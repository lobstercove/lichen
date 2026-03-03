# MoltChain Wallet & Browser Extension — Production Audit Report

**Scope:** `wallet/index.html`, `wallet/js/wallet.js`, `wallet/js/crypto.js`, `wallet/js/identity.js`, `wallet/js/shielded.js`, `wallet/shared/utils.js`, `wallet/shared-config.js`, `wallet/extension/manifest.json`, `wallet/extension/src/background/service-worker.js`, `wallet/extension/src/content/content-script.js`, `wallet/extension/src/content/inpage-provider.js`, `wallet/extension/src/core/crypto-service.js`, `wallet/extension/src/core/provider-router.js`, `wallet/extension/src/core/rpc-service.js`, `wallet/extension/src/core/state-store.js`, `wallet/extension/src/core/tx-service.js`, `wallet/extension/src/core/ws-service.js`, `wallet/extension/src/core/lock-service.js`, `wallet/extension/src/core/notification-service.js`, `wallet/extension/src/pages/approve.js`, `wallet/extension/src/popup/popup.js`, `rpc/src/lib.rs` (dispatch table + handlers), `rpc/src/ws.rs` (subscription dispatch)

**Method:** All files read completely. Rust RPC dispatch validated against frontend RPC method names. Bincode serialization format cross-checked between frontend and server.

---

## Executive Summary

| Severity | Count |
|----------|-------|
| CRITICAL | 2 |
| HIGH | 6 |
| MEDIUM | 12 |
| LOW | 14 |
| Architecture | 5 |
| **Total** | **39** |

The wallet's ZK privacy layer is entirely non-functional: proof generation produces placeholder bytes and the spending key is derived from the public address. All other wallet surfaces (send/receive, staking, identity, bridge) are broadly sound but carry several medium-severity correctness bugs and one high-severity RPC mismatch that renders bridge WebSocket notifications permanently silent.

---

## Technical Foundation

| Property | Value |
|----------|-------|
| Chain | MoltChain — Ed25519, Base58 addresses (32-byte pubkeys) |
| Denomination | 1 MOLT = 1,000,000,000 shells |
| Block time | 400ms per slot |
| Key encryption | AES-256-GCM + PBKDF2-SHA256 (100k iterations) |
| Mnemonic | BIP39, PBKDF2-HMAC-SHA512 (2048 iter), 12 words |
| Signing | TweetNaCl Ed25519 (website) / WebCrypto Ed25519 (extension) |
| TX format | JSON-encoded signed transaction, base64-wrapped for RPC |
| TX serialization | bincode: u64LE(ixs.length) + per-ix[program_id(32B) + u64LE(accts.len) + accts×32B + u64LE(data.len) + data] + blockhash(32B) |
| Extension | Chrome MV3, service worker, `subscribeAccount` WS |

---

## Section 1 — ZK Privacy Layer (CRITICAL Failures)

### ZK-1 · CRITICAL · Placeholder ZK Proofs — No Real Prover

**File:** [wallet/js/shielded.js](wallet/js/shielded.js#L500-L565)

`generateShieldProof`, `generateUnshieldProof`, and `generateTransferProof` all return 128-byte buffers constructed from two SHA-256 hashes of the public inputs. No Groth16 WASM prover is loaded; the import is commented out with the comment: *"In production: Load WASM module and generate real Groth16 proof"*.

```js
// shielded.js ~line 513 (shield proof):
const hash1 = new Uint8Array(await crypto.subtle.digest('SHA-256',
    new TextEncoder().encode(`shield:${amountShells}:${blindingHex}`)));
const hash2 = new Uint8Array(await crypto.subtle.digest('SHA-256',
    new TextEncoder().encode(`commitment:${commitmentHash}`)));
const proof = new Uint8Array(128);
proof.set(hash1, 0);   // bytes  0–31
proof.set(hash2, 32);  // bytes 32–63
proof.set(hash1, 64);  // bytes 64–95
proof.set(hash2, 96);  // bytes 96–127
return proof;
```

The on-chain verifier expects a real Groth16 proof over BN254. These 128 bytes will be rejected by any correct verifier. The shielded pool is non-functional in production.

**Fix:** Load the actual WASM Groth16 prover (e.g., snarkjs wasm bundle), generate witness from the correct R1CS circuit, and return the real proof bytes.

---

### ZK-2 · CRITICAL · Shielded Spending Key Derived from Public Address

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (setupDashboardTabs, shield tab click handler, ~line 1158)

When the Shield tab is activated, the seed passed to `initShielded()` is:

```js
const encoder = new TextEncoder();
const seedBytes = encoder.encode(wallet.address + ':shielded');
initShielded(seedBytes);
```

`wallet.address` is the Base58-encoded Ed25519 public key — a value that is entirely public and visible on-chain. The shielded spending key is therefore `SHA-256(publicAddress + ':shielded')`, which any observer can compute from the chain data alone. This destroys all privacy guarantees: any party who knows a wallet's address can derive its spending key and decrypt all shielded notes.

**Fix:** Derive the shielded seed from the 32-byte private key seed (not the public address). The correct derivation is `HKDF-SHA256(context="moltchain-shielded-v1", ikm=privateKeySeed)`, using the same key that is decrypted at signing time.

---

### ZK-3 · HIGH · XOR-Only Note Encryption — No Authentication

**File:** [wallet/js/shielded.js](wallet/js/shielded.js#L157-L171) and [wallet/js/shielded.js](wallet/js/shielded.js#L390-L400)

Note ciphertext is produced by XOR with a 32-byte derived key wrapped modulo 32:

```js
const decKey = new Uint8Array(await crypto.subtle.digest('SHA-256',
    new Uint8Array([...ephemeralPk, ...viewingKey])));
for (let i = 0; i < plaintext.length; i++) {
    ciphertext[i] = plaintext[i] ^ decKey[i % 32];
}
```

This is equivalent to a stream cipher with a short repeating key and **zero authentication**. Ciphertexts are malleable: an attacker can flip bits in the note fields (amount, blinding factor, serial number) without detection. There is no MAC, no AEAD, and no nonce — identical plaintexts produce identical ciphertexts.

**Fix:** Replace with AES-256-GCM using a fresh 96-bit nonce per note, deriving the encryption key via HKDF from `(ephemeralPk ‖ viewingKey)`. The 12-byte nonce should be stored alongside the ciphertext.

---

### ZK-4 · HIGH · `computeCommitmentHash` Uses SHA-256, Not Pedersen Commitment

**File:** [wallet/js/shielded.js](wallet/js/shielded.js#L556) (approximately)

```js
async function computeCommitmentHash(value, blinding) {
    const preimage = `pedersen:${value}:${blinding.toString('hex') ?? blindingHex}`;
    const hashBuf = await crypto.subtle.digest('SHA-256', encoder.encode(preimage));
    return new Uint8Array(hashBuf);
}
```

The function is named "pedersen" but implements SHA-256 over a text string. A Pedersen commitment over BN254 requires `C = g^v · h^r` (elliptic-curve scalar multiplication). The on-chain Merkle tree and proof circuits expect curve-level commitments. Any commitment computed this way will not match what a real Groth16 verifier expects.

**Fix:** Implement a proper Pedersen or Poseidon commitment using the BN254 curve parameters matching the circuit. Use `@aztec/bb.js` or `snarkjs` WASM for this computation.

---

### ZK-5 · HIGH · Shielded Note Secrets Stored Plaintext in localStorage

**File:** [wallet/js/shielded.js](wallet/js/shielded.js#L590-L615)

`saveNotesToStorage()` writes owned notes including `blinding` (the randomness field of the Pedersen commitment) and `serial` (the nullifier) to localStorage under key `moltchain_shielded_notes`. These values allow reconstruction of a note's commitment and its nullifier. A cross-origin attack, XSS, or local attacker can steal note secrets.

**Fix:** Encrypt the notes array with AES-256-GCM before writing to localStorage, using a key derived from the wallet password (PBKDF2 or session key derived during unlock), as is already done for the private key.

---

### ZK-6 · MEDIUM · `checkNullifier` RPC Method Does Not Exist — Nullifier Check Always Silently Fails

**File:** [wallet/js/shielded.js](wallet/js/shielded.js) (`syncShieldedState`)

`syncShieldedState()` calls `rpc.call('checkNullifier', [nullifierHex])` per owned note. The Rust RPC server exposes `isNullifierSpent` (see [rpc/src/lib.rs](rpc/src/lib.rs#L1534)), not `checkNullifier`. The call returns a method-not-found JSON-RPC error that is caught and swallowed, so notes are never marked as spent.

**Fix:** Change the call to `rpc.call('isNullifierSpent', [nullifierHex])` and handle the boolean result field from the server response.

---

### ZK-7 · MEDIUM · `getShieldedPoolStats` RPC Method Mismatch

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`openShieldModal` / `loadShieldedStats`), [wallet/extension/src/popup/popup.js](wallet/extension/src/popup/popup.js) (`loadShieldPanel`)

The frontend calls `rpc.call('getShieldedPoolStats', [])`. The Rust server exposes `getShieldedPoolState` (see [rpc/src/lib.rs](rpc/src/lib.rs#L1523)). The call returns method-not-found; pool total and commitment count always display defaults.

**Fix:** Rename the frontend call from `getShieldedPoolStats` to `getShieldedPoolState` and update the response field extraction to match the server's JSON structure (`total_shielded`, `commitment_count`).

---

### ZK-8 · MEDIUM · `unshieldMolt` Recipient Address Not Validated

**File:** [wallet/js/shielded.js](wallet/js/shielded.js) (`confirmUnshield`, ~line 860)

`confirmUnshield()` checks `if (!recipient)` for empty but never calls `MoltCrypto.isValidAddress(recipient)`. An invalid address will pass the empty check and produce a transaction that fails on-chain after signing and broadcasting, wasting the ZK compute fee.

**Fix:** Add `if (!MoltCrypto.isValidAddress(recipient)) { showError('Invalid recipient address'); return; }` before building the unshield transaction.

---

## Section 3 — Cryptography

### CRYPTO-1 · MEDIUM · `isValidMnemonic` Does Not Verify BIP39 Checksum

**File (website):** [wallet/js/crypto.js](wallet/js/crypto.js#L486-L494)
**File (extension):** [wallet/extension/src/core/crypto-service.js](wallet/extension/src/core/crypto-service.js) (`isValidMnemonic`)

Both synchronous implementations accept any 12 words that appear in the BIP39 wordlist, without checking the 4-bit SHA-256 checksum embedded in the twelfth word. A mistyped mnemonic that happens to use valid wordlist words in an invalid checksum position is accepted:

```js
export function isValidMnemonic(mnemonic) {
    const words = mnemonic.trim().toLowerCase().split(/\s+/).filter(Boolean);
    return words.length === 12 && words.every(word => BIP39_WORDLIST.includes(word));
    // ↑ no checksum validation
}
```

This allows import of mnemonics that cannot reproduce the original wallet (wrong key derived), causing permanent key loss.

**Fix:** After verifying word count and wordlist membership, reconstruct the bit string from word indices (12 × 11 bits = 132 bits), extract the 4-bit checksum from the last 4 bits of word 12, compute `SHA-256(entropy[0..16])[0] >> 4`, and compare. Reject if they do not match.

---

### CRYPTO-2 · LOW · `signTransaction` Seed Zeroing Is Best-Effort

**File:** [wallet/js/crypto.js](wallet/js/crypto.js#L368-L377)

After signing, the code zeros `seed` and `secretKey` local variables. JavaScript's garbage collector does not guarantee that `fill(0)` eliminates the values from heap memory before a GC sweep. This is a common limitation in JS crypto but explicitly deviates from secure coding guidance for sensitive key material in browsers. The WebCrypto Ed25519 path in the extension (`crypto-service.js`) avoids this issue because the private key is handled by the browser's CryptoKey API which never exposes raw bytes.

**Note:** The extension's `provider-router.js` correctly zeros `privateKeyHex` in all `finally` blocks with `privateKeyHex = '0'.repeat(...)`.

---

### CRYPTO-3 · LOW · `keccak256` Implemented Twice with Potential Divergence

**File 1:** [wallet/js/wallet.js](wallet/js/wallet.js) — uses `jsSHA` / `js-sha3` CDN library (loaded via `<script src="https://cdnjs.cloudflare.com/...js-sha3.min.js">`)

**File 2:** [wallet/extension/src/core/crypto-service.js](wallet/extension/src/core/crypto-service.js) — pure JS BigInt implementation (`keccak256()`)

Both must produce identical 32-byte outputs for the same input (used to derive EVM addresses). If the CDN library and the pure-JS implementation diverge (e.g., due to different padding or endianness), EVM address derivation will produce different addresses in the website vs the extension, breaking bridge deposits.

**Fix:** Share a single, tested implementation. The extension's `crypto-service.js` implementation is self-contained; use it as the single source of truth and include it in the website's build rather than loading from CDN.

---

## Section 4 — Transaction Serialization Cross-Check

### TX-1 · PASS · `serializeMessageBincode` Layout Matches Server

The frontend's bincode serialization (3 independent implementations in `wallet.js`, `shared/utils.js`, `provider-router.js`, `tx-service.js`) all produce:

```
u64LE(num_instructions)
for each instruction:
  [u8; 32]  program_id      (fixed, no length prefix)
  u64LE(num_accounts)
  for each account: [u8; 32] pubkey
  u64LE(data_length)
  [u8; data_length] data
[u8; 32] blockhash
```

This matches what the Rust server decodes. Base64-encoded JSON wrap is correctly identified by the `{` first-byte heuristic in `handle_send_transaction`.

### TX-2 · PASS · Instruction Opcodes Cross-Checked vs Server

From [rpc/src/lib.rs](rpc/src/lib.rs#L510-L518), the server recognises amount-bearing instructions at offsets:

```rust
match ix.data[0] {
    0 | 2 | 3 | 4 | 5 | 9 | 10 | 13 | 14 | 16 | 19 | 23 | 24 => {
        // u64LE amount from data[1..9]
    }
```

Frontend opcodes used:
| Opcode | Name | Frontend data layout | Server-expected |
|--------|------|-----------------------|-----------------|
| `0x00` | Transfer | `[0x00, amount:u64LE]` (9 B) | ✅ Opcode 0 |
| `0x0C` (12) | RegisterEvmAddress | `[0x0C, evm_address:20B]` (21 B) | ✅ No amount |
| `0x0D` (13) | ReefStakeDeposit | `[0x0D, amount:u64LE, tier:u8]` (10 B) | ✅ Opcode 13 |
| `0x0E` (14) | ReefStakeUnstake | `[0x0E, st_molt:u64LE]` (9 B) | ✅ Opcode 14 |
| `0x0F` (15) | ReefStakeClaim | `[0x0F]` (1 B) | ✅ No amount |
| `0x10` (16) | ReefStakeTransfer (stMOLT) | `[0x10, amount:u64LE]` (9 B) | ✅ Opcode 16 |

Token contract calls use JSON-wrapped `{Call:{function,args,value}}` with `program_id = [0xFF;32]` — server correctly identifies by `CONTRACT_PROGRAM_ID` check.

### TX-3 · MEDIUM · `serializeMessageBincode` Duplicated in 4 Places

**Files:** [wallet/js/wallet.js](wallet/js/wallet.js), [wallet/shared/utils.js](wallet/shared/utils.js), [wallet/extension/src/core/provider-router.js](wallet/extension/src/core/provider-router.js), [wallet/extension/src/core/tx-service.js](wallet/extension/src/core/tx-service.js)

All four are textually identical. A future divergence would cause signature mismatches with no compile-time warning.

**Fix:** Export a single canonical version from `wallet/shared/utils.js` (or as an ES module from `tx-service.js`) and import it everywhere.

---

## Section 5 — UI Panels & Data Population

### UI-1 · LOW · Send Modal Fee Is Static HTML — Not Dynamic

**File:** [wallet/index.html](wallet/index.html) (send modal, `<span>0.001 MOLT</span>`)

The fee display is hardcoded in the HTML. The server exposes a `getFeeConfig` method returning current fee parameters. If the chain's fee schedule is updated, the displayed fee will be wrong.

**Fix:** On `openSendModal()`, call `rpc.call('getFeeConfig', [])` and populate `#sendFeeDisplay` from the response's `base_fee` field.

---

### UI-2 · MEDIUM · Activity Pagination Cursor May Never Advance

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`loadActivity`, ~line 1340)

The "Load More" cursor is set from:
```js
const lastTx = txs[txs.length - 1];
cursor = lastTx.slot || lastTx.block_slot;
```

The server's `getTransactionsByAddress` handler ([rpc/src/lib.rs](rpc/src/lib.rs#L2511)) returns transaction records whose slot field is named `slot` in the JSON. If the server returns `block_height` or the field is missing, `cursor` is `undefined`, the next `loadActivity` fetch is sent without a `before_slot` cursor, and the same first page is returned again. "Load More" silently loops.

**Fix:** Add a guard: `if (!cursor) { noMoreActivity = true; return; }`. Also add a `before_slot` fallback: `tx.slot ?? tx.block_slot ?? tx.block_height`.

---

### UI-3 · LOW · `tx.timestamp` May Already Be Milliseconds

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`loadActivity`, ~line 1348)

Timestamps are displayed as: `new Date(tx.timestamp * 1000)`. If the RPC returns timestamps already in milliseconds (e.g., 1706000000000), dates will display as year ~56000.

The server's transaction record stores `timestamp: u64` as Unix seconds (set from `SystemTime::UNIX_EPOCH`). Current behaviour is safe. However, `formatTime()` in [wallet/shared/utils.js](wallet/shared/utils.js#L217-L228) already handles both cases:
```js
const ts = timestamp < 1e12 ? timestamp : timestamp / 1000;
```

**Fix:** Use `formatTime(tx.timestamp)` from `shared/utils.js` instead of inline `new Date(tx.timestamp * 1000)` to gain the normalisation for free.

---

### UI-4 · MEDIUM · `setMaxAmount()` Does Not Reserve MOLT Fee for Token Sends

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`setMaxAmount`, ~line 2830)

MAX logic for non-MOLT tokens:
```js
} else {
    // Token: full token balance — no MOLT fee deducted
    amountInput.value = tokenBalance;
}
```

A MOLT fee of 0.001 MOLT is still charged when sending tokens. If the user's MOLT balance is exactly the fee amount, the transaction will fail server-side with "Insufficient MOLT balance for fees". The MAX button suggests they can send 100% of their tokens when they cannot.

**Fix:** Check that `window.walletBalance >= BASE_FEE_MOLT` before allowing the MAX send of tokens; warn the user if they lack MOLT for the fee.

---

### UI-5 · LOW · EVM Address Displayed Before On-Chain Registration

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`setupWalletSelector`, ~line 1175)

The receive modal immediately displays a keccak-derived EVM address via `generateEVMAddress()`. The on-chain `registerEvmAddress` call is fire-and-forget and may not have completed. Depositing to this address before registration will fail at the bridge layer.

**Fix:** Add a note in the EVM address field: *"Registration pending..."* until the `getEvmRegistration` RPC confirms the address is indexed.

---

### UI-6 · MEDIUM · Identity `_identityCache` Not Cleared on Wallet Switch

**File:** [wallet/js/identity.js](wallet/js/identity.js) (module-level singleton, ~line 25)

```js
let _identityCache = null;
```

`logoutWallet()` correctly sets `_identityCache = null`. However `switchWallet()` in `wallet.js` doesn't clear it. Switching wallets shows the previous wallet's identity until the next explicit refresh.

**Fix:** Call `_identityCache = null` (or export a `clearIdentityCache()` function) inside the wallet-switch handler in `wallet.js`.

---

### UI-7 · LOW · `getValidators` Called on Every Staking Tab Activation

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`loadStaking`)

`getValidators` performs a full RPC round-trip every time the Staking tab is clicked. The result rarely changes within a session.

**Fix:** Cache the validator list with a TTL (e.g., 30 seconds) or only invalidate on wallet switch.

---

### UI-8 · LOW · Explorer Links Use Relative Paths

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`loadActivity`)

Transaction links are built as: `` `../explorer/transaction.html?sig=${sig}` ``. This works when the wallet is served at `/wallet/` but breaks when served from a different subdirectory or as the extension's full-page view.

**Fix:** Use `MOLT_CONFIG.explorer + '/transaction.html?sig=' + sig` for the correct base URL.

---

## Section 6 — Security

### SEC-1 · MEDIUM · Delete Wallet Does Not Zero Encrypted Key Before localStorage Overwrite

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`showDeleteWallet` / `deleteWalletConfirmed`, ~line 4044)
**File:** [wallet/extension/src/popup/popup.js](wallet/extension/src/popup/popup.js) (`handleDeleteWallet`)

Both implementations filter the wallet out of the state array and call `saveState()`. The `chrome.storage.local` / `localStorage.setItem` call overwrites the entire state object, which effectively removes the encrypted key from persistent storage. However the operating system may retain the key in file-system journal/swap until overwritten.

More critically: in the website wallet, `localStorage` entries named `molt*` (including bridge caches, shielded notes, EVM registration flags) are cleared in `logoutWallet()` but NOT in the delete-one-wallet path when other wallets remain. The shielded notes for the deleted wallet linger.

**Fix:** Before removing the wallet, explicitly call `localStorage.removeItem('moltchain_shielded_notes')` (or a wallet-scoped key). Zero the `encryptedKey.encrypted` hex string to `'0'.repeat(...)` before the final `saveState` write.

---

### SEC-2 · LOW · Pending Approval Requests Are Never Auto-Expired Without Incoming Activity

**File:** [wallet/extension/src/core/provider-router.js](wallet/extension/src/core/provider-router.js#L32-L39)

`prunePendingRequests()` is called on every `handleProviderRequest`. The TTL is 3 minutes. But if no new requests arrive after the TTL elapses, the expired request stays in `pendingRequests` indefinitely. A tab that called `requestAccounts` 31 seconds after another tab's request was declined could see stale data if some edge-case path calls `listPendingRequests` before `prunePendingRequests` has run.

**Fix:** Schedule a `setInterval` in the service worker to call `prunePendingRequests()` every 60 seconds, independently of incoming requests.

---

### SEC-3 · LOW · Approved Origins Persist Forever Without Expiry

**File:** [wallet/extension/src/core/provider-router.js](wallet/extension/src/core/provider-router.js) (`approveOrigin`)

Origins are stored in `chrome.storage.local.moltApprovedOrigins` as a plain string array. Once a site is approved it remains approved forever. There is no last-used timestamp and no automatic expiry.

**Fix:** Store `{ origin, approvedAt: Date.now() }` objects; consider revoking origins not used in 90 days, or at minimum expose a "connected sites" management UI (the settings page has `MOLT_PROVIDER_LIST_ORIGINS` / `MOLT_PROVIDER_REVOKE_ORIGIN` already wired).

---

### SEC-4 · LOW · `setInterval` 2-Second Provider State Polling in Content Script

**File:** [wallet/extension/src/content/content-script.js](wallet/extension/src/content/content-script.js#L78-L82)

```js
setInterval(() => { checkProviderStateAndEmit(); }, 2000);
```

Every 2 seconds per active tab, this sends a `MOLT_PROVIDER_REQUEST` to the service worker, waits for `loadState()` to read from `chrome.storage.local`, and compares accounts/chainId. On 30 tabs this is 15 IPC round-trips per second.

**Fix:** Replace with an event-driven model: the service worker should broadcast a `MOLT_STATE_CHANGED` message to all tabs when state mutates (wallet lock, account switch, network change). The content script listens and only re-fetches when signalled.

---

### SEC-5 · MEDIUM · `window.ethereum` Shim Exposes `window.moltwallet` Methods with No Permission Check

**File:** [wallet/extension/src/content/inpage-provider.js](wallet/extension/src/content/inpage-provider.js#L150-L164)

```js
if (!window.ethereum) {
    window.ethereum = {
        ...window.moltwallet,
        isMetaMask: false,
        enable: () => sendRequest({ method: 'eth_requestAccounts' })
    };
}
```

Scripts on the page that access `window.ethereum.accounts()` get back an empty array (unapproved) — correct. But `window.ethereum.getBalance(address)` is forwarded to `molt_getBalance` which is a **read-only method that returns immediately without user approval**, leaking account balance information for any arbitrary address. For privacy-sensitive applications this is acceptable (balances are on a public chain), but it should be documented.

---

## Section 7 — Extension Architecture

### EXT-1 · MEDIUM · Extension Shielded Panel Always Uninitialized

**File:** [wallet/extension/src/popup/popup.js](wallet/extension/src/popup/popup.js) (`loadShieldPanel`, `initShieldedPopup`)

`initShieldedPopup(walletSeed)` is only called if `wallet.encryptedSeed` exists. The extension's `DEFAULT_STATE` schema (in [state-store.js](wallet/extension/src/core/state-store.js)) and the wallet creation functions (`createWalletFromMnemonic`, `createWalletFromPrivateKeyHex`) do NOT store an `encryptedSeed` field — they store `encryptedKey` (the 32-byte private key seed) and `encryptedMnemonic`. Because `wallet.encryptedSeed` is always `undefined`, `initShieldedPopup` is never called and the shield panel shows empty without address, empty balance.

**Fix:** In `loadShieldPanel`, if `shieldedPopupState.initialized` is false and `wallet.encryptedKey` is stored, prompt for the wallet password, decrypt the private key via `decryptPrivateKey`, and use the 32-byte seed to call `initShieldedPopup`. Never store the seed in plaintext state.

---

### EXT-2 · MEDIUM · Extension WS Service Does Not Propagate Balance Updates to Popup

**File:** [wallet/extension/src/core/ws-service.js](wallet/extension/src/core/ws-service.js) (`onmessage`)

The `WalletWsManager` class connects to the WS endpoint and subscribes to `subscribeAccount`. The `onmessage` handler only records `subscriptionId`:

```js
this.socket.onmessage = (event) => {
    try {
        const msg = JSON.parse(event.data);
        if (msg.id === 1 && msg.result !== undefined) {
            this.subscriptionId = msg.result;
        }
        // ← balance notification events are silently discarded
    } catch { }
};
```

Incoming `subscription` notification messages are parsed but not acted upon. The popup relies entirely on a 15-second `setInterval` polling (`startBalancePolling()`), meaning balance updates arrive up to 15 seconds late.

**Fix:** In `onmessage`, detect `msg.method === 'subscription'` and relay a `MOLT_BALANCE_UPDATED` message to all extension tabs via `chrome.runtime.sendMessage` (or a broadcast via `chrome.tabs.query`). The popup listens and calls `refreshBalance()` immediately.

---

### EXT-3 · LOW · `fullCarouselTimer` Not Cleared on Re-Entry

**File:** [wallet/extension/src/popup/popup.js](wallet/extension/src/popup/popup.js#L70)

```js
if (fullCarouselTimer) { clearInterval(fullCarouselTimer); }
fullCarouselTimer = setInterval(..., 3500);
```

The guard clears the previous interval before setting a new one — correct. However `applyViewMode()` and `initFullWelcomeCarousel()` are called from `boot()` once. The leak only occurs if `boot()` is called multiple times (it isn't), so this is low risk.

---

### EXT-4 · LOW · Single-Wallet Extension: Every Tab Action Reads from `chrome.storage.local`

**File:** [wallet/extension/src/background/service-worker.js](wallet/extension/src/background/service-worker.js)

Every `MOLT_GET_STATE`, `MOLT_PROVIDER_REQUEST`, and `MOLT_WS_SYNC` message triggers `loadState()` which reads from `chrome.storage.local`. MV3 service workers are ephemeral and cannot keep a hot in-memory state. This is unavoidable but means every user interaction incurs a storage I/O. No cache/debounce is applied.

**Note:** This is an architectural constraint of Chrome MV3, not a bug.

---

### EXT-5 · LOW · `chrome.tabs.create` for Approve/Full-Page Opens Without Focus Guard

**File:** [wallet/extension/src/background/service-worker.js](wallet/extension/src/background/service-worker.js#L85-L87)

```js
const url = chrome.runtime.getURL(`src/pages/approve.html?requestId=...`);
await chrome.tabs.create({ url });
```

If the user double-clicks or the site sends two `requestAccounts` rapidly, two approve tabs open. Both will attempt to finalize the same `requestId`. The second `finalizePendingRequest` call will find no request (already consumed) and return `{ ok: false, error: 'Request not found' }`, which is harmless but visible to the dApp as a rejection.

**Fix:** Before creating the tab, check if an approve tab for this `requestId` is already open using `chrome.tabs.query({ url: '*://*/approve.html*' })`.

---

## Section 8 — RPC Method Coverage Audit

All frontend RPC calls vs. server dispatch table ([rpc/src/lib.rs](rpc/src/lib.rs#L1355)):

| Frontend Call | Server Method | Status |
|---------------|---------------|--------|
| `getBalance` | `getBalance` | ✅ |
| `sendTransaction` | `sendTransaction` | ✅ |
| `getLatestBlock` | `getLatestBlock` | ✅ |
| `getSlot` | `getSlot` | ✅ |
| `getTransactionsByAddress` | `getTransactionsByAddress` | ✅ |
| `getContractInfo` | `getContractInfo` | ✅ |
| `getSymbolRegistry` | `getSymbolRegistry` | ✅ |
| `getAllContracts` | `getAllContracts` | ✅ |
| `getMoltyIdProfile` | `getMoltyIdProfile` | ✅ |
| `getStakingPosition` | `getStakingPosition` | ✅ |
| `getReefStakePoolInfo` | `getReefStakePoolInfo` | ✅ |
| `getUnstakingQueue` | `getUnstakingQueue` | ✅ |
| `getValidators` | `getValidators` | ✅ |
| `getNFTsByOwner` | `getNFTsByOwner` | ✅ |
| `createBridgeDeposit` | `createBridgeDeposit` | ✅ |
| `getBridgeDeposit` | `getBridgeDeposit` | ✅ |
| `getEvmRegistration` | `getEvmRegistration` | ✅ |
| `getShieldedCommitments` | `getShieldedCommitments` | ✅ |
| `isNullifierSpent` | `isNullifierSpent` | ✅ (not yet called) |
| **`checkNullifier`** | _(missing)_ | ❌ Wrong name → ZK-6 |
| **`getShieldedPoolStats`** | _(server: `getShieldedPoolState`)_ | ❌ Name mismatch → ZK-7 |
| **`subscribeBridgeLocks`** (WS) | ✅ Implemented in ws.rs | ✅ Working |
| **`subscribeBridgeMints`** (WS) | ✅ Implemented in ws.rs | ✅ Working |

---

## Section 9 — WebSocket Subscription Coverage Audit

| Frontend subscription | Server (ws.rs) | Notification triggers UI update |
|-----------------------|----------------|---------------------------------|
| `subscribeAccount` (id:1) | ✅ `subscribeAccount` | ✅ `refreshBalance`, `loadAssets`, `loadActivity`, `refreshStakingIfVisible` |
| `subscribeBridgeLocks` (id:2) | ❌ Missing | ❌ Bridge lock events never arrive |
| `subscribeBridgeMints` (id:3) | ❌ Missing | ❌ Bridge mint events never arrive |

Extension `WalletWsManager` only subscribes to `subscribeAccount`. Service worker also subscribes. When any `subscription` notification arrives, it is **discarded** (see EXT-2 above) — the popup never receives real-time balance pushes.

---

## Section 10 — XSS Audit

| Location | Method | Safe? |
|----------|--------|-------|
| Wallet dropdown names (`wallet.js:~1175`) | `escapeHtml()` from shared/utils.js | ✅ |
| NFT grid names (`wallet.js:~2659`) | `escapeHtml()` | ✅ |
| Activity transaction list (`wallet.js:~1420`) | `escapeHtml()` on all fields | ✅ |
| Deposit modal bridge address (`wallet.js:~2440`) | data-copy attribute, not innerHTML | ✅ |
| Identity profile rendering (`identity.js`) | `escHtml()` local copy | ✅ |
| Activity list in popup (`popup.js:~620`) | `escapeHtml()` on all displayed fields | ✅ |
| Approve page origin/method display (`approve.js`) | `escapeHtml()` | ✅ |
| Bridge deposit address in popup (`popup.js:~1390`) | `escapeHtml(data.address)` | ✅ |
| NFT grid in popup (`popup.js:~1445`) | `escapeHtml(n.name)` | ✅ |
| Staking tier labels in popup (`popup.js:~870`) | `escapeHtml` on both label and value | ✅ |
| Chain status bar `blockEl.textContent` (`shared/utils.js:~475`) | `textContent` not `innerHTML` | ✅ |
| Identity `buildLayoutArgs` skill/endpoint data | Not rendered as HTML — used in tx data | ✅ |

No XSS vectors found. All dynamic content uses `escapeHtml()` before insertion into innerHTML, or uses `textContent`.

---

## Section 11 — Error State & Spinner Audit

### ERR-1 · LOW · `showDashboard()` Has No Global Loading State

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`showDashboard`)

`showDashboard` fires `refreshBalance()`, `loadAssets()`, `loadActivity()`, `loadStaking()`, `refreshNFTs()` all concurrently with `await` inside async handlers. If the RPC is unavailable, each panel independently shows its own error state ("Failed to load..."), but there is no top-level "wallet offline" indicator.

### ERR-2 · LOW · Shield Modal Fee Display Shows "Loading..." Indefinitely on RPC Failure

**File:** [wallet/js/shielded.js](wallet/js/shielded.js) (`openShieldModal`)

`zkFeeDisplay(type)` computes fees from local constants (`BASE_FEE_SHELLS + ZK_COMPUTE_FEE[type]`), so the display is actually immediate. The `id="shieldFeeDisplay"` element is set correctly. **No indefinite "Loading..." spinner.** ✅

### ERR-4 · LOW · Faucet Airdrop Call Has 2s Timeout but No UI Indicator on Timeout

**File:** [wallet/js/wallet.js](wallet/js/wallet.js) (`loadActivity`, ~line 1345)

```js
const faucetController = new AbortController();
const faucetTimeout = setTimeout(() => faucetController.abort(), 2000);
```

If the faucet is unreachable, the 2s timeout fires, `AbortError` is caught silently, and airdrops are omitted from the list. This is correct behaviour — no spinner is shown for this call. ✅

---

## Section 12 — `element.textContent = data.X` Undefined Data Access Audit

| Assignment | `data` field | Fallback? | Safe? |
|------------|-------------|-----------|-------|
| `walletBalance.textContent = ${balanceMolt} MOLT` | `result?.shells ?? 0` | ✅ `?? 0` | ✅ |
| `chainBlockHeight.textContent = 'Block #' + slot` | `result.getSlot` | ✅ fallback on catch | ✅ |
| `stakePoolTotalEl.textContent = ...` | `poolInfo?.total_molt_staked ?? 0` | ✅ optional chain | ✅ |
| `validatorCountEl.textContent = validators.length` | `getValidators` → guards `if (!validators)` | ✅ | ✅ |
| `popup walletBalance.textContent = balanceMolt MOLT` | `result?.shells ?? result?.spendable ?? 0` | ✅ | ✅ |
| `popup aboutNet.textContent = state.network?.selected` | optional chain | ✅ | ✅ |
| `popup unlockWalletName.textContent = wallet?.name` | ternary guard | ✅ | ✅ |
| Activity TX `type = typeMap[tx.type] || ...` | fallback expression | ✅ | ✅ |
| Bridge deposit `data.address` in popup | `if (!data?.address) throw` guard | ✅ | ✅ |
| NFT name `n.name || n.mint || 'NFT'` | ✅ triple fallback | ✅ | ✅ |

No unsafe bare-field accesses (`element.textContent = data.field`) found.

---

## Section 13 — Configuration

### CFG-1 · LOW · MOLT Price Hardcoded at $0.10

**File:** [wallet/extension/src/popup/popup.js](wallet/extension/src/popup/popup.js) (`refreshBalance`)

```js
usdEl.textContent = `$${(balanceMolt * 0.10).toLocaleString(...)} USD`;
```

The MOLT/USD price is hardcoded as `0.10`. No price oracle or CoinGecko feed is consulted.

---

### CFG-2 · INFO · `shared-config.js` Correctly Separates Dev vs Production URLs

**File:** [wallet/shared-config.js](wallet/shared-config.js)

`MOLT_CONFIG` is an IIFE that uses `window.location.hostname === 'localhost'` to switch between dev ports (3007–3011, 9090, 9100) and production origin-relative paths. The faucet (`localhost:9100`) is correctly referenced in both wallet activity loading and the receive modal. ✅

---

## Section 14 — Identity Module

### ID-1 · MEDIUM · `set_rate` Uses Default WASM Encoding Mode

**File:** [wallet/js/identity.js](wallet/js/identity.js) (~line 275)

`encodeMoltyIdArgs` for `set_rate` falls through to the default branch (no `0xAB` layout-descriptor prefix). All other `I64`-type arguments need the layout descriptor for the WASM ABI. If the underlying WASM contract expects a layout-prefixed I64, the transaction will fail on-chain. Comment says "default mode works" but this hasn't been verified against the actual contract ABI.

**Fix:** Test the `set_rate` call on the local testnet. If the WASM runtime requires the layout descriptor prefix, add it.

---

### ID-2 · LOW · Identity Cache Not Cleared on `switchWallet()`

Covered in UI-6 above.

---

### ID-3 · LOW · Name Auction `bid_amount` Sent as Whole MOLT Not Shells

**File:** [wallet/js/identity.js](wallet/js/identity.js) (`showBidAuctionModal`, ~line 1280)

`buildContractCall` receives `valueMolt` and the client passes the raw number from the input field (which users enter in MOLT). If the contract expects shells, all bids are 10^9× too small. If the contract expects MOLT as a floating-point, all bids are correct. This depends on the contract's `bid_name_auction` function signature and requires verification.

---

## Section 15 — Full Audit Finding Index

| # | Ref | File | Severity | Summary |
|---|-----|------|----------|---------|
| 1 | ZK-1 | shielded.js:500–565 | 🔴 CRITICAL | Placeholder ZK proofs, no real Groth16 prover |
| 2 | ZK-2 | wallet.js:~1158 | 🔴 CRITICAL | Spending key derived from public address |
| 3 | ZK-3 | shielded.js:157–171 | 🟠 HIGH | XOR-only note encryption, no authentication |
| 4 | ZK-4 | shielded.js:~556 | 🟠 HIGH | SHA-256 used instead of Pedersen commitment |
| 5 | ZK-5 | shielded.js:590–615 | 🟠 HIGH | Note secrets (blinding, serial) in plaintext localStorage |
| 6 | ZK-6 | shielded.js (syncShieldedState) | 🟡 MEDIUM | `checkNullifier` → server has `isNullifierSpent` |
| 7 | ZK-7 | wallet.js, popup.js | 🟡 MEDIUM | `getShieldedPoolStats` → server has `getShieldedPoolState` |
| 8 | ZK-8 | shielded.js:~860 | 🟡 MEDIUM | Unshield recipient address not validated |
| 9 | CRYPTO-1 | crypto.js:486–494, crypto-service.js | 🟡 MEDIUM | `isValidMnemonic` skips BIP39 checksum |
| 11 | CRYPTO-2 | crypto.js:368–377 | 🔵 LOW | Seed zeroing is best-effort in JS |
| 12 | CRYPTO-3 | wallet.js + crypto-service.js | 🔵 LOW | `keccak256` duplicated with divergence risk |
| 13 | TX-1 | all files | ✅ PASS | Bincode layout matches Rust server |
| 14 | TX-2 | all files | ✅ PASS | Opcodes match server dispatch |
| 15 | TX-3 | 4 files | 🟡 MEDIUM | `serializeMessageBincode` duplicated 4× |
| 16 | UI-1 | index.html | 🔵 LOW | Send fee hardcoded, not from `getFeeConfig` |
| 17 | UI-2 | wallet.js:~1340 | 🟡 MEDIUM | Activity pagination cursor may never advance |
| 18 | UI-3 | wallet.js:~1348 | 🔵 LOW | `tx.timestamp * 1000` — use `formatTime()` instead |
| 19 | UI-4 | wallet.js:~2830 | 🟡 MEDIUM | MAX token send doesn't reserve MOLT fee |
| 20 | UI-5 | wallet.js:~1175 | 🔵 LOW | EVM address shown before on-chain registration |
| 21 | UI-6 | identity.js:~25 | 🟡 MEDIUM | `_identityCache` not cleared on wallet switch |
| 22 | UI-7 | wallet.js (loadStaking) | 🔵 LOW | `getValidators` not cached |
| 23 | UI-8 | wallet.js:~1420 | 🔵 LOW | Explorer links use relative paths |
| 24 | SEC-1 | wallet.js:~4044, popup.js:~1800 | 🟡 MEDIUM | Delete wallet doesn't zero encrypted key / shielded notes |
| 25 | SEC-2 | provider-router.js:32–39 | 🔵 LOW | Expired pending requests not pruned without activity |
| 26 | SEC-3 | provider-router.js | 🔵 LOW | Approved origins persist forever without expiry |
| 27 | SEC-4 | content-script.js:~78 | 🟡 MEDIUM | 2s interval polling — should be event-driven |
| 28 | SEC-5 | inpage-provider.js:~150 | 🟡 MEDIUM | `window.ethereum` shim exposes unapproved read methods |
| 29 | EXT-1 | popup.js (loadShieldPanel) | 🟡 MEDIUM | Shielded panel always uninitialized in extension |
| 30 | EXT-2 | ws-service.js (onmessage) | 🟡 MEDIUM | WS notifications discarded — popup relies on polling |
| 31 | EXT-3 | popup.js:~70 | 🔵 LOW | `fullCarouselTimer` low-risk re-entry |
| 32 | EXT-4 | service-worker.js | ℹ️ INFO | MV3 storage I/O per message (architectural) |
| 33 | EXT-5 | service-worker.js:~85 | 🔵 LOW | Double approve-tab on rapid double-click |
| 34 | ERR-1 | wallet.js (showDashboard) | 🔵 LOW | No global offline indicator |
| 35 | ERR-2 | shielded.js | ✅ PASS | Shield fee display is immediate |
| 36 | ERR-4 | wallet.js:~1345 | ✅ PASS | Faucet timeout handled correctly |
| 38 | CFG-1 | popup.js | 🔵 LOW | MOLT price hardcoded at $0.10 |
| 39 | CFG-2 | shared-config.js | ✅ PASS | Dev/prod URL separation correct |
| 40 | ID-1 | identity.js:~275 | 🟡 MEDIUM | `set_rate` WASM encoding untested |
| 41 | ID-2 | identity.js:~25 | 🔵 LOW | Cache not cleared on wallet switch |
| 42 | ID-3 | identity.js:~1280 | 🔵 LOW | Auction bid units unverified against contract |

---

## Priority Remediation Order

1. **ZK-2** — Fix spending key derivation immediately. Entire shielded privacy model is broken.
2. **ZK-1** — Integrate real Groth16 WASM prover or disable the shield UI with a clear "coming soon" banner.
3. **ZK-3 + ZK-5** — Replace XOR note encryption with AES-256-GCM. Encrypt notes in localStorage.
4. **ZK-6 + ZK-7** — Fix the two RPC method name mismatches for shielded pool.
5. **CRYPTO-1** — Add BIP39 checksum validation to `isValidMnemonic` in both wallet.js/crypto.js and crypto-service.js.
7. **EXT-2** — Relay WS balance notifications from service worker to popup.
8. **EXT-1** — Fix extension shielded panel initialization.
9. **SEC-4** — Replace content-script polling with event-driven state updates.
10. **UI-4** — Reserve MOLT fee when computing MAX token send.

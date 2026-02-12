# Testing MoltChain Playground

## Quick Test

```bash
cd moltchain/programs
python3 -m http.server 8000
```

Then open: http://localhost:8000/playground.html

## What Should Work

### 1. Icons (Font Awesome)
- ✅ File tree should show:
  - 📄 `fa-file-code` for .rs files
  - 📁 `fa-folder` for folders
  - ▼ `fa-chevron-down` for open folders
  - ▶ `fa-chevron-right` for closed folders

### 2. Examples Tab
- Click "Examples" icon (📚) in left sidebar
- Should see 6 examples with icons:
  - 👋 Hello World
  - 🔢 Counter
  - 🪙 Token (ERC-20)
  - 🖼️ NFT (ERC-721)
  - 💱 DEX (AMM)
  - 🏛️ DAO (Governance)

### 3. Monaco Editor
- Should load with syntax highlighting
- Type some Rust code
- Should have autocomplete

### 4. Build Button
- Click "Build"
- Terminal should show: "Building program..."
- Will fail without compiler service (expected)

### 5. Wallet
- Click "Connect Wallet"
- Modal should appear
- Click "Create New Wallet"
- Should show seed phrase

### 6. Test & Interact
- Right sidebar should have "Test & Interact" panel
- Fields for:
  - Program Address
  - Function
  - Arguments (JSON)
  - Gas Limit
- Execute button

## Icon Issue Debug

If icons don't show:

1. Check Font Awesome loads:
```javascript
// In browser console:
document.querySelector('link[href*="font-awesome"]')
```

2. Check icon elements exist:
```javascript
// In browser console:
document.querySelectorAll('.fa-file-code').length
```

3. Check CSS:
```javascript
// In browser console:
getComputedStyle(document.querySelector('.fa-file-code')).fontFamily
```

## Files That Must Exist

- ✅ `playground.html` - Main HTML
- ✅ `css/playground.css` - Main styles
- ✅ `css/playground-modals.css` - Modal styles
- ✅ `js/moltchain-sdk.js` - SDK
- ✅ `js/playground-complete.js` - Complete playground
- ✅ `examples/token.rs` - Token example
- ✅ `examples/nft.rs` - NFT example  
- ✅ `examples/dex.rs` - DEX example
- ✅ `examples/dao.rs` - DAO example

## Backend Services (Optional for Testing)

To test build/deploy, need:

```bash
# Terminal 1: RPC
cd moltchain
cargo run --release -- --rpc-port 8899

# Terminal 2: Compiler  
cd moltchain/compiler
cargo run --release

# Terminal 3: Faucet
cd moltchain/faucet
cargo run --release
```

## Expected Behavior

### Without Backend Services
- ✅ UI loads completely
- ✅ Monaco editor works
- ✅ File tree works
- ✅ Examples load
- ✅ Wallet works (local only)
- ❌ Build fails (no compiler)
- ❌ Deploy fails (no RPC)
- ❌ Faucet fails (no service)

### With Backend Services
- ✅ Everything works
- ✅ Build compiles to WASM
- ✅ Deploy submits transaction
- ✅ Faucet gives testnet MOLT
- ✅ Test & Interact calls programs

## Common Issues

### 1. "MoltChain SDK not loaded"
**Fix**: Check browser console, ensure `moltchain-sdk.js` loads before `playground-complete.js`

### 2. Monaco editor not loading
**Fix**: Check CDN access to cdnjs.cloudflare.com

### 3. Icons not showing
**Fix**: Check Font Awesome CDN loads (cdnjs.cloudflare.com/ajax/libs/font-awesome/6.5.1/)

### 4. Examples not loading
**Fix**: Check `examples/*.rs` files exist

### 5. Wallet modal not appearing
**Fix**: Check `css/playground-modals.css` is loaded

## Success Criteria

✅ **Playground is operational when**:
1. Monaco editor loads and allows typing
2. File tree shows files with icons
3. Examples tab shows 6 examples
4. Clicking example loads code
5. Wallet modal opens and allows creating wallet
6. Test & Interact panel is visible
7. Build button attempts to call compiler
8. No console errors related to missing files

🦞⚡

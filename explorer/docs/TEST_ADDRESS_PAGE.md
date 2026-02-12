# Quick Test Guide - Address Page 🧪

## Start Server
```bash
cd moltchain/explorer
python3 -m http.server 8001
```

---

## Test URLs

### 1. Regular User Address
```
http://localhost:8001/address.html?address=MOLT1234567890
```
**Expected**:
- Status: Active (green)
- Account Type: User
- Balance: ~100-10,000 MOLT
- Executable: No
- Owner: SystemProgram...
- Transaction history: 5-20 txs

---

### 2. System Program
```
http://localhost:8001/address.html?address=SystemProgram11111111111111111111111111
```
**Expected**:
- Status: Active (green)
- Account Type: System
- Balance: 0 MOLT
- Executable: Yes
- Owner: (self-owned)
- Transaction history: Many txs

---

### 3. EVM Format Address
```
http://localhost:8001/address.html?address=0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb
```
**Expected**:
- Both Base58 and EVM formats shown
- Copy buttons work for both
- All other data present

---

### 4. No Address (Error Case)
```
http://localhost:8001/address.html
```
**Expected**:
- Error page displayed
- "No address provided" message
- Back to Dashboard button

---

## Visual Checks

### Page Header ✅
- [ ] Logo in top left
- [ ] Navigation menu (Dashboard, Blocks, Transactions, Validators)
- [ ] Search bar on right
- [ ] **Large space after header** (8rem/128px)
- [ ] Breadcrumb: Home > Address
- [ ] Page title with wallet icon
- [ ] Status badge (Active/Inactive)

### Quick Stats (4 cards) ✅
- [ ] Balance (X MOLT)
- [ ] Token Balance (X tokens)
- [ ] Transactions (X)
- [ ] Account Type (User/Program/System)
- [ ] Cards have hover effect
- [ ] Orange glow on hover

### Account Information Card ✅
- [ ] Card header with icon
- [ ] 8 rows of key-value pairs
- [ ] Address (Base58) with copy button
- [ ] Address (EVM) with copy button
- [ ] Both balances (MOLT and shells)
- [ ] Owner program (linkable if not system)
- [ ] Executable badge (Yes/No)
- [ ] Data Size (bytes/KB/MB)
- [ ] Rent Epoch number

### Transaction History Table ✅
- [ ] Table header with count
- [ ] 7 columns (Hash, Block, Age, From/To, Type, Amount, Status)
- [ ] Transaction hashes are links
- [ ] Block numbers are links
- [ ] Direction badge (IN green / OUT red)
- [ ] Other address is link
- [ ] Amount has +/- with color
- [ ] Status icons (check/x)
- [ ] Hover effect on rows

### Raw Account Data ✅
- [ ] Card header has title AND copy button
- [ ] **Copy button is on the RIGHT**
- [ ] Code block with JSON
- [ ] Scrollbar if content is long
- [ ] Dark background
- [ ] Monospace font

### Footer ✅
- [ ] Copyright text
- [ ] RPC endpoint shown
- [ ] Bottom of page

---

## Functional Tests

### Copy Buttons
1. Click "Copy" button next to Base58 address
   - [ ] Button shows "Copied!" feedback
   - [ ] Button turns green briefly
   - [ ] Clipboard contains full Base58 address
   
2. Click "Copy" button next to EVM address
   - [ ] Button shows "Copied!" feedback
   - [ ] Clipboard contains full 0x... address
   
3. Click "Copy" button in Raw Data header
   - [ ] Button shows "Copied!" feedback
   - [ ] Clipboard contains full JSON data

### Links
1. Click transaction hash in history
   - [ ] Goes to transaction.html with correct hash

2. Click block number in history
   - [ ] Goes to block.html with correct slot

3. Click "From/To" address in history
   - [ ] Goes to address.html with correct address

4. Click owner program link (if not system)
   - [ ] Goes to address.html with owner address

### Search
1. Type a block number in search, press Enter
   - [ ] Goes to block.html

2. Type an address in search, press Enter
   - [ ] Stays on address.html with new address

3. Type a transaction hash in search, press Enter
   - [ ] Goes to transaction.html

### Responsive
1. Resize to tablet width (768-1024px)
   - [ ] 2-column quick stats
   - [ ] Navigation still visible
   - [ ] Tables scroll horizontally if needed

2. Resize to mobile width (<768px)
   - [ ] 1-column quick stats
   - [ ] Detail rows stack vertically
   - [ ] Table is compact with smaller font
   - [ ] Copy buttons still work

---

## Browser Console

### Should See:
```
🦞 Address page loading...
Loading address: MOLT1234567890
RPC failed, using mock data: [error]
✅ Address page ready
```

### Should NOT See:
- ❌ JavaScript errors
- ❌ CSS load failures
- ❌ 404 errors
- ❌ Undefined variable warnings

---

## Compare with Other Pages

### Consistency Check
Open these pages side-by-side:

1. **block.html** vs **address.html**
   - [ ] Same top spacing after header
   - [ ] Same breadcrumb style
   - [ ] Same card designs
   - [ ] Same copy button alignment (right)
   - [ ] Same badge colors
   - [ ] Same footer

2. **transaction.html** vs **address.html**
   - [ ] Same detail-stat cards
   - [ ] Same detail-row layout
   - [ ] Same code block styling
   - [ ] Same hover effects

---

## Known Mock Data Patterns

### User Addresses
- Balance: 100-10,000 MOLT (random)
- Executable: No
- Owner: SystemProgram
- Transactions: 10-500
- Type: User

### Program Addresses (ending in '111...')
- Executable: Yes
- Owner: Self
- Data: 1024 bytes
- Type: Program

### System Program
- Balance: 0
- Executable: Yes
- Transactions: 1,000,000
- Type: System

---

## Performance Check

### Load Time
- [ ] Page loads in < 1 second
- [ ] No layout shift
- [ ] Smooth animations

### Interactions
- [ ] Copy buttons respond instantly
- [ ] Links navigate immediately
- [ ] Hover effects are smooth
- [ ] No lag on scroll

---

## Pass Criteria

**Page passes if:**
- ✅ All visual elements match other detail pages
- ✅ 8rem top spacing confirmed
- ✅ Copy buttons work and aligned right
- ✅ All links navigate correctly
- ✅ Transaction direction colors correct (IN green, OUT red)
- ✅ Both address formats shown
- ✅ Responsive on mobile/tablet
- ✅ No console errors
- ✅ Mock data generates correctly

**SHIP IT!** 🦞⚡

---

**Trading Lobster**  
*Test everything. Ship perfection.*

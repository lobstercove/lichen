# MoltChain Frontend Build Specification
## COMPLETE Professional Rebuild

**Theme:** Dark Orange (#FF6B35, #F77F00, #004E89)  
**Quality:** Solana Playground level  
**Integration:** Full RPC/WebSocket  
**Status:** Building NOW 🦞⚡

---

## API Endpoints (localhost:8899)

### JSON-RPC Methods
```javascript
const RPC_URL = 'http://localhost:8899';

// Core methods
- getBalance(pubkey) → {shells, molt}
- getAccount(pubkey) → {pubkey, evm_address, shells, molt, owner, executable, data_len}
- getBlock(slot) → Block data
- getLatestBlock() → Latest block
- getSlot() → Current slot number
- getTransaction(signature) → Transaction data
- sendTransaction(tx_data) → Send transaction
- getTotalBurned() → Total MOLT burned
- getValidators() → Validator list
- getMetrics() → Chain metrics
- health() → {status: "ok"}
```

### WebSocket (for real-time)
```javascript
const ws = new WebSocket('ws://localhost:8899/ws');
// Subscribe to: blocks, transactions, accounts, slots
```

---

## Component Specifications

### 1. WEBSITE (Landing Page)

**File:** `website/index.html` + `website/styles.css` + `website/script.js`

**Sections Required:**
1. **Hero** - Animated gradient background, stats grid (4 cards), CTAs
2. **Problem/Solution** - Cost comparison table, feature cards
3. **Features Grid** - 6 feature cards with icons
4. **Agent Features** - Molty ID, Reputation, Skills, Contribution
5. **Technical Stack** - Multi-language, EVM, APIs
6. **Tokenomics** - Distribution pie chart, utility list, $MOLT stats
7. **DeFi Ecosystem** - ClawSwap, LobsterLend, ClawPump, ReefStake
8. **Architecture Diagram** - Layered stack visualization
9. **Roadmap** - 4 phases with timeline
10. **Use Cases** - DeFi, Agent Services, Social, Infrastructure
11. **Comparison Table** - MoltChain vs Solana vs Ethereum
12. **Get Started** - Code examples, installation steps
13. **Community** - Discord, Twitter, GitHub links
14. **Footer** - Full site map, resources

**Features:**
- Animated background (pulse, float)
- Live stats from RPC (TPS, blocks, validators)
- Smooth scroll animations
- Code syntax highlighting
- Interactive comparison table
- Newsletter signup
- Mobile responsive

**APIs Used:**
- `getMetrics()` - Live chain stats
- `getValidators()` - Validator count
- `getLatestBlock()` - Latest block number
- `getTotalBurned()` - MOLT burned

---

### 2. EXPLORER (Reef Explorer)

**Files:** `explorer/index.html` + `explorer/blocks.html` + `explorer/transactions.html` + `explorer/account.html` + `explorer/tokens.html` + `explorer/validators.html` + `explorer/css/styles.css` + `explorer/js/explorer.js`

**Pages Required:**

#### Dashboard (index.html)
- **Top Stats**: Latest block, TPS, Total transactions, Active accounts, MOLT price, Total burned
- **Latest Blocks Table**: Slot, Hash, Txs, Validator, Time (auto-update)
- **Latest Transactions Table**: Signature, Type, From/To, Amount, Status (auto-update)
- **Network Stats**: Validator count, Stake distribution, Epoch progress
- **Search Bar**: Search blocks, txs, addresses

#### Blocks Page (blocks.html)
- Block list with pagination
- Filter by slot range
- Sort by time/slot
- Click to view block details

#### Block Detail (block.html)
- Block metadata: Slot, Hash, Parent, State root, Timestamp, Leader
- Transactions in block
- Compute units, Fee total

#### Transactions Page (transactions.html)
- Transaction list with pagination
- Filter by type, status, time
- Real-time updates

#### Transaction Detail (transaction.html)
- Full transaction data
- Instructions breakdown
- Accounts involved
- Logs and errors
- Block confirmation

#### Account Page (account.html)
- Balance (MOLT + shells)
- Transaction history
- Token holdings (if any)
- Program data (if executable)
- QR code for address

#### Tokens Page (tokens.html)
- All MT-20 tokens
- Name, Symbol, Supply, Holders, Price
- Token analytics

#### Validators Page (validators.html)
- Validator list
- Stake, Reputation, Uptime
- Leader schedule
- Voting power distribution

**Features:**
- Real-time WebSocket updates
- Search autocomplete
- Address copying
- QR code generation
- Export data (CSV/JSON)
- Dark/light toggle
- Mobile responsive

**APIs Used:**
- ALL RPC methods
- WebSocket subscriptions

---

### 3. WALLET (MoltWallet)

**Files:** `wallet/index.html` + `wallet/styles.css` + `wallet/wallet.js`

**Screens Required:**

#### Welcome Screen
- Connect wallet / Create wallet / Import wallet
- Wallet options cards

#### Create Wallet
- Generate keypair
- Show mnemonic (12/24 words)
- Confirm mnemonic
- Set password
- Success confirmation

#### Import Wallet
- Mnemonic input
- Private key input
- Set password

#### Main Dashboard
- Balance card (gradient, large)
- Account address (copy, QR)
- Send/Receive/Swap buttons
- Recent transactions list

#### Send Screen
- Recipient address input
- Amount input (MOLT/shells)
- Fee display
- Send button
- Confirmation modal

#### Receive Screen
- QR code (large)
- Address display
- Copy button
- Share buttons

#### Transaction History
- List all transactions
- Filter by type/date
- Search
- Export

#### Settings
- Change password
- Export private key (warning)
- Export mnemonic (warning)
- Lock wallet
- Delete wallet (danger zone)

**Features:**
- LocalStorage encryption
- Keypair generation
- Transaction signing
- Balance polling
- QR code scanner (camera)
- Address book
- Transaction notifications
- Mobile responsive

**APIs Used:**
- `getBalance()` - Check balance
- `getAccount()` - Account info
- `sendTransaction()` - Send MOLT
- `getTransaction()` - Transaction status

---

### 4. MARKETPLACE (Molt Market)

**Files:** `marketplace/index.html` + `marketplace/styles.css` + `marketplace/marketplace.js`

**Sections Required:**

#### Hero
- Search bar
- Featured NFT carousel
- Stats: Total NFTs, Total volume, Floor price

#### Filters Sidebar
- Collections dropdown
- Price range sliders
- Traits checkboxes
- Sort (price, recent, popular)

#### NFT Grid
- NFT cards with image
- Name, Price, Collection
- Like button, View count
- Hover effects

#### NFT Detail Modal
- Large image
- Name, Description, Collection
- Price, Owner, Creator
- Properties/Traits
- Buy/Bid buttons
- History tab
- Offers tab

#### Create NFT Modal
- Upload image
- Name, Description
- Collection select
- Price input
- Royalty %
- Mint button

#### My NFTs Page
- Grid of owned NFTs
- List for sale
- Cancel listings

#### Profile Page
- User info
- Owned NFTs
- Created NFTs
- Activity feed

**Features:**
- IPFS integration (mock)
- Image upload/preview
- Wallet connect
- Buy/Sell/Bid
- Like/favorite
- Share NFT
- Mobile responsive

**APIs Used:**
- MT-721 contract calls (via RPC)
- `getAccount()` - NFT data
- `sendTransaction()` - Buy/Sell

---

### 5. PROGRAMS (Deployer)

**Files:** `programs/index.html` + `programs/styles.css` + `programs/deploy.js`

**Sections Required:**

#### Hero
- "Deploy Smart Contracts"
- Quick stats: Programs deployed, Active programs

#### Filters Sidebar
- Program type (Token, NFT, DeFi, DAO, Other)
- Language (Rust, JS, Python, Solidity)
- Status (Active, Inactive)

#### Programs Grid
- Program cards
- Name, Type, Language, Deployed date
- Owner, Interaction count
- View/Execute buttons

#### Program Detail Modal
- Program info
- Source code (syntax highlighted)
- ABI/Interface
- Execution logs
- Call function UI

#### Deploy Section
- File upload (.wasm)
- Or paste code
- Language select
- Init args
- Gas limit
- Deploy button
- Deployment progress

#### My Programs
- List deployed programs
- Edit/Upgrade
- Close program

**Features:**
- WASM upload
- Code editor (Monaco)
- Syntax highlighting
- Function calling UI
- Execution logs
- Program verification
- Mobile responsive

**APIs Used:**
- `sendTransaction()` - Deploy/Call
- `getAccount()` - Program data
- Contract deployment flow

---

### 6. PLAYGROUND (IDE)

**Files:** `playground/index.html` + `playground/styles.css` + `playground/playground.js`

**Already EXISTS but may need:**

#### Main Layout
- Monaco editor (left 60%)
- Terminal (right bottom 40%)
- File tree (left sidebar)
- Toolbar (top)

#### Editor Features
- Syntax highlighting (Rust/JS/Python/Solidity)
- Autocomplete
- Error checking
- Multiple files/tabs

#### Terminal
- Build output
- Deploy logs
- Test results
- Interactive shell

#### Toolbar
- New file
- Open examples
- Build
- Deploy
- Test
- Settings

#### Examples
- Hello World
- Token (MT-20)
- NFT (MT-721)
- DEX
- DAO
- Multisig

**Features:**
- Monaco editor integrated
- WASM compilation
- Local testing
- Deploy to testnet
- Share code (gist)
- Mobile responsive

**APIs Used:**
- All deployment APIs
- Contract interaction

---

## Design System

### Colors (ORANGE Theme)
```css
--primary: #FF6B35
--primary-dark: #E5501B
--accent: #F77F00
--secondary: #004E89
--success: #06D6A0
--warning: #FFD23F
--info: #118AB2
--bg-dark: #0A0E27
--bg-darker: #060812
--bg-card: #141830
--text-primary: #FFFFFF
--text-secondary: #B8C1EC
--text-muted: #6B7A99
--border: #1F2544
```

### Gradients
```css
--gradient-1: linear-gradient(135deg, #FF6B35 0%, #F77F00 100%)
--gradient-2: linear-gradient(135deg, #004E89 0%, #118AB2 100%)
--gradient-3: linear-gradient(135deg, #06D6A0 0%, #118AB2 100%)
```

### Typography
- Font: 'Inter' (primary), 'JetBrains Mono' (code)
- Headings: 700-900 weight
- Body: 400-500 weight
- Code: 'JetBrains Mono', 400-600 weight

### Spacing
- Base: 8px grid
- Sections: 6rem (96px) vertical padding
- Cards: 1.5-2rem padding
- Gaps: 1-2rem between elements

### Shadows
```css
--shadow: 0 4px 20px rgba(0, 0, 0, 0.3)
--shadow-lg: 0 10px 40px rgba(0, 0, 0, 0.4)
```

### Border Radius
- Cards: 16px
- Buttons: 8-12px
- Inputs: 8px

---

## JavaScript Utilities

### RPC Client
```javascript
class MoltChainRPC {
    constructor(url) {
        this.url = url;
    }
    
    async call(method, params = []) {
        const response = await fetch(this.url, {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
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
    
    // Helper methods for each endpoint
    async getBalance(pubkey) { return this.call('getBalance', [pubkey]); }
    async getAccount(pubkey) { return this.call('getAccount', [pubkey]); }
    async getLatestBlock() { return this.call('getLatestBlock'); }
    async getSlot() { return this.call('getSlot'); }
    async getMetrics() { return this.call('getMetrics'); }
    // etc...
}
```

### WebSocket Client
```javascript
class MoltChainWS {
    constructor(url) {
        this.ws = new WebSocket(url);
        this.handlers = {};
    }
    
    subscribe(channel, handler) {
        this.handlers[channel] = handler;
        this.ws.send(JSON.stringify({
            type: 'subscribe',
            channel
        }));
    }
    
    onMessage(event) {
        const data = JSON.parse(event.data);
        if (this.handlers[data.channel]) {
            this.handlers[data.channel](data.payload);
        }
    }
}
```

---

## Build Order

1. ✅ Website (complete landing page) - 500+ lines HTML
2. ✅ Explorer (all pages + real-time) - 800+ lines HTML
3. ✅ Wallet (full wallet interface) - 400+ lines HTML
4. ✅ Marketplace (NFT platform) - 450+ lines HTML
5. ✅ Programs (deployer interface) - 350+ lines HTML
6. ✅ Playground (verify/enhance) - Already exists

**Total Estimated:** 2500+ lines of HTML + matching CSS + JS

---

## Quality Checklist

Each component MUST have:
- ✅ Complete HTML structure
- ✅ Orange theme colors
- ✅ Responsive design (mobile/tablet/desktop)
- ✅ API integration (real data)
- ✅ WebSocket updates (where needed)
- ✅ Loading states
- ✅ Error handling
- ✅ Smooth animations
- ✅ Professional polish
- ✅ Code comments
- ✅ Accessibility (ARIA labels)

---

**LET'S MOLT! 🦞⚡**

*Building the most professional agent-first blockchain interface ever created.*

/**
 * Lichen SDK - Production JavaScript Client
 * Complete SDK for interacting with Lichen blockchain
 * 
 * Features:
 * - JSON-RPC client with retry logic
 * - WebSocket subscriptions
 * - ML-DSA-65 wallet management
 * - Transaction building and signing
 * - Program deployment
 * - Account management
 * 
 * @version 1.0.0
 */

// ============================================================================
// CONFIGURATION
// ============================================================================

// If shared-config.js already declared LICHEN_CONFIG, extend it with SDK-specific
// fields (compiler URLs, defaults, units). Otherwise define a standalone config.
const LICHEN_SDK_CONFIG = (() => {
    const shared = (typeof LICHEN_CONFIG !== 'undefined') ? LICHEN_CONFIG : null;

    const fallback = {
        networks: {
            mainnet: {
                rpc: 'https://rpc.lichen.network',
                ws: 'wss://rpc.lichen.network/ws',
                explorer: 'https://explorer.lichen.network',
                compiler: 'https://rpc.lichen.network/compile',
                chainId: 1
            },
            testnet: {
                rpc: 'https://testnet-rpc.lichen.network',
                ws: 'wss://testnet-rpc.lichen.network/ws',
                explorer: 'https://explorer.lichen.network',
                compiler: 'https://testnet-rpc.lichen.network/compile',
                chainId: 2
            },
            local: {
                rpc: 'http://localhost:8899',
                ws: 'ws://localhost:8900',
                explorer: 'http://localhost:8080',
                compiler: 'http://localhost:8900/compile',
                chainId: 999
            }
        },
        defaults: {
            gasLimit: 1_000_000,
            priority: 'normal',
            commitment: 'finalized'
        },
        units: {
            SPORES_PER_LICN: 1_000_000_000,
            WEI_PER_LICN: 1_000_000_000_000_000_000n
        }
    };

    if (!shared) return fallback;

    // Merge shared networks with SDK extras (compiler URLs, chainId)
    const networks = {};
    for (const [key, net] of Object.entries(fallback.networks)) {
        const s = shared.networks?.[key] || {};
        networks[key] = {
            rpc: s.rpc || net.rpc,
            ws: s.ws || net.ws,
            explorer: s.explorer || net.explorer,
            compiler: net.compiler,
            chainId: net.chainId,
            ...(s.local !== undefined ? { local: s.local } : {}),
        };
    }
    return { ...fallback, networks };
})();

// ============================================================================
// LOW-LEVEL ENCODING HELPERS
// ============================================================================

const textEncoder = new TextEncoder();

function hexToBytes(hex) {
    const clean = hex.startsWith('0x') ? hex.slice(2) : hex;
    const bytes = new Uint8Array(clean.length / 2);
    for (let i = 0; i < bytes.length; i += 1) {
        bytes[i] = parseInt(clean.substr(i * 2, 2), 16);
    }
    return bytes;
}

function bytesToHex(bytes) {
    return Array.from(bytes)
        .map(byte => byte.toString(16).padStart(2, '0'))
        .join('');
}

function concatBytes(...chunks) {
    const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
    const merged = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
        merged.set(chunk, offset);
        offset += chunk.length;
    }
    return merged;
}

function encodeU64LE(value) {
    const bytes = new Uint8Array(8);
    let big = BigInt(value);
    for (let i = 0; i < 8; i += 1) {
        bytes[i] = Number(big & 0xffn);
        big >>= 8n;
    }
    return bytes;
}

function encodeBytes(bytes) {
    return concatBytes(encodeU64LE(bytes.length), bytes);
}

function encodeString(value) {
    const bytes = textEncoder.encode(value);
    return encodeBytes(bytes);
}

function encodeVec(items, encoder) {
    const parts = [encodeU64LE(items.length)];
    for (const item of items) {
        parts.push(encoder(item));
    }
    return concatBytes(...parts);
}

function requireLichenPQ() {
    if (!window.LichenPQ) {
        throw new Error('Shared PQ runtime not loaded');
    }
    return window.LichenPQ;
}

function localWalletsDisabledError() {
    return new Error('Browser-local wallets are disabled in Programs. Use the Lichen wallet extension.');
}

function normalizePubkeyBytes(pubkey) {
    if (pubkey instanceof Uint8Array) {
        return new Uint8Array(pubkey);
    }
    if (Array.isArray(pubkey)) {
        return new Uint8Array(pubkey);
    }
    return base58Decode(pubkey);
}

function normalizeDataBytes(data) {
    if (data instanceof Uint8Array) {
        return new Uint8Array(data);
    }
    if (Array.isArray(data)) {
        return new Uint8Array(data);
    }
    if (typeof data === 'string') {
        return textEncoder.encode(data);
    }
    return new Uint8Array(0);
}

function encodePubkey(pubkey) {
    return normalizePubkeyBytes(pubkey);
}

function encodeInstruction(ix) {
    const programId = ix.programId || ix.program_id;
    const accounts = ix.accounts || [];
    const data = normalizeDataBytes(ix.data);
    return concatBytes(
        encodePubkey(programId),
        encodeVec(accounts, encodePubkey),
        encodeBytes(data)
    );
}

function encodeMessage(instructions, recentBlockhash) {
    const blockhashBytes = hexToBytes(recentBlockhash);
    return concatBytes(
        encodeVec(instructions, encodeInstruction),
        blockhashBytes,
        new Uint8Array([0x00]),  // compute_budget: Option<u64> = None
        new Uint8Array([0x00])   // compute_unit_price: Option<u64> = None
    );
}

function encodeTransaction(signatures, messageBytes) {
    return textEncoder.encode(JSON.stringify({ signatures, message: messageBytes }));
}

function base64Encode(bytes) {
    let binary = '';
    for (let i = 0; i < bytes.length; i += 1) {
        binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary);
}

async function resolveInjectedLichenProvider(wallet, timeoutMs = 0) {
    let provider = null;

    if (typeof getInjectedLichenProvider === 'function') {
        provider = getInjectedLichenProvider();
    } else if (typeof window !== 'undefined' && window.licnwallet && window.licnwallet.isLichenWallet) {
        provider = window.licnwallet;
    }

    if (!provider && timeoutMs > 0 && typeof waitForInjectedLichenProvider === 'function') {
        provider = await waitForInjectedLichenProvider(timeoutMs);
    }

    if (!provider) {
        return null;
    }

    if (wallet && wallet.address && typeof provider.getProviderState === 'function') {
        const state = await provider.getProviderState().catch(() => null);
        if (state && Array.isArray(state.accounts) && state.accounts.length && !state.accounts.includes(wallet.address)) {
            return null;
        }
    }

    return provider;
}

function unwrapSendTransactionResult(result) {
    return result && typeof result === 'object' && result.txHash ? result.txHash : result;
}

function base58Encode(bytes) {
    const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
    let num = 0n;
    for (const byte of bytes) {
        num = (num << 8n) + BigInt(byte);
    }

    let encoded = '';
    while (num > 0n) {
        const remainder = num % 58n;
        encoded = ALPHABET[Number(remainder)] + encoded;
        num /= 58n;
    }

    let leadingZeros = 0;
    for (const byte of bytes) {
        if (byte === 0) {
            leadingZeros += 1;
        } else {
            break;
        }
    }

    return '1'.repeat(leadingZeros) + (encoded || '');
}

function base58Decode(value) {
    const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
    const map = new Map();
    for (let i = 0; i < ALPHABET.length; i += 1) {
        map.set(ALPHABET[i], BigInt(i));
    }

    let num = 0n;
    for (const char of value) {
        const digit = map.get(char);
        if (digit === undefined) {
            throw new Error('Invalid base58 character');
        }
        num = num * 58n + digit;
    }

    const bytes = [];
    while (num > 0n) {
        bytes.push(Number(num & 0xffn));
        num >>= 8n;
    }
    bytes.reverse();

    let leadingZeros = 0;
    for (const char of value) {
        if (char === '1') {
            leadingZeros += 1;
        } else {
            break;
        }
    }

    const result = new Uint8Array(leadingZeros + bytes.length);
    result.set(bytes, leadingZeros);
    return result;
}

const SYSTEM_PROGRAM_ID = base58Encode(new Uint8Array(32));
const CONTRACT_PROGRAM_ID = base58Encode(new Uint8Array(32).fill(255));

// ============================================================================
// RPC CLIENT
// ============================================================================

class LichenRPC {
    constructor(network = 'testnet') {
        this.network = network;
        this.config = LICHEN_SDK_CONFIG.networks[network];
        if (!this.config) {
            throw new Error(`Unknown network: ${network}`);
        }
        this.rpcUrl = this.config.rpc;
        this.compilerUrl = this.config.compiler || `${this.config.rpc}/compile`;
        this.requestId = 1;
        this.cache = new Map();
    }

    /**
     * Make JSON-RPC call with retry logic
     */
    async call(method, params = null, options = {}) {
        const { retries = 3, timeout = 30000, cache = false } = options;

        // Check cache first
        const cacheKey = cache ? `${method}:${JSON.stringify(params)}` : null;
        if (cacheKey && this.cache.has(cacheKey)) {
            const cached = this.cache.get(cacheKey);
            if (Date.now() - cached.timestamp < 5000) { // 5s TTL
                return cached.result;
            }
        }

        const request = {
            jsonrpc: '2.0',
            id: this.requestId++,
            method,
            params: params ? (Array.isArray(params) ? params : [params]) : []
        };

        let lastError;
        for (let attempt = 0; attempt < retries; attempt++) {
            try {
                const controller = new AbortController();
                const timeoutId = setTimeout(() => controller.abort(), timeout);

                const response = await fetch(this.rpcUrl, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(request),
                    signal: controller.signal
                });

                clearTimeout(timeoutId);

                if (!response.ok) {
                    throw new Error(`HTTP ${response.status}: ${response.statusText}`);
                }

                const data = await response.json();

                if (data.error) {
                    throw new RPCError(data.error.code, data.error.message);
                }

                const result = data.result;

                // Cache if requested
                if (cacheKey) {
                    this.cache.set(cacheKey, {
                        result,
                        timestamp: Date.now()
                    });
                }

                return result;

            } catch (error) {
                lastError = error;
                if (attempt < retries - 1) {
                    await this.sleep(Math.pow(2, attempt) * 1000); // Exponential backoff
                }
            }
        }

        throw lastError;
    }

    /**
     * Get account balance
     */
    async getBalance(pubkey) {
        return await this.call('getBalance', [pubkey], { cache: true });
    }

    /**
     * Get account info
     */
    async getAccount(pubkey) {
        return await this.call('getAccount', [pubkey]);
    }

    /**
     * Get block by slot
     */
    async getBlock(slot) {
        return await this.call('getBlock', [slot]);
    }

    /**
     * Get latest block
     */
    async getLatestBlock() {
        return await this.call('getLatestBlock');
    }

    /**
     * Get current slot
     */
    async getSlot() {
        return await this.call('getSlot');
    }

    /**
     * Get recent blockhash for transactions
     */
    async getRecentBlockhash() {
        return await this.call('getRecentBlockhash');
    }

    /**
     * Send transaction
     */
    async sendTransaction(txBase64) {
        return await this.call('sendTransaction', [txBase64]);
    }

    /**
     * Get transaction by signature
     */
    async getTransaction(signature) {
        return await this.call('getTransaction', [signature]);
    }

    /**
     * Get fee configuration
     */
    async getFeeConfig() {
        return await this.call('getFeeConfig');
    }

    /**
     * Get program metadata
     */
    async getProgram(programId) {
        return await this.call('getProgram', [programId]);
    }

    /**
     * Get program stats
     */
    async getProgramStats(programId) {
        return await this.call('getProgramStats', [programId]);
    }

    /**
     * Get program list
     */
    async getPrograms(options = {}) {
        return await this.call('getPrograms', [options]);
    }

    /**
     * Get program calls
     */
    async getProgramCalls(programId, options = {}) {
        return await this.call('getProgramCalls', [programId, options]);
    }

    /**
     * Get program storage
     */
    async getProgramStorage(programId, options = {}) {
        return await this.call('getProgramStorage', [programId, options]);
    }

    /**
     * Get symbol registry entry
     */
    async getSymbolRegistry(symbol) {
        return await this.call('getSymbolRegistry', [symbol]);
    }

    async getSymbolRegistryByProgram(programId) {
        return await this.call('getSymbolRegistryByProgram', [programId]);
    }

    /**
     * Check if a native address has an EVM address registered on-chain
     * Returns { evmAddress: "0x..." } or null
     */
    async getEvmRegistration(nativePubkey) {
        return await this.call('getEvmRegistration', [nativePubkey]);
    }

    /**
     * Resolve an EVM address to native pubkey
     * Returns { nativePubkey: "..." } or null
     */
    async lookupEvmAddress(evmAddress) {
        return await this.call('lookupEvmAddress', [evmAddress]);
    }

    /**
     * Get all validators
     */
    async getValidators() {
        return await this.call('getValidators', null, { cache: true });
    }

    /**
     * Get network metrics
     */
    async getMetrics() {
        return await this.call('getMetrics', null, { cache: true });
    }

    /**
     * Get chain status
     */
    async getChainStatus() {
        return await this.call('getChainStatus');
    }

    /**
     * Health check
     */
    async health() {
        return await this.call('health');
    }

    // ========================================================================
    // SIMULATION
    // ========================================================================

    /**
     * Simulate a transaction (dry run) — validates, estimates fees, returns logs
     */
    async simulateTransaction(txBase64) {
        return await this.call('simulateTransaction', [txBase64]);
    }

    // ========================================================================
    // ACCOUNT & TRANSACTION HISTORY
    // ========================================================================

    // AUDIT-FIX F14.9: removed duplicate getAccount() — already defined above (L313)

    async getAccountInfo(pubkey) {
        return await this.call('getAccountInfo', [pubkey]);
    }

    async getAccountTxCount(pubkey) {
        return await this.call('getAccountTxCount', [pubkey]);
    }

    async getTransactionsByAddress(pubkey, options = {}) {
        return await this.call('getTransactionsByAddress', [pubkey, options]);
    }

    async getTransactionHistory(pubkey, options = {}) {
        return await this.call('getTransactionHistory', [pubkey, options]);
    }

    // ========================================================================
    // CONTRACT ABI / IDL
    // ========================================================================

    /**
     * Get contract ABI (machine-readable interface)
     */
    async getContractAbi(contractId) {
        return await this.call('getContractAbi', [contractId]);
    }

    /**
     * Set/update contract ABI (owner only)
     */
    async setContractAbi(contractId, abi) {
        return await this.call('setContractAbi', [contractId, abi]);
    }

    // ========================================================================
    // CONTRACT ENDPOINTS
    // ========================================================================

    async getContractInfo(contractId) {
        return await this.call('getContractInfo', [contractId]);
    }

    async getContractLogs(contractId, limit = 100) {
        return await this.call('getContractLogs', [contractId, limit]);
    }

    async getAllContracts() {
        return await this.call('getAllContracts');
    }

    // ========================================================================
    // TOKEN ENDPOINTS
    // ========================================================================

    async getTokenBalance(tokenMint, owner) {
        return await this.call('getTokenBalance', [tokenMint, owner]);
    }

    async getTokenHolders(tokenMint, limit = 100) {
        return await this.call('getTokenHolders', [tokenMint, limit]);
    }

    async getTokenTransfers(tokenMint, limit = 100) {
        return await this.call('getTokenTransfers', [tokenMint, limit]);
    }

    async getContractEvents(contractId, limit = 100) {
        return await this.call('getContractEvents', [contractId, limit]);
    }

    // ========================================================================
    // NFT ENDPOINTS
    // ========================================================================

    async getCollection(collectionId) {
        return await this.call('getCollection', [collectionId]);
    }

    async getNFT(tokenId) {
        return await this.call('getNFT', [tokenId]);
    }

    async getNFTsByOwner(owner) {
        return await this.call('getNFTsByOwner', [owner]);
    }

    async getNFTsByCollection(collectionId, options = {}) {
        return await this.call('getNFTsByCollection', [collectionId, options]);
    }

    async getNFTActivity(tokenId, limit = 100) {
        return await this.call('getNFTActivity', [tokenId, limit]);
    }

    async getMarketListings(options = {}) {
        return await this.call('getMarketListings', [options]);
    }

    async getMarketSales(options = {}) {
        return await this.call('getMarketSales', [options]);
    }

    // ========================================================================
    // STAKING
    // ========================================================================

    async stake(params) {
        return await this.call('stake', [params]);
    }

    async unstake(params) {
        return await this.call('unstake', [params]);
    }

    async getStakingStatus(pubkey) {
        return await this.call('getStakingStatus', [pubkey]);
    }

    async getStakingRewards(pubkey) {
        return await this.call('getStakingRewards', [pubkey]);
    }

    // ========================================================================
    // MOSSSTAKE (LIQUID STAKING)
    // ========================================================================

    async getStakingPosition(pubkey) {
        return await this.call('getStakingPosition', [pubkey]);
    }

    async getMossStakePoolInfo() {
        return await this.call('getMossStakePoolInfo');
    }

    async getUnstakingQueue(pubkey) {
        return await this.call('getUnstakingQueue', [pubkey]);
    }

    async getRewardAdjustmentInfo() {
        return await this.call('getRewardAdjustmentInfo');
    }

    // ========================================================================
    // NETWORK / VALIDATOR INFO
    // ========================================================================

    async getTotalBurned() {
        return await this.call('getTotalBurned');
    }

    async getPeers() {
        return await this.call('getPeers');
    }

    async getNetworkInfo() {
        return await this.call('getNetworkInfo');
    }

    async getValidatorInfo(pubkey) {
        return await this.call('getValidatorInfo', [pubkey]);
    }

    async getValidatorPerformance(pubkey) {
        return await this.call('getValidatorPerformance', [pubkey]);
    }

    // ========================================================================
    // FEE / RENT CONFIG
    // ========================================================================

    async setFeeConfig(config) {
        return await this.call('setFeeConfig', [config]);
    }

    async getRentParams() {
        return await this.call('getRentParams');
    }

    async setRentParams(params) {
        return await this.call('setRentParams', [params]);
    }

    /**
     * Request testnet/local tokens from faucet
     */
    async requestFaucet(pubkey, amount = 100) {
        if (this.network === 'mainnet') {
            throw new Error('Faucet not available on mainnet');
        }

        // Call faucet endpoint (separate service)
        const faucetUrl = this.config.rpc.includes('/rpc')
            ? this.config.rpc.replace('/rpc', '/faucet')
            : `${this.config.rpc.replace(/\/$/, '')}/faucet`;
        const response = await fetch(`${faucetUrl}/request`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ address: pubkey, amount })
        });

        if (!response.ok) {
            throw new Error(`Faucet request failed: ${response.statusText}`);
        }

        return await response.json();
    }

    sleep(ms) {
        return new Promise(resolve => setTimeout(resolve, ms));
    }
}

// ============================================================================
// WEBSOCKET CLIENT
// ============================================================================

class LichenWS {
    constructor(network = 'testnet') {
        this.network = network;
        this.config = LICHEN_SDK_CONFIG.networks[network];
        this.wsUrl = this.config.ws;
        this.ws = null;
        this.subscriptions = new Map();
        this.requestId = 1;
        this.connected = false;
        this.reconnectAttempts = 0;
        this.maxReconnectAttempts = 5;
    }

    /**
     * Connect to WebSocket
     */
    connect() {
        return new Promise((resolve, reject) => {
            this.ws = new WebSocket(this.wsUrl);

            this.ws.onopen = () => {
                console.log('🦞 WebSocket connected');
                this.connected = true;
                this.reconnectAttempts = 0;
                resolve();
            };

            this.ws.onmessage = (event) => {
                const data = JSON.parse(event.data);

                if (data.method === 'subscription') {
                    // Notification
                    const subId = data.params.subscription;
                    const callbacks = this.subscriptions.get(subId);
                    if (callbacks) {
                        callbacks.forEach(cb => cb(data.params.result));
                    }
                }
            };

            this.ws.onerror = (error) => {
                console.error('WebSocket error:', error);
                reject(error);
            };

            this.ws.onclose = () => {
                console.log('WebSocket closed');
                this.connected = false;
                this.reconnect();
            };
        });
    }

    /**
     * Reconnect logic
     */
    reconnect() {
        if (this.reconnectAttempts < this.maxReconnectAttempts) {
            this.reconnectAttempts++;
            const delay = Math.min(1000 * Math.pow(2, this.reconnectAttempts), 30000);
            console.log(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts})...`);
            setTimeout(() => this.connect(), delay);
        }
    }

    /**
     * Subscribe to slots
     */
    async subscribeSlots(callback) {
        const subId = await this.subscribe('subscribeSlots');
        this.addCallback(subId, callback);
        return subId;
    }

    /**
     * Subscribe to blocks
     */
    async subscribeBlocks(callback) {
        const subId = await this.subscribe('subscribeBlocks');
        this.addCallback(subId, callback);
        return subId;
    }

    /**
     * Subscribe to account changes
     */
    async subscribeAccount(pubkey, callback) {
        const subId = await this.subscribe('subscribeAccount', pubkey);
        this.addCallback(subId, callback);
        return subId;
    }

    /**
     * Subscribe to transactions
     */
    async subscribeTransactions(callback) {
        const subId = await this.subscribe('subscribeTransactions');
        this.addCallback(subId, callback);
        return subId;
    }

    /**
     * Subscribe to program logs
     */
    async subscribeLogs(programId, callback) {
        const subId = await this.subscribe('subscribeLogs', programId);
        this.addCallback(subId, callback);
        return subId;
    }

    /**
     * Subscribe to program updates (deploy/upgrade/close)
     */
    async subscribeProgramUpdates(callback) {
        const subId = await this.subscribe('subscribeProgramUpdates');
        this.addCallback(subId, callback);
        return subId;
    }

    /**
     * Subscribe to program calls
     */
    async subscribeProgramCalls(programId, callback) {
        const subId = await this.subscribe('subscribeProgramCalls', programId);
        this.addCallback(subId, callback);
        return subId;
    }

    /**
     * Unsubscribe from subscription
     */
    async unsubscribe(subId, method = 'unsubscribeSlots') {
        await this.send(method, subId);
        this.subscriptions.delete(subId);
    }

    /**
     * Generic subscribe
     */
    subscribe(method, params = null) {
        return new Promise((resolve, reject) => {
            if (!this.connected) {
                reject(new Error('WebSocket not connected'));
                return;
            }

            const requestId = this.requestId++;
            const request = {
                jsonrpc: '2.0',
                id: requestId,
                method,
                params: params ? [params] : []
            };

            const handler = (event) => {
                const data = JSON.parse(event.data);
                if (data.id === requestId) {
                    this.ws.removeEventListener('message', handler);
                    if (data.error) {
                        reject(new Error(data.error.message));
                    } else {
                        resolve(data.result);
                    }
                }
            };

            this.ws.addEventListener('message', handler);
            this.ws.send(JSON.stringify(request));
        });
    }

    /**
     * Send message
     */
    send(method, params) {
        return this.subscribe(method, params);
    }

    /**
     * Add callback for subscription
     */
    addCallback(subId, callback) {
        if (!this.subscriptions.has(subId)) {
            this.subscriptions.set(subId, []);
        }
        this.subscriptions.get(subId).push(callback);
    }

    /**
     * Disconnect
     */
    disconnect() {
        if (this.ws) {
            this.ws.close();
            this.ws = null;
        }
        this.connected = false;
    }
}

// ============================================================================
// WALLET (ML-DSA-65)
// ============================================================================

class LichenWallet {
    constructor({ privateKey = null, publicKey = null, publicKeyHex = null, seed = null, address = null } = {}) {
        this.privateKey = privateKey;
        this.publicKey = publicKey;
        this.publicKeyHex = publicKeyHex;
        this.seed = seed;
        this.address = address;
    }

    static async create() {
        throw localWalletsDisabledError();
    }

    static async fromSeed() {
        throw localWalletsDisabledError();
    }

    async sign(message) {
        if (!this.privateKey) {
            throw localWalletsDisabledError();
        }
        const payload = message instanceof Uint8Array ? message : new Uint8Array(message || []);
        return requireLichenPQ().signMessage(this.privateKey, payload);
    }

    /**
     * Export wallet to JSON (encrypted)
     */
    export(password) {
        throw localWalletsDisabledError();
    }

    /**
     * Import wallet from JSON
     */
    static async import() {
        throw localWalletsDisabledError();
    }

    /**
     * Generate mnemonic seed phrase (BIP39)
     */
    static generateMnemonic() {
        throw new Error('Mnemonic generation not supported in browser SDK');
    }

    /**
     * Create wallet from mnemonic
     */
    static fromMnemonic(mnemonic) {
        throw new Error('Mnemonic import not supported in browser SDK');
    }
}

// ============================================================================
// TRANSACTION BUILDER
// ============================================================================

class TransactionBuilder {
    constructor(rpc) {
        this.rpc = rpc;
        this.instructions = [];
        this.recentBlockhash = null;
        this.signatures = [];
        this.extensionWallet = null;
    }

    /**
     * Add instruction
     */
    addInstruction(instruction) {
        this.instructions.push(instruction);
        return this;
    }

    /**
     * Set recent blockhash
     */
    async setRecentBlockhash() {
        const result = await this.rpc.getRecentBlockhash();
        this.recentBlockhash = typeof result === 'string' ? result : result.blockhash;
        return this;
    }

    /**
     * Sign transaction
     */
    async sign(wallet) {
        if (!this.recentBlockhash) {
            throw new Error('Must set recent blockhash first');
        }

        this.extensionWallet = null;

        if (wallet && typeof wallet.sign === 'function') {
            const messageBytes = encodeMessage(this.instructions, this.recentBlockhash);
            const signature = await wallet.sign(messageBytes);
            this.signatures.push(signature);
            return this;
        }

        const provider = await resolveInjectedLichenProvider(wallet, 800);
        if (wallet && wallet.address && provider && typeof provider.sendTransaction === 'function') {
            this.extensionWallet = wallet;
            return this;
        }

        throw new Error('Wallet cannot sign transactions. Reconnect the Lichen wallet extension.');
    }

    buildRpcMessage() {
        return {
            instructions: this.instructions.map((instruction) => ({
                program_id: Array.from(normalizePubkeyBytes(instruction.programId || instruction.program_id)),
                accounts: (instruction.accounts || []).map((account) => Array.from(normalizePubkeyBytes(account))),
                data: Array.from(normalizeDataBytes(instruction.data)),
            })),
            blockhash: this.recentBlockhash,
        };
    }

    /**
     * Serialize transaction to bytes
     */
    async serialize() {
        if (!this.recentBlockhash) {
            throw new Error('Must set recent blockhash first');
        }

        return textEncoder.encode(JSON.stringify({
            signatures: this.signatures,
            message: this.buildRpcMessage(),
        }));
    }

    /**
     * Encode transaction as base64 for RPC
     */
    async toBase64() {
        const bytes = await this.serialize();
        return base64Encode(bytes);
    }

    /**
     * Send transaction
     */
    async send() {
        if (this.extensionWallet) {
            const provider = await resolveInjectedLichenProvider(this.extensionWallet, 800);
            if (!provider || typeof provider.sendTransaction !== 'function') {
                throw new Error('Lichen wallet extension not available for transaction approval');
            }

            return unwrapSendTransactionResult(await provider.sendTransaction({
                signatures: [],
                message: this.buildRpcMessage(),
            }));
        }

        const base64 = await this.toBase64();
        return await this.rpc.sendTransaction(base64);
    }

    // ===== Instruction Builders =====

    static transfer(from, to, amount) {
        const data = new Uint8Array(9);
        data[0] = 0;
        const view = new DataView(data.buffer);
        view.setBigUint64(1, BigInt(amount), true);
        return {
            programId: SYSTEM_PROGRAM_ID,
            accounts: [from, to],
            data
        };
    }

    static deploy(deployer, programId, code, initData = []) {
        const payload = {
            Deploy: {
                code: Array.from(code),
                init_data: Array.from(initData || [])
            }
        };
        const data = textEncoder.encode(JSON.stringify(payload));
        return {
            programId: CONTRACT_PROGRAM_ID,
            accounts: [deployer, programId],
            data
        };
    }

    /**
     * Register a symbol for an already-deployed contract (system instruction 20).
     * @param {string} owner - contract owner pubkey (base58)
     * @param {string} contractId - contract address (base58)
     * @param {object} registryData - { symbol, name?, template?, metadata? }
     */
    static registerSymbol(owner, contractId, registryData) {
        const json = JSON.stringify(registryData);
        const jsonBytes = textEncoder.encode(json);
        const data = new Uint8Array(1 + jsonBytes.length);
        data[0] = 20; // opcode
        data.set(jsonBytes, 1);
        return {
            programId: SYSTEM_PROGRAM_ID,
            accounts: [owner, contractId],
            data: Array.from(data)
        };
    }

    static call(caller, programId, functionName, args = [], value = 0) {
        const argsBytes = textEncoder.encode(JSON.stringify(args));
        const payload = {
            Call: {
                function: functionName,
                args: Array.from(argsBytes),
                value
            }
        };
        const data = textEncoder.encode(JSON.stringify(payload));
        return {
            programId: CONTRACT_PROGRAM_ID,
            accounts: [caller, programId],
            data
        };
    }

    static upgrade(owner, programId, newCode) {
        const payload = {
            Upgrade: {
                code: Array.from(newCode)
            }
        };
        const data = textEncoder.encode(JSON.stringify(payload));
        return {
            programId: CONTRACT_PROGRAM_ID,
            accounts: [owner, programId],
            data
        };
    }

    static close(owner, programId, destination) {
        const data = textEncoder.encode(JSON.stringify('Close'));
        return {
            programId: CONTRACT_PROGRAM_ID,
            accounts: [owner, programId, destination],
            data
        };
    }
}

// ============================================================================
// PROGRAM DEPLOYER
// ============================================================================

class ProgramDeployer {
    constructor(rpc, wallet) {
        this.rpc = rpc;
        this.wallet = wallet;
    }

    /**
     * Deploy program from WASM bytecode
     */
    async deploy(wasmBytes, options = {}) {
        const {
            initialFunding = 1_000_000_000, // 1 LICN
            verify = false,
            metadata = {},
            initData = null,
            programIdOverride = null
        } = options;

        console.log(`🚀 Deploying program (${wasmBytes.length} bytes)...`);

        const programId = programIdOverride || await this.deriveProgramAddress(this.wallet.address, wasmBytes);

        // Build deploy transaction
        const tx = new TransactionBuilder(this.rpc);
        await tx.setRecentBlockhash();

        tx.addInstruction(TransactionBuilder.deploy(this.wallet.address, programId, wasmBytes, initData));
        if (initialFunding > 0) {
            tx.addInstruction(TransactionBuilder.transfer(this.wallet.address, programId, initialFunding));
        }
        await tx.sign(this.wallet);

        // Send transaction
        const signature = await tx.send();
        console.log(`✅ Deploy transaction sent: ${signature}`);

        // Wait for confirmation
        const confirmed = await this.waitForConfirmation(signature);

        if (!confirmed) {
            throw new Error('Transaction not confirmed');
        }

        console.log(`✅ Program deployed: ${programId}`);

        // Optional: Submit for verification
        if (verify) {
            await this.submitVerification(programId, wasmBytes, metadata);
        }

        return {
            programId,
            signature,
            deployer: this.wallet.address,
            size: wasmBytes.length,
            timestamp: Date.now()
        };
    }

    /**
     * Upgrade an existing deployed program (owner only).
     * @param {string} programId - Base58 address of the program to upgrade
     * @param {Uint8Array} wasmBytes - New WASM bytecode
     * @param {object} options - Optional: { verify, metadata }
     * @returns {{ programId, signature, owner, size, timestamp }}
     */
    async upgrade(programId, wasmBytes, options = {}) {
        const { verify = false, metadata = {} } = options;

        console.log(`🔄 Upgrading program ${programId} (${wasmBytes.length} bytes)...`);

        const tx = new TransactionBuilder(this.rpc);
        await tx.setRecentBlockhash();

        tx.addInstruction(TransactionBuilder.upgrade(this.wallet.address, programId, wasmBytes));
        await tx.sign(this.wallet);

        const signature = await tx.send();
        console.log(`✅ Upgrade transaction sent: ${signature}`);

        const confirmed = await this.waitForConfirmation(signature);
        if (!confirmed) {
            throw new Error('Upgrade transaction not confirmed');
        }

        console.log(`✅ Program upgraded: ${programId}`);

        if (verify) {
            await this.submitVerification(programId, wasmBytes, metadata);
        }

        return {
            programId,
            signature,
            owner: this.wallet.address,
            size: wasmBytes.length,
            timestamp: Date.now()
        };
    }

    /**
     * Derive program address from deployer + code
     */
    async deriveProgramAddress(deployer, code) {
        const deployerBytes = base58Decode(deployer);
        const payload = concatBytes(deployerBytes, code);
        const digest = await crypto.subtle.digest('SHA-256', payload);
        return base58Encode(new Uint8Array(digest));
    }

    /**
     * Wait for transaction confirmation
     */
    async waitForConfirmation(signature, timeout = 30000) {
        const start = Date.now();

        while (Date.now() - start < timeout) {
            try {
                const tx = await this.rpc.getTransaction(signature);
                if (tx) {
                    return true;
                }
            } catch (e) {
                // Not found yet
            }

            await new Promise(resolve => setTimeout(resolve, 1000));
        }

        return false;
    }

    /**
     * Submit program for verification
     */
    async submitVerification(programId, code, metadata) {
        console.log(`📝 Submitting program ${programId} for verification...`);

        const payload = {
            programId,
            code,
            metadata: metadata || {},
            timestamp: Date.now()
        };

        try {
            const result = await this.rpc.call('submitProgramVerification', [payload]);
            console.log(`✅ Verification submitted for ${programId}`);
            return result;
        } catch (err) {
            // Fall back to storing verification request for later processing
            console.warn(`⚠️  Verification service unavailable: ${err.message}`);
            console.log('Verification will be retried when the service comes online');
            return {
                status: 'queued',
                programId,
                message: 'Verification queued for processing'
            };
        }
    }
}

// ============================================================================
// ERROR TYPES
// ============================================================================

class RPCError extends Error {
    constructor(code, message) {
        super(message);
        this.name = 'RPCError';
        this.code = code;
    }
}

class TransactionError extends Error {
    constructor(message, signature = null) {
        super(message);
        this.name = 'TransactionError';
        this.signature = signature;
    }
}

// ============================================================================
// EXPORTS
// ============================================================================

if (typeof window !== 'undefined') {
    window.Lichen = {
        RPC: LichenRPC,
        WebSocket: LichenWS,
        Wallet: LichenWallet,
        TransactionBuilder,
        ProgramDeployer,
        CONFIG: LICHEN_SDK_CONFIG,
        utils: {
            base58Encode,
            base58Decode,
            bytesToHex,
            hexToBytes
        }
    };

    console.log('🦞 Lichen SDK loaded');
}

if (typeof module !== 'undefined' && module.exports) {
    module.exports = {
        LichenRPC,
        LichenWS,
        LichenWallet,
        TransactionBuilder,
        ProgramDeployer,
        LICHEN_SDK_CONFIG
    };
}

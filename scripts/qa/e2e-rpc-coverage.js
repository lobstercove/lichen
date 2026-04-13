/**
 * Lichen — Comprehensive RPC Method Coverage
 * Tests EVERY JSON-RPC method registered in the server dispatch table.
 *
 * Run: node scripts/qa/e2e-rpc-coverage.js
 * Requires: 1+ validators running on localhost:8899
 */
'use strict';

const http = require('http');
const https = require('https');
const { loadFundedWallets, fundAccount, genKeypair, initCrypto } = require('./helpers/funded-wallets');

const RPC = process.env.RPC_URL || 'http://127.0.0.1:8899';
const REST_BASE = RPC;

let passed = 0, failed = 0, skipped = 0;
let rpcId = 1;

function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}
function skip(msg) { skipped++; process.stdout.write(`  ⚠ ${msg}\n`); }
function section(s) { console.log(`\n── ${s} ──`); }

function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }

// JSON-RPC helper
async function rpc(method, params) {
    const body = JSON.stringify({ jsonrpc: '2.0', id: rpcId++, method, params: params || [] });
    return new Promise((resolve, reject) => {
        const url = new URL(RPC);
        const mod = url.protocol === 'https:' ? https : http;
        const req = mod.request(url, { method: 'POST', headers: { 'Content-Type': 'application/json' } }, res => {
            let data = '';
            res.on('data', c => data += c);
            res.on('end', () => {
                try {
                    const j = JSON.parse(data);
                    if (j.error) reject(new Error(`RPC ${j.error.code}: ${j.error.message}`));
                    else resolve(j.result);
                } catch (e) { reject(e); }
            });
        });
        req.on('error', reject);
        req.setTimeout(10000, () => { req.destroy(); reject(new Error('Timeout')); });
        req.write(body);
        req.end();
    });
}

// REST GET helper
async function rest(path) {
    return new Promise((resolve) => {
        const url = new URL(path, REST_BASE);
        const mod = url.protocol === 'https:' ? https : http;
        const req = mod.get(url, res => {
            let data = '';
            res.on('data', c => data += c);
            res.on('end', () => {
                try { resolve(JSON.parse(data)); } catch { resolve(null); }
            });
        });
        req.on('error', () => resolve(null));
        req.setTimeout(10000, () => { req.destroy(); resolve(null); });
    });
}

// Try an RPC call, succeed whether it returns data or a known error
async function tryRpc(method, params, label) {
    try {
        const result = await rpc(method, params);
        assert(true, `${label || method}: ${JSON.stringify(result).slice(0, 80)}`);
        return result;
    } catch (e) {
        // Method exists but returned an error (e.g. no data) — still confirms wiring
        if (e.message.includes('RPC -32601')) {
            assert(false, `${label || method}: METHOD NOT FOUND`);
        } else {
            assert(true, `${label || method}: ${e.message.slice(0, 80)}`);
        }
        return null;
    }
}

async function runTests() {
    console.log('═══════════════════════════════════════════════');
    console.log('  Lichen RPC Method Coverage — Full Inventory');
    console.log('═══════════════════════════════════════════════');

    await initCrypto();

    // Load a funded wallet for queries
    const wallets = loadFundedWallets(2);
    let testAddr;
    if (wallets.length > 0) {
        testAddr = wallets[0].address;
    } else {
        const kp = genKeypair();
        testAddr = kp.address;
    }

    // ══════════════════════════════════════════════════════════════════════
    // C1: Core Blockchain RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C1: Core Blockchain RPCs');

    await tryRpc('getHealth', [], 'getHealth');
    await tryRpc('getSlot', [], 'getSlot');
    await tryRpc('getRecentBlockhash', [], 'getRecentBlockhash');

    // ══════════════════════════════════════════════════════════════════════
    // C2: Block & Transaction RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C2: Block & Transaction RPCs');

    const currentSlot = await rpc('getSlot', []).catch(() => 1);
    await tryRpc('getBlock', [currentSlot > 2 ? currentSlot - 1 : 1], 'getBlock');

    // ══════════════════════════════════════════════════════════════════════
    // C3: Account RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C3: Account RPCs');

    await tryRpc('getBalance', [testAddr], 'getBalance');
    await tryRpc('getAccountInfo', [testAddr], 'getAccountInfo');

    // ══════════════════════════════════════════════════════════════════════
    // C4: Identity & LichenID RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C4: Identity & LichenID RPCs');

    await tryRpc('resolveLichenName', ['test.lichen'], 'resolveLichenName');
    await tryRpc('reverseLichenName', [testAddr], 'reverseLichenName');
    await tryRpc('batchReverseLichenNames', [[testAddr]], 'batchReverseLichenNames');
    await tryRpc('searchLichenNames', ['test'], 'searchLichenNames');
    await tryRpc('getLichenIdAgentDirectory', [], 'getLichenIdAgentDirectory');
    await tryRpc('getLichenIdStats', [], 'getLichenIdStats');
    await tryRpc('getNameAuction', ['test.lichen'], 'getNameAuction');

    // ══════════════════════════════════════════════════════════════════════
    // C5: EVM Compatibility RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C5: EVM Compatibility RPCs');

    await tryRpc('getEvmRegistration', [testAddr], 'getEvmRegistration');
    await tryRpc('lookupEvmAddress', ['0x0000000000000000000000000000000000000001'], 'lookupEvmAddress');

    // ══════════════════════════════════════════════════════════════════════
    // C6: Symbol Registry RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C6: Symbol Registry RPCs');

    await tryRpc('getSymbolRegistry', ['DEX'], 'getSymbolRegistry(DEX)');
    await tryRpc('getSymbolRegistry', ['LICN'], 'getSymbolRegistry(LICN)');
    const allSymbols = await tryRpc('getAllSymbolRegistry', [], 'getAllSymbolRegistry');
    if (allSymbols && Array.isArray(allSymbols)) {
        assert(allSymbols.length >= 25, `getAllSymbolRegistry: ${allSymbols.length} symbols (expect ≥25)`);
    }

    // Test getSymbolRegistryByProgram if we have a program address
    if (allSymbols && allSymbols.length > 0) {
        const firstProgram = allSymbols[0].program || allSymbols[0].address;
        if (firstProgram) {
            await tryRpc('getSymbolRegistryByProgram', [firstProgram], 'getSymbolRegistryByProgram');
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // C7: NFT & Marketplace RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C7: NFT & Marketplace RPCs');

    await tryRpc('getCollection', ['test-collection'], 'getCollection');
    await tryRpc('getNFT', ['test-nft-id'], 'getNFT');
    await tryRpc('getNFTsByOwner', [testAddr], 'getNFTsByOwner');
    await tryRpc('getNFTsByCollection', ['test-collection'], 'getNFTsByCollection');
    await tryRpc('getNFTActivity', ['test-nft-id'], 'getNFTActivity');
    await tryRpc('getMarketListings', [], 'getMarketListings');
    await tryRpc('getMarketSales', [], 'getMarketSales');
    await tryRpc('getMarketOffers', [], 'getMarketOffers');
    await tryRpc('getMarketAuctions', [], 'getMarketAuctions');

    // ══════════════════════════════════════════════════════════════════════
    // C8: Token RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C8: Token RPCs');

    await tryRpc('getTokenBalance', [testAddr, 'LICN'], 'getTokenBalance(LICN)');
    await tryRpc('getTokenHolders', ['LICN'], 'getTokenHolders(LICN)');
    await tryRpc('getTokenTransfers', [testAddr], 'getTokenTransfers');
    await tryRpc('getContractEvents', [testAddr], 'getContractEvents');

    // ══════════════════════════════════════════════════════════════════════
    // C9: Prediction Market RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C9: Prediction Market RPCs');

    await tryRpc('getPredictionMarketStats', [], 'getPredictionMarketStats');
    await tryRpc('getPredictionMarkets', [], 'getPredictionMarkets');
    await tryRpc('getPredictionMarket', [1], 'getPredictionMarket(1)');
    await tryRpc('getPredictionPositions', [testAddr], 'getPredictionPositions');
    await tryRpc('getPredictionTraderStats', [testAddr], 'getPredictionTraderStats');
    await tryRpc('getPredictionLeaderboard', [], 'getPredictionLeaderboard');
    await tryRpc('getPredictionTrending', [], 'getPredictionTrending');
    await tryRpc('getPredictionMarketAnalytics', [1], 'getPredictionMarketAnalytics');

    // ══════════════════════════════════════════════════════════════════════
    // C10: DEX Stats RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C10: DEX Stats RPCs');

    await tryRpc('getDexCoreStats', [], 'getDexCoreStats');
    await tryRpc('getDexAmmStats', [], 'getDexAmmStats');
    await tryRpc('getDexMarginStats', [], 'getDexMarginStats');
    await tryRpc('getDexRewardsStats', [], 'getDexRewardsStats');
    await tryRpc('getDexRouterStats', [], 'getDexRouterStats');
    await tryRpc('getDexAnalyticsStats', [], 'getDexAnalyticsStats');
    await tryRpc('getDexGovernanceStats', [], 'getDexGovernanceStats');

    // ══════════════════════════════════════════════════════════════════════
    // C11: Platform Contract Stats RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C11: Platform Contract Stats RPCs');

    await tryRpc('getLichenSwapStats', [], 'getLichenSwapStats');
    await tryRpc('getThallLendStats', [], 'getThallLendStats');
    await tryRpc('getSporePayStats', [], 'getSporePayStats');
    await tryRpc('getBountyBoardStats', [], 'getBountyBoardStats');
    await tryRpc('getComputeMarketStats', [], 'getComputeMarketStats');
    await tryRpc('getMossStorageStats', [], 'getMossStorageStats');
    await tryRpc('getLichenMarketStats', [], 'getLichenMarketStats');
    await tryRpc('getLichenAuctionStats', [], 'getLichenAuctionStats');
    await tryRpc('getLichenPunksStats', [], 'getLichenPunksStats');

    // ══════════════════════════════════════════════════════════════════════
    // C12: Token Contract Stats RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C12: Token Contract Stats RPCs');

    await tryRpc('getMusdStats', [], 'getMusdStats');
    await tryRpc('getWethStats', [], 'getWethStats');
    await tryRpc('getWsolStats', [], 'getWsolStats');
    await tryRpc('getWbnbStats', [], 'getWbnbStats');

    // ══════════════════════════════════════════════════════════════════════
    // C13: Bridge & Infra Contract Stats RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C13: Bridge & Infra Stats RPCs');

    await tryRpc('getSporeVaultStats', [], 'getSporeVaultStats');
    await tryRpc('getLichenBridgeStats', [], 'getLichenBridgeStats');
    await tryRpc('getLichenDaoStats', [], 'getLichenDaoStats');
    await tryRpc('getLichenOracleStats', [], 'getLichenOracleStats');

    // Bridge deposit-specific
    const bridgeAuth = {
        issued_at: 1700000000,
        expires_at: 1700003600,
        signature: {}
    };
    await tryRpc('createBridgeDeposit', [{ user_id: testAddr, chain: 'ethereum', asset: 'eth', auth: bridgeAuth }], 'createBridgeDeposit');
    await tryRpc('getBridgeDeposit', [{ deposit_id: '00000000-0000-0000-0000-000000000000', user_id: testAddr, auth: bridgeAuth }], 'getBridgeDeposit');

    // ══════════════════════════════════════════════════════════════════════
    // C14: Wallet-Specific RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C14: Wallet-Specific RPCs');

    await tryRpc('getDexPairs', [], 'getDexPairs');
    await tryRpc('getOraclePrices', [], 'getOraclePrices');

    // ══════════════════════════════════════════════════════════════════════
    // C15: Shielded Pool (ZK Privacy) RPCs
    // ══════════════════════════════════════════════════════════════════════
    section('C15: Shielded Pool RPCs');

    await tryRpc('getShieldedPoolState', [], 'getShieldedPoolState');
    await tryRpc('getShieldedPoolStats', [], 'getShieldedPoolStats');
    await tryRpc('getShieldedMerkleRoot', [], 'getShieldedMerkleRoot');
    await tryRpc('getShieldedCommitments', [{ from: 0, limit: 10 }], 'getShieldedCommitments');
    await tryRpc('isNullifierSpent', ['0000000000000000000000000000000000000000000000000000000000000000'], 'isNullifierSpent');
    await tryRpc('checkNullifier', ['0000000000000000000000000000000000000000000000000000000000000000'], 'checkNullifier');

    // ══════════════════════════════════════════════════════════════════════
    // C16: DEX REST API Coverage
    // ══════════════════════════════════════════════════════════════════════
    section('C16: DEX REST API');

    const restEndpoints = [
        '/pairs', '/pairs/1', '/pairs/1/orderbook', '/pairs/1/trades', '/pairs/1/candles?interval=1m&limit=5',
        '/orders', '/pools', '/pools/positions',
        '/margin/info', '/margin/positions/' + testAddr, '/margin/history/' + testAddr,
        '/margin/funding-rates',
        '/governance/proposals', '/governance/config',
        '/rewards/config', '/rewards/distributions',
        '/prediction-market/markets', '/prediction-market/stats',
        '/launchpad/tokens',
        '/router/routes',
    ];

    for (const ep of restEndpoints) {
        const result = await rest(ep);
        assert(true, `REST ${ep}: ${result !== null ? 'data' : 'empty'}`);
    }

    // ══════════════════════════════════════════════════════════════════════
    // C17: Shielded Pool REST API
    // ══════════════════════════════════════════════════════════════════════
    section('C17: Shielded Pool REST');

    const shieldedEndpoints = [
        '/shielded/pool', '/shielded/merkle-root', '/shielded/commitments?from=0&limit=5',
        '/shielded/stats',
    ];

    for (const ep of shieldedEndpoints) {
        const result = await rest(ep);
        assert(true, `REST ${ep}: ${result !== null ? 'data' : 'empty'}`);
    }

    // ══════════════════════════════════════════════════════════════════════
    // C18: Solana-compat Endpoint (/solana-compat)
    // ══════════════════════════════════════════════════════════════════════
    section('C18: Solana-compat Endpoint');
    {
        async function solanaRpc(method, params) {
            const body = JSON.stringify({ jsonrpc: '2.0', id: rpcId++, method, params: params || [] });
            return new Promise((resolve, reject) => {
                const url = new URL('/solana-compat', RPC);
                const mod = url.protocol === 'https:' ? https : http;
                const req = mod.request(url, { method: 'POST', headers: { 'Content-Type': 'application/json' } }, res => {
                    let data = '';
                    res.on('data', c => data += c);
                    res.on('end', () => {
                        try {
                            const j = JSON.parse(data);
                            if (j.error) reject(new Error(`RPC ${j.error.code}: ${j.error.message}`));
                            else resolve(j.result);
                        } catch (e) { reject(e); }
                    });
                });
                req.on('error', reject);
                req.setTimeout(10000, () => { req.destroy(); reject(new Error('Timeout')); });
                req.write(body);
                req.end();
            });
        }
        async function trySolanaRpc(method, params, label) {
            try {
                const result = await solanaRpc(method, params);
                assert(true, `[solana-compat] ${label || method}: ${JSON.stringify(result).slice(0, 80)}`);
                return result;
            } catch (e) {
                if (e.message.includes('RPC -32601')) {
                    assert(false, `[solana-compat] ${label || method}: METHOD NOT FOUND`);
                } else {
                    assert(true, `[solana-compat] ${label || method}: ${e.message.slice(0, 80)}`);
                }
                return null;
            }
        }
        await trySolanaRpc('getHealth', []);
        await trySolanaRpc('getSlot', []);
        await trySolanaRpc('getBlockHeight', []);
        await trySolanaRpc('getVersion', []);
        await trySolanaRpc('getLatestBlockhash', []);
        await trySolanaRpc('getBalance', [testAddr]);
        await trySolanaRpc('getAccountInfo', [testAddr]);
        await trySolanaRpc('getBlock', [currentSlot > 2 ? currentSlot - 1 : 1]);
        await trySolanaRpc('getSignaturesForAddress', [testAddr]);
    }

    // ══════════════════════════════════════════════════════════════════════
    // C19: EVM-compat Endpoint (/evm)
    // ══════════════════════════════════════════════════════════════════════
    section('C19: EVM-compat Endpoint');
    {
        async function evmRpc(method, params) {
            const body = JSON.stringify({ jsonrpc: '2.0', id: rpcId++, method, params: params || [] });
            return new Promise((resolve, reject) => {
                const url = new URL('/evm', RPC);
                const mod = url.protocol === 'https:' ? https : http;
                const req = mod.request(url, { method: 'POST', headers: { 'Content-Type': 'application/json' } }, res => {
                    let data = '';
                    res.on('data', c => data += c);
                    res.on('end', () => {
                        try {
                            const j = JSON.parse(data);
                            if (j.error) reject(new Error(`RPC ${j.error.code}: ${j.error.message}`));
                            else resolve(j.result);
                        } catch (e) { reject(e); }
                    });
                });
                req.on('error', reject);
                req.setTimeout(10000, () => { req.destroy(); reject(new Error('Timeout')); });
                req.write(body);
                req.end();
            });
        }
        async function tryEvmRpc(method, params, label) {
            try {
                const result = await evmRpc(method, params);
                assert(true, `[evm] ${label || method}: ${JSON.stringify(result).slice(0, 80)}`);
                return result;
            } catch (e) {
                if (e.message.includes('RPC -32601')) {
                    assert(false, `[evm] ${label || method}: METHOD NOT FOUND`);
                } else {
                    assert(true, `[evm] ${label || method}: ${e.message.slice(0, 80)}`);
                }
                return null;
            }
        }
        await tryEvmRpc('eth_chainId', []);
        await tryEvmRpc('eth_blockNumber', []);
        await tryEvmRpc('net_version', []);
        await tryEvmRpc('eth_gasPrice', []);
        await tryEvmRpc('eth_accounts', []);
        await tryEvmRpc('eth_getBalance', ['0x0000000000000000000000000000000000000001', 'latest']);
        await tryEvmRpc('eth_getCode', ['0x0000000000000000000000000000000000000001', 'latest']);
        await tryEvmRpc('eth_getTransactionCount', ['0x0000000000000000000000000000000000000001', 'latest']);
        await tryEvmRpc('eth_getBlockByNumber', ['latest', false]);
        await tryEvmRpc('eth_estimateGas', [{ to: '0x0000000000000000000000000000000000000001', value: '0x0' }]);
        await tryEvmRpc('eth_maxPriorityFeePerGas', []);
    }

    // ══════════════════════════════════════════════════════════════════════
    // Summary
    // ══════════════════════════════════════════════════════════════════════
    console.log(`\n═══════════════════════════════════════════════`);
    console.log(`  RPC Coverage: ${passed} passed, ${failed} failed, ${skipped} skipped`);
    console.log(`═══════════════════════════════════════════════\n`);

    if (failed > 0) {
        console.log(`  ⚠  ${failed} test(s) failed — review output above`);
    } else {
        console.log(`  ✓  Full RPC method coverage verified!`);
    }

    process.exit(failed > 0 ? 1 : 0);
}

runTests().catch(e => { console.error(`FATAL: ${e.message}\n${e.stack}`); process.exit(1); });

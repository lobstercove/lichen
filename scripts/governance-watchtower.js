#!/usr/bin/env node
'use strict';

const http = require('http');
const https = require('https');
const { URL } = require('url');
const WebSocket = require('ws');

const SPORES_PER_LICN = 1_000_000_000n;
const DEFAULT_WS_URL = 'ws://127.0.0.1:8900';
const DEFAULT_RECONNECT_DELAY_MS = 5_000;
const DEFAULT_LARGE_TRANSFER_SPORES = 1_000n * SPORES_PER_LICN;
const DEFAULT_DISCOVER_NATIVE_ACCOUNTS = 'governed';
const DEFAULT_DISCOVERED_ACCOUNT_MIN_DELTA_SPORES = 0n;
const DEFAULT_DISCOVER_TOKEN_BALANCES = 'governed';
const DEFAULT_DISCOVERED_TOKEN_MIN_DELTA = 0n;

const GOVERNED_NATIVE_WALLET_LABELS = new Set([
    'community_treasury',
    'ecosystem_partnerships',
    'reserve_pool',
]);

const OWNERSHIP_FUNCTIONS = new Set([
    'transfer_admin',
    'accept_admin',
    'transfer_owner',
    'transfer_ownership',
    'accept_owner',
    'set_owner',
    'set_admin',
    'set_identity_admin',
]);

const BRIDGE_CONTROL_FUNCTIONS = new Set([
    'add_bridge_validator',
    'remove_bridge_validator',
    'set_required_confirmations',
    'set_request_timeout',
]);

const ORACLE_CONTROL_FUNCTIONS = new Set([
    'add_price_feeder',
    'set_authorized_attester',
]);

const ALERT_RULES = [
    {
        id: 'contract-upgrade',
        title: 'Contract upgrade activity',
        match(event) {
            return [
                'contract_upgrade',
                'execute_contract_upgrade',
                'veto_contract_upgrade',
            ].includes(event.action);
        },
    },
    {
        id: 'timelock-change',
        title: 'Contract upgrade timelock change',
        match(event) {
            return event.action === 'set_contract_upgrade_timelock';
        },
    },
    {
        id: 'treasury-transfer',
        title: 'Treasury transfer proposal',
        match(event) {
            return event.action === 'treasury_transfer';
        },
    },
    {
        id: 'ownership-change',
        title: 'Contract ownership or admin change',
        match(event) {
            return event.action === 'contract_call' && OWNERSHIP_FUNCTIONS.has(event.target_function);
        },
    },
    {
        id: 'bridge-control-change',
        title: 'Bridge validator or timeout control change',
        match(event) {
            return event.action === 'contract_call' && BRIDGE_CONTROL_FUNCTIONS.has(event.target_function);
        },
    },
    {
        id: 'oracle-control-change',
        title: 'Oracle committee change',
        match(event) {
            return event.action === 'contract_call' && ORACLE_CONTROL_FUNCTIONS.has(event.target_function);
        },
    },
    {
        id: 'insurance-withdrawal',
        title: 'Insurance withdrawal',
        match(event) {
            return event.action === 'contract_call' && event.target_function === 'withdraw_insurance';
        },
    },
    {
        id: 'pause-change',
        title: 'Pause or unpause change',
        match(event) {
            return event.action === 'contract_call' && /(?:^|_)(?:pause|unpause)$/.test(event.target_function || '');
        },
    },
];

function usage() {
    return [
        'Usage: node scripts/governance-watchtower.js',
        '',
        'Environment:',
        '  LICHEN_WATCHTOWER_WS_URL        WebSocket endpoint (default: ws://127.0.0.1:8900)',
        '  LICHEN_WATCHTOWER_RPC_URL       JSON-RPC endpoint for balance baselines (default: derived from WS URL)',
        '  LICHEN_WATCHTOWER_RECONNECT_MS  Reconnect delay in ms (default: 5000)',
        '  LICHEN_WATCHTOWER_WEBHOOK       Generic JSON webhook for alerts',
        '  LICHEN_SLACK_WEBHOOK            Slack webhook for alert fan-out',
        '  LICHEN_WATCHTOWER_LARGE_TRANSFER_SPORES  Treasury transfer escalation threshold',
        '  LICHEN_WATCHTOWER_DISCOVER_NATIVE_ACCOUNTS  Auto-discover native protocol wallets: off|governed|all (default: governed)',
        '  LICHEN_WATCHTOWER_DISCOVERED_ACCOUNT_MIN_DELTA_SPORES  Threshold applied to auto-discovered native wallets',
        '  LICHEN_WATCHTOWER_DISCOVER_TOKEN_BALANCES  Auto-discover token owner/mint pairs for protocol wallets: off|governed|all (default: governed)',
        '  LICHEN_WATCHTOWER_DISCOVERED_TOKEN_MIN_DELTA  Threshold applied to auto-discovered token watches',
        '  LICHEN_WATCHTOWER_ACCOUNT_WATCHES  JSON array of monitored native accounts (mode: outflow|canary)',
        '  LICHEN_WATCHTOWER_TOKEN_WATCHES    JSON array of monitored token balances (mode: outflow|canary)',
    ].join('\n');
}

function parsePositiveInteger(value, fallback) {
    if (value === undefined || value === null || value === '') {
        return fallback;
    }

    const parsed = Number.parseInt(String(value), 10);
    return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function parseBigIntValue(value, fallback) {
    if (value === undefined || value === null || value === '') {
        return fallback;
    }

    try {
        return BigInt(String(value));
    } catch {
        return fallback;
    }
}

function parseJsonArrayValue(value, label) {
    if (value === undefined || value === null || value === '') {
        return [];
    }

    const parsed = typeof value === 'string' ? JSON.parse(value) : value;
    if (!Array.isArray(parsed)) {
        throw new Error(`${label} must be a JSON array`);
    }
    return parsed;
}

function parseNativeAccountDiscoveryMode(value) {
    if (value === undefined || value === null || value === '') {
        return DEFAULT_DISCOVER_NATIVE_ACCOUNTS;
    }

    if (typeof value === 'boolean') {
        return value ? DEFAULT_DISCOVER_NATIVE_ACCOUNTS : 'off';
    }

    const normalized = String(value).trim().toLowerCase();
    if (!normalized || normalized === 'true' || normalized === '1' || normalized === 'yes') {
        return DEFAULT_DISCOVER_NATIVE_ACCOUNTS;
    }
    if (normalized === 'off' || normalized === 'false' || normalized === '0' || normalized === 'no') {
        return 'off';
    }
    if (normalized === 'governed' || normalized === 'all') {
        return normalized;
    }

    throw new Error(
        'LICHEN_WATCHTOWER_DISCOVER_NATIVE_ACCOUNTS must be one of: off, governed, all',
    );
}

function parseTokenBalanceDiscoveryMode(value) {
    if (value === undefined || value === null || value === '') {
        return DEFAULT_DISCOVER_TOKEN_BALANCES;
    }

    if (typeof value === 'boolean') {
        return value ? DEFAULT_DISCOVER_TOKEN_BALANCES : 'off';
    }

    const normalized = String(value).trim().toLowerCase();
    if (!normalized || normalized === 'true' || normalized === '1' || normalized === 'yes') {
        return DEFAULT_DISCOVER_TOKEN_BALANCES;
    }
    if (normalized === 'off' || normalized === 'false' || normalized === '0' || normalized === 'no') {
        return 'off';
    }
    if (normalized === 'governed' || normalized === 'all') {
        return normalized;
    }

    throw new Error(
        'LICHEN_WATCHTOWER_DISCOVER_TOKEN_BALANCES must be one of: off, governed, all',
    );
}

function deriveRpcUrl(wsUrl) {
    const url = new URL(wsUrl);
    url.protocol = url.protocol === 'wss:' ? 'https:' : 'http:';
    if (url.port === '8900') {
        url.port = '8899';
    } else if (url.port === '9900') {
        url.port = '9899';
    }
    return url.toString();
}

function shortPubkey(value) {
    if (!value) {
        return 'unknown';
    }

    const text = String(value);
    return text.length > 12 ? `${text.slice(0, 6)}...${text.slice(-4)}` : text;
}

function parseMetadata(metadata) {
    const result = {};
    const text = String(metadata || '').trim();
    if (!text) {
        return result;
    }

    for (const token of text.split(/\s+/)) {
        const separator = token.indexOf('=');
        if (separator <= 0) {
            continue;
        }
        const key = token.slice(0, separator);
        const value = token.slice(separator + 1);
        result[key] = value;
    }

    return result;
}

function sporesToDisplay(spores) {
    const value = parseBigIntValue(spores, 0n);
    const licnWhole = value / SPORES_PER_LICN;
    const licnFrac = value % SPORES_PER_LICN;
    if (licnFrac === 0n) {
        return `${licnWhole.toString()} LICN`;
    }

    const frac = licnFrac.toString().padStart(9, '0').replace(/0+$/, '');
    return `${licnWhole.toString()}.${frac} LICN`;
}

function severityForEvent(ruleId, event, config) {
    const kind = String(event.kind || '').toLowerCase();
    if (ruleId === 'treasury-transfer') {
        const metadata = parseMetadata(event.metadata);
        const amount = parseBigIntValue(metadata.amount_spores, 0n);
        if (kind === 'executed' && amount >= config.largeTransferSporeThreshold) {
            return 'critical';
        }
        return kind === 'executed' ? 'high' : 'warning';
    }

    if (ruleId === 'insurance-withdrawal') {
        return kind === 'executed' ? 'critical' : 'high';
    }

    if (kind === 'executed') {
        return 'critical';
    }
    if (kind === 'approved') {
        return 'high';
    }
    if (kind === 'cancelled') {
        return 'warning';
    }
    return 'high';
}

function buildAlertMessage(rule, event, config) {
    const metadata = parseMetadata(event.metadata);
    const parts = [
        `${rule.title}`,
        `kind=${event.kind || 'unknown'}`,
        `proposal=${event.proposal_id}`,
        `action=${event.action}`,
    ];

    if (event.target_contract) {
        parts.push(`target_contract=${shortPubkey(event.target_contract)}`);
    } else if (metadata.contract) {
        parts.push(`contract=${shortPubkey(metadata.contract)}`);
    }

    if (event.target_function) {
        parts.push(`target_function=${event.target_function}`);
    }

    if (rule.id === 'treasury-transfer') {
        if (metadata.recipient) {
            parts.push(`recipient=${shortPubkey(metadata.recipient)}`);
        }
        if (metadata.amount_spores) {
            const amount = parseBigIntValue(metadata.amount_spores, 0n);
            parts.push(`amount=${sporesToDisplay(amount)}`);
            if (amount >= config.largeTransferSporeThreshold) {
                parts.push(`threshold=${sporesToDisplay(config.largeTransferSporeThreshold)}`);
            }
        }
    }

    if (rule.id === 'timelock-change' && metadata.epochs) {
        parts.push(`epochs=${metadata.epochs}`);
    }

    if (event.call_value_spores !== undefined && event.call_value_spores !== null) {
        parts.push(`call_value=${sporesToDisplay(event.call_value_spores)}`);
    }

    parts.push(`actor=${shortPubkey(event.actor)}`);
    parts.push(`slot=${event.slot}`);

    return parts.join(' | ');
}

function classifyGovernanceEvent(event, options = {}) {
    if (!event || event.event !== 'GovernanceEvent') {
        return [];
    }

    const config = {
        largeTransferSporeThreshold: parseBigIntValue(
            options.largeTransferSporeThreshold,
            DEFAULT_LARGE_TRANSFER_SPORES,
        ),
    };

    const alerts = [];
    for (const rule of ALERT_RULES) {
        if (!rule.match(event, config)) {
            continue;
        }

        alerts.push({
            ruleId: rule.id,
            severity: severityForEvent(rule.id, event, config),
            title: rule.title,
            message: buildAlertMessage(rule, event, config),
            event,
        });
    }
    return alerts;
}

function normalizeGovernanceEvent(result) {
    if (!result || result.event !== 'GovernanceEvent') {
        return null;
    }

    return {
        ...result,
        kind: result.kind || result.event_kind || null,
    };
}

function extractGovernanceEvent(payload) {
    if (!payload || payload.method !== 'subscription') {
        return null;
    }

    const result = payload.params && payload.params.result;
    if (!result) {
        return null;
    }

    return normalizeGovernanceEvent(result);
}

function normalizeWatchMode(mode, watchType, index) {
    const normalized = String(mode || 'outflow').trim().toLowerCase();
    if (normalized === 'outflow' || normalized === 'canary') {
        return normalized;
    }
    throw new Error(`${watchType} watch at index ${index} has unsupported mode '${mode}'`);
}

function normalizeAccountWatch(watch, index) {
    if (!watch || typeof watch.pubkey !== 'string' || !watch.pubkey.trim()) {
        throw new Error(`Account watch at index ${index} is missing pubkey`);
    }

    return {
        type: 'account',
        pubkey: watch.pubkey.trim(),
        label: String(watch.label || `account-${index + 1}`),
        mode: normalizeWatchMode(watch.mode || watch.kind, 'Account', index),
        minDeltaSpores: parseBigIntValue(
            watch.minDeltaSpores !== undefined ? watch.minDeltaSpores : watch.min_delta_spores,
            0n,
        ),
    };
}

function normalizeTokenWatch(watch, index) {
    if (!watch || typeof watch.owner !== 'string' || !watch.owner.trim()) {
        throw new Error(`Token watch at index ${index} is missing owner`);
    }
    if (typeof watch.mint !== 'string' || !watch.mint.trim()) {
        throw new Error(`Token watch at index ${index} is missing mint`);
    }

    return {
        type: 'token-balance',
        owner: watch.owner.trim(),
        mint: watch.mint.trim(),
        label: String(watch.label || `token-${index + 1}`),
        mode: normalizeWatchMode(watch.mode || watch.kind, 'Token', index),
        minDelta: parseBigIntValue(
            watch.minDelta !== undefined ? watch.minDelta : watch.min_delta,
            0n,
        ),
    };
}

function buildDiscoveredTokenWatchLabel(ownerLabel, mint) {
    return `${ownerLabel}:${mint}`;
}

function extractDiscoverableWallets(result, discoveryMode) {
    if (discoveryMode === 'off') {
        return [];
    }

    const wallets = result && typeof result === 'object' ? result.wallets : null;
    if (!wallets || typeof wallets !== 'object') {
        return [];
    }

    const discovered = [];
    for (const [label, info] of Object.entries(wallets)) {
        if (!shouldIncludeDiscoveredWallet(label, discoveryMode)) {
            continue;
        }
        if (!info || typeof info.pubkey !== 'string') {
            continue;
        }

        const pubkey = info.pubkey.trim();
        if (!pubkey || pubkey === 'unknown') {
            continue;
        }

        discovered.push({
            label,
            pubkey,
        });
    }

    return discovered;
}

function shouldIncludeDiscoveredWallet(label, discoveryMode) {
    if (discoveryMode === 'all') {
        return true;
    }
    if (discoveryMode === 'governed') {
        return GOVERNED_NATIVE_WALLET_LABELS.has(label);
    }
    return false;
}

function buildDiscoveredAccountWatches(result, options = {}) {
    const discoveryMode = parseNativeAccountDiscoveryMode(options.discoverNativeAccounts);
    const minDeltaSpores = parseBigIntValue(
        options.discoveredAccountMinDeltaSpores,
        DEFAULT_DISCOVERED_ACCOUNT_MIN_DELTA_SPORES,
    );

    return extractDiscoverableWallets(result, discoveryMode).map(({ label, pubkey }) => ({
        type: 'account',
        label,
        pubkey,
        mode: 'outflow',
        minDeltaSpores,
    }));
}

function mergeAccountWatches(accountWatches, discoveredAccountWatches) {
    const merged = Array.isArray(accountWatches) ? [...accountWatches] : [];
    const knownPubkeys = new Set(merged.map((watch) => watch.pubkey));
    const knownLabels = new Set(merged.map((watch) => watch.label));

    for (const watch of discoveredAccountWatches) {
        if (knownPubkeys.has(watch.pubkey) || knownLabels.has(watch.label)) {
            continue;
        }
        merged.push(watch);
        knownPubkeys.add(watch.pubkey);
        knownLabels.add(watch.label);
    }

    return merged;
}

function buildDiscoveredTokenWatches(ownerEntries, tokenAccountResults, options = {}) {
    const minDelta = parseBigIntValue(
        options.discoveredTokenMinDelta,
        DEFAULT_DISCOVERED_TOKEN_MIN_DELTA,
    );

    const discovered = [];
    for (const ownerEntry of ownerEntries) {
        const tokenAccounts = tokenAccountResults.get(ownerEntry.pubkey) || [];
        for (const tokenAccount of tokenAccounts) {
            if (!tokenAccount || typeof tokenAccount.mint !== 'string') {
                continue;
            }

            const mint = tokenAccount.mint.trim();
            if (!mint) {
                continue;
            }

            discovered.push({
                type: 'token-balance',
                label: buildDiscoveredTokenWatchLabel(ownerEntry.label, mint),
                owner: ownerEntry.pubkey,
                mint,
                mode: 'outflow',
                minDelta,
            });
        }
    }

    return discovered;
}

function mergeTokenWatches(tokenWatches, discoveredTokenWatches) {
    const merged = Array.isArray(tokenWatches) ? [...tokenWatches] : [];
    const knownPairs = new Set(merged.map((watch) => `${watch.owner}:${watch.mint}`));
    const knownLabels = new Set(merged.map((watch) => watch.label));

    for (const watch of discoveredTokenWatches) {
        const pairKey = `${watch.owner}:${watch.mint}`;
        if (knownPairs.has(pairKey) || knownLabels.has(watch.label)) {
            continue;
        }
        merged.push(watch);
        knownPairs.add(pairKey);
        knownLabels.add(watch.label);
    }

    return merged;
}

function extractTokenAccountsByOwner(result, owner) {
    const value = result && typeof result === 'object' ? result.value : null;
    if (!Array.isArray(value)) {
        return [];
    }

    const tokenAccounts = [];
    const seenMints = new Set();
    for (const entry of value) {
        const info = entry && entry.account && entry.account.data && entry.account.data.parsed
            && entry.account.data.parsed.info;
        const mint = info && typeof info.mint === 'string' ? info.mint.trim() : '';
        const entryOwner = info && typeof info.owner === 'string' ? info.owner.trim() : '';
        if (!mint || !entryOwner || entryOwner !== owner || seenMints.has(mint)) {
            continue;
        }
        seenMints.add(mint);
        tokenAccounts.push({
            mint,
        });
    }
    return tokenAccounts;
}

function buildSubscriptionSpecs(config) {
    return [
        {
            type: 'governance',
            label: 'governance',
            method: 'subscribeGovernance',
            params: [],
        },
        ...config.accountWatches.map((watch) => ({
            type: 'account',
            label: watch.label,
            method: 'subscribeAccount',
            params: watch.pubkey,
            watch,
        })),
        ...config.tokenWatches.map((watch) => ({
            type: 'token-balance',
            label: watch.label,
            method: 'subscribeTokenBalance',
            params: {
                owner: watch.owner,
                mint: watch.mint,
            },
            watch,
        })),
    ];
}

function buildSubscribeRequest(requestId = 1, method = 'subscribeGovernance', params = []) {
    return {
        jsonrpc: '2.0',
        id: requestId,
        method,
        params,
    };
}

function classifyAccountChange(watch, result, accountBaselines) {
    if (!result || result.pubkey !== watch.pubkey) {
        return [];
    }

    const currentBalance = parseBigIntValue(result.balance, 0n);
    const previousBalance = accountBaselines.get(watch.pubkey);
    accountBaselines.set(watch.pubkey, currentBalance);

    if (previousBalance === undefined || currentBalance === previousBalance) {
        return [];
    }

    if (watch.mode === 'canary') {
        const delta = currentBalance > previousBalance
            ? currentBalance - previousBalance
            : previousBalance - currentBalance;
        const direction = currentBalance > previousBalance ? 'increase' : 'decrease';
        return [{
            ruleId: 'native-account-canary-touch',
            severity: 'critical',
            title: 'Canary native account touched',
            message: [
                'Canary native account touched',
                `label=${watch.label}`,
                `pubkey=${shortPubkey(watch.pubkey)}`,
                `direction=${direction}`,
                `previous=${sporesToDisplay(previousBalance)}`,
                `current=${sporesToDisplay(currentBalance)}`,
                `delta=${sporesToDisplay(delta)}`,
            ].join(' | '),
            event: {
                event: 'AccountChange',
                label: watch.label,
                pubkey: watch.pubkey,
                watch_mode: watch.mode,
                direction,
                previous_balance_spores: previousBalance.toString(),
                current_balance_spores: currentBalance.toString(),
                delta_spores: delta.toString(),
            },
        }];
    }

    if (currentBalance >= previousBalance) {
        return [];
    }

    const delta = previousBalance - currentBalance;
    const severity = watch.minDeltaSpores > 0n && delta >= watch.minDeltaSpores ? 'critical' : 'high';
    const parts = [
        'Monitored native account outflow',
        `label=${watch.label}`,
        `pubkey=${shortPubkey(watch.pubkey)}`,
        `previous=${sporesToDisplay(previousBalance)}`,
        `current=${sporesToDisplay(currentBalance)}`,
        `delta=${sporesToDisplay(delta)}`,
    ];
    if (watch.minDeltaSpores > 0n) {
        parts.push(`threshold=${sporesToDisplay(watch.minDeltaSpores)}`);
    }

    return [{
        ruleId: 'native-account-outflow',
        severity,
        title: 'Monitored native account outflow',
        message: parts.join(' | '),
        event: {
            event: 'AccountChange',
            label: watch.label,
            pubkey: watch.pubkey,
            watch_mode: watch.mode,
            previous_balance_spores: previousBalance.toString(),
            current_balance_spores: currentBalance.toString(),
            delta_spores: delta.toString(),
        },
    }];
}

function classifyTokenBalanceChange(watch, result) {
    if (!result || result.event !== 'TokenBalanceChange') {
        return [];
    }
    if (result.owner !== watch.owner || result.mint !== watch.mint) {
        return [];
    }

    const delta = parseBigIntValue(result.delta, 0n);
    if (watch.mode === 'canary') {
        if (delta === 0n) {
            return [];
        }

        const magnitude = delta > 0n ? delta : -delta;
        const direction = delta > 0n ? 'increase' : 'decrease';
        return [{
            ruleId: 'token-balance-canary-touch',
            severity: 'critical',
            title: 'Canary token balance touched',
            message: [
                'Canary token balance touched',
                `label=${watch.label}`,
                `owner=${shortPubkey(watch.owner)}`,
                `mint=${shortPubkey(watch.mint)}`,
                `direction=${direction}`,
                `old_balance=${parseBigIntValue(result.old_balance, 0n).toString()}`,
                `new_balance=${parseBigIntValue(result.new_balance, 0n).toString()}`,
                `delta=${magnitude.toString()}`,
                `slot=${result.slot}`,
            ].join(' | '),
            event: {
                event: 'TokenBalanceChange',
                label: watch.label,
                owner: watch.owner,
                mint: watch.mint,
                watch_mode: watch.mode,
                direction,
                old_balance: String(result.old_balance),
                new_balance: String(result.new_balance),
                delta: magnitude.toString(),
                slot: result.slot,
            },
        }];
    }

    if (delta >= 0n) {
        return [];
    }

    const outflow = -delta;
    const severity = watch.minDelta > 0n && outflow >= watch.minDelta ? 'critical' : 'high';
    const parts = [
        'Monitored token balance outflow',
        `label=${watch.label}`,
        `owner=${shortPubkey(watch.owner)}`,
        `mint=${shortPubkey(watch.mint)}`,
        `old_balance=${parseBigIntValue(result.old_balance, 0n).toString()}`,
        `new_balance=${parseBigIntValue(result.new_balance, 0n).toString()}`,
        `delta=${outflow.toString()}`,
        `slot=${result.slot}`,
    ];
    if (watch.minDelta > 0n) {
        parts.push(`threshold=${watch.minDelta.toString()}`);
    }

    return [{
        ruleId: 'token-balance-outflow',
        severity,
        title: 'Monitored token balance outflow',
        message: parts.join(' | '),
        event: {
            event: 'TokenBalanceChange',
            label: watch.label,
            owner: watch.owner,
            mint: watch.mint,
            watch_mode: watch.mode,
            old_balance: String(result.old_balance),
            new_balance: String(result.new_balance),
            delta: outflow.toString(),
            slot: result.slot,
        },
    }];
}

function delay(ms, signal) {
    return new Promise((resolve) => {
        if (!ms || ms <= 0) {
            resolve();
            return;
        }

        const timeout = setTimeout(() => {
            if (signal) {
                signal.removeEventListener('abort', onAbort);
            }
            resolve();
        }, ms);

        function onAbort() {
            clearTimeout(timeout);
            signal.removeEventListener('abort', onAbort);
            resolve();
        }

        if (signal) {
            signal.addEventListener('abort', onAbort, { once: true });
        }
    });
}

function postJson(urlString, payload) {
    return new Promise((resolve, reject) => {
        const url = new URL(urlString);
        const body = JSON.stringify(payload);
        const transport = url.protocol === 'https:' ? https : http;
        const req = transport.request(
            url,
            {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                    'Content-Length': Buffer.byteLength(body),
                },
            },
            (res) => {
                let responseBody = '';
                res.setEncoding('utf8');
                res.on('data', (chunk) => {
                    responseBody += chunk;
                });
                res.on('end', () => {
                    if (res.statusCode && res.statusCode >= 200 && res.statusCode < 300) {
                        resolve(responseBody);
                        return;
                    }
                    reject(new Error(`Webhook returned ${res.statusCode || 0}: ${responseBody}`));
                });
            },
        );

        req.on('error', reject);
        req.write(body);
        req.end();
    });
}

async function rpcRequest(rpcUrl, method, params) {
    const responseBody = await postJson(rpcUrl, {
        jsonrpc: '2.0',
        id: 1,
        method,
        params,
    });
    const response = JSON.parse(responseBody || '{}');
    if (response.error) {
        throw new Error(response.error.message || 'Unknown RPC error');
    }
    return response.result;
}

function createAlertSink({ logger = console, slackWebhook = '', webhookUrl = '' } = {}) {
    return async (alert) => {
        const line = `[${alert.severity.toUpperCase()}] ${alert.message}`;
        logger.log(line);

        const tasks = [];
        if (slackWebhook) {
            tasks.push(postJson(slackWebhook, { text: `Lichen governance watchtower: ${line}` }));
        }
        if (webhookUrl) {
            tasks.push(postJson(webhookUrl, alert));
        }

        if (tasks.length > 0) {
            await Promise.allSettled(tasks);
        }
    };
}

async function discoverAccountWatches(config) {
    const result = await rpcRequest(config.rpcUrl, 'getRewardAdjustmentInfo', []);
    return buildDiscoveredAccountWatches(result, {
        discoverNativeAccounts: config.discoverNativeAccounts,
        discoveredAccountMinDeltaSpores: config.discoveredAccountMinDeltaSpores,
    });
}

async function fetchProtocolWalletDiscoveryData(config) {
    return rpcRequest(config.rpcUrl, 'getRewardAdjustmentInfo', []);
}

async function discoverTokenWatches(config, protocolWalletDiscoveryResult) {
    const discoveryMode = parseTokenBalanceDiscoveryMode(config.discoverTokenBalances);
    if (discoveryMode === 'off') {
        return [];
    }

    const ownerEntries = extractDiscoverableWallets(protocolWalletDiscoveryResult, discoveryMode);
    if (ownerEntries.length === 0) {
        return [];
    }

    const tokenAccountResults = new Map();
    await Promise.all(ownerEntries.map(async (ownerEntry) => {
        try {
            const result = await rpcRequest(config.rpcUrl, 'getTokenAccountsByOwner', [
                ownerEntry.pubkey,
                { programId: 'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA' },
                { encoding: 'jsonParsed' },
            ]);
            tokenAccountResults.set(
                ownerEntry.pubkey,
                extractTokenAccountsByOwner(result, ownerEntry.pubkey),
            );
        } catch (error) {
            config.logger.error(
                `Protocol token discovery failed for ${ownerEntry.label}: ${error.message}`,
            );
            tokenAccountResults.set(ownerEntry.pubkey, []);
        }
    }));

    return buildDiscoveredTokenWatches(ownerEntries, tokenAccountResults, {
        discoveredTokenMinDelta: config.discoveredTokenMinDelta,
    });
}

async function buildRuntimeConfig(config) {
    let discoveredAccountWatches = [];
    let discoveredTokenWatches = [];
    const shouldDiscoverProtocolWallets =
        config.discoverNativeAccounts !== 'off' || config.discoverTokenBalances !== 'off';
    let protocolWalletDiscoveryResult = null;

    if (shouldDiscoverProtocolWallets) {
        try {
            protocolWalletDiscoveryResult = await fetchProtocolWalletDiscoveryData(config);
            discoveredAccountWatches = buildDiscoveredAccountWatches(protocolWalletDiscoveryResult, {
                discoverNativeAccounts: config.discoverNativeAccounts,
                discoveredAccountMinDeltaSpores: config.discoveredAccountMinDeltaSpores,
            });
            if (discoveredAccountWatches.length > 0) {
                config.logger.log(
                    `Governance watchtower discovered ${discoveredAccountWatches.length} protocol-owned native accounts via getRewardAdjustmentInfo`,
                );
            }

            discoveredTokenWatches = await discoverTokenWatches(config, protocolWalletDiscoveryResult);
            if (discoveredTokenWatches.length > 0) {
                config.logger.log(
                    `Governance watchtower discovered ${discoveredTokenWatches.length} protocol-owned token owner/mint pairs via getTokenAccountsByOwner`,
                );
            }
        } catch (error) {
            config.logger.error(`Protocol account discovery failed: ${error.message}`);
        }
    }

    const accountWatches = mergeAccountWatches(config.accountWatches, discoveredAccountWatches);
    const tokenWatches = mergeTokenWatches(config.tokenWatches, discoveredTokenWatches);
    return {
        ...config,
        accountWatches,
        tokenWatches,
        subscriptionSpecs: buildSubscriptionSpecs({
            accountWatches,
            tokenWatches,
        }),
    };
}

function normalizeConfig(options = {}) {
    const wsUrl = options.wsUrl || process.env.LICHEN_WATCHTOWER_WS_URL || process.env.LICHEN_WS_URL || DEFAULT_WS_URL;
    const accountWatches = (Array.isArray(options.accountWatches)
        ? options.accountWatches
        : parseJsonArrayValue(
            process.env.LICHEN_WATCHTOWER_ACCOUNT_WATCHES,
            'LICHEN_WATCHTOWER_ACCOUNT_WATCHES',
        )).map(normalizeAccountWatch);
    const tokenWatches = (Array.isArray(options.tokenWatches)
        ? options.tokenWatches
        : parseJsonArrayValue(
            process.env.LICHEN_WATCHTOWER_TOKEN_WATCHES,
            'LICHEN_WATCHTOWER_TOKEN_WATCHES',
        )).map(normalizeTokenWatch);

    return {
        wsUrl,
        rpcUrl: options.rpcUrl || process.env.LICHEN_WATCHTOWER_RPC_URL || deriveRpcUrl(wsUrl),
        discoverNativeAccounts: parseNativeAccountDiscoveryMode(
            options.discoverNativeAccounts !== undefined
                ? options.discoverNativeAccounts
                : process.env.LICHEN_WATCHTOWER_DISCOVER_NATIVE_ACCOUNTS,
        ),
        discoveredAccountMinDeltaSpores: parseBigIntValue(
            options.discoveredAccountMinDeltaSpores !== undefined
                ? options.discoveredAccountMinDeltaSpores
                : process.env.LICHEN_WATCHTOWER_DISCOVERED_ACCOUNT_MIN_DELTA_SPORES,
            DEFAULT_DISCOVERED_ACCOUNT_MIN_DELTA_SPORES,
        ),
        discoverTokenBalances: parseTokenBalanceDiscoveryMode(
            options.discoverTokenBalances !== undefined
                ? options.discoverTokenBalances
                : process.env.LICHEN_WATCHTOWER_DISCOVER_TOKEN_BALANCES,
        ),
        discoveredTokenMinDelta: parseBigIntValue(
            options.discoveredTokenMinDelta !== undefined
                ? options.discoveredTokenMinDelta
                : process.env.LICHEN_WATCHTOWER_DISCOVERED_TOKEN_MIN_DELTA,
            DEFAULT_DISCOVERED_TOKEN_MIN_DELTA,
        ),
        reconnectDelayMs: parsePositiveInteger(
            options.reconnectDelayMs !== undefined
                ? options.reconnectDelayMs
                : process.env.LICHEN_WATCHTOWER_RECONNECT_MS,
            DEFAULT_RECONNECT_DELAY_MS,
        ),
        largeTransferSporeThreshold: parseBigIntValue(
            options.largeTransferSporeThreshold !== undefined
                ? options.largeTransferSporeThreshold
                : process.env.LICHEN_WATCHTOWER_LARGE_TRANSFER_SPORES,
            DEFAULT_LARGE_TRANSFER_SPORES,
        ),
        accountWatches,
        tokenWatches,
        subscriptionSpecs: buildSubscriptionSpecs({ accountWatches, tokenWatches }),
        signal: options.signal,
        logger: options.logger || console,
        alertSink: options.alertSink || createAlertSink({
            logger: options.logger || console,
            slackWebhook: options.slackWebhook || process.env.LICHEN_SLACK_WEBHOOK || '',
            webhookUrl: options.webhookUrl || process.env.LICHEN_WATCHTOWER_WEBHOOK || '',
        }),
    };
}

async function seedAccountBaselines(config) {
    const baselines = new Map();
    for (const watch of config.accountWatches) {
        const result = await rpcRequest(config.rpcUrl, 'getBalance', [watch.pubkey]);
        baselines.set(watch.pubkey, parseBigIntValue(result && result.spores, 0n));
    }
    return baselines;
}

function connectOnce(config) {
    return new Promise((resolve) => {
        const pendingAlerts = new Set();
        let settled = false;
        let signalCleanup = null;
        let ws = null;
        const session = {
            pendingRequests: new Map(),
            subscriptions: new Map(),
            accountBaselines: new Map(),
        };

        function finish() {
            if (settled) {
                return;
            }
            settled = true;
            if (signalCleanup) {
                signalCleanup();
                signalCleanup = null;
            }
            Promise.allSettled(Array.from(pendingAlerts)).finally(resolve);
        }

        function track(promise) {
            pendingAlerts.add(promise);
            promise.finally(() => pendingAlerts.delete(promise));
        }

        if (config.signal) {
            const onAbort = () => {
                try {
                    if (ws) {
                        ws.close();
                    } else {
                        finish();
                    }
                } catch {
                    finish();
                }
            };
            config.signal.addEventListener('abort', onAbort, { once: true });
            signalCleanup = () => config.signal.removeEventListener('abort', onAbort);
        }

        Promise.resolve(seedAccountBaselines(config))
            .then((accountBaselines) => {
                session.accountBaselines = accountBaselines;
                ws = new WebSocket(config.wsUrl);

                ws.on('open', () => {
                    let requestId = 1;
                    for (const spec of config.subscriptionSpecs) {
                        session.pendingRequests.set(requestId, spec);
                        ws.send(JSON.stringify(buildSubscribeRequest(requestId, spec.method, spec.params)));
                        requestId += 1;
                    }
                    config.logger.log(`Governance watchtower connected to ${config.wsUrl}`);
                });

                ws.on('message', (raw) => {
                    let payload;
                    try {
                        payload = JSON.parse(raw.toString());
                    } catch (error) {
                        config.logger.error(`Ignoring malformed WS payload: ${error.message}`);
                        return;
                    }

                    if (payload && payload.id && session.pendingRequests.has(payload.id)) {
                        const spec = session.pendingRequests.get(payload.id);
                        session.pendingRequests.delete(payload.id);
                        if (payload.error) {
                            config.logger.error(`Subscription failed for ${spec.label}: ${payload.error.message}`);
                            return;
                        }
                        if (typeof payload.result === 'number') {
                            session.subscriptions.set(payload.result, spec);
                        }
                        return;
                    }

                    if (!payload || payload.method !== 'subscription' || !payload.params) {
                        return;
                    }

                    const spec = session.subscriptions.get(payload.params.subscription);
                    const result = payload.params.result;
                    if (!spec) {
                        return;
                    }

                    let alerts = [];
                    if (spec.type === 'governance') {
                        const event = normalizeGovernanceEvent(result);
                        if (!event) {
                            return;
                        }
                        alerts = classifyGovernanceEvent(event, config);
                    } else if (spec.type === 'account') {
                        alerts = classifyAccountChange(spec.watch, result, session.accountBaselines);
                    } else if (spec.type === 'token-balance') {
                        alerts = classifyTokenBalanceChange(spec.watch, result);
                    }

                    for (const alert of alerts) {
                        const task = Promise.resolve(config.alertSink(alert)).catch((error) => {
                            config.logger.error(`Alert delivery failed: ${error.message}`);
                        });
                        track(task);
                    }
                });

                ws.on('error', (error) => {
                    config.logger.error(`Governance watchtower socket error: ${error.message}`);
                });

                ws.on('close', () => {
                    finish();
                });
            })
            .catch((error) => {
                config.logger.error(`Failed to seed watchtower baselines: ${error.message}`);
                finish();
            });
    });
}

async function runGovernanceWatchtower(options = {}) {
    const baseConfig = normalizeConfig(options);

    while (!baseConfig.signal || !baseConfig.signal.aborted) {
        const runtimeConfig = await buildRuntimeConfig(baseConfig);
        await connectOnce(runtimeConfig);
        if (baseConfig.signal && baseConfig.signal.aborted) {
            break;
        }
        baseConfig.logger.log(`Governance watchtower reconnecting in ${baseConfig.reconnectDelayMs}ms`);
        await delay(baseConfig.reconnectDelayMs, baseConfig.signal);
    }
}

async function main() {
    if (process.argv.includes('--help') || process.argv.includes('-h')) {
        process.stdout.write(`${usage()}\n`);
        return;
    }

    const controller = new AbortController();
    process.on('SIGINT', () => controller.abort());
    process.on('SIGTERM', () => controller.abort());

    await runGovernanceWatchtower({ signal: controller.signal });
}

if (require.main === module) {
    main().catch((error) => {
        process.stderr.write(`Governance watchtower failed: ${error.message}\n`);
        process.exitCode = 1;
    });
}

module.exports = {
    ALERT_RULES,
    DEFAULT_LARGE_TRANSFER_SPORES,
    buildAlertMessage,
    buildDiscoveredAccountWatches,
    buildDiscoveredTokenWatches,
    buildSubscribeRequest,
    classifyAccountChange,
    classifyGovernanceEvent,
    classifyTokenBalanceChange,
    createAlertSink,
    deriveRpcUrl,
    discoverTokenWatches,
    discoverAccountWatches,
    extractDiscoverableWallets,
    extractTokenAccountsByOwner,
    extractGovernanceEvent,
    mergeAccountWatches,
    mergeTokenWatches,
    normalizeConfig,
    parseMetadata,
    parseNativeAccountDiscoveryMode,
    parseTokenBalanceDiscoveryMode,
    runGovernanceWatchtower,
    severityForEvent,
    sporesToDisplay,
};
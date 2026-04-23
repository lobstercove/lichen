// ============================================================
// Lichen Mission Control - Dashboard Engine
// Real-time monitoring with auto-refresh
// ============================================================

const NETWORKS = {
    'mainnet': 'https://rpc.lichen.network',
    'testnet': 'https://testnet-rpc.lichen.network',
    'local-testnet': 'http://localhost:8899',
    'local-mainnet': 'http://localhost:9899'
};

// INF-06: VALIDATOR_RPCS removed — all validator discovery is dynamic via
// getClusterInfo RPC with getValidators fallback. Kill switch, health checks,
// and validator rendering all use live cluster data, never a hardcoded list.

const SYMBOLS = [
    'LUSD', 'WETH', 'WSOL', 'WBNB', 'DEX', 'DEXAMM', 'DEXGOV', 'DEXMARGIN',
    'DEXREWARDS', 'DEXROUTER', 'BRIDGE', 'DAO', 'SPOREVAULT', 'SPOREPAY',
    'SPOREPUMP', 'ORACLE', 'LEND', 'MARKET', 'AUCTION', 'BOUNTY', 'ANALYTICS',
    'COMPUTE', 'LICHENSWAP', 'PUNKS', 'MOSS', 'SHIELDED', 'PREDICT', 'YID'
];

const REFRESH_MS = 3000;
const CONTRACT_REFRESH_MS = 30000;
const WS_STALE_MS = 12000;
const WS_RECONNECT_MS = 3000;
// SPORES_PER_LICN loaded from ../shared/utils.js

const _monIsProduction = (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.isProduction) ||
    (window.location.hostname !== 'localhost' && window.location.hostname !== '127.0.0.1');
const _monProductionNetwork = 'testnet';
const _monDefaultNetwork = _monIsProduction ? _monProductionNetwork : 'local-testnet';

function normalizeMonitoringNetwork(network) {
    const resolved = NETWORKS[network] ? network : _monDefaultNetwork;
    if (_monIsProduction && resolved === 'mainnet') {
        return _monProductionNetwork;
    }
    return resolved;
}

function currentMonitoringNetwork() {
    const stored = localStorage.getItem('lichen_mon_network');
    const normalized = normalizeMonitoringNetwork(stored || _monDefaultNetwork);
    if (stored !== normalized) {
        localStorage.setItem('lichen_mon_network', normalized);
    }
    return normalized;
}

function resolveRpcUrl(network) {
    const selected = normalizeMonitoringNetwork(network || currentMonitoringNetwork());
    if (typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.rpc === 'function') {
        return LICHEN_CONFIG.rpc(selected);
    }
    return NETWORKS[selected] || NETWORKS[_monDefaultNetwork];
}

function resolveWsUrl(network) {
    const selected = normalizeMonitoringNetwork(network || currentMonitoringNetwork());
    if (typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.ws === 'function') {
        return LICHEN_CONFIG.ws(selected);
    }
    const rpc = resolveRpcUrl(selected);
    return rpc.replace(/^http/, 'ws').replace(/\/$/, '') + '/ws';
}

let rpcUrl = resolveRpcUrl(currentMonitoringNetwork());
let tpsHistory = [];
let lastSlot = 0;
let genesisTimestampSecs = null;
let eventLog = [];
let rejectedTxCount = 0;
let alertCount = 0;
let lastRpcLatencyMs = null;
let lastMetricsSnapshot = null;
let lastPeersSnapshot = null;
let lastCadenceSnapshot = null;
const cadenceTracker = {
    head: null,
    validators: new Map(),
};
const LEGACY_ADMIN_TOKEN_STORAGE_KEY = 'lichen_admin_token';
const wsProbe = {
    socket: null,
    reconnectTimer: null,
    subscriptionRequestId: null,
    subscriptionId: null,
    status: 'connecting',
    lastMessageAt: 0,
    lastSlot: null,
    url: '',
};

// ── RPC Client ──────────────────────────────────────────────

async function rpc(method, params = [], url = null) {
    try {
        const resp = await fetch(url || rpcUrl, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params })
        });
        if (!resp.ok) {
            console.warn(`RPC ${method}: HTTP ${resp.status}`);
            return null;
        }
        const text = await resp.text();
        let data;
        try {
            data = JSON.parse(text);
        } catch (parseErr) {
            console.warn(`RPC ${method}: invalid JSON`, text.slice(0, 200));
            return null;
        }
        if (data.error) return null;
        return data.result ?? null;
    } catch (e) {
        console.warn(`RPC ${method}: fetch failed`, e.message);
        return null;
    }
}

function getTrustedMonitoringNetwork() {
    if (typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.currentNetwork === 'function') {
        return LICHEN_CONFIG.currentNetwork('lichen_mon_network');
    }
    return currentMonitoringNetwork();
}

async function trustedMonitoringRpc(method, params = []) {
    if (typeof signedMetadataRpcCall === 'function') {
        return signedMetadataRpcCall(method, params, getTrustedMonitoringNetwork(), function (resolvedMethod, resolvedParams) {
            if (typeof trustedLichenRpcCall === 'function') {
                return trustedLichenRpcCall(resolvedMethod, resolvedParams, getTrustedMonitoringNetwork());
            }
            return rpc(resolvedMethod, resolvedParams, typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.rpc === 'function'
                ? LICHEN_CONFIG.rpc(getTrustedMonitoringNetwork())
                : null);
        });
    }
    if (typeof trustedLichenRpcCall === 'function') {
        return trustedLichenRpcCall(method, params, getTrustedMonitoringNetwork());
    }
    return rpc(method, params, typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.rpc === 'function'
        ? LICHEN_CONFIG.rpc(getTrustedMonitoringNetwork())
        : null);
}

function isLocalMonitoringNetwork(network) {
    const normalized = normalizeMonitoringNetwork(network || getTrustedMonitoringNetwork());
    return normalized === 'local-testnet' || normalized === 'local-mainnet';
}

async function getMonitoringSymbolRegistryEntry(symbol) {
    if (isLocalMonitoringNetwork()) {
        const liveEntry = await rpc('getSymbolRegistry', [symbol]).catch(() => null);
        if (liveEntry && liveEntry.program) {
            return liveEntry;
        }
    }

    return trustedMonitoringRpc('getSymbolRegistry', [symbol]).catch(() => null);
}

// ── Helpers ─────────────────────────────────────────────────

function sporesToLicn(spores) {
    return (spores / SPORES_PER_LICN).toFixed(2);
}

function formatLicn(spores) {
    const licn = spores / SPORES_PER_LICN;
    if (licn >= 1e9) return (licn / 1e9).toFixed(2) + 'B';
    if (licn >= 1e6) return (licn / 1e6).toFixed(2) + 'M';
    if (licn >= 1e3) return (licn / 1e3).toFixed(1) + 'K';
    return licn.toFixed(2);
}

function trimTrailingZeros(value) {
    return String(value).replace(/(?:\.0+|(?:(\.[0-9]*?)0+))$/, '$1');
}

function formatLicnPrecise(spores) {
    const numeric = Number(spores);
    if (!Number.isFinite(numeric)) return '--';

    const licn = numeric / SPORES_PER_LICN;
    const absLicn = Math.abs(licn);

    if (absLicn === 0) return '0';
    if (absLicn >= 1e3) return formatLicn(numeric);
    if (absLicn >= 1) return trimTrailingZeros(licn.toFixed(3));
    if (absLicn >= 0.01) return trimTrailingZeros(licn.toFixed(4));
    if (absLicn >= 0.000001) return trimTrailingZeros(licn.toFixed(6));
    return trimTrailingZeros(licn.toFixed(9));
}

function formatNum(n) {
    if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
    if (n >= 1e3) return (n / 1e3).toFixed(1) + 'K';
    return n.toLocaleString();
}

function formatExactNum(n) {
    const numeric = Number(n);
    if (!Number.isFinite(numeric)) return '--';
    return Math.trunc(numeric).toLocaleString();
}

function formatPercent(value, digits = 2) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric)) return '--';
    return `${numeric.toFixed(digits)}%`;
}

function truncAddr(addr) {
    if (!addr || addr.length < 12) return addr || '--';
    return addr.slice(0, 6) + '...' + addr.slice(-4);
}

function timeAgo(ts) {
    const s = Math.floor((Date.now() / 1000) - ts);
    if (s < 5) return 'just now';
    if (s < 60) return s + 's ago';
    if (s < 3600) return Math.floor(s / 60) + 'm ago';
    return Math.floor(s / 3600) + 'h ago';
}

function now() {
    return new Date().toLocaleTimeString('en-US', { hour12: false });
}

function formatDateTime(value) {
    if (!value) return '--';
    const parsed = new Date(value);
    if (Number.isNaN(parsed.getTime())) {
        return String(value);
    }
    return parsed.toLocaleString('en-US', {
        month: 'short',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
    });
}

function normalizeTimestampMs(value) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric) || numeric <= 0) return 0;
    return numeric > 1e12 ? numeric : numeric * 1000;
}

function timeAgoFromTimestamp(value) {
    const ms = normalizeTimestampMs(value);
    if (!ms) return '--';
    return timeAgo(Math.floor(ms / 1000));
}

function formatSignedPercent(value, digits = 1) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric)) return '--';
    const prefix = numeric > 0 ? '+' : '';
    return `${prefix}${numeric.toFixed(digits)}%`;
}

function clampPercentage(value) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric)) return 0;
    return Math.max(0, Math.min(100, numeric));
}

function parseGovernanceMetadata(metadata) {
    const result = {};
    const text = String(metadata || '').trim();
    if (!text) return result;

    text.split(/\s+/).forEach((token) => {
        const separator = token.indexOf('=');
        if (separator <= 0) return;
        const key = token.slice(0, separator);
        const value = token.slice(separator + 1);
        result[key] = value;
    });

    return result;
}

function governanceSeverityForMonitoring(ruleId, event, metadata) {
    const kind = String(event.kind || '').toLowerCase();
    if (ruleId === 'treasury-transfer') {
        const amount = Number(metadata.amount_spores || 0);
        if (kind === 'executed' && amount >= GOVERNANCE_LARGE_TRANSFER_SPORES) return 'critical';
        return kind === 'executed' ? 'high' : 'warning';
    }
    if (ruleId === 'insurance-withdrawal') {
        return kind === 'executed' ? 'critical' : 'high';
    }
    if (kind === 'executed') return 'critical';
    if (kind === 'approved') return 'high';
    if (kind === 'cancelled') return 'warning';
    return 'high';
}

function classifyGovernanceEventForMonitoring(event) {
    if (!event) return [];
    const metadata = parseGovernanceMetadata(event.metadata);
    const matches = [];

    const pushAlert = (ruleId, title) => {
        matches.push({
            ruleId,
            title,
            severity: governanceSeverityForMonitoring(ruleId, event, metadata),
            event,
            metadata,
        });
    };

    if (['contract_upgrade', 'execute_contract_upgrade', 'veto_contract_upgrade'].includes(event.action)) {
        pushAlert('contract-upgrade', 'Contract upgrade activity');
    }
    if (event.action === 'set_contract_upgrade_timelock') {
        pushAlert('timelock-change', 'Contract upgrade timelock change');
    }
    if (event.action === 'treasury_transfer') {
        pushAlert('treasury-transfer', 'Treasury transfer proposal');
    }
    if (event.action === 'contract_call' && GOVERNANCE_OWNERSHIP_FUNCTIONS.has(event.target_function)) {
        pushAlert('ownership-change', 'Contract ownership or admin change');
    }
    if (event.action === 'contract_call' && GOVERNANCE_BRIDGE_FUNCTIONS.has(event.target_function)) {
        pushAlert('bridge-control-change', 'Bridge validator or timeout control change');
    }
    if (event.action === 'contract_call' && GOVERNANCE_ORACLE_FUNCTIONS.has(event.target_function)) {
        pushAlert('oracle-control-change', 'Oracle committee change');
    }
    if (event.action === 'contract_call' && event.target_function === 'withdraw_insurance') {
        pushAlert('insurance-withdrawal', 'Insurance withdrawal');
    }
    if (event.action === 'contract_call' && /(?:^|_)(?:pause|unpause)$/.test(event.target_function || '')) {
        pushAlert('pause-change', 'Pause or unpause change');
    }

    return matches;
}

function buildGovernedWalletEntries(rewardInfo) {
    const wallets = rewardInfo?.wallets || {};
    return Object.entries(wallets)
        .filter(([label, info]) => GOVERNANCE_WATCH_GOVERNED_LABELS.has(label) && info?.pubkey && info.pubkey !== 'unknown')
        .map(([label, info]) => ({
            label,
            pubkey: info.pubkey,
            balance: Number(info.balance_spores ?? info.balance ?? 0),
        }));
}

function buildGovernanceAlertMeta(alert) {
    const metadata = alert.metadata || {};
    const event = alert.event || {};
    const target = event.target_function || metadata.function || metadata.contract || 'watch';
    const amount = metadata.amount_spores ? ` · ${formatLicn(Number(metadata.amount_spores || 0))} LICN` : '';
    return `${String(event.kind || 'unknown').toUpperCase()} · ${target} · proposal ${formatNum(Number(event.proposal_id || 0))}${amount}`;
}

function buildSolanaCompatUrl(baseUrl) {
    return String(baseUrl || '').replace(/\/$/, '') + '/solana-compat';
}

async function solanaCompatRpc(method, params = []) {
    return rpc(method, params, buildSolanaCompatUrl(rpcUrl));
}

function renderOperatorTierCard(label, value, meta, icon, color, barPct = null) {
    return `<div class="tier-card">
        <div class="tier-label"><i class="fas fa-${icon}" style="margin-right:4px;color:${color}"></i>${escapeHtml(label)}</div>
        <div class="tier-value">${escapeHtml(value)}</div>
        <div class="tier-meta">${escapeHtml(meta)}</div>
        ${barPct === null ? '' : `<div class="tier-bar"><div class="tier-fill" style="width:${clampPercentage(barPct)}%;background:${color}"></div></div>`}
    </div>`;
}

function formatDurationSeconds(totalSeconds) {
    const seconds = Math.max(0, Math.floor(Number(totalSeconds) || 0));
    const days = Math.floor(seconds / 86400);
    const hours = Math.floor((seconds % 86400) / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const secs = seconds % 60;

    if (days > 0) return `${days}d ${hours}h ${minutes}m`;
    return `${hours}h ${minutes}m ${secs}s`;
}

async function ensureGenesisTimestamp() {
    if (Number.isFinite(genesisTimestampSecs) && genesisTimestampSecs > 0) {
        return genesisTimestampSecs;
    }

    const genesisBlock = await rpc('getBlock', [0]).catch(() => null);
    const timestamp = Number(genesisBlock?.timestamp || genesisBlock?.blockTime || 0);
    if (Number.isFinite(timestamp) && timestamp > 0) {
        genesisTimestampSecs = timestamp;
    }
    return genesisTimestampSecs;
}

function networkAge() {
    if (!Number.isFinite(genesisTimestampSecs) || genesisTimestampSecs <= 0) {
        return '--';
    }
    const nowSecs = Math.floor(Date.now() / 1000);
    return formatDurationSeconds(nowSecs - genesisTimestampSecs);
}

function setText(id, text) {
    const element = document.getElementById(id);
    if (element) element.textContent = text;
}

function median(values) {
    if (!Array.isArray(values) || values.length === 0) return 0;
    const sorted = values
        .map(value => Number(value))
        .filter(value => Number.isFinite(value) && value > 0)
        .sort((a, b) => a - b);
    if (sorted.length === 0) return 0;
    return sorted[Math.floor(sorted.length / 2)];
}

function formatCadenceMs(value) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric) || numeric <= 0) return '--';
    return `${Math.round(numeric)}ms`;
}

function appendCadenceSample(samples, previousSlot, nextSlot, deltaMs) {
    const slotDelta = Number(nextSlot) - Number(previousSlot);
    if (!Number.isFinite(slotDelta) || slotDelta <= 0 || slotDelta > 8) {
        return;
    }
    const elapsedMs = Number(deltaMs);
    if (!Number.isFinite(elapsedMs) || elapsedMs <= 0) {
        return;
    }
    samples.push(elapsedMs / slotDelta);
}

function deriveCadenceSnapshot(metrics, cluster, currentSlot) {
    const sampledAtMs = Date.now();
    const cadenceTargetMs = Number(metrics?.cadence_target_ms || metrics?.slot_duration_ms || 0);
    const nodes = Array.isArray(cluster?.cluster_nodes) ? cluster.cluster_nodes : [];
    const samples = [];

    if (Number.isFinite(currentSlot) && currentSlot > 0) {
        if (cadenceTracker.head && currentSlot > cadenceTracker.head.slot) {
            appendCadenceSample(
                samples,
                cadenceTracker.head.slot,
                currentSlot,
                sampledAtMs - cadenceTracker.head.sampledAtMs,
            );
        }
        cadenceTracker.head = { slot: currentSlot, sampledAtMs };
    }

    const activePubkeys = new Set();
    nodes.forEach((node) => {
        const pubkey = node?.pubkey;
        const blockSlot = Number(node?.last_observed_block_slot || 0);
        if (!pubkey) return;
        activePubkeys.add(pubkey);
        if (!Number.isFinite(blockSlot) || blockSlot <= 0) {
            cadenceTracker.validators.set(pubkey, { slot: 0, sampledAtMs });
            return;
        }

        const previous = cadenceTracker.validators.get(pubkey);
        if (previous && blockSlot > previous.slot) {
            appendCadenceSample(
                samples,
                previous.slot,
                blockSlot,
                sampledAtMs - previous.sampledAtMs,
            );
        }
        cadenceTracker.validators.set(pubkey, { slot: blockSlot, sampledAtMs });
    });

    Array.from(cadenceTracker.validators.keys()).forEach((pubkey) => {
        if (!activePubkeys.has(pubkey)) {
            cadenceTracker.validators.delete(pubkey);
        }
    });

    const clusterStalenessMs = median(nodes
        .map(node => Number(node?.head_staleness_ms || 0))
        .filter(value => Number.isFinite(value) && value > 0)
        .sort((a, b) => a - b)) || 0;

    const intervalMs = median(samples) || Number(metrics?.observed_block_interval_ms || 0);
    const sampleCount = samples.length > 0 ? samples.length : Number(metrics?.cadence_samples || 0);
    const pacePct = cadenceTargetMs > 0 && intervalMs > 0
        ? clampPercentage((cadenceTargetMs / intervalMs) * 100)
        : clampPercentage(metrics?.slot_pace_pct || 0);

    lastCadenceSnapshot = {
        intervalMs,
        targetMs: cadenceTargetMs,
        pacePct: Math.round(pacePct),
        sampleCount,
        source: samples.length > 0 ? 'cluster_level_observer' : (metrics?.cadence_source || 'observer_wall_clock'),
        headStalenessMs: clusterStalenessMs || Number(metrics?.head_staleness_ms || 0),
        lastObservedBlockSlot: Number(metrics?.last_observed_block_slot || currentSlot || 0),
    };

    return lastCadenceSnapshot;
}

function validatorAvailabilityScore(probes) {
    const total = Array.isArray(probes) ? probes.length : 0;
    if (total === 0) return 0;
    const online = probes.filter((probe) => probe.online).length;
    return Math.round((online / total) * 100);
}

function p2pConnectivityScore(probes, peers) {
    const onlineProbes = Array.isArray(probes) ? probes.filter((probe) => probe.online) : [];
    const peerCount = Number(peers?.peer_count ?? peers?.count ?? 0);
    if (onlineProbes.length === 0) return 0;
    if (onlineProbes.length === 1) return 100;
    const expectedPeers = Math.max(1, onlineProbes.length - 1);
    return Math.min(100, Math.round((peerCount / expectedPeers) * 100));
}

function formatLatency(ms) {
    return Number.isFinite(ms) ? `${Math.round(ms)} ms` : '--';
}

function endpointHost(url) {
    try {
        return new URL(url).host;
    } catch (_) {
        return url || '--';
    }
}

function currentWsState() {
    if (wsProbe.status === 'online' && wsProbe.lastMessageAt && Date.now() - wsProbe.lastMessageAt > WS_STALE_MS) {
        return 'stale';
    }
    return wsProbe.status;
}

function updateStatusBeacon(rpcOnline) {
    const beacon = document.getElementById('statusBeacon');
    const beaconText = document.getElementById('beaconText');
    if (!beacon || !beaconText) return;

    const beaconDot = beacon.querySelector('.beacon-dot');
    if (!beaconDot) return;

    if (!rpcOnline) {
        beaconDot.className = 'beacon-dot offline';
        beaconText.textContent = 'RPC offline';
        return;
    }

    const wsState = currentWsState();
    const rpcLabel = formatLatency(lastRpcLatencyMs);

    if (wsState === 'online') {
        beaconDot.className = 'beacon-dot online';
        beaconText.textContent = `RPC ${rpcLabel} · WS live`;
        return;
    }

    beaconDot.className = 'beacon-dot degraded';
    if (wsState === 'connecting' || wsState === 'subscribing') {
        beaconText.textContent = `RPC ${rpcLabel} · WS connecting`;
    } else if (wsState === 'stale') {
        beaconText.textContent = `RPC ${rpcLabel} · WS stale`;
    } else {
        beaconText.textContent = `RPC ${rpcLabel} · WS down`;
    }
}

function updateEndpointTelemetry(peers = lastPeersSnapshot) {
    const peerCount = peers?.peer_count || peers?.count || 0;
    const wsState = currentWsState();
    const wsStatusText = wsState === 'online'
        ? 'LIVE'
        : wsState === 'stale'
            ? 'STALE'
            : wsState === 'connecting' || wsState === 'subscribing'
                ? 'CONNECTING'
                : 'DOWN';

    setText('endpointRpcLatency', formatLatency(lastRpcLatencyMs));
    setText('endpointRpcStatus', lastRpcLatencyMs !== null ? 'READY' : 'UNREACHABLE');
    setText('endpointWsStatus', wsStatusText);
    setText(
        'endpointWsLastPush',
        wsProbe.lastMessageAt ? timeAgo(Math.floor(wsProbe.lastMessageAt / 1000)) : 'No slot push yet'
    );
    setText('endpointWsSlot', wsProbe.lastSlot !== null ? formatNum(wsProbe.lastSlot) : '--');
    setText('endpointWsHost', endpointHost(resolveWsUrl(currentMonitoringNetwork())));
    setText('endpointPeerCount', formatNum(peerCount));
    setText('endpointRpcHost', endpointHost(rpcUrl));
}

function clearWsProbeReconnect() {
    if (wsProbe.reconnectTimer) {
        clearTimeout(wsProbe.reconnectTimer);
        wsProbe.reconnectTimer = null;
    }
}

function scheduleWsProbeReconnect() {
    if (wsProbe.reconnectTimer) return;
    wsProbe.reconnectTimer = setTimeout(() => {
        wsProbe.reconnectTimer = null;
        connectWsProbe();
    }, WS_RECONNECT_MS);
}

function teardownWsProbe(reconnect = false) {
    clearWsProbeReconnect();
    const socket = wsProbe.socket;
    wsProbe.socket = null;
    wsProbe.subscriptionId = null;
    wsProbe.subscriptionRequestId = null;

    if (socket) {
        socket.onopen = null;
        socket.onmessage = null;
        socket.onerror = null;
        socket.onclose = null;
        try {
            if (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING) {
                socket.close();
            }
        } catch (_) {
            // Ignore teardown errors on stale sockets.
        }
    }

    if (reconnect) {
        scheduleWsProbeReconnect();
    }
}

function connectWsProbe() {
    if (typeof WebSocket === 'undefined') return;

    const network = currentMonitoringNetwork();
    const url = resolveWsUrl(network);
    if (
        wsProbe.socket &&
        wsProbe.url === url &&
        (wsProbe.socket.readyState === WebSocket.OPEN || wsProbe.socket.readyState === WebSocket.CONNECTING)
    ) {
        return;
    }

    teardownWsProbe(false);
    wsProbe.url = url;
    wsProbe.status = 'connecting';
    wsProbe.lastSlot = null;
    updateEndpointTelemetry();
    updateStatusBeacon(lastRpcLatencyMs !== null);

    const socket = new WebSocket(url);
    const subscriptionRequestId = Date.now();
    wsProbe.socket = socket;
    wsProbe.subscriptionRequestId = subscriptionRequestId;

    socket.onopen = () => {
        if (wsProbe.socket !== socket) return;
        wsProbe.status = 'subscribing';
        socket.send(JSON.stringify({
            jsonrpc: '2.0',
            id: subscriptionRequestId,
            method: 'subscribeSlots',
            params: [],
        }));
        updateEndpointTelemetry();
        updateStatusBeacon(lastRpcLatencyMs !== null);
    };

    socket.onmessage = (event) => {
        if (wsProbe.socket !== socket) return;

        let msg;
        try {
            msg = JSON.parse(event.data);
        } catch (_) {
            return;
        }

        if (msg.id === subscriptionRequestId) {
            if (msg.error) {
                wsProbe.status = 'offline';
                updateEndpointTelemetry();
                updateStatusBeacon(lastRpcLatencyMs !== null);
                teardownWsProbe(true);
                return;
            }

            wsProbe.subscriptionId = msg.result;
            wsProbe.status = 'online';
            wsProbe.lastMessageAt = Date.now();
            updateEndpointTelemetry();
            updateStatusBeacon(lastRpcLatencyMs !== null);
            return;
        }

        if (msg.method === 'subscription' && msg.params) {
            const result = msg.params.result || {};
            if (typeof result.slot === 'number') {
                wsProbe.lastSlot = result.slot;
            }
            wsProbe.lastMessageAt = Date.now();
            wsProbe.status = 'online';
            updateEndpointTelemetry();
            updateStatusBeacon(lastRpcLatencyMs !== null);
        }
    };

    socket.onerror = () => {
        if (wsProbe.socket !== socket) return;
        wsProbe.status = 'offline';
        updateEndpointTelemetry();
        updateStatusBeacon(lastRpcLatencyMs !== null);
    };

    socket.onclose = () => {
        if (wsProbe.socket !== socket) return;
        wsProbe.socket = null;
        wsProbe.subscriptionId = null;
        wsProbe.subscriptionRequestId = null;
        wsProbe.status = 'offline';
        updateEndpointTelemetry();
        updateStatusBeacon(lastRpcLatencyMs !== null);
        scheduleWsProbeReconnect();
    };
}

function resetMonitoringCaches() {
    tpsHistory = [];
    displayedBlocks = [];
    contractsLoaded = false;
    contractsLoadedAt = 0;
    contractMonitorLoaded = false;
    contractMonitorLoadedAt = 0;
    dexDataLoaded = false;
    identityMonitorLoaded = false;
    tradingMetricsLoaded = false;
    predictionMonitorLoaded = false;
    ecosystemMonitorLoaded = false;
    controlPlaneMonitorLoaded = false;
    controlPlaneMonitorLoadedAt = 0;
    missionControlExpansionLoaded = false;
    missionControlExpansionLoadedAt = 0;
    lastRpcLatencyMs = null;
    lastMetricsSnapshot = null;
    lastPeersSnapshot = null;
    lastCadenceSnapshot = null;
    cadenceTracker.head = null;
    cadenceTracker.validators.clear();
    lastSlot = 0;
}

// ── Event Feed ──────────────────────────────────────────────

function addEvent(type, icon, text) {
    eventLog.unshift({ type, icon, text, time: now() });
    if (eventLog.length > 100) eventLog.pop();
    renderEvents();
}

function renderEvents() {
    const el = document.getElementById('eventFeed');
    // F17.1 fix: escape all dynamic text in event feed
    el.innerHTML = eventLog.slice(0, 50).map(e => `
        <div class="event-item ${escapeHtml(e.type)}">
            <span class="event-time">${escapeHtml(e.time)}</span>
            <span class="event-icon"><i class="fas fa-${escapeHtml(e.icon)}"></i></span>
            <span class="event-text">${escapeHtml(e.text)}</span>
        </div>
    `).join('');
}

function clearEvents() {
    eventLog = [];
    renderEvents();
}

// ── Network Switch ──────────────────────────────────────────

function switchNetwork(network) {
    const normalized = normalizeMonitoringNetwork(network);
    localStorage.setItem('lichen_mon_network', normalized);
    rpcUrl = resolveRpcUrl(normalized);
    resetMonitoringCaches();
    connectWsProbe();
    void LICHEN_CONFIG.refreshIncidentStatusBanner(normalized);
    addEvent('info', 'exchange-alt', `Switched to ${normalized}`);
    refresh();
}

function dismissAlert() {
    document.getElementById('alertBanner').style.display = 'none';
}

function showAlert(msg) {
    document.getElementById('alertText').textContent = msg;
    document.getElementById('alertBanner').style.display = 'flex';
    alertCount++;
    document.getElementById('secAlerts').textContent = alertCount;
}

// ── Vitals Flash ────────────────────────────────────────────

function flashVital(id, value) {
    const el = document.getElementById(id);
    if (!el) return;
    const changed = el.textContent !== String(value);
    el.textContent = value;
    if (changed) {
        el.classList.add('updated');
        setTimeout(() => el.classList.remove('updated'), 800);
    }
}

// ── TPS Chart (pure canvas) ─────────────────────────────────

let tpsRange = 60; // seconds

// F17.7 fix: accept event parameter explicitly instead of relying on implicit global
function setTPSRange(range, evt) {
    document.querySelectorAll('.panel-controls .btn-sm').forEach(b => b.classList.remove('active'));
    if (evt && evt.target) evt.target.classList.add('active');
    if (range === '1m') tpsRange = 60;
    else if (range === '5m') tpsRange = 300;
    else tpsRange = 900;
    drawTPSChart();
}

function drawTPSChart() {
    const canvas = document.getElementById('tpsChart');
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    const rect = canvas.parentElement.getBoundingClientRect();
    canvas.width = rect.width;
    canvas.height = 200;

    const w = canvas.width;
    const h = canvas.height;
    const pad = { top: 20, right: 20, bottom: 30, left: 50 };
    const plotW = w - pad.left - pad.right;
    const plotH = h - pad.top - pad.bottom;

    // Filter to range
    const cutoff = Date.now() - tpsRange * 1000;
    const data = tpsHistory.filter(d => d.t >= cutoff);

    ctx.clearRect(0, 0, w, h);

    // Background
    ctx.fillStyle = '#060812';
    ctx.fillRect(0, 0, w, h);

    if (data.length < 2) {
        ctx.fillStyle = '#6B7A99';
        ctx.font = '13px Inter';
        ctx.textAlign = 'center';
        ctx.fillText('Collecting data...', w / 2, h / 2);
        return;
    }

    const maxTPS = Math.max(1, ...data.map(d => d.v));
    const yScale = plotH / (maxTPS * 1.1);
    const xScale = plotW / (data[data.length - 1].t - data[0].t || 1);

    // Grid lines
    ctx.strokeStyle = '#1F2544';
    ctx.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
        const y = pad.top + plotH - (plotH * i / 4);
        ctx.beginPath();
        ctx.moveTo(pad.left, y);
        ctx.lineTo(w - pad.right, y);
        ctx.stroke();

        ctx.fillStyle = '#6B7A99';
        ctx.font = '11px JetBrains Mono';
        ctx.textAlign = 'right';
        ctx.fillText((maxTPS * i / 4).toFixed(1), pad.left - 8, y + 4);
    }

    // Line
    ctx.beginPath();
    ctx.strokeStyle = '#00C9DB';
    ctx.lineWidth = 2;
    ctx.lineJoin = 'round';

    data.forEach((d, i) => {
        const x = pad.left + (d.t - data[0].t) * xScale;
        const y = pad.top + plotH - d.v * yScale;
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
    });
    ctx.stroke();

    // Fill under curve
    const grad = ctx.createLinearGradient(0, pad.top, 0, pad.top + plotH);
    grad.addColorStop(0, 'rgba(0, 201, 219,0.2)');
    grad.addColorStop(1, 'rgba(0, 201, 219,0)');

    ctx.lineTo(pad.left + (data[data.length - 1].t - data[0].t) * xScale, pad.top + plotH);
    ctx.lineTo(pad.left, pad.top + plotH);
    ctx.closePath();
    ctx.fillStyle = grad;
    ctx.fill();

    // X-axis label
    ctx.fillStyle = '#6B7A99';
    ctx.font = '11px Inter';
    ctx.textAlign = 'center';
    ctx.fillText(`Last ${tpsRange}s`, w / 2, h - 5);
}

// ── Performance Rings ───────────────────────────────────────

function setRing(id, pct) {
    const el = document.getElementById(id);
    if (!el) return;
    const fill = el.querySelector('.ring-fill');
    const val = el.querySelector('.ring-value');
    if (fill) fill.setAttribute('stroke-dasharray', `${pct}, 100`);
    if (val) val.textContent = pct + '%';
}

// ── Refresh Logic ───────────────────────────────────────────

async function refresh() {
    try {
        if (!wsProbe.socket) {
            connectWsProbe();
        }

        const refreshStartedAt = performance.now();

        // Fetch all data in parallel
        const [slot, metrics, peers, cluster] = await Promise.all([
            rpc('getSlot'),
            rpc('getMetrics'),
            rpc('getPeers'),
            rpc('getClusterInfo'),
        ]);

        lastRpcLatencyMs = Math.round(performance.now() - refreshStartedAt);
        lastMetricsSnapshot = metrics;
        lastPeersSnapshot = peers;
        updateStatusBeacon(slot !== null);
        updateEndpointTelemetry(peers);

        // Online status
        if (slot === null) {
            addEvent('danger', 'plug', 'Lost connection to RPC');
            return;
        }

        // ─ Vitals ─
        if (slot !== null) {
            if (slot !== lastSlot && lastSlot > 0) {
                addEvent('primary', 'cube', `New block #${slot}`);
            }
            lastSlot = slot;
            flashVital('vitalSlot', formatNum(slot));
        }

        if (metrics) {
            await ensureGenesisTimestamp();
            flashVital('vitalTPS', metrics.tps !== undefined ? metrics.tps.toFixed(1) : '--');
            flashVital('vitalTotalTx', formatNum(metrics.total_transactions || 0));
            flashVital('vitalUptime', networkAge());

            // TPS history
            tpsHistory.push({ t: Date.now(), v: metrics.tps || 0 });
            if (tpsHistory.length > 3000) tpsHistory.shift();
            drawTPSChart();

            // Supply
            const totalSupply = metrics.total_supply || 0;
            const totalBurned = metrics.total_burned || 0;
            const totalStaked = metrics.total_staked || 0;
            const effectiveSupply = totalSupply - totalBurned;
            const genesisSpores = metrics.genesis_balance || 0;
            const circulating = metrics.circulating_supply || 0;
            const nonCirculating = Math.max(0, effectiveSupply - circulating);
            document.getElementById('supplyTotal').textContent = formatLicn(totalSupply) + ' LICN';
            document.getElementById('supplyEffective').textContent = formatLicn(effectiveSupply) + ' LICN';
            document.getElementById('supplyStaked').textContent = formatLicn(totalStaked) + ' LICN';
            document.getElementById('supplyBurned').textContent = formatLicn(totalBurned) + ' LICN';
            document.getElementById('supplyNonCirculating').textContent = formatLicn(nonCirculating) + ' LICN';
            document.getElementById('supplyGenesis').textContent = formatLicn(genesisSpores) + ' LICN';
            setText(
                'supplyFormulaNote',
                `Circulating = minted − burned − genesis (${formatLicn(genesisSpores)} LICN) − staked (${formatLicn(totalStaked)} LICN).`
            );

            // Whitepaper distribution wallets from getMetrics.distribution_wallets
            const dw = metrics.distribution_wallets || {};
            const vrBal = dw.validator_rewards_balance || 0;
            const ctBal = dw.community_treasury_balance || 0;
            const bgBal = dw.builder_grants_balance || 0;
            const fmBal = dw.founding_symbionts_balance || 0;
            const epBal = dw.ecosystem_partnerships_balance || 0;
            const rpBal = dw.reserve_pool_balance || 0;

            document.getElementById('supplyValidatorRewards').textContent = formatLicn(vrBal) + ' LICN';
            document.getElementById('supplyCommunityTreasury').textContent = formatLicn(ctBal) + ' LICN';
            document.getElementById('supplyBuilderGrants').textContent = formatLicn(bgBal) + ' LICN';
            document.getElementById('supplyFoundingSymbionts').textContent = formatLicn(fmBal) + ' LICN';
            document.getElementById('supplyEcosystemPartnerships').textContent = formatLicn(epBal) + ' LICN';
            document.getElementById('supplyReservePool').textContent = formatLicn(rpBal) + ' LICN';

            // Circulating supply from RPC
            document.getElementById('supplyCirculating').textContent = formatLicn(circulating) + ' LICN';

            // Supply bar: distribution wallets proportional segments
            const totalDist = vrBal + ctBal + bgBal + fmBal + epBal + rpBal;
            const base = totalDist > 0 ? totalDist : total;
            document.getElementById('segTreasury').style.width = (vrBal / base * 100).toFixed(1) + '%';
            document.getElementById('segCommunity').style.width = (ctBal / base * 100).toFixed(1) + '%';
            document.getElementById('segBuilder').style.width = (bgBal / base * 100).toFixed(1) + '%';
            document.getElementById('segFounding').style.width = (fmBal / base * 100).toFixed(1) + '%';
            document.getElementById('segEcosystem').style.width = (epBal / base * 100).toFixed(1) + '%';
            document.getElementById('segReserve').style.width = (rpBal / base * 100).toFixed(1) + '%';

            // Contract count
            document.getElementById('contractCount').textContent = metrics.total_contracts || '--';

        }

        // ─ Validators ─
        const probes = await renderValidators(cluster, slot);
        const cadence = deriveCadenceSnapshot(metrics, cluster, slot);

        flashVital('vitalBlockTime', formatCadenceMs(cadence.intervalMs));

        if (metrics) {
            // Performance stats
            document.getElementById('perfAvgBlock').textContent = formatCadenceMs(cadence.intervalMs);
            document.getElementById('perfAvgTxBlock').textContent = (metrics.avg_txs_per_block || 0).toFixed(2);
            document.getElementById('perfAccounts').textContent = formatNum(metrics.total_accounts || 0);
            document.getElementById('perfActive').textContent = formatNum(metrics.active_accounts || 0);

            // INF-05: Performance rings are heuristic proxies derived from
            // on-chain metrics, NOT real OS-level CPU/Memory/Disk stats.
            // Labels are explicit about what each ring actually measures.
            const blockRate = cadence.pacePct || 0;
            // TPS vs Peak: current TPS relative to observed peak TPS.
            const peakTps = Math.max(1, metrics.peak_tps || metrics.tps || 1);
            const tpsLoadPct = Math.min(100, Math.round(((metrics.tps || 0) / peakTps) * 100));
            // Accounts: on-chain account count relative to capacity (~100k = 100%)
            const accountsPct = Math.min(100, Math.round((metrics.total_accounts || 0) / 1000));
            // Chain Size: slot height as proxy for data growth (~1.2M slots = 100%)
            const chainSizePct = Math.min(100, Math.round(slot / 12000));
            setRing('perfCPU', tpsLoadPct);
            setRing('perfMem', accountsPct);
            setRing('perfDisk', chainSizePct);
            setRing('perfNet', Math.min(95, blockRate));
        }

        // ─ Network Health ─
        await updateHealth(metrics, probes, peers, cadence);

        // ─ Threat Detection ─
        detectThreats(metrics, probes);

        // ─ Recent Blocks ─
        await updateRecentBlocks(slot);

        // ─ Contract Registry ─
        await updateContracts();

        // ─ DEX Operations Monitor (every 10s) ─
        if (!dexDataLoaded || Date.now() % 10000 < REFRESH_MS) {
            await updateDexMonitor();
            dexDataLoaded = true;
        }

        // ─ Smart Contracts Monitor (once) ─
        if (!contractMonitorLoaded || Date.now() - contractMonitorLoadedAt >= CONTRACT_REFRESH_MS) {
            await updateContractMonitor();
        }

        // ─ LichenID Identity Monitor (every 10s) ─
        if (!identityMonitorLoaded || Date.now() % 10000 < REFRESH_MS) {
            await updateIdentitiesMonitor();
        }

        // ─ Trading Metrics (every 10s) ─
        if (!tradingMetricsLoaded || Date.now() % 10000 < REFRESH_MS) {
            await updateTradingMetrics();
        }

        // ─ Prediction Markets (every 10s) ─
        if (!predictionMonitorLoaded || Date.now() % 10000 < REFRESH_MS) {
            await updatePredictionMonitor();
        }

        // ─ Platform Ecosystem (every 10s) ─
        if (!ecosystemMonitorLoaded || Date.now() % 10000 < REFRESH_MS) {
            await updateEcosystemMonitor();
        }

        // ─ Protocol Control Plane (every 30s) ─
        if (!controlPlaneMonitorLoaded || Date.now() - controlPlaneMonitorLoadedAt >= CONTRACT_REFRESH_MS) {
            await updateControlPlaneMonitor();
        }

        // ─ Mission Control Expansion Boards (every 30s) ─
        if (!missionControlExpansionLoaded || Date.now() - missionControlExpansionLoadedAt >= CONTRACT_REFRESH_MS) {
            await updateMissionControlExpansionBoards();
        }

        // ─ Footer ─
        document.getElementById('lastUpdate').textContent = now();

    } catch (e) {
        lastRpcLatencyMs = null;
        lastMetricsSnapshot = null;
        lastPeersSnapshot = null;
        updateStatusBeacon(false);
        updateEndpointTelemetry();
        console.error('Refresh error:', e);
    }
}

// ── Validator Rendering (DYNAMIC — queries cluster, no hardcoded ports) ──

async function renderValidators(cluster, currentSlot) {
    const grid = document.getElementById('validatorGrid');
    const badge = document.getElementById('valClusterBadge');

    let probes = [];

    if (cluster && cluster.cluster_nodes && cluster.cluster_nodes.length > 0) {
        // Dynamic path: build probe list from live cluster data
        probes = cluster.cluster_nodes
            .filter((node) => {
                const stake = Number(node.stake || 0);
                const blocks = Number(node.blocks_proposed || 0);
                const lastActive = Number(node.last_active_slot || 0);
                return stake > 0 || blocks > 0 || lastActive > 0;
            })
            .map((node, idx) => {
                const lastActive = Number(node.last_active_slot || 0);
                return {
                    name: `V${idx + 1}`,
                    rpc: rpcUrl,
                    pubkey: node.pubkey || null,
                    slot: lastActive,
                    observed_block_slot: Number(node.last_observed_block_slot || 0),
                    head_staleness_ms: Number(node.head_staleness_ms || 0),
                    head_hash: node.head_hash || node.tip_hash || node.block_hash || node.last_block_hash || null,
                    online: node.active !== false && (currentSlot === null || currentSlot - lastActive <= 100),
                    stake: node.stake || 0,
                    reputation: node.reputation || 0,
                    blocks_proposed: node.blocks_proposed || 0,
                    last_active_slot: lastActive,
                };
            });
    } else {
        // Fallback: if getClusterInfo not available, use getValidators
        const vals = await rpc('getValidators');
        if (vals && vals.validators) {
            probes = vals.validators.map((v, idx) => {
                const lastActive = v.last_active_slot || v.lastActiveSlot || 0;
                const isOnline = currentSlot !== null && currentSlot - lastActive <= 100;
                return {
                    name: `V${idx + 1}`,
                    rpc: rpcUrl,
                    pubkey: v.pubkey || null,
                    slot: lastActive,
                    observed_block_slot: Number(v.last_observed_block_slot || 0),
                    head_staleness_ms: Number(v.head_staleness_ms || 0),
                    head_hash: v.head_hash || v.tip_hash || v.block_hash || v.last_block_hash || null,
                    online: isOnline,
                    stake: v.stake || 0,
                    reputation: v.reputation || 0,
                    blocks_proposed: v.blocks_proposed || 0,
                    last_active_slot: lastActive,
                };
            });
        }
    }

    // If still nothing, show a single "this node" entry
    if (probes.length === 0) {
        probes = [{
            name: 'Node',
            rpc: rpcUrl,
            pubkey: null,
            slot: currentSlot,
            observed_block_slot: currentSlot,
            head_staleness_ms: 0,
            head_hash: null,
            online: currentSlot !== null,
            stake: 0,
            reputation: 0,
            blocks_proposed: 0,
            last_active_slot: 0,
        }];
    }

    const onlineCount = probes.filter(p => p.online).length;
    badge.textContent = `${onlineCount}/${probes.length} Online`;
    badge.className = 'panel-badge ' + (onlineCount === probes.length ? 'success' : onlineCount > 0 ? 'warning' : 'danger');

    // Update vitals validator count with actual online nodes
    flashVital('vitalValidators', onlineCount);

    // F17.6 fix: escape all RPC-derived values in validator grid
    grid.innerHTML = probes.map(p => `
        <div class="validator-card ${p.online ? '' : 'offline'}">
            <span class="val-status ${p.online ? '' : 'offline'}"></span>
            <div class="val-info">
                <div class="val-name">${escapeHtml(p.name)} - Validator</div>
                <div class="val-addr">${p.pubkey ? escapeHtml(truncAddr(p.pubkey)) : (p.online ? 'Unstaked' : 'Offline')}</div>
            </div>
            <div class="val-meta">
                <span><i class="fas fa-cube"></i> ${p.slot !== null ? formatExactNum(p.slot) : 'N/A'}</span>
                <span><i class="fas fa-coins"></i> ${p.stake ? formatLicn(p.stake) : '--'}</span>
                <span title="Blocks proposed"><i class="fas fa-hammer"></i> ${formatNum(p.blocks_proposed)}</span>
            </div>
        </div>`).join('');

    return probes;
}

// ── Health Update ───────────────────────────────────────────

async function updateHealth(metrics, probes, peers, cadence) {
    // Consensus: based on validator agreement on same slot
    const onlineProbes = probes ? probes.filter(p => p.online) : [];
    const slots = onlineProbes.map(p => p.slot).filter(s => s !== null);
    const slotDiff = slots.length >= 2 ? Math.max(...slots) - Math.min(...slots) : 0;
    const consensusPct = onlineProbes.length >= 2
        ? (slotDiff <= 1 ? 100 : slotDiff <= 3 ? 75 : 50)
        : (onlineProbes.length === 1 ? 100 : 0);
    setBar('healthConsensus', consensusPct);

    // Block cadence: measured from observer wall-clock deltas, preferring
    // cluster-level validator samples over the single-node RPC fallback.
    const blockPct = clampPercentage(cadence?.pacePct || metrics?.slot_pace_pct || 0);
    setBar('healthBlocks', blockPct);

    // TX Rate
    const txPct = Math.min(100, Math.round((metrics?.tps || 0) * 10));
    setBar('healthTxRate', txPct);

    // P2P: compare current peers to the minimum mesh expected from the visible validator set.
    const p2pPct = p2pConnectivityScore(probes, peers);
    setBar('healthP2P', p2pPct);

    // Account footprint: derive from account count (same formula as perf ring)
    const memPct = Math.min(100, Math.round((metrics?.total_accounts || 0) / 1000));
    setBar('healthMemory', memPct);

    // Overall badge uses cluster liveness signals, not cadence heuristics.
    const availabilityPct = validatorAvailabilityScore(probes || []);
    const avg = (consensusPct + availabilityPct + p2pPct) / 3;
    const badge = document.getElementById('healthBadge');
    if (avg >= 80) {
        badge.textContent = 'HEALTHY';
        badge.className = 'panel-badge health-badge success';
    } else if (avg >= 50) {
        badge.textContent = 'DEGRADED';
        badge.className = 'panel-badge health-badge warning';
    } else {
        badge.textContent = 'CRITICAL';
        badge.className = 'panel-badge health-badge danger';
    }
}

function setBar(id, pct) {
    const bar = document.getElementById(id);
    const pctEl = document.getElementById(id + 'Pct');
    if (bar) bar.style.width = pct + '%';
    if (pctEl) pctEl.textContent = pct + '%';
}

function bindStaticControls() {
    document.getElementById('networkSelect')?.addEventListener('change', (event) => {
        switchNetwork(event.target.value);
    });
    document.getElementById('alertDismissBtn')?.addEventListener('click', dismissAlert);
    document.getElementById('clearEventsBtn')?.addEventListener('click', clearEvents);
    document.getElementById('clearThreatsBtn')?.addEventListener('click', clearThreats);

    document.querySelectorAll('[data-tps-range]').forEach((button) => {
        button.addEventListener('click', (event) => {
            setTPSRange(button.dataset.tpsRange, event);
        });
    });

    document.getElementById('killswitchBanIpBtn')?.addEventListener('click', killswitchBanIP);
    document.getElementById('killswitchRateLimitBtn')?.addEventListener('click', killswitchRateLimit);
    document.getElementById('killswitchBlockMethodBtn')?.addEventListener('click', killswitchBlockMethod);
    document.getElementById('killswitchFreezeAccountBtn')?.addEventListener('click', killswitchFreezeAccount);
    document.getElementById('killswitchEmergencyShutdownBtn')?.addEventListener('click', killswitchEmergencyShutdown);
    document.getElementById('killswitchDenyAllBtn')?.addEventListener('click', killswitchDenyAll);
}

// ── Recent Blocks ───────────────────────────────────────────

let displayedBlocks = [];

async function updateRecentBlocks(currentSlot) {
    if (!currentSlot || currentSlot < 1) return;

    const newBlocks = [];
    for (let s = currentSlot; s > Math.max(0, currentSlot - 10); s--) {
        if (displayedBlocks.find(b => b.slot === s)) continue;
        const block = await rpc('getBlock', [s]);
        if (block && !block.code) {
            newBlocks.push({
                slot: s,
                hash: block.hash || block.blockhash || '--',
                txCount: block.transactions?.length || block.transaction_count || 0,
                time: block.timestamp || block.blockTime || 0,
            });
        }
    }

    if (newBlocks.length > 0) {
        displayedBlocks = [...newBlocks, ...displayedBlocks].slice(0, 20);
        renderBlocks();
    }
}

function renderBlocks() {
    const el = document.getElementById('blockList');
    // F17.4 fix: escape RPC-derived block hash
    el.innerHTML = displayedBlocks.map(b => `
        <div class="block-row">
            <span class="block-slot">#${b.slot}</span>
            <span class="block-hash">${escapeHtml(b.hash)}</span>
            <span class="block-txs">${b.txCount} tx</span>
            <span class="block-time">${b.time ? timeAgo(b.time) : '--'}</span>
        </div>
    `).join('');
}

// ── Contract Registry ───────────────────────────────────────

let contractsLoaded = false;
let contractsLoadedAt = 0;

async function updateContracts(force = false) {
    if (!force && contractsLoaded && Date.now() - contractsLoadedAt < CONTRACT_REFRESH_MS) return;

    const list = document.getElementById('contractList');
    const rows = (await Promise.all(SYMBOLS.map(async (sym) => {
        const info = await getMonitoringSymbolRegistryEntry(sym);
        if (!info || !info.program) return null;
        return {
            symbol: info.symbol || sym,
            template: info.template || '?',
            program: info.program,
        };
    }))).filter(Boolean);

    if (rows.length > 0) {
        contractsLoaded = true;
        contractsLoadedAt = Date.now();
        // F17.5 fix: escape RPC-derived contract metadata
        list.innerHTML = rows.map(c => `
            <div class="contract-row">
                <span class="contract-status"></span>
                <span class="contract-symbol">${escapeHtml(c.symbol)}</span>
                <span class="contract-template">${escapeHtml(c.template)}</span>
                <span class="contract-addr">${escapeHtml(c.program)}</span>
            </div>
        `).join('');
        addEvent('success', 'file-contract', `Loaded ${rows.length} contracts`);
    } else {
        contractsLoaded = false;
        contractsLoadedAt = Date.now();
    }
}

// ── Incident Response Engine ────────────────────────────────

let threats = [];
let activeBans = [];
let threatStats = { critical: 0, high: 0, medium: 0, low: 0 };
let lastThreatCheck = 0;

function addThreat(severity, type, source, method, details) {
    // Debounce identical threats within 10s
    const key = `${severity}:${type}:${source}`;
    const dup = threats.find(t => `${t.severity}:${t.type}:${t.source}` === key && Date.now() - t.timestamp < 10000);
    if (dup) return;

    const threat = {
        id: Date.now() + Math.random(),
        time: now(),
        timestamp: Date.now(),
        severity,
        type,
        source,
        method,
        details,
        status: 'detected'
    };
    threats.unshift(threat);
    if (threats.length > 200) threats.pop();
    threatStats[severity]++;
    renderThreats();
    updateThreatLevel();

    const iconMap = { critical: 'skull-crossbones', high: 'exclamation-triangle', medium: 'exclamation-circle', low: 'info-circle' };
    addEvent(severity === 'critical' || severity === 'high' ? 'danger' : 'warning',
        iconMap[severity] || 'exclamation-circle',
        `[${severity.toUpperCase()}] ${type}: ${details}`);
}

function renderThreats() {
    const el = id => document.getElementById(id);
    if (el('threatCritical')) el('threatCritical').textContent = threatStats.critical;
    if (el('threatHigh')) el('threatHigh').textContent = threatStats.high;
    if (el('threatMedium')) el('threatMedium').textContent = threatStats.medium;
    if (el('threatLow')) el('threatLow').textContent = threatStats.low;

    const log = document.getElementById('attackLog');
    if (!log) return;

    log.innerHTML = threats.slice(0, 50).map(t => {
        // F17.2 fix: escape all RPC/user-derived threat data + use data-attributes
        // instead of interpolating t.source into onclick to prevent quote injection
        const escaped = {
            time: escapeHtml(t.time),
            severity: escapeHtml(t.severity),
            type: escapeHtml(t.type),
            source: escapeHtml(t.source),
            method: escapeHtml(t.method),
            details: escapeHtml(t.details),
        };
        return `
        <div class="attack-row severity-${escaped.severity}">
            <span class="attack-time">${escaped.time}</span>
            <span class="attack-severity ${escaped.severity}">${escaped.severity.toUpperCase()}</span>
            <span class="attack-type">${escaped.type}</span>
            <span class="attack-source">${escaped.source}</span>
            <span class="attack-method">${escaped.method}</span>
            <span class="attack-details">${escaped.details}</span>
        </div>`;
    }).join('') || '<div class="ir-empty">No threats detected - system clear</div>';
}

function updateThreatLevel() {
    const badge = document.getElementById('threatLevel');
    if (!badge) return;
    if (threatStats.critical > 0) {
        badge.textContent = 'CRITICAL';
        badge.className = 'panel-badge danger pulse';
    } else if (threatStats.high > 0) {
        badge.textContent = 'ELEVATED';
        badge.className = 'panel-badge warning';
    } else if (threatStats.medium > 0) {
        badge.textContent = 'GUARDED';
        badge.className = 'panel-badge info';
    } else {
        badge.textContent = 'CLEAR';
        badge.className = 'panel-badge success';
    }
}

function clearThreats() {
    threats = [];
    threatStats = { critical: 0, high: 0, medium: 0, low: 0 };
    renderThreats();
    updateThreatLevel();
}

// ── Kill Switches ───────────────────────────────────────────
// AUDIT-FIX I8-01 / P0-7: All admin actions require per-action authentication
// via admin_token. Tokens are never persisted in browser storage.

function purgeLegacyAdminToken() {
    try {
        sessionStorage.removeItem(LEGACY_ADMIN_TOKEN_STORAGE_KEY);
    } catch (e) {
        // Ignore storage access failures.
    }
}

function promptAdminToken(actionLabel) {
    const label = actionLabel ? ` for ${actionLabel}` : '';
    const token = prompt(`Admin authentication required${label}.\nEnter admin token:`);
    const normalized = String(token || '').trim();
    return normalized || null;
}

function showIncidentControlUnavailable(actionLabel) {
    const label = actionLabel || 'This incident control';
    showAlert(`${label} is not exposed by production RPC. Use the validator CLI / SSH operator runbook instead.`);
}

async function killswitchBanIP() {
    showIncidentControlUnavailable('Ban IP');
}

async function killswitchRateLimit() {
    showIncidentControlUnavailable('Throttle');
}

async function killswitchBlockMethod() {
    showIncidentControlUnavailable('Block Method');
}

async function killswitchFreezeAccount() {
    showIncidentControlUnavailable('Freeze Account');
}

async function killswitchEmergencyShutdown() {
    showIncidentControlUnavailable('Emergency Stop');
}

async function killswitchDenyAll() {
    showIncidentControlUnavailable('Deny All');
}

function quickBan(source) {
    if (!source || source === 'System' || source === 'Network') return;
    showIncidentControlUnavailable('Threat-log ban');
}

function quickThrottle(source) {
    if (!source || source === 'System' || source === 'Network') return;
    showIncidentControlUnavailable('Threat-log throttle');
}

function addBan(type, target, reason) {
    activeBans.unshift({ type, target, reason, time: now(), timestamp: Date.now() });
    renderBans();
}

function removeBan(index) {
    const ban = activeBans[index];
    if (!ban) return;
    activeBans.splice(index, 1);
    renderBans();
    addEvent('info', 'unlock', `Removed restriction: ${ban.target}`);
}

function renderBans() {
    const el = document.getElementById('banList');
    if (!el) return;
    if (activeBans.length === 0) {
        el.innerHTML = '<div class="ir-empty">No active restrictions</div>';
        return;
    }
    // F17.3 fix: escape user-supplied ban target/reason + use data-attributes for removeBan
    el.innerHTML = activeBans.map((b, i) => `
        <div class="ban-item">
            <span class="ban-type ${escapeHtml(b.type)}">${escapeHtml(b.type.toUpperCase())}</span>
            <span class="ban-target">${escapeHtml(b.target)}</span>
            <span class="ban-reason">${escapeHtml(b.reason)}</span>
            <span class="ban-time">${escapeHtml(b.time)}</span>
            <button class="btn-xs" data-remove-ban="${i}" title="Remove">
                <i class="fas fa-times"></i>
            </button>
        </div>
    `).join('');
    el.querySelectorAll('[data-remove-ban]').forEach(btn => {
        btn.addEventListener('click', () => removeBan(parseInt(btn.dataset.removeBan, 10)));
    });
}

// ── Threat Detection ────────────────────────────────────────

function shortHash(hash) {
    if (!hash || typeof hash !== 'string') return null;
    if (hash.length <= 12) return hash;
    return `${hash.slice(0, 8)}...${hash.slice(-4)}`;
}

function classifyConsensusIssue(onlineProbes) {
    if (onlineProbes.length < 2) return null;

    const slots = onlineProbes.map(p => p.slot);
    const maxDiff = Math.max(...slots) - Math.min(...slots);

    // A real fork requires conflicting block hashes for the same slot.
    // Mere last_active_slot spread is validator lag, not proof of divergent chain state.
    const hashesBySlot = new Map();
    for (const probe of onlineProbes) {
        if (probe.slot === null || !probe.head_hash) continue;
        if (!hashesBySlot.has(probe.slot)) hashesBySlot.set(probe.slot, new Set());
        hashesBySlot.get(probe.slot).add(probe.head_hash);
    }

    for (const [slot, hashes] of hashesBySlot.entries()) {
        if (hashes.size > 1) {
            const conflictingHashes = Array.from(hashes).map(shortHash).join(' vs ');
            return {
                severity: 'critical',
                title: 'Consensus Fork',
                details: `Conflicting block hashes at slot ${slot} (${conflictingHashes})`,
            };
        }
    }

    if (maxDiff > 5) {
        return {
            severity: 'high',
            title: 'Validator Lag',
            details: `Validator activity divergence: ${maxDiff} blocks (last_active_slot ${slots.join(' vs ')})`,
        };
    }

    if (maxDiff > 2) {
        return {
            severity: 'medium',
            title: 'Slot Drift',
            details: `Minor slot drift: ${maxDiff} blocks`,
        };
    }

    return null;
}

function detectThreats(metrics, probes) {
    // Throttle: max once per 5s
    if (Date.now() - lastThreatCheck < 5000) return;
    lastThreatCheck = Date.now();

    if (!probes) return;

    // Validator offline detection
    const offlineNodes = probes.filter(p => !p.online);
    offlineNodes.forEach(n => {
        addThreat('high', 'Node Offline', n.rpc, 'P2P', `${n.name} not responding on :${n.rpc.split(':').pop()}`);
    });

    // Consensus/finality health detection
    const onlineProbes = probes.filter(p => p.online && p.slot !== null);
    const consensusIssue = classifyConsensusIssue(onlineProbes);
    if (consensusIssue) {
        addThreat(consensusIssue.severity, consensusIssue.title, 'Network', 'Consensus',
            consensusIssue.details);
    }

    // Stake anomaly detection
    const totalStake = probes.reduce((sum, p) => sum + (p.stake || 0), 0);
    probes.forEach(p => {
        if (p.online && p.stake === 0) {
            addThreat('low', 'Unstaked Node', p.name, 'Staking',
                `${p.name} running without stake - no slashing protection`);
        }
    });

    // TPS crash detection
    if (tpsHistory.length > 15) {
        const recent = tpsHistory.slice(-5).reduce((a, b) => a + b.v, 0) / 5;
        const older = tpsHistory.slice(-15, -10).reduce((a, b) => a + b.v, 0) / 5;
        if (older > 1 && recent < older * 0.1) {
            addThreat('high', 'TPS Crash', 'Network', 'Performance',
                `TPS dropped ${((1 - recent / older) * 100).toFixed(0)}% (${older.toFixed(1)} -> ${recent.toFixed(1)})`);
        }
    }

    // Single validator dominance
    if (onlineProbes.length >= 2) {
        const maxBlocks = Math.max(...onlineProbes.map(p => p.blocks_proposed || 0));
        const totalBlocks = onlineProbes.reduce((s, p) => s + (p.blocks_proposed || 0), 0);
        if (totalBlocks > 10 && maxBlocks / totalBlocks > 0.8) {
            const dominant = onlineProbes.find(p => p.blocks_proposed === maxBlocks);
            addThreat('medium', 'Centralization Risk', dominant?.name || 'Unknown', 'Consensus',
                `${dominant?.name} produced ${((maxBlocks / totalBlocks) * 100).toFixed(0)}% of blocks`);
        }
    }
}

// ── DEX Operations Monitor ──────────────────────────────────

const DEX_SUBSYSTEMS = [
    {
        id: 'dex_core', symbol: 'DEX', name: 'DEX Core (CLOB)', desc: 'Central Limit Order Book engine', icon: 'fas fa-exchange-alt', color: '#4ea8de',
        metrics: ['pairs', 'orders', 'trades', 'volume']
    },
    {
        id: 'dex_amm', symbol: 'DEXAMM', name: 'AMM Pools', desc: 'Concentrated liquidity AMM', icon: 'fas fa-water', color: '#06d6a0',
        metrics: ['pools', 'positions', 'swaps', 'volume']
    },
    {
        id: 'dex_router', symbol: 'DEXROUTER', name: 'Smart Router', desc: 'Optimal routing across CLOB + AMM', icon: 'fas fa-route', color: '#ffd166',
        metrics: ['routes', 'swaps', 'volume', 'status']
    },
    {
        id: 'dex_margin', symbol: 'DEXMARGIN', name: 'Margin Trading', desc: 'Leveraged positions (up to 100x)', icon: 'fas fa-chart-line', color: '#ef4444',
        metrics: ['positions', 'volume', 'liquidations', 'max_leverage']
    },
    {
        id: 'dex_governance', symbol: 'DEXGOV', name: 'DEX Governance', desc: 'Proposals, voting, fee updates', icon: 'fas fa-landmark', color: '#a78bfa',
        metrics: ['proposals', 'votes', 'voters', 'status']
    },
    {
        id: 'dex_rewards', symbol: 'DEXREWARDS', name: 'Rewards Program', desc: 'Trader reward accounting and distribution', icon: 'fas fa-gift', color: '#f59e0b',
        metrics: ['trades', 'traders', 'distributed', 'epoch']
    },
    {
        id: 'dex_analytics', symbol: 'ANALYTICS', name: 'Analytics Engine', desc: 'OHLCV, trade history, metrics', icon: 'fas fa-chart-area', color: '#60a5fa',
        metrics: ['candles', 'pairs', 'records', 'traders']
    },
    {
        id: 'lichenswap', symbol: 'LICHENSWAP', name: 'LichenSwap', desc: 'Simple token swap interface', icon: 'fas fa-arrows-rotate', color: '#00C9DB',
        metrics: ['swaps', 'volume_a', 'volume_b', 'status']
    },
    {
        id: 'prediction_market', symbol: 'PREDICT', name: 'Prediction Markets', desc: 'Binary/multi-outcome markets + lUSD', icon: 'fas fa-chart-pie', color: '#e879f9',
        metrics: ['markets', 'open_markets', 'volume', 'traders']
    },
];

let dexDataLoaded = false;

async function updateDexMonitor() {
    const grid = document.getElementById('dexSubsystemGrid');
    const badge = document.getElementById('dexStatusBadge');
    if (!grid) return;

    let onlineCount = 0;
    const cards = [];

    for (const sub of DEX_SUBSYSTEMS) {
        let deployed = false;
        let program = null;
        let metricsData = {};

        // Check if contract is deployed
        if (sub.symbol) {
            const info = await getMonitoringSymbolRegistryEntry(sub.symbol);
            if (info && info.program) {
                deployed = true;
                program = info.program;
            }
        }

        // Try to load subsystem-specific metrics
        try {
            if (sub.id === 'dex_core') {
                const stats = await rpc('getDexCoreStats');
                if (stats) {
                    metricsData = {
                        pairs: stats.pair_count || 0, orders: stats.order_count || 0,
                        trades: stats.trade_count || 0, volume: stats.total_volume || 0
                    };
                    deployed = true;
                }
            } else if (sub.id === 'dex_amm') {
                const stats = await rpc('getDexAmmStats');
                if (stats) {
                    metricsData = {
                        pools: stats.pool_count || 0, positions: stats.position_count || 0,
                        swaps: stats.swap_count || 0, volume: stats.total_volume || 0
                    };
                    deployed = true;
                }
            } else if (sub.id === 'dex_margin') {
                const stats = await rpc('getDexMarginStats');
                if (stats) {
                    metricsData = {
                        positions: stats.position_count || 0, volume: stats.total_volume || 0,
                        liquidations: stats.liquidation_count || 0, max_leverage: (stats.max_leverage || 100) + 'x'
                    };
                    deployed = true;
                }
            } else if (sub.id === 'prediction_market') {
                const stats = await rpc('getPredictionMarketStats');
                if (stats) {
                    metricsData = {
                        markets: stats.total_markets || 0, open_markets: stats.open_markets || 0,
                        volume: stats.total_volume || 0, traders: stats.total_traders || 0
                    };
                    deployed = true;
                }
            } else if (sub.id === 'dex_router') {
                const stats = await rpc('getDexRouterStats');
                if (stats) {
                    metricsData = {
                        routes: stats.route_count || 0, swaps: stats.swap_count || 0,
                        volume: stats.total_volume || 0, status: stats.paused ? 'PAUSED' : 'LIVE'
                    };
                    deployed = true;
                }
            } else if (sub.id === 'dex_governance') {
                const stats = await rpc('getDexGovernanceStats');
                if (stats) {
                    metricsData = {
                        proposals: stats.proposal_count || 0, votes: stats.total_votes || 0,
                        voters: stats.voter_count || 0, status: stats.paused ? 'PAUSED' : 'LIVE'
                    };
                    deployed = true;
                }
            } else if (sub.id === 'dex_rewards') {
                const stats = await rpc('getDexRewardsStats');
                if (stats) {
                    metricsData = {
                        trades: stats.trade_count || 0, traders: stats.trader_count || 0,
                        distributed: stats.total_distributed || 0, epoch: stats.epoch || 0
                    };
                    deployed = true;
                }
            } else if (sub.id === 'dex_analytics') {
                const stats = await rpc('getDexAnalyticsStats');
                if (stats) {
                    metricsData = {
                        candles: stats.total_candles || 0, pairs: stats.tracked_pairs || 0,
                        records: stats.record_count || 0, traders: stats.trader_count || 0
                    };
                    deployed = true;
                }
            } else if (sub.id === 'lichenswap') {
                const stats = await rpc('getLichenSwapStats');
                if (stats) {
                    metricsData = {
                        swaps: stats.swap_count || 0, volume_a: stats.volume_a || 0,
                        volume_b: stats.volume_b || 0, status: stats.paused ? 'PAUSED' : 'LIVE'
                    };
                    deployed = true;
                }
            }
        } catch { /* stats endpoint not yet available */ }

        if (deployed) onlineCount++;

        const statusClass = deployed ? 'success' : 'warning';
        const statusText = deployed ? 'DEPLOYED' : 'PENDING';

        const metricLabels = {
            pairs: 'Pairs',
            orders: 'Orders',
            trades: 'Trades',
            pools: 'Pools',
            positions: 'Positions',
            swaps: 'Swaps',
            volume: 'Volume',
            routes: 'Routes',
            liquidations: 'Liquidations',
            max_leverage: 'Max Lev',
            proposals: 'Proposals',
            votes: 'Votes',
            voters: 'Voters',
            distributed: 'Distributed',
            epoch: 'Epoch',
            candles: 'Candles',
            records: 'Records',
            traders: 'Traders',
            volume_a: 'Volume A',
            volume_b: 'Volume B',
            markets: 'Markets',
            open_markets: 'Open',
            status: 'Status'
        };

        const metricsHtml = sub.metrics.map(m => {
            const val = metricsData[m];
            let display = '--';
            if (val !== undefined && val !== null) {
                if (typeof val === 'string') display = val;
                else if (['volume', 'volume_a', 'volume_b', 'distributed'].includes(m)) {
                    display = formatLicn(val);
                } else { display = formatNum(val); }
            }
            return `<div class="sub-metric"><span class="sub-metric-label">${metricLabels[m] || m}</span><span class="sub-metric-value">${display}</span></div>`;
        }).join('');

        cards.push(`
            <div class="dex-subsystem-card">
                <div class="sub-header">
                    <div class="sub-icon" style="background:${sub.color}18;color:${sub.color};"><i class="${sub.icon}"></i></div>
                    <div>
                        <div class="sub-name">${sub.name}</div>
                        <div class="sub-desc">${sub.desc}</div>
                    </div>
                    <span class="sub-badge ${statusClass}" style="background:${statusClass === 'success' ? 'rgba(6,214,160,0.12)' : 'rgba(255,210,63,0.12)'};color:${statusClass === 'success' ? '#4ade80' : '#f59e0b'};">${statusText}</span>
                </div>
                <div class="sub-metrics">${metricsHtml}</div>
                ${program ? `<div class="cm-addr" title="${escapeHtml(program)}" style="font-size:0.65rem;color:var(--text-muted);font-family:'JetBrains Mono',monospace;margin-top:0.35rem;padding:0.2rem 0.5rem;background:var(--bg-card);border-radius:var(--radius-xs);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escapeHtml(program)}</div>` : ''}
            </div>
        `);
    }

    grid.innerHTML = cards.join('');
    if (badge) {
        badge.textContent = `${onlineCount}/${DEX_SUBSYSTEMS.length} Active`;
        badge.className = 'panel-badge ' + (onlineCount === DEX_SUBSYSTEMS.length ? 'success' : onlineCount > 0 ? 'info' : 'warning');
    }

    // Update summary stats from real RPC endpoints
    const el = id => document.getElementById(id);
    const dexCoreStats = await rpc('getDexCoreStats').catch(() => null);
    if (dexCoreStats) {
        if (el('dexTotalPairs')) el('dexTotalPairs').textContent = formatNum(dexCoreStats.pair_count || 0);
        if (el('dexVolume24h')) el('dexVolume24h').textContent = formatLicn(dexCoreStats.total_volume || 0);
        if (el('dexOpenOrders')) el('dexOpenOrders').textContent = formatNum(dexCoreStats.order_count || 0);
    }
    const ammStats = await rpc('getDexAmmStats').catch(() => null);
    if (ammStats) {
        if (el('dexTVL')) el('dexTVL').textContent = formatLicn(ammStats.total_volume || 0);
    }
    const marginStats = await rpc('getDexMarginStats').catch(() => null);
    if (marginStats) {
        if (el('dexMarginPos')) el('dexMarginPos').textContent = formatNum(marginStats.position_count || 0);
    }
    const predictStats = await rpc('getPredictionMarketStats').catch(() => null);
    if (predictStats) {
        if (el('dexPredictMkts')) el('dexPredictMkts').textContent = formatNum(predictStats.open_markets || 0);
    }
}

// ── Smart Contracts Monitor ─────────────────────────────────

const ALL_CONTRACTS = [
    { symbol: 'LUSD', name: 'lUSD Stablecoin', cat: 'token', icon: 'fas fa-dollar-sign', color: '#4ade80' },
    { symbol: 'WETH', name: 'Wrapped ETH', cat: 'token', icon: 'fab fa-ethereum', color: '#627eea' },
    { symbol: 'WSOL', name: 'Wrapped SOL', cat: 'token', icon: 'fas fa-sun', color: '#9945ff' },
    { symbol: 'WBNB', name: 'Wrapped BNB', cat: 'token', icon: 'fas fa-cubes', color: '#fbbf24' },
    { symbol: 'DEX', name: 'DEX Core', cat: 'dex', icon: 'fas fa-exchange-alt', color: '#4ea8de' },
    { symbol: 'DEXAMM', name: 'DEX AMM', cat: 'dex', icon: 'fas fa-water', color: '#06d6a0' },
    { symbol: 'DEXROUTER', name: 'DEX Router', cat: 'dex', icon: 'fas fa-route', color: '#ffd166' },
    { symbol: 'DEXMARGIN', name: 'DEX Margin', cat: 'dex', icon: 'fas fa-chart-line', color: '#ef4444' },
    { symbol: 'DEXGOV', name: 'DEX Governance', cat: 'dex', icon: 'fas fa-landmark', color: '#a78bfa' },
    { symbol: 'DEXREWARDS', name: 'DEX Rewards', cat: 'dex', icon: 'fas fa-gift', color: '#f59e0b' },
    { symbol: 'ANALYTICS', name: 'DEX Analytics', cat: 'dex', icon: 'fas fa-chart-area', color: '#60a5fa' },
    { symbol: 'LICHENSWAP', name: 'LichenSwap', cat: 'dex', icon: 'fas fa-arrows-rotate', color: '#00C9DB' },
    { symbol: 'BRIDGE', name: 'LichenBridge', cat: 'infra', icon: 'fas fa-bridge', color: '#38bdf8' },
    { symbol: 'DAO', name: 'LichenDAO', cat: 'infra', icon: 'fas fa-users-cog', color: '#a78bfa' },
    { symbol: 'SPOREVAULT', name: 'SporeVault', cat: 'defi', icon: 'fas fa-vault', color: '#f472b6' },
    { symbol: 'SPOREPAY', name: 'SporePay', cat: 'defi', icon: 'fas fa-credit-card', color: '#34d399' },
    { symbol: 'SPOREPUMP', name: 'SporePump', cat: 'defi', icon: 'fas fa-rocket', color: '#fb923c' },
    { symbol: 'ORACLE', name: 'LichenOracle', cat: 'infra', icon: 'fas fa-eye', color: '#c084fc' },
    { symbol: 'LEND', name: 'ThallLend', cat: 'defi', icon: 'fas fa-hand-holding-usd', color: '#2dd4bf' },
    { symbol: 'MARKET', name: 'LichenMarket', cat: 'nft', icon: 'fas fa-store', color: '#f97316' },
    { symbol: 'AUCTION', name: 'LichenAuction', cat: 'nft', icon: 'fas fa-gavel', color: '#e879f9' },
    { symbol: 'BOUNTY', name: 'BountyBoard', cat: 'infra', icon: 'fas fa-bullhorn', color: '#fbbf24' },
    { symbol: 'COMPUTE', name: 'Compute Market', cat: 'infra', icon: 'fas fa-microchip', color: '#94a3b8' },
    { symbol: 'MOSS', name: 'Moss Storage', cat: 'infra', icon: 'fas fa-database', color: '#22d3ee' },
    { symbol: 'SHIELDED', name: 'Shielded Pool', cat: 'privacy', icon: 'fas fa-user-shield', color: '#14b8a6' },
    { symbol: 'PUNKS', name: 'LichenPunks', cat: 'nft', icon: 'fas fa-image', color: '#f43f5e' },
    { symbol: 'YID', name: 'LichenID', cat: 'identity', icon: 'fas fa-fingerprint', color: '#818cf8' },
    { symbol: 'PREDICT', name: 'Prediction Markets', cat: 'defi', icon: 'fas fa-chart-pie', color: '#e879f9' },
];

let contractMonitorLoaded = false;
let contractMonitorLoadedAt = 0;
let contractMonitorFiltersBound = false;
let activeContractCategory = 'all';

function bindContractMonitorFilters() {
    if (contractMonitorFiltersBound) return;
    document.querySelectorAll('.contract-cat-btn').forEach(btn => {
        btn.addEventListener('click', () => applyContractMonitorFilter(btn.dataset.cat || 'all'));
    });
    contractMonitorFiltersBound = true;
}

function applyContractMonitorFilter(category) {
    activeContractCategory = category || 'all';
    document.querySelectorAll('.contract-cat-btn').forEach(btn => {
        btn.classList.toggle('active', btn.dataset.cat === activeContractCategory);
    });
    document.querySelectorAll('.contract-monitor-card').forEach(card => {
        card.style.display = (activeContractCategory === 'all' || card.dataset.cat === activeContractCategory) ? '' : 'none';
    });
}

async function updateContractMonitor(force = false) {
    const grid = document.getElementById('contractMonitorGrid');
    const badge = document.getElementById('contractMonitorBadge');
    if (!grid) return;
    if (!force && contractMonitorLoaded && Date.now() - contractMonitorLoadedAt < CONTRACT_REFRESH_MS) {
        return;
    }

    const entries = await Promise.all(ALL_CONTRACTS.map(async (contract) => {
        const info = await getMonitoringSymbolRegistryEntry(contract.symbol);
        return { contract, info };
    }));

    let deployedCount = 0;
    const cards = entries.map(({ contract, info }) => {
        const deployed = !!(info && info.program);
        if (deployed) deployedCount++;

        const program = info?.program || '';
        const template = info?.template || '—';
        const statusText = deployed ? 'LIVE' : 'PENDING';

        return `
            <div class="contract-monitor-card" data-cat="${contract.cat}">
                <div class="cm-header">
                    <div class="cm-icon" style="background:${contract.color}18;color:${contract.color};"><i class="${contract.icon}"></i></div>
                    <div>
                        <div class="cm-name">${contract.name}</div>
                        <div class="cm-symbol">${contract.symbol} · ${template}</div>
                    </div>
                    <span class="cm-badge" style="background:${deployed ? 'rgba(74,222,128,0.12)' : 'rgba(245,158,11,0.12)'};color:${deployed ? '#4ade80' : '#f59e0b'};">${statusText}</span>
                </div>
                ${program ? `<div class="cm-addr" title="${escapeHtml(program)}">${escapeHtml(program)}</div>` : ''}
            </div>
        `;
    });

    grid.innerHTML = cards.join('');
    contractMonitorLoaded = true;
    contractMonitorLoadedAt = Date.now();
    bindContractMonitorFilters();
    applyContractMonitorFilter(activeContractCategory);

    if (badge) {
        badge.textContent = `${deployedCount}/${ALL_CONTRACTS.length} Deployed`;
        badge.className = 'panel-badge ' + (deployedCount >= ALL_CONTRACTS.length ? 'success' : deployedCount >= ALL_CONTRACTS.length / 2 ? 'info' : 'warning');
    }

    // Update deployment progress bar
    const progressBar = document.getElementById('contractDeployProgress');
    if (progressBar) {
        const pct = Math.round((deployedCount / ALL_CONTRACTS.length) * 100);
        progressBar.style.width = pct + '%';
        progressBar.style.background = deployedCount >= ALL_CONTRACTS.length ? 'var(--gradient-3)' : 'var(--gradient-1)';
    }
    const progressText = document.getElementById('contractDeployText');
    if (progressText) progressText.textContent = `${deployedCount} of ${ALL_CONTRACTS.length} contracts deployed (${Math.round((deployedCount / ALL_CONTRACTS.length) * 100)}%)`;
}

// ── LichenID Identity Monitor ────────────────────────────────

let identityMonitorLoaded = false;

async function updateIdentitiesMonitor() {
    const badge = document.getElementById('identityMonitorBadge');
    const tierGrid = document.getElementById('identityTierGrid');
    const el = id => document.getElementById(id);

    const stats = await rpc('getLichenIdStats').catch(() => null);
    if (!stats) {
        if (badge) { badge.textContent = 'OFFLINE'; badge.className = 'panel-badge warning'; }
        return;
    }

    const totalIdentities = stats.total_identities || 0;
    const totalNames = stats.total_names || 0;
    const tier = stats.tier_distribution || {};

    // Summary bar
    if (el('idTotalIdentities')) el('idTotalIdentities').textContent = formatNum(totalIdentities);
    if (el('idTotalNames')) el('idTotalNames').textContent = formatNum(totalNames);
    if (el('idTierNewcomer')) el('idTierNewcomer').textContent = formatNum(tier.newcomer || 0);
    if (el('idTierVerified')) el('idTierVerified').textContent = formatNum(tier.verified || 0);
    if (el('idTierTrusted')) el('idTierTrusted').textContent = formatNum(tier.trusted || 0);
    if (el('idTierEstablished')) el('idTierEstablished').textContent = formatNum(tier.established || 0);
    if (el('idTierElite')) el('idTierElite').textContent = formatNum(tier.elite || 0);
    if (el('idTierLegendary')) el('idTierLegendary').textContent = formatNum(tier.legendary || 0);

    // Tier distribution visual cards
    if (tierGrid) {
        const tiers = [
            { name: 'Newcomer', count: tier.newcomer || 0, color: '#94a3b8', icon: 'fas fa-seedling' },
            { name: 'Verified', count: tier.verified || 0, color: '#4ade80', icon: 'fas fa-check-circle' },
            { name: 'Trusted', count: tier.trusted || 0, color: '#60a5fa', icon: 'fas fa-shield-alt' },
            { name: 'Established', count: tier.established || 0, color: '#a78bfa', icon: 'fas fa-star' },
            { name: 'Elite', count: tier.elite || 0, color: '#f59e0b', icon: 'fas fa-crown' },
            { name: 'Legendary', count: tier.legendary || 0, color: '#ef4444', icon: 'fas fa-gem' },
        ];
        const maxCount = Math.max(1, ...tiers.map(t => t.count));
        tierGrid.innerHTML = tiers.map(t => {
            const pct = Math.round((t.count / maxCount) * 100);
            return `<div class="identity-tier-card">
                <div class="identity-tier-header">
                    <i class="${t.icon}" style="color:${t.color};background:${t.color}15;"></i>
                    <span class="identity-tier-name">${t.name}</span>
                    <span class="identity-tier-count" style="color:${t.color};">${formatNum(t.count)}</span>
                </div>
                <div class="identity-tier-bar-bg">
                    <div class="identity-tier-bar" style="width:${pct}%;background:${t.color};box-shadow:0 0 8px ${t.color}40;"></div>
                </div>
            </div>`;
        }).join('');
    }

    if (badge) {
        badge.textContent = `${formatNum(totalIdentities)} Identities`;
        badge.className = 'panel-badge ' + (totalIdentities > 0 ? 'success' : 'info');
    }

    identityMonitorLoaded = true;
}

// ── Trading Metrics Monitor ─────────────────────────────────

let tradingMetricsLoaded = false;

async function updateTradingMetrics() {
    const badge = document.getElementById('tradingMetricsBadge');
    const el = id => document.getElementById(id);

    // Fetch all trading data in parallel
    const [dexCore, amm, margin, router, analytics, lichenswap, rewards, governance, metrics] = await Promise.all([
        rpc('getDexCoreStats').catch(() => null),
        rpc('getDexAmmStats').catch(() => null),
        rpc('getDexMarginStats').catch(() => null),
        rpc('getDexRouterStats').catch(() => null),
        rpc('getDexAnalyticsStats').catch(() => null),
        rpc('getLichenSwapStats').catch(() => null),
        rpc('getDexRewardsStats').catch(() => null),
        rpc('getDexGovernanceStats').catch(() => null),
        rpc('getMetrics').catch(() => null),
    ]);

    let activeFeeds = 0;

    // DEX Core
    if (dexCore) {
        activeFeeds++;
        if (el('tradeTotalVolume')) el('tradeTotalVolume').textContent = formatLicn(dexCore.total_volume || 0);
        if (el('tradeOrderCount')) el('tradeOrderCount').textContent = formatNum(dexCore.order_count || 0);
        if (el('tradeFills24h')) el('tradeFills24h').textContent = formatNum(dexCore.trade_count || 0);
        if (el('tradeFeeTreasury')) el('tradeFeeTreasury').textContent = formatLicn(dexCore.fee_treasury || 0);
        if (el('tradePairCount')) el('tradePairCount').textContent = formatNum(dexCore.pair_count || 0);
    }

    // AMM
    if (amm) {
        activeFeeds++;
        if (el('tradeAmmPools')) el('tradeAmmPools').textContent = formatNum(amm.pool_count || 0);
        if (el('tradeAmmSwaps')) el('tradeAmmSwaps').textContent = formatNum(amm.swap_count || 0);
        if (el('tradeAmmTVL')) el('tradeAmmTVL').textContent = formatLicn(amm.total_volume || 0);
        if (el('tradeAmmFees')) el('tradeAmmFees').textContent = formatLicn(amm.total_fees || 0);
    }

    // Margin
    if (margin) {
        activeFeeds++;
        if (el('tradeMarginPos')) el('tradeMarginPos').textContent = formatNum(margin.position_count || 0);
        if (el('tradeMaxLeverage')) el('tradeMaxLeverage').textContent = (margin.max_leverage || 100) + 'x';
        if (el('tradeLiquidations')) el('tradeLiquidations').textContent = formatNum(margin.liquidation_count || 0);
        if (el('tradeInsurance')) el('tradeInsurance').textContent = formatLicn(margin.insurance_fund || 0);
    }

    // Router
    if (router) {
        activeFeeds++;
        if (el('tradeRoutes')) el('tradeRoutes').textContent = formatNum(router.route_count || 0);
    }

    // Analytics
    if (analytics) {
        activeFeeds++;
        if (el('tradeAnalyticsRecords')) el('tradeAnalyticsRecords').textContent = formatNum(analytics.record_count || 0);
        if (el('tradeTrackedPairs')) el('tradeTrackedPairs').textContent = formatNum(analytics.tracked_pairs || 0);
    }

    // LichenSwap
    if (lichenswap) {
        activeFeeds++;
        if (el('tradeLichenSwaps')) el('tradeLichenSwaps').textContent = formatNum(lichenswap.swap_count || 0);
    }

    // Rewards
    if (rewards) {
        activeFeeds++;
        if (el('tradeRewardsDistributed')) el('tradeRewardsDistributed').textContent = formatLicn(rewards.total_distributed || 0);
        if (el('tradeRewardsEpoch')) el('tradeRewardsEpoch').textContent = formatNum(rewards.epoch || 0);
    }

    // Governance
    if (governance) {
        activeFeeds++;
        if (el('tradeGovProposals')) el('tradeGovProposals').textContent = formatNum(governance.proposal_count || 0);
        if (el('tradeGovVoters')) el('tradeGovVoters').textContent = formatNum(governance.voter_count || 0);
    }

    // Peak TPS from getMetrics
    if (metrics) {
        activeFeeds++;
        if (el('tradePeakTPS')) el('tradePeakTPS').textContent = (metrics.peak_tps || 0).toFixed(1);
    }

    if (badge) {
        badge.textContent = `${activeFeeds}/9 Feeds`;
        badge.className = 'panel-badge ' + (activeFeeds >= 8 ? 'success' : activeFeeds > 0 ? 'info' : 'warning');
    }

    tradingMetricsLoaded = true;
}

// ── Prediction Markets Monitor ──────────────────────────────

let predictionMonitorLoaded = false;

async function updatePredictionMonitor() {
    const badge = document.getElementById('predictionMarketBadge');
    const el = id => document.getElementById(id);

    const stats = await rpc('getPredictionMarketStats').catch(() => null);
    if (!stats) {
        if (badge) { badge.textContent = 'OFFLINE'; badge.className = 'panel-badge warning'; }
        return;
    }

    if (el('predTotalMarkets')) el('predTotalMarkets').textContent = formatNum(stats.total_markets || 0);
    if (el('predOpenMarkets')) el('predOpenMarkets').textContent = formatNum(stats.open_markets || 0);
    if (el('predTotalVolume')) el('predTotalVolume').textContent = formatLicn(stats.total_volume || 0);
    if (el('predTotalCollateral')) el('predTotalCollateral').textContent = formatLicn(stats.total_collateral || 0);
    if (el('predFeesCollected')) el('predFeesCollected').textContent = formatLicn(stats.fees_collected || 0);
    if (el('predTotalTraders')) el('predTotalTraders').textContent = formatNum(stats.total_traders || 0);
    if (el('predStatus')) el('predStatus').textContent = stats.paused ? 'PAUSED' : 'ACTIVE';

    // Render detail grid
    const grid = document.getElementById('predictionDetailGrid');
    if (grid) {
        const total = stats.total_markets || 0;
        const open = stats.open_markets || 0;
        const closed = total - open;
        const avgVolPerMarket = total > 0 ? Math.round((stats.total_volume || 0) / total) : 0;
        const avgCollateral = total > 0 ? Math.round((stats.total_collateral || 0) / total) : 0;
        grid.innerHTML = `
            <div class="tier-card">
                <div class="tier-label">Open / Total</div>
                <div class="tier-value">${formatNum(open)} / ${formatNum(total)}</div>
                <div class="tier-bar"><div class="tier-fill" style="width:${total > 0 ? (open / total * 100) : 0}%;background:var(--accent-green)"></div></div>
            </div>
            <div class="tier-card">
                <div class="tier-label">Closed / Resolved</div>
                <div class="tier-value">${formatNum(closed)}</div>
                <div class="tier-bar"><div class="tier-fill" style="width:${total > 0 ? (closed / total * 100) : 0}%;background:var(--accent-purple)"></div></div>
            </div>
            <div class="tier-card">
                <div class="tier-label">Avg Volume / Market</div>
                <div class="tier-value">${formatLicn(avgVolPerMarket)}</div>
            </div>
            <div class="tier-card">
                <div class="tier-label">Avg Collateral / Market</div>
                <div class="tier-value">${formatLicn(avgCollateral)}</div>
            </div>
            <div class="tier-card">
                <div class="tier-label">Total Fees Collected</div>
                <div class="tier-value">${formatLicn(stats.fees_collected || 0)}</div>
            </div>
            <div class="tier-card">
                <div class="tier-label">Unique Traders</div>
                <div class="tier-value">${formatNum(stats.total_traders || 0)}</div>
            </div>
        `;
    }

    if (badge) {
        const total = stats.total_markets || 0;
        badge.textContent = `${formatNum(total)} Markets`;
        badge.className = 'panel-badge ' + (total > 0 ? 'success' : 'info');
    }

    predictionMonitorLoaded = true;
}

// ── Platform Ecosystem Monitor ──────────────────────────────

let ecosystemMonitorLoaded = false;

async function updateEcosystemMonitor() {
    const badge = document.getElementById('ecosystemBadge');
    const el = id => document.getElementById(id);
    const totalFeeds = 18;

    // Fetch all platform contract stats in parallel
    const [lusd, weth, wsol, wbnb, lend, sporepay, vault, pump, bridge, dao, oracle,
        mossStorage, market, auction, punks, bounty, compute, shieldedState] = await Promise.all([
            rpc('getLusdStats').catch(() => null),
            rpc('getWethStats').catch(() => null),
            rpc('getWsolStats').catch(() => null),
            rpc('getWbnbStats').catch(() => null),
            rpc('getThallLendStats').catch(() => null),
            rpc('getSporePayStats').catch(() => null),
            rpc('getSporeVaultStats').catch(() => null),
            rpc('getSporePumpStats').catch(() => null),
            rpc('getLichenBridgeStats').catch(() => null),
            rpc('getLichenDaoStats').catch(() => null),
            rpc('getLichenOracleStats').catch(() => null),
            rpc('getMossStorageStats').catch(() => null),
            rpc('getLichenMarketStats').catch(() => null),
            rpc('getLichenAuctionStats').catch(() => null),
            rpc('getLichenPunksStats').catch(() => null),
            rpc('getBountyBoardStats').catch(() => null),
            rpc('getComputeMarketStats').catch(() => null),
            rpc('getShieldedPoolState').catch(() => null),
        ]);

    let activeFeeds = 0;

    // Tokens
    if (lusd) {
        activeFeeds++;
        if (el('ecoLusdSupply')) el('ecoLusdSupply').textContent = formatLicn(lusd.supply || 0);
        if (el('ecoLusdMinted')) el('ecoLusdMinted').textContent = formatNum(lusd.mint_events || 0);
        if (el('ecoLusdTransfers')) el('ecoLusdTransfers').textContent = formatNum(lusd.transfer_count || 0);
    }
    if (weth) {
        activeFeeds++;
        if (el('ecoWethSupply')) el('ecoWethSupply').textContent = formatLicn(weth.supply || 0);
    }
    if (wsol) {
        activeFeeds++;
        if (el('ecoWsolSupply')) el('ecoWsolSupply').textContent = formatLicn(wsol.supply || 0);
    }
    if (wbnb) {
        activeFeeds++;
        if (el('ecoWbnbSupply')) el('ecoWbnbSupply').textContent = formatLicn(wbnb.total_supply || wbnb.supply || 0);
    }

    // Platform services
    if (lend) {
        activeFeeds++;
        if (el('ecoLendDeposits')) el('ecoLendDeposits').textContent = formatLicn(lend.total_deposits || 0);
        if (el('ecoLendBorrows')) el('ecoLendBorrows').textContent = formatLicn(lend.total_borrows || 0);
    }
    if (sporepay) {
        activeFeeds++;
        if (el('ecoSporePayStreams')) el('ecoSporePayStreams').textContent = formatNum(sporepay.stream_count || 0);
    }
    if (vault) {
        activeFeeds++;
        if (el('ecoVaultAssets')) el('ecoVaultAssets').textContent = formatLicn(vault.total_assets || 0);
    }
    if (pump) {
        activeFeeds++;
        if (el('ecoPumpTokens')) el('ecoPumpTokens').textContent = formatNum(pump.token_count || 0);
    }

    // Infrastructure
    if (bridge) {
        activeFeeds++;
        if (el('ecoBridgeTxs')) el('ecoBridgeTxs').textContent = formatNum(bridge.nonce || 0);
        if (el('ecoBridgeLocked')) el('ecoBridgeLocked').textContent = formatLicn(bridge.locked_amount || 0);
    }
    if (dao) {
        activeFeeds++;
        if (el('ecoDaoProposals')) el('ecoDaoProposals').textContent = formatNum(dao.proposal_count || 0);
    }
    if (oracle) {
        activeFeeds++;
        if (el('ecoOracleFeeds')) el('ecoOracleFeeds').textContent = formatNum(oracle.feeds || 0);
    }
    if (mossStorage) {
        activeFeeds++;
        if (el('ecoMossData')) el('ecoMossData').textContent = formatNum(mossStorage.data_count || 0);
    }
    if (shieldedState) {
        activeFeeds++;
        if (el('ecoShieldedBalance')) el('ecoShieldedBalance').textContent = formatLicn(shieldedState.total_shielded || 0);
        if (el('ecoShieldedCommitments')) el('ecoShieldedCommitments').textContent = formatNum(shieldedState.pool_size || 0);
    }

    // NFT & Marketplace
    if (market) {
        activeFeeds++;
        if (el('ecoMarketListings')) el('ecoMarketListings').textContent = formatNum(market.listing_count || 0);
    }
    if (auction) {
        activeFeeds++;
        if (el('ecoAuctionVolume')) el('ecoAuctionVolume').textContent = formatLicn(auction.total_volume || 0);
    }
    if (punks) {
        activeFeeds++;
        if (el('ecoPunksMinted')) el('ecoPunksMinted').textContent = formatNum(punks.total_minted || 0);
    }
    if (bounty) {
        activeFeeds++;
        if (el('ecoBounties')) el('ecoBounties').textContent = formatNum(bounty.bounty_count || 0);
    }
    if (compute) {
        activeFeeds++;
        if (el('ecoComputeJobs')) el('ecoComputeJobs').textContent = formatNum(compute.job_count || 0);
    }

    // Detail grid
    const grid = document.getElementById('ecosystemDetailGrid');
    if (grid) {
        const cards = [];
        const addCard = (label, value, icon, color) => {
            cards.push(`<div class="tier-card">
                <div class="tier-label"><i class="fas fa-${icon}" style="margin-right:4px;color:${color}"></i>${label}</div>
                <div class="tier-value">${value}</div>
            </div>`);
        };

        if (lend) {
            addCard('Lending TVL', formatLicn((lend.total_deposits || 0) - (lend.total_borrows || 0)), 'piggy-bank', 'var(--accent-green)');
            addCard('Liquidations', formatNum(lend.liquidation_count || 0), 'gavel', 'var(--accent-red)');
        }
        if (sporepay) {
            addCard('Total Streamed', formatLicn(sporepay.total_streamed || 0), 'stream', 'var(--accent-blue)');
            addCard('Stream Cancels', formatNum(sporepay.cancel_count || 0), 'times-circle', 'var(--cyan-accent)');
        }
        if (vault) {
            addCard('Vault Strategies', formatNum(vault.strategy_count || 0), 'layer-group', 'var(--accent-purple)');
            addCard('Vault Earnings', formatLicn(vault.total_earned || 0), 'chart-line', 'var(--accent-green)');
        }
        if (pump) {
            addCard('SporePump Tokens', formatNum(pump.token_count || 0), 'rocket', 'var(--accent)');
            addCard('Graduated Tokens', formatNum(pump.total_graduated || 0), 'arrow-up-right-dots', 'var(--accent-orange)');
        }
        if (bridge) {
            addCard('Bridge Validators', formatNum(bridge.validator_count || 0), 'link', 'var(--accent-blue)');
            addCard('Required Confirms', formatNum(bridge.required_confirms || 0), 'check-double', 'var(--cyan-accent)');
        }
        if (bounty) {
            addCard('Bounties Completed', formatNum(bounty.completed_count || 0), 'trophy', 'var(--accent-green)');
            addCard('Reward Volume', formatLicn(bounty.reward_volume || 0), 'coins', 'var(--accent-purple)');
        }
        if (compute) {
            addCard('Jobs Completed', formatNum(compute.completed_count || 0), 'microchip', 'var(--accent-green)');
            addCard('Payment Volume', formatLicn(compute.payment_volume || 0), 'money-bill', 'var(--accent-blue)');
        }
        if (mossStorage) {
            addCard('Storage Bytes', formatNum(mossStorage.total_bytes || 0), 'database', 'var(--accent-purple)');
            addCard('Challenges', formatNum(mossStorage.challenge_count || 0), 'shield-alt', 'var(--cyan-accent)');
        }
        if (shieldedState) {
            addCard('Shielded Root', truncAddr(shieldedState.merkle_root || '--'), 'user-shield', 'var(--accent-purple)');
            addCard('Shielded Pool Size', formatNum(shieldedState.pool_size || 0), 'eye-slash', 'var(--cyan-accent)');
        }

        grid.innerHTML = cards.join('');
    }

    if (badge) {
        const healthyThreshold = Math.ceil(totalFeeds * 0.7);
        badge.textContent = `${activeFeeds}/${totalFeeds} Contracts`;
        badge.className = 'panel-badge ' + (activeFeeds >= healthyThreshold ? 'success' : activeFeeds > 0 ? 'info' : 'warning');
    }

    ecosystemMonitorLoaded = true;
}

// ── Protocol Control Plane ──────────────────────────────────

let controlPlaneMonitorLoaded = false;
let controlPlaneMonitorLoadedAt = 0;

async function updateControlPlaneMonitor() {
    const badge = document.getElementById('controlPlaneBadge');
    const grid = document.getElementById('controlPlaneDetailGrid');
    const element = id => document.getElementById(id);

    const [feeConfig, rentParams, mossStakePool, rewardInfo, incidentStatus, signedManifest] = await Promise.all([
        rpc('getFeeConfig').catch(() => null),
        rpc('getRentParams').catch(() => null),
        rpc('getMossStakePoolInfo').catch(() => null),
        rpc('getRewardAdjustmentInfo').catch(() => null),
        rpc('getIncidentStatus').catch(() => null),
        rpc('getSignedMetadataManifest').catch(() => null),
    ]);

    const registryEntries = signedManifest?.payload?.symbol_registry?.length || 0;
    const metadataHealthy = Boolean(signedManifest?.signer && registryEntries > 0);
    const incidentMode = String(incidentStatus?.mode || 'unknown').toUpperCase();
    const incidentSeverity = incidentStatus?.severity || 'warning';

    if (element('controlPlaneBaseFee')) {
        element('controlPlaneBaseFee').textContent = feeConfig
            ? `${formatLicnPrecise(feeConfig.base_fee_spores || 0)} LICN`
            : '--';
    }
    if (element('controlPlaneRentRate')) {
        element('controlPlaneRentRate').textContent = rentParams
            ? `${formatNum(rentParams.rent_rate_spores_per_kb_month || 0)} spores`
            : '--';
    }
    if (element('controlPlaneStakeTvl')) {
        element('controlPlaneStakeTvl').textContent = mossStakePool
            ? `${formatLicn(mossStakePool.total_licn_staked || 0)} LICN`
            : '--';
    }
    if (element('controlPlaneApy')) {
        element('controlPlaneApy').textContent = mossStakePool
            ? formatPercent(mossStakePool.average_apy_percent || 0)
            : '--';
    }
    if (element('controlPlaneIncidentMode')) {
        element('controlPlaneIncidentMode').textContent = incidentMode;
    }
    if (element('controlPlaneRegistryEntries')) {
        element('controlPlaneRegistryEntries').textContent = metadataHealthy
            ? formatNum(registryEntries)
            : '--';
    }

    if (grid) {
        const cards = [];
        const addCard = (label, value, meta, icon, color) => {
            cards.push(`<div class="tier-card">
                <div class="tier-label"><i class="fas fa-${icon}" style="margin-right:4px;color:${color}"></i>${escapeHtml(label)}</div>
                <div class="tier-value">${escapeHtml(value)}</div>
                <div class="tier-meta">${escapeHtml(meta)}</div>
            </div>`);
        };

        if (feeConfig) {
            addCard(
                'Fee Split',
                `${feeConfig.fee_burn_percent}/${feeConfig.fee_producer_percent}/${feeConfig.fee_voters_percent}/${feeConfig.fee_treasury_percent}/${feeConfig.fee_community_percent}`,
                'Burn / Producer / Voters / Treasury / Community',
                'percent',
                'var(--accent-blue)'
            );
            addCard('Deploy Fee', `${formatLicnPrecise(feeConfig.contract_deploy_fee_spores || 0)} LICN`, 'Per contract deployment', 'upload', 'var(--accent-purple)');
            addCard('Upgrade Fee', `${formatLicnPrecise(feeConfig.contract_upgrade_fee_spores || 0)} LICN`, 'Per contract upgrade', 'wrench', 'var(--cyan-accent)');
            addCard('NFT Mint Fee', `${formatLicnPrecise(feeConfig.nft_mint_fee_spores || 0)} LICN`, 'Per NFT mint', 'image', 'var(--accent-green)');
        }

        if (rentParams) {
            addCard(
                'Rent-Free Tier',
                `${formatNum(rentParams.rent_free_kb || 0)} KB`,
                `${formatNum(rentParams.rent_rate_spores_per_kb_month || 0)} spores / KB / month`,
                'warehouse',
                'var(--accent-blue)'
            );
        }

        if (rewardInfo) {
            addCard('Inflation Rate', formatPercent(rewardInfo.inflationRatePercent, 4), `Est. APY ${rewardInfo.estimatedApy || '--'}%`, 'chart-line', 'var(--accent-green)');
            addCard('Min Validator Stake', `${formatLicn(rewardInfo.minValidatorStake || 0)} LICN`, `${formatNum(rewardInfo.activeValidators || 0)} active validators`, 'shield-alt', 'var(--accent-red)');
        }

        if (mossStakePool) {
            addCard('stLICN Exchange', `${Number(mossStakePool.exchange_rate || 0).toFixed(4)}x`, `${formatNum(mossStakePool.total_stakers || 0)} stakers · ${mossStakePool.cooldown_days || 0} day cooldown`, 'seedling', 'var(--accent-purple)');
            addCard('MossStake Tiers', `${formatNum((mossStakePool.tiers || []).length)} tiers`, `${formatNum(mossStakePool.total_validators || 0)} validators routing rewards`, 'layer-group', 'var(--cyan-accent)');
        }

        if (signedManifest) {
            addCard('Metadata Signer', truncAddr(signedManifest.signer || '--'), `${formatDateTime(signedManifest.signed_at)} · ${signedManifest.payload?.network || '--'}`, 'signature', 'var(--accent-green)');
            addCard('Manifest Scope', formatNum(registryEntries), `${signedManifest.payload?.source_rpc || '--'}`, 'file-signature', 'var(--accent-blue)');
        }

        if (incidentStatus) {
            addCard('Incident Summary', incidentStatus.headline || incidentMode, incidentStatus.summary || 'No operator incident summary available.', 'triangle-exclamation', 'var(--accent-red)');
        }

        grid.innerHTML = cards.join('');
    }

    if (badge) {
        const badgeState = incidentMode === 'NORMAL'
            ? (metadataHealthy ? 'success' : 'info')
            : (incidentSeverity === 'critical' || incidentSeverity === 'high' ? 'danger' : 'warning');
        badge.textContent = metadataHealthy
            ? `${incidentMode} · ${formatNum(registryEntries)} Trusted`
            : `${incidentMode} · Partial`;
        badge.className = `panel-badge ${badgeState}`;
    }

    controlPlaneMonitorLoaded = true;
    controlPlaneMonitorLoadedAt = Date.now();
}

// ── Mission Control Expansion Boards ───────────────────────

const GOVERNANCE_WATCH_GOVERNED_LABELS = new Set([
    'community_treasury',
    'ecosystem_partnerships',
    'reserve_pool',
]);
const GOVERNANCE_WATCH_LIMIT = 25;
const GOVERNANCE_LARGE_TRANSFER_SPORES = 1000 * SPORES_PER_LICN;
const GOVERNANCE_OWNERSHIP_FUNCTIONS = new Set([
    'transfer_admin',
    'accept_admin',
    'transfer_owner',
    'transfer_ownership',
    'accept_owner',
    'set_owner',
    'set_admin',
    'set_identity_admin',
]);
const GOVERNANCE_BRIDGE_FUNCTIONS = new Set([
    'add_bridge_validator',
    'remove_bridge_validator',
    'set_required_confirmations',
    'set_request_timeout',
]);
const GOVERNANCE_ORACLE_FUNCTIONS = new Set([
    'add_price_feeder',
    'set_authorized_attester',
]);

const TREASURY_WATCH_BUCKETS = [
    { key: 'validator_rewards', label: 'Validator Rewards', pct: 10, warningRatio: 0.35, icon: 'coins', color: 'var(--primary)' },
    { key: 'community_treasury', label: 'Community Treasury', pct: 25, warningRatio: 0.35, icon: 'landmark', color: 'var(--accent-blue)' },
    { key: 'builder_grants', label: 'Builder Grants', pct: 35, warningRatio: 0.30, icon: 'hammer', color: 'var(--accent-purple)' },
    { key: 'founding_symbionts', label: 'Founding Symbionts', pct: 10, warningRatio: 0.20, icon: 'seedling', color: 'var(--accent)' },
    { key: 'ecosystem_partnerships', label: 'Ecosystem Partners', pct: 10, warningRatio: 0.20, icon: 'handshake', color: 'var(--accent-green)' },
    { key: 'reserve_pool', label: 'Reserve Pool', pct: 10, warningRatio: 0.20, icon: 'shield-alt', color: 'var(--cyan-accent)' },
];

const PROGRAM_HOTSPOT_WINDOW_MS = 15 * 60 * 1000;
const PROGRAM_HOTSPOT_LIMIT = 8;

let missionControlExpansionLoaded = false;
let missionControlExpansionLoadedAt = 0;

async function updateMissionControlExpansionBoards(force = false) {
    if (!force && missionControlExpansionLoaded && Date.now() - missionControlExpansionLoadedAt < CONTRACT_REFRESH_MS) {
        return;
    }

    await Promise.allSettled([
        updateTreasuryDistributionBoard(),
        updateNetworkInfrastructureBoard(),
        updateProgramHotspotsBoard(),
        updateOracleBridgeHealthBoard(),
        updatePrivacyPoolAuditBoard(),
        updateGovernanceWatchBoard(),
        updateServiceFleetBoard(),
    ]);

    missionControlExpansionLoaded = true;
    missionControlExpansionLoadedAt = Date.now();
}

async function updateTreasuryDistributionBoard() {
    const badge = document.getElementById('treasuryBoardBadge');
    const grid = document.getElementById('treasuryDetailGrid');
    const rewardInfo = await rpc('getRewardAdjustmentInfo').catch(() => null);

    if (!rewardInfo) {
        setText('treasuryTrackedBalance', '--');
        setText('treasuryTrackedShare', '--');
        setText('treasuryLargestDrift', '--');
        setText('treasuryWatchAlerts', '--');
        setText('treasuryHealthyWallets', '--');
        setText('treasuryWatchSummary', 'Reward-distribution state is unavailable from RPC.');
        if (grid) grid.innerHTML = '';
        if (badge) {
            badge.textContent = 'UNAVAILABLE';
            badge.className = 'panel-badge warning';
        }
        return;
    }

    const wallets = rewardInfo.wallets || {};
    const genesisSupply = Number(rewardInfo.genesisSupply || 0);
    const totalSupply = Number(rewardInfo.totalSupply || 0);

    let totalTracked = 0;
    let belowFloorCount = 0;
    let tightCount = 0;
    let largestDriftLabel = '--';
    let largestDriftPct = 0;
    const cards = [];

    TREASURY_WATCH_BUCKETS.forEach((bucket) => {
        const wallet = wallets[bucket.key] || {};
        const balance = Number(wallet.balance_spores ?? wallet.balance ?? 0);
        const baseline = Math.round(genesisSupply * (bucket.pct / 100));
        const driftPct = baseline > 0 ? ((balance - baseline) / baseline) * 100 : 0;
        const remainingPct = baseline > 0 ? (balance / baseline) * 100 : 0;
        const floorAmount = baseline * bucket.warningRatio;

        totalTracked += balance;

        if (Math.abs(driftPct) >= Math.abs(largestDriftPct)) {
            largestDriftPct = driftPct;
            largestDriftLabel = bucket.label;
        }

        let color = bucket.color;
        let status = 'Nominal';
        if (balance <= floorAmount) {
            belowFloorCount++;
            color = 'var(--danger)';
            status = 'Below floor';
        } else if (remainingPct < 70) {
            tightCount++;
            color = 'var(--warning)';
            status = 'Tight';
        }

        const meta = `${status} · ${truncAddr(wallet.pubkey || 'unknown')} · Drift ${formatSignedPercent(driftPct, 1)} · Floor ${formatLicn(floorAmount)} LICN`;
        cards.push(renderOperatorTierCard(
            bucket.label,
            `${formatLicn(balance)} LICN`,
            meta,
            bucket.icon,
            color,
            remainingPct
        ));
    });

    const aboveFloorCount = TREASURY_WATCH_BUCKETS.length - belowFloorCount;
    const trackedSharePct = totalSupply > 0 ? (totalTracked / totalSupply) * 100 : 0;
    const alertLabel = belowFloorCount > 0
        ? `${belowFloorCount} below floor`
        : tightCount > 0
            ? `${tightCount} tight`
            : 'Clear';

    setText('treasuryTrackedBalance', `${formatLicn(totalTracked)} LICN`);
    setText('treasuryTrackedShare', formatPercent(trackedSharePct, 1));
    setText('treasuryLargestDrift', `${largestDriftLabel} ${formatSignedPercent(largestDriftPct, 1)}`);
    setText('treasuryWatchAlerts', alertLabel);
    setText('treasuryHealthyWallets', `${aboveFloorCount}/${TREASURY_WATCH_BUCKETS.length}`);
    setText(
        'treasuryWatchSummary',
        `Governed wallet watch floors are configured from genesis allocation. Baseline tracked balance is ${formatLicn(genesisSupply)} LICN across ${TREASURY_WATCH_BUCKETS.length} treasury buckets.`
    );

    if (grid) {
        grid.innerHTML = cards.join('');
    }

    if (badge) {
        badge.textContent = belowFloorCount > 0
            ? `${belowFloorCount} Below Floor`
            : tightCount > 0
                ? `${tightCount} Tight`
                : 'All Guarded';
        badge.className = `panel-badge ${belowFloorCount > 0 ? 'danger' : tightCount > 0 ? 'warning' : 'success'}`;
    }
}

async function updateNetworkInfrastructureBoard() {
    const badge = document.getElementById('networkInfrastructureBadge');
    const grid = document.getElementById('networkInfrastructureGrid');

    const [networkInfo, clusterInfo, versionInfo, genesisBlock] = await Promise.all([
        rpc('getNetworkInfo').catch(() => null),
        rpc('getClusterInfo').catch(() => null),
        solanaCompatRpc('getVersion').catch(() => null),
        rpc('getBlock', [0]).catch(() => null),
    ]);

    const currentSlot = Number(networkInfo?.current_slot ?? clusterInfo?.current_slot ?? 0);
    const nodes = Array.isArray(clusterInfo?.cluster_nodes) ? clusterInfo.cluster_nodes.slice() : [];
    const peerCount = Number(networkInfo?.peer_count ?? clusterInfo?.peer_count ?? clusterInfo?.connected_peers?.length ?? 0);
    const validatorCount = Number(networkInfo?.validator_count ?? clusterInfo?.validator_count ?? nodes.length ?? 0);
    const version = versionInfo?.['solana-core'] || networkInfo?.version || '--';
    const genesisHash = genesisBlock?.hash || genesisBlock?.blockhash || '--';
    const maxDelta = nodes.reduce((maxDeltaSoFar, node) => {
        const delta = currentSlot > 0 ? Math.max(0, currentSlot - Number(node.last_active_slot || 0)) : 0;
        return Math.max(maxDeltaSoFar, delta);
    }, 0);

    setText('networkChainId', networkInfo?.chain_id || '--');
    setText('networkNetworkId', networkInfo?.network_id || '--');
    setText('networkNodeVersion', version);
    setText('networkPeerCount', formatNum(peerCount));
    setText('networkClusterMembers', formatNum(validatorCount));
    setText('networkGenesisHash', genesisHash !== '--' ? truncAddr(genesisHash) : '--');
    setText(
        'networkInfrastructureNote',
        `Genesis hash is derived from block 0. Connected peers: ${formatNum(peerCount)}. Largest validator liveness delta is ${formatNum(maxDelta)} slots.`
    );

    if (grid) {
        const cards = [];
        const peersPreview = Array.isArray(clusterInfo?.connected_peers) && clusterInfo.connected_peers.length > 0
            ? clusterInfo.connected_peers.slice(0, 3).join(', ')
            : 'No peer addresses published';

        cards.push(renderOperatorTierCard(
            'Peer Mesh',
            `${formatNum(peerCount)} peers`,
            peersPreview,
            'share-nodes',
            peerCount > 0 ? 'var(--primary)' : 'var(--warning)',
            peerCount > 0 ? 100 : 0
        ));

        if (nodes.length === 0) {
            cards.push(renderOperatorTierCard(
                'Cluster Membership',
                'Unavailable',
                'Validator membership is not currently visible from getClusterInfo.',
                'triangle-exclamation',
                'var(--warning)',
                0
            ));
        } else {
            nodes
                .sort((left, right) => Number(right.stake || 0) - Number(left.stake || 0))
                .forEach((node, index) => {
                    const delta = currentSlot > 0 ? Math.max(0, currentSlot - Number(node.last_active_slot || 0)) : 0;
                    const active = node.active !== false && delta <= 100;
                    const color = delta <= 2
                        ? 'var(--success)'
                        : delta <= 10
                            ? 'var(--warning)'
                            : 'var(--danger)';
                    const label = `Validator ${index + 1}`;
                    const value = active ? `${formatNum(delta)} slot delta` : 'STALE';
                    const meta = `${formatLicn(Number(node.stake || 0))} LICN · ${formatNum(Number(node.blocks_proposed || 0))} blocks · ${truncAddr(node.pubkey || '--')}`;
                    const healthPct = active ? Math.max(6, 100 - clampPercentage(delta * 8)) : 4;

                    cards.push(renderOperatorTierCard(label, value, meta, active ? 'server' : 'triangle-exclamation', color, healthPct));
                });
        }

        grid.innerHTML = cards.join('');
    }

    if (badge) {
        const badgeState = validatorCount === 0
            ? 'warning'
            : maxDelta <= 2 && peerCount > 0
                ? 'success'
                : maxDelta <= 10
                    ? 'warning'
                    : 'danger';
        badge.textContent = validatorCount === 0
            ? 'PARTIAL'
            : maxDelta <= 2 && peerCount > 0
                ? 'SYNCED'
                : `DELTA ${formatNum(maxDelta)}`;
        badge.className = `panel-badge ${badgeState}`;
    }
}

async function updateProgramHotspotsBoard() {
    const badge = document.getElementById('programHotspotsBadge');
    const grid = document.getElementById('programHotspotGrid');

    const registryEntries = (await Promise.all(ALL_CONTRACTS.map(async (contract) => {
        const info = await getMonitoringSymbolRegistryEntry(contract.symbol);
        if (!info || !info.program) return null;
        return { contract, program: info.program };
    }))).filter(Boolean);

    let statEntries = (await Promise.all(registryEntries.map(async (entry) => {
        const stats = await rpc('getProgramStats', [entry.program]).catch(() => null);
        if (!stats) return null;
        return { ...entry, stats };
    }))).filter(Boolean);

    const hasProgramStats = statEntries.length > 0;

    if (!hasProgramStats && registryEntries.length > 0) {
        statEntries = registryEntries.map((entry) => ({
            ...entry,
            stats: { call_count: 0 },
        }));
    }

    if (statEntries.length === 0) {
        setText('programTrackedCount', '--');
        setText('programTotalCalls', '--');
        setText('programRecentCalls', '--');
        setText('programHottest', '--');
        setText('programActiveFunctions', '--');
        setText('programLastActivity', '--');
        setText('programHotspotNote', 'Program call telemetry is unavailable from RPC.');
        if (grid) grid.innerHTML = '';
        if (badge) {
            badge.textContent = 'UNAVAILABLE';
            badge.className = 'panel-badge warning';
        }
        return;
    }

    if (hasProgramStats) {
        statEntries.sort((left, right) => Number(right.stats.call_count || 0) - Number(left.stats.call_count || 0));
    } else {
        statEntries.sort((left, right) => String(left.contract.symbol || '').localeCompare(String(right.contract.symbol || '')));
    }
    const topEntries = statEntries.slice(0, PROGRAM_HOTSPOT_LIMIT);

    const enrichedEntries = await Promise.all(topEntries.map(async (entry) => {
        const callsEnvelope = await rpc('getProgramCalls', [entry.program, { limit: 25 }]).catch(() => null);
        const calls = Array.isArray(callsEnvelope?.calls) ? callsEnvelope.calls : [];
        const recentCalls = calls.filter((call) => normalizeTimestampMs(call.timestamp) >= Date.now() - PROGRAM_HOTSPOT_WINDOW_MS);
        const functionCounts = recentCalls.reduce((counts, call) => {
            const functionName = call.function || 'call';
            counts[functionName] = (counts[functionName] || 0) + 1;
            return counts;
        }, {});
        const topFunction = Object.entries(functionCounts).sort((left, right) => right[1] - left[1])[0] || null;
        const lastCallMs = calls.reduce((latest, call) => Math.max(latest, normalizeTimestampMs(call.timestamp)), 0);

        return {
            ...entry,
            calls,
            recentCalls,
            topFunction: topFunction ? topFunction[0] : 'n/a',
            topFunctionCount: topFunction ? topFunction[1] : 0,
            lastCallMs,
        };
    }));

    const totalCalls = statEntries.reduce((sum, entry) => sum + Number(entry.stats.call_count || 0), 0);
    const recentCallsTotal = enrichedEntries.reduce((sum, entry) => sum + entry.recentCalls.length, 0);
    const distinctFunctions = new Set(enrichedEntries.flatMap((entry) => entry.recentCalls.map((call) => call.function || 'call')));
    const hottestEntry = enrichedEntries
        .slice()
        .sort((left, right) => {
            const recentDelta = right.recentCalls.length - left.recentCalls.length;
            if (recentDelta !== 0) return recentDelta;
            return Number(right.stats.call_count || 0) - Number(left.stats.call_count || 0);
        })[0] || null;
    const lastActivityMs = enrichedEntries.reduce((latest, entry) => Math.max(latest, entry.lastCallMs), 0);
    const maxCallCount = enrichedEntries.reduce((maxCalls, entry) => Math.max(maxCalls, Number(entry.stats.call_count || 0)), 0);
    const maxRecentCount = enrichedEntries.reduce((maxCalls, entry) => Math.max(maxCalls, entry.recentCalls.length), 0);

    setText('programTrackedCount', formatNum(statEntries.length));
    setText('programTotalCalls', formatNum(totalCalls));
    setText('programRecentCalls', formatNum(recentCallsTotal));
    setText('programHottest', hasProgramStats && hottestEntry ? hottestEntry.contract.symbol : '--');
    setText('programActiveFunctions', formatNum(distinctFunctions.size));
    setText('programLastActivity', hasProgramStats
        ? (lastActivityMs ? timeAgo(Math.floor(lastActivityMs / 1000)) : 'No recent calls')
        : '--');
    setText(
        'programHotspotNote',
        hasProgramStats
            ? `Hotspots rank all-time call volume and a 15 minute recent call window from getProgramCalls. RPC does not expose failed call rates yet.`
            : 'Program registry is wired, but this RPC has not returned program call counters yet. Showing tracked program coverage only.'
    );

    if (grid) {
        const cards = enrichedEntries.map((entry) => {
            const totalCallCount = Number(entry.stats.call_count || 0);
            const recentCount = entry.recentCalls.length;
            const burstPct = totalCallCount > 0 ? (recentCount / totalCallCount) * 100 : 0;
            const barPct = maxRecentCount > 0
                ? (recentCount / maxRecentCount) * 100
                : maxCallCount > 0
                    ? (totalCallCount / maxCallCount) * 100
                    : 0;
            const color = recentCount > 0 ? 'var(--primary)' : 'var(--text-muted)';
            const meta = `Recent ${formatNum(recentCount)} · Burst ${formatPercent(burstPct, 1)} · Last ${entry.lastCallMs ? timeAgo(Math.floor(entry.lastCallMs / 1000)) : 'none'} · Top fn ${entry.topFunction}`;
            return renderOperatorTierCard(
                `${entry.contract.symbol} · ${entry.contract.name}`,
                `${formatNum(totalCallCount)} calls`,
                meta,
                'microchip',
                color,
                barPct
            );
        });

        grid.innerHTML = cards.join('');
    }

    if (badge) {
        badge.textContent = hasProgramStats
            ? (recentCallsTotal > 0 ? `${formatNum(recentCallsTotal)} Hot` : `${formatNum(statEntries.length)} Tracked`)
            : `${formatNum(statEntries.length)} Tracked`;
        badge.className = `panel-badge ${hasProgramStats ? (recentCallsTotal > 0 ? 'success' : 'info') : 'info'}`;
    }
}

async function updateOracleBridgeHealthBoard() {
    const badge = document.getElementById('oracleBridgeBadge');
    const grid = document.getElementById('oracleBridgeDetailGrid');

    const [bridge, oracle] = await Promise.all([
        rpc('getLichenBridgeStats').catch(() => null),
        rpc('getLichenOracleStats').catch(() => null),
    ]);

    setText('oracleFeedCount', oracle ? formatNum(Number(oracle.feeds || 0)) : '--');
    setText('oracleQueryCount', oracle ? formatNum(Number(oracle.queries || 0)) : '--');
    setText('oracleAttestationCount', oracle ? formatNum(Number(oracle.attestations || 0)) : '--');
    setText('bridgeValidatorCount', bridge ? formatNum(Number(bridge.validator_count || 0)) : '--');
    setText('bridgeRequiredConfirms', bridge ? formatNum(Number(bridge.required_confirms || 0)) : '--');
    setText('bridgeLockedAmount', bridge ? `${formatLicn(Number(bridge.locked_amount || 0))} LICN` : '--');

    const oraclePaused = Boolean(oracle?.paused);
    const bridgePaused = Boolean(bridge?.paused);
    const requestTimeout = Number(bridge?.request_timeout || 0);
    setText(
        'oracleBridgeNote',
        `Oracle is ${oraclePaused ? 'paused' : 'live'} and bridge is ${bridgePaused ? 'paused' : 'live'}. Bridge request timeout is ${formatNum(requestTimeout)} seconds.`
    );

    if (grid) {
        const cards = [];

        if (oracle) {
            const feeds = Number(oracle.feeds || 0);
            const queries = Number(oracle.queries || 0);
            const attestations = Number(oracle.attestations || 0);
            const queriesPerFeed = feeds > 0 ? queries / feeds : 0;
            const attestationsPerFeed = feeds > 0 ? attestations / feeds : 0;
            const oracleColor = oraclePaused ? 'var(--warning)' : 'var(--success)';

            cards.push(renderOperatorTierCard(
                'Oracle Feed Coverage',
                `${formatNum(feeds)} feeds`,
                `${formatNum(queries)} queries · ${formatNum(attestations)} attestations`,
                'satellite-dish',
                oracleColor,
                feeds > 0 ? 100 : 0
            ));
            cards.push(renderOperatorTierCard(
                'Oracle Density',
                `${queriesPerFeed.toFixed(1)} queries / feed`,
                `${attestationsPerFeed.toFixed(1)} attestations / feed`,
                'chart-line',
                oracleColor,
                clampPercentage(attestationsPerFeed * 10)
            ));
        }

        if (bridge) {
            const validators = Number(bridge.validator_count || 0);
            const confirms = Number(bridge.required_confirms || 0);
            const lockedAmount = Number(bridge.locked_amount || 0);
            const nonce = Number(bridge.nonce || 0);
            const bridgeColor = bridgePaused ? 'var(--warning)' : 'var(--primary)';
            const lockedPerValidator = validators > 0 ? lockedAmount / validators : 0;

            cards.push(renderOperatorTierCard(
                'Bridge Finality',
                `${formatNum(confirms)} confirms`,
                `${formatNum(validators)} validators · nonce ${formatNum(nonce)}`,
                'bridge',
                bridgeColor,
                validators > 0 ? clampPercentage((confirms / validators) * 100) : 0
            ));
            cards.push(renderOperatorTierCard(
                'Bridge Liquidity',
                `${formatLicn(lockedAmount)} LICN`,
                `${formatLicn(lockedPerValidator)} LICN / validator · timeout ${formatNum(requestTimeout)}s`,
                'vault',
                bridgeColor,
                validators > 0 ? 100 : 0
            ));
        }

        grid.innerHTML = cards.join('');
    }

    if (badge) {
        const allAvailable = Boolean(bridge) && Boolean(oracle);
        const healthy = allAvailable && !bridgePaused && !oraclePaused;
        badge.textContent = !allAvailable
            ? 'PARTIAL'
            : healthy
                ? 'ACTIVE'
                : 'PAUSED';
        badge.className = `panel-badge ${!allAvailable ? 'warning' : healthy ? 'success' : 'warning'}`;
    }
}

async function updatePrivacyPoolAuditBoard() {
    const badge = document.getElementById('privacyAuditBadge');
    const grid = document.getElementById('privacyAuditGrid');

    const shieldedState = await rpc('getShieldedPoolState').catch(() => null);
    const metrics = lastMetricsSnapshot || await rpc('getMetrics').catch(() => null);

    const merkleRoot = shieldedState?.merkleRoot || shieldedState?.merkle_root || '--';
    const commitmentCount = Number(shieldedState?.commitmentCount ?? shieldedState?.commitment_count ?? shieldedState?.pool_size ?? 0);
    const nullifierCount = Number(shieldedState?.nullifierCount ?? shieldedState?.nullifier_count ?? 0);
    const shieldCount = Number(shieldedState?.shieldCount ?? shieldedState?.shield_count ?? 0);
    const unshieldCount = Number(shieldedState?.unshieldCount ?? shieldedState?.unshield_count ?? 0);
    const transferCount = Number(shieldedState?.transferCount ?? shieldedState?.transfer_count ?? 0);
    const totalShielded = Number(shieldedState?.totalShielded ?? shieldedState?.total_shielded ?? shieldedState?.pool_balance ?? 0);
    const liveNotes = Math.max(0, commitmentCount - nullifierCount);
    const effectiveSupply = Math.max(0, Number(metrics?.total_supply || 0) - Number(metrics?.total_burned || 0));
    const shieldedSharePct = effectiveSupply > 0 ? (totalShielded / effectiveSupply) * 100 : 0;
    const zkScheme = shieldedState?.zkScheme || shieldedState?.zk_scheme || '--';

    setText('privacyMerkleRoot', merkleRoot !== '--' ? truncAddr(merkleRoot) : '--');
    setText('privacyCommitmentCount', formatNum(commitmentCount));
    setText('privacyNullifierCount', formatNum(nullifierCount));
    setText('privacyLiveNotes', formatNum(liveNotes));
    setText('privacyShieldedBalance', `${formatLicn(totalShielded)} LICN`);
    setText('privacySupplyShare', formatPercent(shieldedSharePct, 2));
    setText(
        'privacyAuditNote',
        `Balance share uses effective supply. Live notes are commitments minus nullifiers. ZK scheme: ${zkScheme}.`
    );

    if (grid) {
        const cards = [
            renderOperatorTierCard(
                'Commitment Set',
                `${formatNum(commitmentCount)} commitments`,
                `${formatNum(nullifierCount)} nullifiers · root ${truncAddr(merkleRoot)}`,
                'database',
                'var(--primary)',
                commitmentCount > 0 ? 100 : 0
            ),
            renderOperatorTierCard(
                'Shield Flow',
                `${formatNum(shieldCount)} shield`,
                `${formatNum(unshieldCount)} unshield · ${formatNum(transferCount)} private transfers`,
                'eye-slash',
                'var(--accent-purple)',
                commitmentCount > 0 ? clampPercentage((shieldCount / commitmentCount) * 100) : 0
            ),
            renderOperatorTierCard(
                'Live Notes',
                formatNum(liveNotes),
                `${formatPercent(commitmentCount > 0 ? (liveNotes / commitmentCount) * 100 : 0, 1)} of commitments remain live`,
                'layer-group',
                'var(--accent-green)',
                commitmentCount > 0 ? (liveNotes / commitmentCount) * 100 : 0
            ),
            renderOperatorTierCard(
                'Shielded Share',
                formatPercent(shieldedSharePct, 2),
                `${formatLicn(totalShielded)} LICN shielded against ${formatLicn(effectiveSupply)} LICN effective supply`,
                'percent',
                'var(--cyan-accent)',
                shieldedSharePct
            ),
        ];

        grid.innerHTML = cards.join('');
    }

    if (badge) {
        badge.textContent = commitmentCount > 0 ? `${formatNum(commitmentCount)} Commitments` : 'IDLE';
        badge.className = `panel-badge ${commitmentCount > 0 ? 'success' : 'info'}`;
    }
}

async function updateGovernanceWatchBoard() {
    const badge = document.getElementById('governanceWatchBadge');
    const grid = document.getElementById('governanceWatchGrid');

    const [incidentStatus, governanceEnvelope, rewardInfo] = await Promise.all([
        rpc('getIncidentStatus').catch(() => null),
        rpc('getGovernanceEvents', [GOVERNANCE_WATCH_LIMIT]).catch(() => null),
        rpc('getRewardAdjustmentInfo').catch(() => null),
    ]);

    const wallets = buildGovernedWalletEntries(rewardInfo);
    const tokenWatchEntries = (await Promise.all(wallets.map(async (wallet) => {
        const envelope = await solanaCompatRpc('getTokenAccountsByOwner', [
            wallet.pubkey,
            { programId: 'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA' },
            { encoding: 'jsonParsed' },
        ]).catch(() => null);
        const values = Array.isArray(envelope?.value) ? envelope.value : [];
        const discovered = [];
        const seen = new Set();

        values.forEach((entry) => {
            const info = entry?.account?.data?.parsed?.info;
            const mint = typeof info?.mint === 'string' ? info.mint : '';
            if (!mint || seen.has(mint)) return;
            seen.add(mint);
            discovered.push({
                owner: wallet.pubkey,
                ownerLabel: wallet.label,
                mint,
                amount: Number(info?.tokenAmount?.amount || 0),
            });
        });

        return discovered;
    }))).flat();

    const governanceAlerts = (Array.isArray(governanceEnvelope?.events) ? governanceEnvelope.events : [])
        .flatMap(classifyGovernanceEventForMonitoring)
        .sort((left, right) => Number(right.event?.slot || 0) - Number(left.event?.slot || 0))
        .slice(0, GOVERNANCE_WATCH_LIMIT);
    const criticalOrHigh = governanceAlerts.filter((alert) => ['critical', 'high'].includes(alert.severity)).length;
    const latestAlert = governanceAlerts[0] || null;
    const incidentMode = String(incidentStatus?.mode || '--').toUpperCase();

    setText('governanceIncidentMode', incidentMode);
    setText('governanceAlertCount', formatNum(governanceAlerts.length));
    setText('governanceCriticalCount', governanceAlerts.length > 0 ? `${formatNum(criticalOrHigh)}/${formatNum(governanceAlerts.length)}` : '0/0');
    setText('governanceWalletWatchCount', formatNum(wallets.length));
    setText('governanceTokenWatchCount', formatNum(tokenWatchEntries.length));
    const lastAlertTime = latestAlert ? timeAgoFromTimestamp(latestAlert.event?.slot_time || latestAlert.event?.timestamp) : '--';
    setText('governanceLastAlert', latestAlert ? (lastAlertTime !== '--' ? lastAlertTime : `slot ${formatNum(Number(latestAlert.event?.slot || 0))}`) : 'Clear');
    setText(
        'governanceWatchNote',
        rewardInfo
            ? `Recent governance-sensitive alerts are classified from getGovernanceEvents. Guarded wallet coverage auto-discovers governed native wallets and current token pairs.`
            : 'Governance watch coverage is unavailable from RPC.'
    );

    if (grid) {
        const cards = [];

        if (governanceAlerts.length === 0) {
            cards.push(renderOperatorTierCard(
                'Governance Alerts',
                'No sensitive events',
                'No recent contract upgrade, pause, bridge, oracle, treasury, or ownership alerts were detected.',
                'shield-alt',
                'var(--success)',
                100
            ));
        } else {
            governanceAlerts.slice(0, 4).forEach((alert) => {
                const color = alert.severity === 'critical'
                    ? 'var(--danger)'
                    : alert.severity === 'high'
                        ? 'var(--warning)'
                        : 'var(--accent-blue)';
                cards.push(renderOperatorTierCard(
                    alert.title,
                    String(alert.severity || 'info').toUpperCase(),
                    buildGovernanceAlertMeta(alert),
                    alert.ruleId === 'treasury-transfer' ? 'landmark' : 'gavel',
                    color,
                    alert.severity === 'critical' ? 100 : alert.severity === 'high' ? 72 : 48
                ));
            });
        }

        wallets.forEach((wallet) => {
            const tokenPairs = tokenWatchEntries.filter((entry) => entry.owner === wallet.pubkey);
            cards.push(renderOperatorTierCard(
                wallet.label.replace(/_/g, ' '),
                `${formatLicn(wallet.balance)} LICN`,
                `${formatNum(tokenPairs.length)} token pairs watched · ${truncAddr(wallet.pubkey)}`,
                'wallet',
                wallet.balance > 0 ? 'var(--primary)' : 'var(--text-muted)',
                tokenPairs.length > 0 ? clampPercentage(tokenPairs.length * 25) : 12
            ));
        });

        grid.innerHTML = cards.join('');
    }

    if (badge) {
        const badgeState = governanceAlerts.length === 0
            ? 'success'
            : criticalOrHigh > 0
                ? 'warning'
                : 'info';
        badge.textContent = governanceAlerts.length === 0
            ? 'ARMED'
            : `${formatNum(governanceAlerts.length)} WATCHED`;
        badge.className = `panel-badge ${badgeState}`;
    }
}

async function updateServiceFleetBoard() {
    const badge = document.getElementById('serviceFleetBadge');
    const grid = document.getElementById('serviceFleetGrid');
    const fleet = await rpc('getServiceFleetStatus').catch(() => null);

    if (!fleet) {
        setText('serviceFleetHostCount', '--');
        setText('serviceFleetHealthyServices', '--');
        setText('serviceFleetDegradedServices', '--');
        setText('serviceFleetIntentionalAbsence', '--');
        setText('serviceFleetLastSuccess', '--');
        setText('serviceFleetNote', 'Service fleet probes are unavailable from RPC.');
        if (grid) grid.innerHTML = '';
        if (badge) {
            badge.textContent = 'UNAVAILABLE';
            badge.className = 'panel-badge warning';
        }
        return;
    }

    const summary = fleet.summary || {};
    const hosts = Array.isArray(fleet.hosts) ? fleet.hosts : [];
    const services = hosts.flatMap((host) => {
        const hostLabel = host.label || host.id || '--';
        return (Array.isArray(host.services) ? host.services : []).map((service) => ({
            ...service,
            hostLabel,
        }));
    });
    const healthyServices = Number(summary.healthy_services ?? services.filter((service) => service.state === 'healthy').length);
    const degradedServices = Number(summary.degraded_services ?? services.filter((service) => !service.intentionally_absent && service.state !== 'healthy').length);
    const intentionallyAbsent = Number(summary.intentionally_absent_services ?? services.filter((service) => service.intentionally_absent).length);
    const lastSuccessAt = summary.last_success_at;

    setText('serviceFleetHostCount', formatNum(Number(summary.host_count ?? hosts.length)));
    setText('serviceFleetHealthyServices', formatNum(healthyServices));
    setText('serviceFleetDegradedServices', formatNum(degradedServices));
    setText('serviceFleetIntentionalAbsence', formatNum(intentionallyAbsent));
    setText('serviceFleetLastSuccess', lastSuccessAt ? timeAgoFromTimestamp(lastSuccessAt) : 'No successful probes');
    setText(
        'serviceFleetNote',
        fleet.state === 'probe_error'
            ? (services[0]?.message || 'Service fleet probing is not configured on this RPC node.')
            : `Probe timeout is ${formatNum(Number(fleet.probe_timeout_ms || 0))}ms. Intentionally absent services stay explicit instead of appearing unhealthy.`
    );

    if (grid) {
        const cards = services.length === 0
            ? [renderOperatorTierCard(
                'Service Fleet',
                'Not configured',
                'No fleet probe targets are configured for this RPC node yet.',
                'server',
                'var(--warning)',
                0
            )]
            : services.map((service) => {
                const state = String(service.state || 'unknown').toLowerCase();
                const intentionallyAbsentService = Boolean(service.intentionally_absent);
                const color = intentionallyAbsentService
                    ? 'var(--text-muted)'
                    : state === 'healthy'
                        ? 'var(--success)'
                        : 'var(--danger)';
                const icon = service.service === 'custody'
                    ? 'shield-alt'
                    : service.service === 'faucet'
                        ? 'faucet'
                        : 'server';
                const value = intentionallyAbsentService
                    ? 'ABSENT BY DESIGN'
                    : state.toUpperCase();
                const meta = `${service.message || 'No probe message'} · ${service.last_success_at ? `last ok ${timeAgoFromTimestamp(service.last_success_at)}` : 'no successful probe yet'} · ${service.hostLabel}`;
                return renderOperatorTierCard(
                    `${service.hostLabel} · ${service.label || service.id}`,
                    value,
                    meta,
                    icon,
                    color,
                    intentionallyAbsentService ? 0 : state === 'healthy' ? 100 : 18
                );
            });

        grid.innerHTML = cards.join('');
    }

    if (badge) {
        const badgeState = degradedServices > 0
            ? 'danger'
            : healthyServices > 0
                ? 'success'
                : 'warning';
        badge.textContent = degradedServices > 0
            ? `${formatNum(degradedServices)} DEGRADED`
            : healthyServices > 0
                ? 'CLEAR'
                : 'PARTIAL';
        badge.className = `panel-badge ${badgeState}`;
    }
}

// ── Clock ───────────────────────────────────────────────────

function updateClock() {
    const el = document.getElementById('navClock');
    if (el) el.textContent = now();
}

// ── Init ────────────────────────────────────────────────────

async function init() {
    purgeLegacyAdminToken();
    bindStaticControls();
    addEvent('info', 'power-off', 'Mission Control initializing...');

    // Set network selector — rebuild options, hide local-* in production
    const savedNet = currentMonitoringNetwork();
    const sel = document.getElementById('networkSelect');
    if (sel) {
        sel.innerHTML = '';
        const labels = { mainnet: 'Mainnet', testnet: 'Testnet', 'local-testnet': 'Local Testnet', 'local-mainnet': 'Local Mainnet' };
        for (const key of Object.keys(NETWORKS)) {
            if (_monIsProduction && (key === 'mainnet' || key === 'local-testnet' || key === 'local-mainnet')) continue;
            const opt = document.createElement('option');
            opt.value = key;
            opt.textContent = labels[key] || key;
            sel.appendChild(opt);
        }
        sel.value = savedNet;
    }
    void LICHEN_CONFIG.refreshIncidentStatusBanner(savedNet);
    updateEndpointTelemetry();
    connectWsProbe();

    // Clock
    setInterval(updateClock, 1000);
    updateClock();

    // Initial refresh
    await refresh();
    addEvent('success', 'check-circle', 'Mission Control online');

    // Auto-refresh loop
    setInterval(refresh, REFRESH_MS);
}

// Start
document.addEventListener('DOMContentLoaded', init);

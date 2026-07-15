'use strict';

const RPC_URL = '/api/rpc';
const PUBLIC_RPC_LABEL = 'https://testnet-api.lichen.network';
const WS_URL = 'wss://testnet-api.lichen.network/ws';
const EXPLORER_URL = 'https://explorer.lichen.network/';
const REFRESH_MS = 30000;
const WS_STALE_MS = 15000;

let ws;
let wsLastSlot = null;
let wsLastMessageAt = 0;
let latestHttpChecks = null;

function byId(id) {
    return document.getElementById(id);
}

function setText(id, value) {
    const el = byId(id);
    if (el) el.textContent = value;
}

function formatNumber(value) {
    const numeric = Number(value);
    return Number.isFinite(numeric) ? Math.trunc(numeric).toLocaleString() : '--';
}

function formatMs(value) {
    const numeric = Number(value);
    return Number.isFinite(numeric) ? `${Math.round(numeric)}ms` : '--';
}

function formatFee(spores) {
    const numeric = Number(spores);
    if (!Number.isFinite(numeric)) return '--';
    return `${numeric.toLocaleString()} spores`;
}

function setCardStatus(cardName, label, detail, state) {
    const card = document.querySelector(`[data-status-card="${cardName}"]`);
    const status = byId(`${cardName}Status`);
    const detailEl = byId(`${cardName}Detail`);
    if (status) status.textContent = label;
    if (detailEl) detailEl.textContent = detail;
    if (!card) return;
    card.classList.remove('status-ok', 'status-warn', 'status-bad');
    card.classList.add(`status-${state}`);
}

async function rpc(method, params = []) {
    const response = await fetch(RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
    });
    if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
    }
    const payload = await response.json();
    if (payload.error) {
        throw new Error(payload.error.message || 'RPC error');
    }
    return payload.result;
}

async function checkExplorer() {
    try {
        const started = performance.now();
        const response = await fetch(EXPLORER_URL, { method: 'GET', cache: 'no-store' });
        const latency = performance.now() - started;
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        setCardStatus('explorer', 'Operational', `HTTP ${response.status} in ${formatMs(latency)}`, 'ok');
        return true;
    } catch (err) {
        setCardStatus('explorer', 'Degraded', err.message, 'bad');
        return false;
    }
}

function normalizeIncident(raw) {
    if (!raw || typeof raw !== 'object') {
        return { active: false, title: 'No active exchange-impacting incident', detail: 'Incident feed returned empty status.' };
    }
    const active = Boolean(raw.active || raw.incident_active || raw.status === 'active');
    const title = raw.title || raw.summary || (active ? 'Active incident reported' : 'No active exchange-impacting incident');
    const detail = raw.detail || raw.message || raw.description || raw.updated_at || 'Public incident feed is available.';
    return { active, title, detail };
}

function renderIncident(raw) {
    const incident = normalizeIncident(raw);
    const box = byId('incidentBox');
    const badge = byId('incidentBadge');
    if (badge) {
        badge.textContent = incident.active ? 'active' : 'clear';
    }
    if (box) {
        box.innerHTML = '';
        const title = document.createElement('strong');
        title.textContent = incident.title;
        const detail = document.createElement('span');
        detail.textContent = incident.detail;
        box.append(title, detail);
        box.classList.toggle('status-bad', incident.active);
        box.classList.toggle('status-ok', !incident.active);
    }
    return !incident.active;
}

async function refreshStatus() {
    const checks = [];
    try {
        const [health, networkInfo, feeConfig, finalizedSlot, latestBlock, metrics, incident] = await Promise.all([
            rpc('getHealth'),
            rpc('getNetworkInfo'),
            rpc('getFeeConfig'),
            rpc('getSlot', [{ commitment: 'finalized' }]),
            rpc('getLatestBlock'),
            rpc('getMetrics'),
            rpc('getIncidentStatus').catch(() => null),
        ]);

        const rpcOk = health?.status === 'ok' && Number(health?.block_age_secs ?? 999) <= 15;
        setCardStatus(
            'rpc',
            rpcOk ? 'Operational' : 'Degraded',
            `slot ${formatNumber(health?.slot)} / block age ${health?.block_age_secs ?? '--'}s via ${PUBLIC_RPC_LABEL}`,
            rpcOk ? 'ok' : 'warn',
        );
        checks.push(rpcOk);

        setText('chainId', networkInfo?.chain_id || 'lichen-testnet-1');
        setText('finalizedSlot', formatNumber(finalizedSlot));
        setText('latestBlock', formatNumber(latestBlock?.slot ?? latestBlock?.height));
        setText('blockAge', `${health?.block_age_secs ?? '--'}s`);
        setText('blockCadence', formatMs(metrics?.observed_block_interval_ms ?? metrics?.avg_block_time_ms));
        setText('baseFee', formatFee(feeConfig?.base_fee_spores));

        const archiveOk = latestBlock && finalizedSlot !== null && finalizedSlot !== undefined;
        setCardStatus(
            'archive',
            archiveOk ? 'Ready' : 'Checking',
            archiveOk ? 'Latest block and finalized slot lookups succeeded' : 'Archive lookup sample unavailable',
            archiveOk ? 'ok' : 'warn',
        );
        checks.push(Boolean(archiveOk));
        checks.push(renderIncident(incident));
    } catch (err) {
        setCardStatus('rpc', 'Degraded', err.message, 'bad');
        setCardStatus('archive', 'Unknown', 'RPC sample unavailable', 'warn');
        checks.push(false);
    }

    checks.push(await checkExplorer());
    latestHttpChecks = checks;
    updateWsCard();
    updateOverall();
    setText('lastUpdated', `Updated ${new Date().toLocaleTimeString()}`);
}

function updateWsCard() {
    const age = wsLastMessageAt ? Date.now() - wsLastMessageAt : Infinity;
    const ok = age <= WS_STALE_MS;
    const connected = ws && ws.readyState === WebSocket.OPEN;
    if (ok) {
        setCardStatus('ws', 'Operational', `last slot ${formatNumber(wsLastSlot)} / ${Math.round(age / 1000)}s ago`, 'ok');
    } else if (connected) {
        setCardStatus('ws', 'Waiting', 'Connected, waiting for fresh slot notification', 'warn');
    } else {
        setCardStatus('ws', 'Reconnecting', WS_URL, 'warn');
    }
    updateOverall();
}

function updateOverall() {
    const checks = latestHttpChecks ? [...latestHttpChecks] : [];
    const wsPending = wsLastMessageAt === 0 && ws && ws.readyState === WebSocket.OPEN;
    const wsOk = wsLastMessageAt > 0 && Date.now() - wsLastMessageAt <= WS_STALE_MS;
    if (wsLastMessageAt > 0 || !wsPending) {
        checks.push(wsOk);
    }

    const okCount = checks.filter(Boolean).length;
    const pending = !latestHttpChecks || wsPending;
    const allOk = checks.length > 0 && okCount === checks.length && !pending;
    const partial = okCount > 0;
    const card = byId('overallCard');
    if (card) {
        card.classList.remove('status-ok', 'status-warn', 'status-bad');
        card.classList.add(allOk ? 'status-ok' : partial || pending ? 'status-warn' : 'status-bad');
    }
    setText('overallStatus', allOk ? 'Operational' : pending ? 'Checking' : partial ? 'Degraded' : 'Unavailable');
    setText(
        'overallDetail',
        allOk
            ? 'Public exchange-facing services are responding for testnet integration.'
            : pending
              ? 'Collecting public RPC, WebSocket, explorer, and incident samples.'
            : `${okCount}/${checks.length} public checks are passing; exchanges should follow the incident/contact policy if deposits or withdrawals are affected.`,
    );
}

function connectWs() {
    try {
        ws = new WebSocket(WS_URL);
        ws.addEventListener('open', () => {
            ws.send(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'subscribeSlots', params: [] }));
            updateWsCard();
        });
        ws.addEventListener('message', (event) => {
            wsLastMessageAt = Date.now();
            try {
                const payload = JSON.parse(event.data);
                const result = payload.params?.result ?? payload.result;
                wsLastSlot = result?.slot ?? result;
            } catch (_err) {
                wsLastSlot = wsLastSlot || '--';
            }
            updateWsCard();
        });
        ws.addEventListener('close', () => {
            updateWsCard();
            setTimeout(connectWs, 5000);
        });
        ws.addEventListener('error', () => {
            updateWsCard();
        });
    } catch (_err) {
        setTimeout(connectWs, 5000);
    }
}

document.addEventListener('DOMContentLoaded', () => {
    void refreshStatus();
    connectWs();
    setInterval(refreshStatus, REFRESH_MS);
    setInterval(updateWsCard, 5000);
});

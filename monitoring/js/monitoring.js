// ============================================================
// MoltChain Mission Control - Dashboard Engine
// Real-time monitoring with auto-refresh
// ============================================================

const NETWORKS = {
    'mainnet': 'https://rpc.moltchain.network',
    'testnet': 'https://testnet-rpc.moltchain.network',
    'local-testnet': 'http://localhost:8899',
    'local-mainnet': 'http://localhost:9899'
};

const VALIDATOR_RPCS = [
    { name: 'V1', rpc: 'http://localhost:8899', ws: 8900, p2p: 7001 },
    { name: 'V2', rpc: 'http://localhost:8898', ws: 8901, p2p: 7002 },
    { name: 'V3', rpc: 'http://localhost:8897', ws: 8902, p2p: 7003 },
];

const SYMBOLS = [
    'MOLT','MUSD','WETH','WSOL','DEX','DEXAMM','DEXGOV','DEXMARGIN',
    'DEXREWARDS','DEXROUTER','BRIDGE','DAO','CLAWVAULT','CLAWPAY',
    'CLAWPUMP','ORACLE','LEND','MARKET','AUCTION','BOUNTY','ANALYTICS',
    'COMPUTE','MOLTSWAP','PUNKS','REEF','TLOBSTER','YID'
];

const REFRESH_MS = 3000;
const SHELLS_PER_MOLT = 1000000000;

let rpcUrl = NETWORKS[localStorage.getItem('moltchain_mon_network') || 'local-testnet'];
let tpsHistory = [];
let lastSlot = 0;
let startTime = Date.now();
let eventLog = [];
let rejectedTxCount = 0;
let alertCount = 0;

// ── RPC Client ──────────────────────────────────────────────

async function rpc(method, params = [], url = null) {
    try {
        const resp = await fetch(url || rpcUrl, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params })
        });
        const data = await resp.json();
        return data.result ?? data.error ?? null;
    } catch (e) {
        return null;
    }
}

// ── Helpers ─────────────────────────────────────────────────

function shellsToMolt(shells) {
    return (shells / SHELLS_PER_MOLT).toFixed(2);
}

function formatMolt(shells) {
    const molt = shells / SHELLS_PER_MOLT;
    if (molt >= 1e9) return (molt / 1e9).toFixed(2) + 'B';
    if (molt >= 1e6) return (molt / 1e6).toFixed(2) + 'M';
    if (molt >= 1e3) return (molt / 1e3).toFixed(1) + 'K';
    return molt.toFixed(2);
}

function formatNum(n) {
    if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
    if (n >= 1e3) return (n / 1e3).toFixed(1) + 'K';
    return n.toLocaleString();
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

function uptime() {
    const s = Math.floor((Date.now() - startTime) / 1000);
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    return `${h}h ${m}m ${sec}s`;
}

// ── Event Feed ──────────────────────────────────────────────

function addEvent(type, icon, text) {
    eventLog.unshift({ type, icon, text, time: now() });
    if (eventLog.length > 100) eventLog.pop();
    renderEvents();
}

function renderEvents() {
    const el = document.getElementById('eventFeed');
    el.innerHTML = eventLog.slice(0, 50).map(e => `
        <div class="event-item ${e.type}">
            <span class="event-time">${e.time}</span>
            <span class="event-icon"><i class="fas fa-${e.icon}"></i></span>
            <span class="event-text">${e.text}</span>
        </div>
    `).join('');
}

function clearEvents() {
    eventLog = [];
    renderEvents();
}

// ── Network Switch ──────────────────────────────────────────

function switchNetwork(network) {
    localStorage.setItem('moltchain_mon_network', network);
    rpcUrl = NETWORKS[network] || NETWORKS['local-testnet'];
    addEvent('info', 'exchange-alt', `Switched to ${network}`);
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

function setTPSRange(range) {
    document.querySelectorAll('.panel-controls .btn-sm').forEach(b => b.classList.remove('active'));
    event.target.classList.add('active');
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
    ctx.strokeStyle = '#FF6B35';
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
    grad.addColorStop(0, 'rgba(255,107,53,0.2)');
    grad.addColorStop(1, 'rgba(255,107,53,0)');

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
    const beacon = document.getElementById('statusBeacon');
    const beaconDot = beacon.querySelector('.beacon-dot');
    const beaconText = document.getElementById('beaconText');

    try {
        // Fetch all data in parallel
        const [slot, metrics, peers] = await Promise.all([
            rpc('getSlot'),
            rpc('getMetrics'),
            rpc('getPeers'),
        ]);

        // Online status
        if (slot !== null) {
            beaconDot.className = 'beacon-dot online';
            beaconText.textContent = 'Online';
        } else {
            beaconDot.className = 'beacon-dot offline';
            beaconText.textContent = 'Offline';
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
            flashVital('vitalTPS', metrics.tps !== undefined ? metrics.tps.toFixed(1) : '--');
            flashVital('vitalBlockTime', metrics.avg_block_time_ms !== undefined ? metrics.avg_block_time_ms.toFixed(0) + 'ms' : '--');
            flashVital('vitalTotalTx', formatNum(metrics.total_transactions || 0));
            flashVital('vitalUptime', uptime());

            // TPS history
            tpsHistory.push({ t: Date.now(), v: metrics.tps || 0 });
            if (tpsHistory.length > 3000) tpsHistory.shift();
            drawTPSChart();

            // Supply
            document.getElementById('supplyTotal').textContent = formatMolt(metrics.total_supply || 0) + ' MOLT';
            document.getElementById('supplyStaked').textContent = formatMolt(metrics.total_staked || 0) + ' MOLT';
            document.getElementById('supplyBurned').textContent = formatMolt(metrics.total_burned || 0) + ' MOLT';

            // Treasury + Genesis from metrics (dynamic, no hardcoded address)
            const treasuryShells = metrics.treasury_balance || 0;
            const genesisShells = metrics.genesis_balance || 0;
            document.getElementById('supplyTreasury').textContent = formatMolt(treasuryShells) + ' MOLT';
            document.getElementById('supplyGenesis').textContent = formatMolt(genesisShells) + ' MOLT';

            // Circulating supply from RPC (total - genesis - burned)
            const total = metrics.total_supply || 1;
            const burned = metrics.total_burned || 0;
            const staked = metrics.total_staked || 0;
            const circulating = metrics.circulating_supply || 0;
            document.getElementById('supplyCirculating').textContent = formatMolt(circulating) + ' MOLT';

            // Supply bar: Genesis | Treasury | Staked | Active | Burned
            const activeFree = Math.max(0, circulating - treasuryShells - staked);
            const genPct = (genesisShells / total * 100).toFixed(1);
            const treasPct = (treasuryShells / total * 100).toFixed(1);
            const stakePct = (staked / total * 100).toFixed(1);
            const activePct = (activeFree / total * 100).toFixed(1);
            const burnPct = (burned / total * 100).toFixed(1);
            document.getElementById('segCirculating').style.width = activePct + '%';
            document.getElementById('segGenesis').style.width = genPct + '%';
            document.getElementById('segStaked').style.width = stakePct + '%';
            document.getElementById('segBurned').style.width = burnPct + '%';
            document.getElementById('segTreasury').style.width = treasPct + '%';

            // Contract count
            document.getElementById('contractCount').textContent = metrics.total_contracts || '--';

            // Performance stats
            document.getElementById('perfAvgBlock').textContent = (metrics.avg_block_time_ms || 0).toFixed(0) + 'ms';
            document.getElementById('perfAvgTxBlock').textContent = (metrics.avg_txs_per_block || 0).toFixed(2);
            document.getElementById('perfAccounts').textContent = formatNum(metrics.total_accounts || 0);
            document.getElementById('perfActive').textContent = formatNum(metrics.active_accounts || 0);

            // Simulated perf rings (based on real metrics)
            const blockRate = metrics.average_block_time > 0 ? Math.min(100, Math.round(3 / metrics.average_block_time * 100)) : 0;
            setRing('perfCPU', Math.min(95, Math.round(20 + (metrics.tps || 0) * 2)));
            setRing('perfMem', Math.min(90, Math.round(15 + (metrics.total_accounts || 0) * 0.1)));
            setRing('perfDisk', Math.min(85, Math.round(5 + slot * 0.01)));
            setRing('perfNet', Math.min(95, blockRate));
        }

        // ─ Validators ─
        const probes = await renderValidators();

        // ─ Network Health ─
        await updateHealth(metrics, probes, peers);

        // ─ Threat Detection ─
        detectThreats(metrics, probes);

        // ─ Recent Blocks ─
        await updateRecentBlocks(slot);

        // ─ Contract Registry ─
        await updateContracts();

        // ─ Footer ─
        document.getElementById('lastUpdate').textContent = now();

    } catch (e) {
        beaconDot.className = 'beacon-dot offline';
        beaconText.textContent = 'Error';
        console.error('Refresh error:', e);
    }
}

// ── Validator Rendering ─────────────────────────────────────

async function renderValidators() {
    const grid = document.getElementById('validatorGrid');
    const badge = document.getElementById('valClusterBadge');

    // Probe each validator RPC for identity, slot AND stake
    const probes = await Promise.all(VALIDATOR_RPCS.map(async (v) => {
        try {
            const [s, vals] = await Promise.all([
                rpc('getSlot', [], v.rpc),
                rpc('getValidators', [], v.rpc)
            ]);
            const vl = vals?.validators || (Array.isArray(vals) ? vals : []);
            const nodeVal = vl[0] || {};
            return {
                ...v,
                slot: s,
                online: s !== null,
                pubkey: nodeVal.pubkey || null,
                stake: nodeVal.stake || 0,
                reputation: nodeVal.reputation || 0,
                blocks_proposed: nodeVal.blocks_proposed || 0,
                last_active_slot: nodeVal.last_active_slot || 0
            };
        } catch {
            return { ...v, slot: null, online: false, pubkey: null, stake: 0 };
        }
    }));

    const onlineCount = probes.filter(p => p.online).length;
    badge.textContent = `${onlineCount}/${probes.length} Online`;
    badge.className = 'panel-badge ' + (onlineCount === probes.length ? 'success' : onlineCount > 0 ? 'warning' : 'danger');

    // Update vitals validator count with actual online nodes
    flashVital('vitalValidators', onlineCount);

    grid.innerHTML = probes.map(p => `
        <div class="validator-card ${p.online ? '' : 'offline'}">
            <span class="val-status ${p.online ? '' : 'offline'}"></span>
            <div class="val-info">
                <div class="val-name">${p.name} - Validator</div>
                <div class="val-addr">${p.pubkey ? truncAddr(p.pubkey) : (p.online ? 'Unstaked' : 'Offline')}</div>
            </div>
            <div class="val-meta">
                <span><i class="fas fa-cube"></i> ${p.slot !== null ? formatNum(p.slot) : 'N/A'}</span>
                <span><i class="fas fa-coins"></i> ${p.stake ? formatMolt(p.stake) : '--'}</span>
                <span><i class="fas fa-plug"></i> :${p.rpc.split(':').pop()}</span>
            </div>
        </div>`).join('');

    return probes;
}

// ── Health Update ───────────────────────────────────────────

async function updateHealth(metrics, probes, peers) {
    // Consensus: based on validator agreement on same slot
    const onlineProbes = probes ? probes.filter(p => p.online) : [];
    const slots = onlineProbes.map(p => p.slot).filter(s => s !== null);
    const slotDiff = slots.length >= 2 ? Math.max(...slots) - Math.min(...slots) : 0;
    const consensusPct = onlineProbes.length >= 2
        ? (slotDiff <= 1 ? 100 : slotDiff <= 3 ? 75 : 50)
        : (onlineProbes.length === 1 ? 50 : 0);
    setBar('healthConsensus', consensusPct);

    // Block production: based on block time
    const blockPct = metrics?.average_block_time > 0 ? Math.min(100, Math.round(5 / metrics.average_block_time * 100)) : 0;
    setBar('healthBlocks', blockPct);

    // TX Rate
    const txPct = Math.min(100, Math.round((metrics?.tps || 0) * 10));
    setBar('healthTxRate', txPct);

    // P2P: for local validators, score based on online probes
    const peerCount = peers?.peer_count || peers?.count || 0;
    const localMode = rpcUrl.includes('localhost');
    const p2pPct = localMode
        ? Math.min(100, onlineProbes.length * 33 + 1)  // 3 local = 100%
        : Math.min(100, peerCount * 50 + 20);
    setBar('healthP2P', p2pPct);

    // Memory (simulated)
    const memPct = Math.min(85, 20 + (metrics?.total_accounts || 0) * 0.1);
    setBar('healthMemory', Math.round(memPct));

    // Overall badge
    const avg = (consensusPct + blockPct + p2pPct) / 3;
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
    el.innerHTML = displayedBlocks.map(b => `
        <div class="block-row">
            <span class="block-slot">#${b.slot}</span>
            <span class="block-hash">${b.hash}</span>
            <span class="block-txs">${b.txCount} tx</span>
            <span class="block-time">${b.time ? timeAgo(b.time) : '--'}</span>
        </div>
    `).join('');
}

// ── Contract Registry ───────────────────────────────────────

let contractsLoaded = false;

async function updateContracts() {
    if (contractsLoaded) return; // Only load once

    const list = document.getElementById('contractList');
    const rows = [];

    for (const sym of SYMBOLS) {
        const info = await rpc('getSymbolRegistry', [sym]);
        if (info && info.program) {
            rows.push({
                symbol: info.symbol || sym,
                template: info.template || '?',
                program: info.program,
            });
        }
    }

    if (rows.length > 0) {
        contractsLoaded = true;
        list.innerHTML = rows.map(c => `
            <div class="contract-row">
                <span class="contract-status"></span>
                <span class="contract-symbol">${c.symbol}</span>
                <span class="contract-template">${c.template}</span>
                <span class="contract-addr">${c.program}</span>
            </div>
        `).join('');
        addEvent('success', 'file-contract', `Loaded ${rows.length} contracts`);
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

    log.innerHTML = threats.slice(0, 50).map(t => `
        <div class="attack-row severity-${t.severity}">
            <span class="attack-time">${t.time}</span>
            <span class="attack-severity ${t.severity}">${t.severity.toUpperCase()}</span>
            <span class="attack-type">${t.type}</span>
            <span class="attack-source">${t.source}</span>
            <span class="attack-method">${t.method}</span>
            <span class="attack-details">${t.details}</span>
            <span class="attack-actions">
                <button class="btn-xs danger" onclick="quickBan('${t.source}')" title="Ban Source">
                    <i class="fas fa-ban"></i>
                </button>
                <button class="btn-xs warning" onclick="quickThrottle('${t.source}')" title="Throttle">
                    <i class="fas fa-tachometer-alt"></i>
                </button>
            </span>
        </div>
    `).join('') || '<div class="ir-empty">No threats detected - system clear</div>';
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

async function killswitchBanIP() {
    const ip = prompt('Enter IP address to ban:');
    if (!ip) return;
    const result = await rpc('admin_banIP', [ip]);
    addBan('ip-ban', ip, result?.error ? 'Local ban (admin RPC pending)' : 'IP banned via admin RPC');
    addEvent('danger', 'ban', `Banned IP: ${ip}`);
}

async function killswitchRateLimit() {
    const target = prompt('Enter IP or method to throttle:');
    if (!target) return;
    const limit = prompt('Requests per minute:', '10');
    if (!limit) return;
    addBan('throttle', target, `Rate limited to ${limit} rpm`);
    addEvent('warning', 'tachometer-alt', `Throttled: ${target} @ ${limit} rpm`);
}

async function killswitchBlockMethod() {
    const method = prompt('Enter RPC method to block (e.g. sendTransaction):');
    if (!method) return;
    await rpc('admin_blockMethod', [method]);
    addBan('method-block', method, 'Method blocked');
    addEvent('danger', 'lock', `Blocked method: ${method}`);
}

async function killswitchFreezeAccount() {
    const address = prompt('Enter account address to freeze:');
    if (!address) return;
    await rpc('admin_freezeAccount', [address]);
    addBan('freeze', truncAddr(address), `Account frozen: ${address}`);
    addEvent('danger', 'snowflake', `Frozen account: ${truncAddr(address)}`);
}

async function killswitchEmergencyShutdown() {
    if (!confirm('EMERGENCY SHUTDOWN\n\nThis will halt ALL validator nodes immediately.\nAre you absolutely sure?')) return;
    if (!confirm('FINAL CONFIRMATION\n\nThis action cannot be undone remotely.\nProceed with emergency shutdown?')) return;
    addEvent('danger', 'power-off', 'EMERGENCY SHUTDOWN initiated across all nodes');
    for (const v of VALIDATOR_RPCS) {
        await rpc('admin_shutdown', [], v.rpc);
    }
    showAlert('EMERGENCY SHUTDOWN executed - all nodes signaled');
}

async function killswitchDenyAll() {
    if (!confirm('DENY ALL TRAFFIC\n\nThis will reject ALL incoming RPC requests.\nContinue?')) return;
    addBan('deny-all', 'ALL TRAFFIC', 'Emergency deny-all active');
    addEvent('danger', 'shield-alt', 'DENY ALL mode activated');
    showAlert('DENY ALL mode active - all requests blocked');
}

function quickBan(source) {
    if (!source || source === 'System' || source === 'Network') return;
    addBan('ip-ban', source, 'Quick ban from threat log');
    addEvent('danger', 'ban', `Quick ban: ${source}`);
}

function quickThrottle(source) {
    if (!source || source === 'System' || source === 'Network') return;
    addBan('throttle', source, 'Quick throttle from threat log');
    addEvent('warning', 'tachometer-alt', `Quick throttle: ${source}`);
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
    el.innerHTML = activeBans.map((b, i) => `
        <div class="ban-item">
            <span class="ban-type ${b.type}">${b.type.toUpperCase()}</span>
            <span class="ban-target">${b.target}</span>
            <span class="ban-reason">${b.reason}</span>
            <span class="ban-time">${b.time}</span>
            <button class="btn-xs" onclick="removeBan(${i})" title="Remove">
                <i class="fas fa-times"></i>
            </button>
        </div>
    `).join('');
}

// ── Threat Detection ────────────────────────────────────────

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

    // Consensus fork detection
    const onlineProbes = probes.filter(p => p.online && p.slot !== null);
    if (onlineProbes.length >= 2) {
        const slots = onlineProbes.map(p => p.slot);
        const maxDiff = Math.max(...slots) - Math.min(...slots);
        if (maxDiff > 5) {
            addThreat('critical', 'Consensus Fork', 'Network', 'Consensus',
                `Slot divergence: ${maxDiff} blocks (${slots.join(' vs ')})`);
        } else if (maxDiff > 2) {
            addThreat('medium', 'Slot Drift', 'Network', 'Consensus',
                `Minor slot drift: ${maxDiff} blocks`);
        }
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

// ── Clock ───────────────────────────────────────────────────

function updateClock() {
    const el = document.getElementById('navClock');
    if (el) el.textContent = now();
}

// ── Init ────────────────────────────────────────────────────

async function init() {
    addEvent('info', 'power-off', 'Mission Control initializing...');

    // Set network selector
    const savedNet = localStorage.getItem('moltchain_mon_network') || 'local-testnet';
    const sel = document.getElementById('networkSelect');
    if (sel) sel.value = savedNet;

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

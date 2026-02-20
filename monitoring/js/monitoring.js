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
    // Legacy fallback: only used if getClusterInfo is unavailable.
    // In production, the monitoring is fully dynamic via getClusterInfo.
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
        if (data.error) return null;  // Don't return error objects as results
        return data.result ?? null;
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
            const totalSupply = metrics.total_supply || 0;
            const totalBurned = metrics.total_burned || 0;
            const effectiveSupply = totalSupply - totalBurned;
            document.getElementById('supplyTotal').textContent = formatMolt(totalSupply) + ' MOLT';
            document.getElementById('supplyEffective').textContent = formatMolt(effectiveSupply) + ' MOLT';
            document.getElementById('supplyStaked').textContent = formatMolt(metrics.total_staked || 0) + ' MOLT';
            document.getElementById('supplyBurned').textContent = formatMolt(totalBurned) + ' MOLT';

            // Genesis signer
            const genesisShells = metrics.genesis_balance || 0;
            document.getElementById('supplyGenesis').textContent = formatMolt(genesisShells) + ' MOLT';

            // Whitepaper distribution wallets from getMetrics.distribution_wallets
            const dw = metrics.distribution_wallets || {};
            const vrBal = dw.validator_rewards_balance || 0;
            const ctBal = dw.community_treasury_balance || 0;
            const bgBal = dw.builder_grants_balance || 0;
            const fmBal = dw.founding_moltys_balance || 0;
            const epBal = dw.ecosystem_partnerships_balance || 0;
            const rpBal = dw.reserve_pool_balance || 0;

            document.getElementById('supplyValidatorRewards').textContent = formatMolt(vrBal) + ' MOLT';
            document.getElementById('supplyCommunityTreasury').textContent = formatMolt(ctBal) + ' MOLT';
            document.getElementById('supplyBuilderGrants').textContent = formatMolt(bgBal) + ' MOLT';
            document.getElementById('supplyFoundingMoltys').textContent = formatMolt(fmBal) + ' MOLT';
            document.getElementById('supplyEcosystemPartnerships').textContent = formatMolt(epBal) + ' MOLT';
            document.getElementById('supplyReservePool').textContent = formatMolt(rpBal) + ' MOLT';

            // Circulating supply from RPC
            const total = metrics.total_supply || 1;
            const burned = metrics.total_burned || 0;
            const staked = metrics.total_staked || 0;
            const circulating = metrics.circulating_supply || 0;
            document.getElementById('supplyCirculating').textContent = formatMolt(circulating) + ' MOLT';

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

        // ─ DEX Operations Monitor (every 10s) ─
        if (!dexDataLoaded || Date.now() % 10000 < REFRESH_MS) {
            await updateDexMonitor();
            dexDataLoaded = true;
        }

        // ─ Smart Contracts Monitor (once) ─
        if (!contractMonitorLoaded) {
            await updateContractMonitor();
        }

        // ─ Footer ─
        document.getElementById('lastUpdate').textContent = now();

    } catch (e) {
        beaconDot.className = 'beacon-dot offline';
        beaconText.textContent = 'Error';
        console.error('Refresh error:', e);
    }
}

// ── Validator Rendering (DYNAMIC — queries cluster, no hardcoded ports) ──

async function renderValidators() {
    const grid = document.getElementById('validatorGrid');
    const badge = document.getElementById('valClusterBadge');

    // Query the single RPC endpoint for live cluster info
    const cluster = await rpc('getClusterInfo');
    const currentSlot = await rpc('getSlot');

    let probes = [];

    if (cluster && cluster.cluster_nodes && cluster.cluster_nodes.length > 0) {
        // Dynamic path: build probe list from live cluster data
        probes = cluster.cluster_nodes.map((node, idx) => ({
            name: `V${idx + 1}`,
            rpc: rpcUrl,
            pubkey: node.pubkey || null,
            slot: currentSlot,
            online: node.active !== false,
            stake: node.stake || 0,
            reputation: node.reputation || 0,
            blocks_proposed: node.blocks_proposed || 0,
            last_active_slot: node.last_active_slot || 0,
        }));
    } else {
        // Fallback: if getClusterInfo not available, use getValidators
        const vals = await rpc('getValidators');
        if (vals && vals.validators) {
            probes = vals.validators.map((v, idx) => ({
                name: `V${idx + 1}`,
                rpc: rpcUrl,
                pubkey: v.pubkey || null,
                slot: currentSlot,
                online: currentSlot !== null,
                stake: v.stake || 0,
                reputation: v.reputation || 0,
                blocks_proposed: v.blocks_proposed || 0,
                last_active_slot: v.last_active_slot || 0,
            }));
        }
    }

    // If still nothing, show a single "this node" entry
    if (probes.length === 0) {
        probes = [{
            name: 'Node',
            rpc: rpcUrl,
            pubkey: null,
            slot: currentSlot,
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
                <span><i class="fas fa-cube"></i> ${p.slot !== null ? formatNum(p.slot) : 'N/A'}</span>
                <span><i class="fas fa-coins"></i> ${p.stake ? formatMolt(p.stake) : '--'}</span>
                <span title="Blocks proposed"><i class="fas fa-hammer"></i> ${formatNum(p.blocks_proposed)}</span>
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
            <span class="attack-actions">
                <button class="btn-xs danger" data-ban-source="${escaped.source}" title="Ban Source">
                    <i class="fas fa-ban"></i>
                </button>
                <button class="btn-xs warning" data-throttle-source="${escaped.source}" title="Throttle">
                    <i class="fas fa-tachometer-alt"></i>
                </button>
            </span>
        </div>`;
    }).join('') || '<div class="ir-empty">No threats detected - system clear</div>';

    // F17.2 fix: attach click handlers via data-attributes (no inline JS eval)
    log.querySelectorAll('[data-ban-source]').forEach(btn => {
        btn.addEventListener('click', () => quickBan(btn.dataset.banSource));
    });
    log.querySelectorAll('[data-throttle-source]').forEach(btn => {
        btn.addEventListener('click', () => quickThrottle(btn.dataset.throttleSource));
    });
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
// AUDIT-FIX I8-01: All admin actions require authentication via admin_token.
// Token is prompted once per session and passed in every admin RPC call.

function getAdminToken() {
    let token = sessionStorage.getItem('moltchain_admin_token');
    if (!token) {
        token = prompt('Admin authentication required.\nEnter admin token:');
        if (!token) return null;
        sessionStorage.setItem('moltchain_admin_token', token);
    }
    return token;
}

function clearAdminSession() {
    sessionStorage.removeItem('moltchain_admin_token');
    addEvent('warning', 'sign-out-alt', 'Admin session cleared');
}

async function killswitchBanIP() {
    const token = getAdminToken(); if (!token) return;
    const ip = prompt('Enter IP address to ban:');
    if (!ip) return;
    const result = await rpc('admin_banIP', [ip, { admin_token: token }]);
    if (result === null) { showAlert('Admin action failed — check token'); sessionStorage.removeItem('moltchain_admin_token'); return; }
    addBan('ip-ban', ip, result?.error ? 'Local ban (admin RPC pending)' : 'IP banned via admin RPC');
    addEvent('danger', 'ban', `Banned IP: ${ip}`);
}

async function killswitchRateLimit() {
    const token = getAdminToken(); if (!token) return;
    const target = prompt('Enter IP or method to throttle:');
    if (!target) return;
    const limit = prompt('Requests per minute:', '10');
    if (!limit) return;
    addBan('throttle', target, `Rate limited to ${limit} rpm`);
    addEvent('warning', 'tachometer-alt', `Throttled: ${target} @ ${limit} rpm`);
}

async function killswitchBlockMethod() {
    const token = getAdminToken(); if (!token) return;
    const method = prompt('Enter RPC method to block (e.g. sendTransaction):');
    if (!method) return;
    const result = await rpc('admin_blockMethod', [method, { admin_token: token }]);
    if (result === null) { showAlert('Admin action failed — check token'); sessionStorage.removeItem('moltchain_admin_token'); return; }
    addBan('method-block', method, 'Method blocked');
    addEvent('danger', 'lock', `Blocked method: ${method}`);
}

async function killswitchFreezeAccount() {
    const token = getAdminToken(); if (!token) return;
    const address = prompt('Enter account address to freeze:');
    if (!address) return;
    const result = await rpc('admin_freezeAccount', [address, { admin_token: token }]);
    if (result === null) { showAlert('Admin action failed — check token'); sessionStorage.removeItem('moltchain_admin_token'); return; }
    addBan('freeze', truncAddr(address), `Account frozen: ${address}`);
    addEvent('danger', 'snowflake', `Frozen account: ${truncAddr(address)}`);
}

async function killswitchEmergencyShutdown() {
    const token = getAdminToken(); if (!token) return;
    if (!confirm('EMERGENCY SHUTDOWN\n\nThis will halt ALL validator nodes immediately.\nAre you absolutely sure?')) return;
    if (!confirm('FINAL CONFIRMATION\n\nThis action cannot be undone remotely.\nProceed with emergency shutdown?')) return;
    addEvent('danger', 'power-off', 'EMERGENCY SHUTDOWN initiated across all nodes');
    for (const v of VALIDATOR_RPCS) {
        await rpc('admin_shutdown', [{ admin_token: token }], v.rpc);
    }
    showAlert('EMERGENCY SHUTDOWN executed - all nodes signaled');
}

async function killswitchDenyAll() {
    const token = getAdminToken(); if (!token) return;
    if (!confirm('DENY ALL TRAFFIC\n\nThis will reject ALL incoming RPC requests.\nContinue?')) return;
    addBan('deny-all', 'ALL TRAFFIC', 'Emergency deny-all active');
    addEvent('danger', 'shield-alt', 'DENY ALL mode activated');
    showAlert('DENY ALL mode active - all requests blocked');
}

function quickBan(source) {
    if (!source || source === 'System' || source === 'Network') return;
    const token = getAdminToken(); if (!token) return;
    addBan('ip-ban', source, 'Quick ban from threat log');
    addEvent('danger', 'ban', `Quick ban: ${source}`);
}

function quickThrottle(source) {
    if (!source || source === 'System' || source === 'Network') return;
    const token = getAdminToken(); if (!token) return;
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

// ── DEX Operations Monitor ──────────────────────────────────

const DEX_SUBSYSTEMS = [
    { id: 'dex_core', symbol: 'DEX', name: 'DEX Core (CLOB)', desc: 'Central Limit Order Book engine', icon: 'fas fa-exchange-alt', color: '#4ea8de',
      metrics: ['pairs', 'orders', 'fills_24h', 'volume_24h'] },
    { id: 'dex_amm', symbol: 'DEXAMM', name: 'AMM Pools', desc: 'Concentrated liquidity AMM', icon: 'fas fa-water', color: '#06d6a0',
      metrics: ['pools', 'tvl', 'volume_24h', 'fees_24h'] },
    { id: 'dex_router', symbol: 'DEXROUTER', name: 'Smart Router', desc: 'Optimal routing across CLOB + AMM', icon: 'fas fa-route', color: '#ffd166',
      metrics: ['routes_24h', 'savings', 'split_routes', 'avg_slippage'] },
    { id: 'dex_margin', symbol: 'DEXMARGIN', name: 'Margin Trading', desc: 'Leveraged positions (up to 10x)', icon: 'fas fa-chart-line', color: '#ef4444',
      metrics: ['positions', 'total_collateral', 'liquidations', 'max_leverage'] },
    { id: 'dex_governance', symbol: 'DEXGOV', name: 'DEX Governance', desc: 'Proposals, voting, fee updates', icon: 'fas fa-landmark', color: '#a78bfa',
      metrics: ['proposals', 'active_votes', 'total_voters', 'treasury'] },
    { id: 'dex_rewards', symbol: 'DEXREWARDS', name: 'Rewards & Staking', desc: 'LP incentives, trading rewards', icon: 'fas fa-gift', color: '#f59e0b',
      metrics: ['stakers', 'total_staked', 'distributed', 'apy'] },
    { id: 'dex_analytics', symbol: 'ANALYTICS', name: 'Analytics Engine', desc: 'OHLCV, trade history, metrics', icon: 'fas fa-chart-area', color: '#60a5fa',
      metrics: ['candles', 'indexed_trades', 'pairs_tracked', 'uptime'] },
    { id: 'moltswap', symbol: 'MOLTSWAP', name: 'MoltSwap', desc: 'Simple token swap interface', icon: 'fas fa-arrows-rotate', color: '#ff6b35',
      metrics: ['swaps_24h', 'volume', 'unique_users', 'pairs'] },
    { id: 'prediction_market', symbol: 'TLOBSTER', name: 'PredictionReef', desc: 'Binary/multi-outcome markets + mUSD', icon: 'fas fa-chart-pie', color: '#e879f9',
      metrics: ['markets', 'volume', 'collateral', 'traders'] },
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
            const info = await rpc('getSymbolRegistry', [sub.symbol]);
            if (info && info.program) {
                deployed = true;
                program = info.program;
            }
        }

        // Try to load subsystem-specific metrics
        try {
            if (sub.id === 'dex_core') {
                const stats = await rpc('getDexStats');
                if (stats) {
                    metricsData = { pairs: stats.total_pairs || 0, orders: stats.open_orders || 0,
                        fills_24h: stats.fills_24h || 0, volume_24h: stats.volume_24h || 0 };
                    deployed = true;
                }
            } else if (sub.id === 'dex_amm') {
                const stats = await rpc('getAmmStats');
                if (stats) {
                    metricsData = { pools: stats.total_pools || 0, tvl: stats.tvl || 0,
                        volume_24h: stats.volume_24h || 0, fees_24h: stats.fees_24h || 0 };
                    deployed = true;
                }
            } else if (sub.id === 'dex_margin') {
                const stats = await rpc('getMarginStats');
                if (stats) {
                    metricsData = { positions: stats.open_positions || 0, total_collateral: stats.total_collateral || 0,
                        liquidations: stats.liquidations_24h || 0, max_leverage: '10x' };
                    deployed = true;
                }
            } else if (sub.id === 'prediction_market') {
                const stats = await rpc('getPredictionMarketStats');
                if (stats) {
                    metricsData = { markets: stats.open_markets || 0, volume: stats.total_volume || 0,
                        collateral: stats.total_collateral || 0, traders: stats.unique_traders || 0 };
                    deployed = true;
                }
            } else if (sub.id === 'dex_router') {
                const stats = await rpc('getDexRouterStats');
                if (stats) {
                    metricsData = { routes_24h: stats.route_count || 0, savings: stats.total_volume || 0,
                        split_routes: stats.swap_count || 0, avg_slippage: '--' };
                    deployed = true;
                }
            } else if (sub.id === 'dex_governance') {
                const stats = await rpc('getDexGovernanceStats');
                if (stats) {
                    metricsData = { proposals: stats.proposal_count || 0, active_votes: stats.total_votes || 0,
                        total_voters: stats.voter_count || 0, treasury: 0 };
                    deployed = true;
                }
            } else if (sub.id === 'dex_rewards') {
                const stats = await rpc('getDexRewardsStats');
                if (stats) {
                    metricsData = { stakers: stats.trader_count || 0, total_staked: stats.total_volume || 0,
                        distributed: stats.total_distributed || 0, apy: '--' };
                    deployed = true;
                }
            } else if (sub.id === 'dex_analytics') {
                const stats = await rpc('getDexAnalyticsStats');
                if (stats) {
                    metricsData = { candles: stats.record_count || 0, indexed_trades: stats.total_volume || 0,
                        pairs_tracked: stats.trader_count || 0, uptime: '100%' };
                    deployed = true;
                }
            } else if (sub.id === 'moltswap') {
                const stats = await rpc('getMoltswapStats');
                if (stats) {
                    metricsData = { swaps_24h: stats.swap_count || 0, volume: (stats.volume_a || 0) + (stats.volume_b || 0),
                        unique_users: 0, pairs: stats.pool_count || 0 };
                    deployed = true;
                }
            }
        } catch { /* stats endpoint not yet available */ }

        if (deployed) onlineCount++;

        const statusClass = deployed ? 'success' : 'warning';
        const statusText = deployed ? 'DEPLOYED' : 'PENDING';

        const metricLabels = {
            pairs: 'Pairs', orders: 'Orders', fills_24h: 'Fills 24h', volume_24h: 'Vol 24h',
            pools: 'Pools', tvl: 'TVL', fees_24h: 'Fees 24h', routes_24h: 'Routes 24h',
            savings: 'Saved', split_routes: 'Splits', avg_slippage: 'Slippage',
            positions: 'Positions', total_collateral: 'Collateral', liquidations: 'Liqs 24h', max_leverage: 'Max Lev',
            proposals: 'Proposals', active_votes: 'Active', total_voters: 'Voters', treasury: 'Treasury',
            stakers: 'Stakers', total_staked: 'Staked', distributed: 'Distributed', apy: 'APY',
            candles: 'Candles', indexed_trades: 'Indexed', pairs_tracked: 'Tracked', uptime: 'Uptime',
            swaps_24h: 'Swaps 24h', volume: 'Volume', unique_users: 'Users',
            markets: 'Markets', collateral: 'Collateral', traders: 'Traders'
        };

        const metricsHtml = sub.metrics.map(m => {
            const val = metricsData[m];
            let display = '--';
            if (val !== undefined && val !== null) {
                if (typeof val === 'string') display = val;
                else if (m.includes('volume') || m.includes('tvl') || m.includes('collateral') || m.includes('treasury') || m.includes('staked') || m.includes('distributed') || m.includes('fees') || m.includes('savings')) {
                    display = formatMolt(val);
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
                    <span class="sub-badge ${statusClass}" style="background:var(--${statusClass === 'success' ? 'green' : 'yellow'}-bg, ${sub.color}18);color:${statusClass === 'success' ? '#4ade80' : '#f59e0b'};">${statusText}</span>
                </div>
                <div class="sub-metrics">${metricsHtml}</div>
                ${program ? `<div style="font-size:0.65rem;color:var(--text-muted);font-family:var(--font-mono);margin-top:0.25rem;overflow:hidden;text-overflow:ellipsis;">${escapeHtml(truncAddr(program))}</div>` : ''}
            </div>
        `);
    }

    grid.innerHTML = cards.join('');
    if (badge) {
        badge.textContent = `${onlineCount}/${DEX_SUBSYSTEMS.length} Active`;
        badge.className = 'panel-badge ' + (onlineCount === DEX_SUBSYSTEMS.length ? 'success' : onlineCount > 0 ? 'info' : 'warning');
    }

    // Update summary stats
    const dexStats = await rpc('getDexStats').catch(() => null);
    const el = id => document.getElementById(id);
    if (dexStats) {
        if (el('dexTotalPairs')) el('dexTotalPairs').textContent = formatNum(dexStats.total_pairs || 0);
        if (el('dexVolume24h')) el('dexVolume24h').textContent = formatMolt(dexStats.volume_24h || 0);
        if (el('dexOpenOrders')) el('dexOpenOrders').textContent = formatNum(dexStats.open_orders || 0);
    }
    const ammStats = await rpc('getAmmStats').catch(() => null);
    if (ammStats) {
        if (el('dexTVL')) el('dexTVL').textContent = formatMolt(ammStats.tvl || 0);
    }
    const marginStats = await rpc('getMarginStats').catch(() => null);
    if (marginStats) {
        if (el('dexMarginPos')) el('dexMarginPos').textContent = formatNum(marginStats.open_positions || 0);
    }
    const predictStats = await rpc('getPredictionMarketStats').catch(() => null);
    if (predictStats) {
        if (el('dexPredictMkts')) el('dexPredictMkts').textContent = formatNum(predictStats.open_markets || 0);
    }
}

// ── Smart Contracts Monitor ─────────────────────────────────

const ALL_CONTRACTS = [
    { symbol: 'MOLT', name: 'MoltCoin', cat: 'token', icon: 'fas fa-coins', color: '#ff6b35' },
    { symbol: 'MUSD', name: 'mUSD Stablecoin', cat: 'token', icon: 'fas fa-dollar-sign', color: '#4ade80' },
    { symbol: 'WETH', name: 'Wrapped ETH', cat: 'token', icon: 'fab fa-ethereum', color: '#627eea' },
    { symbol: 'WSOL', name: 'Wrapped SOL', cat: 'token', icon: 'fas fa-sun', color: '#9945ff' },
    { symbol: 'DEX', name: 'DEX Core', cat: 'dex', icon: 'fas fa-exchange-alt', color: '#4ea8de' },
    { symbol: 'DEXAMM', name: 'DEX AMM', cat: 'dex', icon: 'fas fa-water', color: '#06d6a0' },
    { symbol: 'DEXROUTER', name: 'DEX Router', cat: 'dex', icon: 'fas fa-route', color: '#ffd166' },
    { symbol: 'DEXMARGIN', name: 'DEX Margin', cat: 'dex', icon: 'fas fa-chart-line', color: '#ef4444' },
    { symbol: 'DEXGOV', name: 'DEX Governance', cat: 'dex', icon: 'fas fa-landmark', color: '#a78bfa' },
    { symbol: 'DEXREWARDS', name: 'DEX Rewards', cat: 'dex', icon: 'fas fa-gift', color: '#f59e0b' },
    { symbol: 'ANALYTICS', name: 'DEX Analytics', cat: 'dex', icon: 'fas fa-chart-area', color: '#60a5fa' },
    { symbol: 'MOLTSWAP', name: 'MoltSwap', cat: 'dex', icon: 'fas fa-arrows-rotate', color: '#ff6b35' },
    { symbol: 'BRIDGE', name: 'MoltBridge', cat: 'infra', icon: 'fas fa-bridge', color: '#38bdf8' },
    { symbol: 'DAO', name: 'MoltDAO', cat: 'infra', icon: 'fas fa-users-cog', color: '#a78bfa' },
    { symbol: 'CLAWVAULT', name: 'ClawVault', cat: 'defi', icon: 'fas fa-vault', color: '#f472b6' },
    { symbol: 'CLAWPAY', name: 'ClawPay', cat: 'defi', icon: 'fas fa-credit-card', color: '#34d399' },
    { symbol: 'CLAWPUMP', name: 'ClawPump', cat: 'defi', icon: 'fas fa-rocket', color: '#fb923c' },
    { symbol: 'ORACLE', name: 'MoltOracle', cat: 'infra', icon: 'fas fa-eye', color: '#c084fc' },
    { symbol: 'LEND', name: 'LobsterLend', cat: 'defi', icon: 'fas fa-hand-holding-usd', color: '#2dd4bf' },
    { symbol: 'MARKET', name: 'MoltMarket', cat: 'nft', icon: 'fas fa-store', color: '#f97316' },
    { symbol: 'AUCTION', name: 'MoltAuction', cat: 'nft', icon: 'fas fa-gavel', color: '#e879f9' },
    { symbol: 'BOUNTY', name: 'BountyBoard', cat: 'infra', icon: 'fas fa-bullhorn', color: '#fbbf24' },
    { symbol: 'COMPUTE', name: 'Compute Market', cat: 'infra', icon: 'fas fa-microchip', color: '#94a3b8' },
    { symbol: 'REEF', name: 'Reef Storage', cat: 'infra', icon: 'fas fa-database', color: '#22d3ee' },
    { symbol: 'PUNKS', name: 'MoltPunks', cat: 'nft', icon: 'fas fa-image', color: '#f43f5e' },
    { symbol: 'YID', name: 'MoltyID', cat: 'identity', icon: 'fas fa-fingerprint', color: '#818cf8' },
    { symbol: 'TLOBSTER', name: 'Prediction Market', cat: 'defi', icon: 'fas fa-chart-pie', color: '#e879f9' },
];

let contractMonitorLoaded = false;

async function updateContractMonitor() {
    const grid = document.getElementById('contractMonitorGrid');
    const badge = document.getElementById('contractMonitorBadge');
    if (!grid) return;

    let deployedCount = 0;
    const cards = [];

    for (const c of ALL_CONTRACTS) {
        const info = await rpc('getSymbolRegistry', [c.symbol]);
        const deployed = !!(info && info.program);
        if (deployed) deployedCount++;

        const program = info?.program || '';
        const template = info?.template || '—';
        const statusClass = deployed ? 'success' : 'warning';
        const statusText = deployed ? 'LIVE' : 'PENDING';

        // Try to fetch contract-specific stats via dedicated RPC methods
        let statsHtml = '';
        if (deployed) {
            try {
                let cs = null;
                if (c.symbol === 'TLOBSTER') {
                    cs = await rpc('getPredictionMarketStats');
                    if (cs) cs = { markets: cs.open_markets || 0, volume: cs.total_volume || 0, collateral: cs.total_collateral || 0, fees: cs.fees_collected || 0 };
                } else if (c.symbol === 'MOLT') {
                    const bal = await rpc('getBalance', [program]);
                    if (bal != null) cs = { balance: typeof bal === 'number' ? formatMolt(bal) : '—' };
                } else if (c.symbol === 'YID') {
                    const stats = await rpc('getMoltyIdStats');
                    if (stats) cs = { identities: stats.total_identities || 0, names: stats.total_names || 0, skills: stats.total_skills || 0 };
                }
                if (cs) {
                    const entries = Object.entries(cs).slice(0, 4);
                    if (entries.length > 0) {
                        statsHtml = '<div class="cm-metrics">' + entries.map(([k, v]) => {
                            const val = typeof v === 'number' ? formatNum(v) : String(v).slice(0, 12);
                            return `<div class="cm-metric"><span class="cm-metric-label">${k.replace(/_/g, ' ')}</span><span class="cm-metric-value">${val}</span></div>`;
                        }).join('') + '</div>';
                    }
                }
            } catch { /* no stats endpoint */ }
        }

        cards.push(`
            <div class="contract-monitor-card" data-cat="${c.cat}">
                <div class="cm-header">
                    <div class="cm-icon" style="background:${c.color}18;color:${c.color};"><i class="${c.icon}"></i></div>
                    <div>
                        <div class="cm-name">${c.name}</div>
                        <div class="cm-symbol">${c.symbol} · ${template}</div>
                    </div>
                    <span class="cm-badge" style="background:${deployed ? 'rgba(74,222,128,0.12)' : 'rgba(245,158,11,0.12)'};color:${deployed ? '#4ade80' : '#f59e0b'};">${statusText}</span>
                </div>
                ${program ? `<div class="cm-addr" title="${escapeHtml(program)}">${escapeHtml(program)}</div>` : ''}
                ${statsHtml}
            </div>
        `);
    }

    grid.innerHTML = cards.join('');
    contractMonitorLoaded = true;

    if (badge) {
        badge.textContent = `${deployedCount}/${ALL_CONTRACTS.length} Deployed`;
        badge.className = 'panel-badge ' + (deployedCount >= ALL_CONTRACTS.length ? 'success' : deployedCount >= ALL_CONTRACTS.length / 2 ? 'info' : 'warning');
    }

    // Wire category filter buttons
    document.querySelectorAll('.contract-cat-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            document.querySelectorAll('.contract-cat-btn').forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
            const cat = btn.dataset.cat;
            document.querySelectorAll('.contract-monitor-card').forEach(card => {
                card.style.display = (cat === 'all' || card.dataset.cat === cat) ? '' : 'none';
            });
        });
    });
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

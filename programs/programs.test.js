/**
 * Programs Playground Tests — Phase 14 audit fixes
 * Run: node programs.test.js
 *
 * Tests all fixes applied during Phase 14 audit:
 *  F14.1  — addTerminalLine XSS + sanitizeUrl (javascript: URI blocked)
 *  F14.2  — updateDeployedProgramsList XSS + onclick injection → data-attributes
 *  F14.3  — updateProblemsPanel XSS (compiler error messages)
 *  F14.4  — renderProgramCalls XSS (RPC data)
 *  F14.5  — Storage viewer XSS (RPC data)
 *  F14.6  — displayProgramAbi XSS (RPC ABI fields)
 *  F14.7  — updateWalletDropdown XSS (localStorage names)
 *  F14.8  — sendTransfer amount validation
 *  F14.9  — SDK duplicate getAccount removed
 *  F14.10 — SDK wallet export password note (no code fix — by design)
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
// Extract pure functions from source (must match playground-complete.js)
// ═══════════════════════════════════════════════════════════════════════════

function escapeHtml(str) {
    if (typeof str !== 'string') return String(str ?? '');
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

function sanitizeUrl(url) {
    if (typeof url !== 'string') return '';
    try {
        const parsed = new URL(url, 'http://localhost');
        if (!['http:', 'https:'].includes(parsed.protocol)) return '';
        return url;
    } catch {
        return '';
    }
}

function truncateAddress(addr, start = 8, end = 6) {
    if (!addr) return '';
    return `${addr.substring(0, start)}...${addr.substring(addr.length - end)}`;
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.1 — escapeHtml and sanitizeUrl
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.1: escapeHtml XSS sanitization ──');

assertEqual(escapeHtml('<script>alert(1)</script>'), '&lt;script&gt;alert(1)&lt;/script&gt;', 'Escapes script tags');
assertEqual(escapeHtml('"onload="alert(1)'), '&quot;onload=&quot;alert(1)', 'Escapes double quotes');
assertEqual(escapeHtml("test'ing"), "test&#39;ing", 'Escapes single quotes');
assertEqual(escapeHtml('a&b<c>d'), 'a&amp;b&lt;c&gt;d', 'Escapes ampersand, angle brackets');
assertEqual(escapeHtml('hello world'), 'hello world', 'Passes safe strings through');
assertEqual(escapeHtml(''), '', 'Handles empty string');
assertEqual(escapeHtml(null), '', 'Handles null');
assertEqual(escapeHtml(undefined), '', 'Handles undefined');
assertEqual(escapeHtml(42), '42', 'Handles numbers');

console.log('\n── F14.1: sanitizeUrl blocks javascript: URIs ──');

assertEqual(sanitizeUrl('javascript:alert(1)'), '', 'Blocks javascript: URI');
assertEqual(sanitizeUrl('JAVASCRIPT:alert(1)'), '', 'Blocks case-insensitive javascript:');
assertEqual(sanitizeUrl('data:text/html,<h1>XSS</h1>'), '', 'Blocks data: URI');
assertEqual(sanitizeUrl('vbscript:alert(1)'), '', 'Blocks vbscript: URI');
assertEqual(sanitizeUrl('https://example.com'), 'https://example.com', 'Allows https');
assertEqual(sanitizeUrl('http://localhost:8899'), 'http://localhost:8899', 'Allows http localhost');
assertEqual(sanitizeUrl('/relative/path'), '/relative/path', 'Allows relative paths');
assertEqual(sanitizeUrl(''), '', 'Handles empty string');
assertEqual(sanitizeUrl(null), '', 'Handles null');
assertEqual(sanitizeUrl(undefined), '', 'Handles undefined');

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.2 — Deployed programs list XSS protection
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.2: Deployed programs list escaping ──');

{
    const maliciousName = '<img src=x onerror=alert(1)>';
    const maliciousId = "'); alert('xss'); //";

    // Simulate what the fixed template does
    const escapedName = escapeHtml(maliciousName);
    const escapedId = escapeHtml(maliciousId);

    assert(!escapedName.includes('<img'), 'Metadata name: img tag escaped');
    assert(escapedName.includes('&lt;img'), 'Metadata name: angle brackets are entities');
    assert(!escapedId.includes("'"), 'ProgramId: single quotes escaped (no onclick injection)');

    // Test that data-attribute approach is safe
    const dataAttr = `data-program-id="${escapedId}"`;
    assert(dataAttr.includes('&'), 'Data attribute uses HTML entity escaping');
    assert(!dataAttr.includes("');"), 'Data attribute has no quote breakout');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.3 — Problems panel escaping
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.3: Problems panel compiler error escaping ──');

{
    const errorMsg = '<b onmouseover="alert(1)">error</b>';
    const errorFile = '"><img/src/onerror=alert(1)>';
    
    const escapedMsg = escapeHtml(errorMsg);
    const escapedFile = escapeHtml(errorFile);
    
    assert(!escapedMsg.includes('<b'), 'Error message: HTML tags escaped');
    assert(escapedMsg.includes('&lt;b'), 'Error message: angle brackets are entities');
    assert(!escapedFile.includes('<img'), 'Error file: img tag escaped');
    assertEqual(escapedFile.startsWith('&quot;'), true, 'Error file: leading quote escaped');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.4 — Program calls escaping
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.4: Program calls RPC data escaping ──');

{
    const malFn = '<script>steal()</script>';
    const malCaller = '"><svg onload=alert(1)>';
    
    const escapedFn = escapeHtml(malFn);
    const escapedCaller = escapeHtml(truncateAddress(malCaller));
    
    assert(!escapedFn.includes('<script'), 'Function name: script tag escaped');
    assert(!escapedCaller.includes('<svg'), 'Caller address: svg tag escaped');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.5 — Storage viewer escaping
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.5: Storage viewer RPC data escaping ──');

{
    const malKey = '<details open ontoggle=alert(1)>';
    const malValue = '"><iframe src="javascript:alert(1)">';
    
    const escapedKey = escapeHtml(malKey);
    const escapedValue = escapeHtml(malValue);
    
    assert(!escapedKey.includes('<details'), 'Storage key: details tag escaped');
    assert(!escapedValue.includes('<iframe'), 'Storage value: iframe tag escaped');
    assert(escapedValue.includes('&lt;iframe'), 'Storage value: angle brackets are entities');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.6 — ABI display escaping
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.6: ABI display RPC field escaping ──');

{
    const malFnName = '<img/src/onerror=alert(1)>';
    const malDesc = '"><script>document.cookie</script>';
    const malParam = '<b>bold</b>';
    const malType = 'u64<script>';
    const malEvName = '<svg onload=fetch("evil.com")>';
    const malTemplate = '<marquee>';
    
    assertEqual(escapeHtml(malFnName).includes('<img'), false, 'ABI fn.name: img escaped');
    assertEqual(escapeHtml(malDesc).includes('<script'), false, 'ABI fn.description: script escaped');
    assertEqual(escapeHtml(malParam).includes('<b>'), false, 'ABI p.name: bold tag escaped');
    assertEqual(escapeHtml(malType).includes('<script'), false, 'ABI p.type: script escaped');
    assertEqual(escapeHtml(malEvName).includes('<svg'), false, 'ABI ev.name: svg escaped');
    assertEqual(escapeHtml(malTemplate), '&lt;marquee&gt;', 'ABI template: marquee escaped');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.7 — Wallet dropdown escaping
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.7: Wallet dropdown name escaping ──');

{
    const malName = '<img src=x onerror=alert(document.cookie)>';
    const malId = '" onclick="alert(1)" x="';
    
    const escapedName = escapeHtml(malName);
    const escapedId = escapeHtml(malId);
    
    assert(!escapedName.includes('<img'), 'Wallet name: img tag escaped');
    assert(!escapedId.includes('"'), 'Wallet id: double quotes escaped');
    assertEqual(escapedId.includes('&quot;'), true, 'Wallet id: quotes are HTML entities');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.8 — Transfer amount validation
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.8: Transfer amount validation ──');

{
    // Simulate the validation logic from sendTransfer
    function validateTransferAmount(amount) {
        return Number.isFinite(amount) && amount > 0 && amount <= 1_000_000_000;
    }
    
    assert(!validateTransferAmount(NaN), 'Rejects NaN');
    assert(!validateTransferAmount(Infinity), 'Rejects Infinity');
    assert(!validateTransferAmount(-1), 'Rejects negative');
    assert(!validateTransferAmount(0), 'Rejects zero');
    assert(!validateTransferAmount(1_000_000_001), 'Rejects over 1B');
    assert(validateTransferAmount(1), 'Accepts 1 MOLT');
    assert(validateTransferAmount(0.001), 'Accepts 0.001 MOLT');
    assert(validateTransferAmount(1_000_000_000), 'Accepts max 1B MOLT');
    assert(!validateTransferAmount(parseFloat('')), 'Rejects empty string parsed');
    assert(!validateTransferAmount(parseFloat('abc')), 'Rejects non-numeric string parsed');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F14.9 — SDK duplicate getAccount removed
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F14.9: SDK duplicate getAccount removed ──');

{
    const fs = require('fs');
    const sdkSource = fs.readFileSync(__dirname + '/js/moltchain-sdk.js', 'utf8');
    
    // Count occurrences of "async getAccount(" — should be exactly 1
    const matches = sdkSource.match(/async getAccount\(/g);
    assertEqual(matches ? matches.length : 0, 1, 'SDK has exactly one getAccount method');
    
    // The comment about removing the duplicate should exist
    assert(sdkSource.includes('removed duplicate getAccount'), 'Removal comment exists');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: Playground source uses escapeHtml
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── Source verification: escapeHtml usage ──');

{
    const fs = require('fs');
    const pgSource = fs.readFileSync(__dirname + '/js/playground-complete.js', 'utf8');
    
    // escapeHtml helper must be defined
    assert(pgSource.includes('function escapeHtml(str)'), 'escapeHtml helper defined');
    assert(pgSource.includes('function sanitizeUrl(url)'), 'sanitizeUrl helper defined');
    
    // Key injection points must now use escapeHtml
    assert(pgSource.includes('escapeHtml(text)'), 'addTerminalLine uses escapeHtml');
    assert(pgSource.includes('escapeHtml(safeUrl)'), 'Terminal link href uses escapeHtml(sanitizeUrl)');
    assert(pgSource.includes("escapeHtml(program.metadata?.name || 'Unnamed')"), 'Program name escaped');
    assert(pgSource.includes('data-program-id='), 'Programs list uses data-attribute (not onclick interp.)');
    assert(pgSource.includes('escapeHtml(err.message)'), 'Problems panel error message escaped');
    assert(pgSource.includes('escapeHtml(call.function)'), 'Program calls function escaped');
    assert(pgSource.includes('escapeHtml(entry.key)'), 'Storage key escaped');
    assert(pgSource.includes('escapeHtml(entry.value)'), 'Storage value escaped');
    assert(pgSource.includes('escapeHtml(fn.name)'), 'ABI fn.name escaped');
    assert(pgSource.includes('escapeHtml(fn.description)'), 'ABI fn.description escaped');
    assert(pgSource.includes('escapeHtml(item.name)'), 'Wallet dropdown name escaped');
    assert(pgSource.includes("amount > 1_000_000_000"), 'Transfer amount upper bound check');
    
    // No remaining unescaped innerHTML with RPC data
    // The old onclick pattern should be gone
    assert(!pgSource.includes("onclick=\"Playground.viewProgram('${program.programId}')\""), 'Old onclick interpolation removed');
}

// ═══════════════════════════════════════════════════════════════════════════
// Summary
// ═══════════════════════════════════════════════════════════════════════════
console.log(`\n${'═'.repeat(60)}`);
console.log(`Phase 14 Programs Playground: ${passed} passed, ${failed} failed`);
console.log('═'.repeat(60));
process.exit(failed > 0 ? 1 : 0);

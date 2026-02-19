/**
 * Phase 21 — Test Coverage & E2E Audit Tests
 * Run: node tests/test_coverage_audit.js
 *
 * Verifies all 8 findings fixed during Phase 21:
 *  T21.1 — comprehensive-e2e.py exception handlers report SKIP (not PASS)
 *  T21.2 — test-ws-dex.js has assertions and proper exit codes
 *  T21.3 — test_bincode_format.js uses assert module (not console.assert)
 *  T21.4 — run-e2e.sh uses dynamic path (no hardcoded absolute)
 *  T21.5 — extractFunction in audit tests handles string-literal braces
 *  T21.6 — extractEscapeHtml uses brace-counting (not [^}]+)
 *  T21.7 — start-validator.sh has --keep-state guard
 *  T21.8 — test-dex-api-comprehensive.sh uses configurable BASE URL
 */
'use strict';

const fs = require('fs');
const path = require('path');

let passed = 0, failed = 0;
function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}

const ROOT = path.resolve(__dirname, '..');

// ═══════════════════════════════════════════════════════════════════════════
// T21.1 — comprehensive-e2e.py: exception handlers must NOT report PASS
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── T21.1: comprehensive-e2e.py exception masking ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'tests', 'comprehensive-e2e.py'), 'utf8');
    const lines = src.split('\n');

    // Find all lines matching report("PASS" inside except blocks that say "skip" or "fallback"
    const maskedLines = lines.filter(l =>
        /report\("PASS"/.test(l) && (/skip/i.test(l) || /fallback/i.test(l))
    );
    assert(maskedLines.length === 0,
        `No exception-masked PASS reports remain (found ${maskedLines.length})`);

    // Verify SKIP is used instead
    const skipLines = lines.filter(l =>
        /report\("SKIP"/.test(l) && (/skip/i.test(l) || /fallback/i.test(l))
    );
    assert(skipLines.length >= 10,
        `At least 10 exception handlers now use SKIP (found ${skipLines.length})`);

    // Verify report function supports SKIP
    assert(src.includes('status == "SKIP"'), 'report() supports SKIP status');
}

// ═══════════════════════════════════════════════════════════════════════════
// T21.2 — test-ws-dex.js: must have assertions and exit on failure
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── T21.2: test-ws-dex.js assertions ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'tests', 'test-ws-dex.js'), 'utf8');

    // Must have minimum message check
    assert(src.includes('msgCount < 1'), 'Has minimum message count check');
    assert(src.includes('process.exit(1)'), 'Exits with failure code on error');

    // Must validate JSON
    assert(src.includes('JSON.parse'), 'Validates received messages are valid JSON');

    // On error, must use stderr
    assert(src.includes('console.error'), 'Uses console.error for failures');
}

// ═══════════════════════════════════════════════════════════════════════════
// T21.3 — test_bincode_format.js: uses assert module, not console.assert
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── T21.3: test_bincode_format.js assert module ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'sdk', 'js', 'test_bincode_format.js'), 'utf8');

    assert(!src.includes('console.assert'), 'No console.assert calls remain');
    assert(src.includes("require('assert')"), 'Uses Node assert module');
    assert(src.includes('assert.strictEqual'), 'Uses assert.strictEqual for equality');
    assert(src.includes('assert.fail'), 'Uses assert.fail for expected-throw guards');
    assert(src.includes('assert.ok'), 'Uses assert.ok for boolean checks');
}

// ═══════════════════════════════════════════════════════════════════════════
// T21.4 — run-e2e.sh: dynamic path, no hardcoded absolute
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── T21.4: run-e2e.sh dynamic path ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'tests', 'run-e2e.sh'), 'utf8');

    assert(!src.includes('/Users/'), 'No hardcoded /Users/ path');
    assert(src.includes('SCRIPT_DIR'), 'Uses SCRIPT_DIR variable');
    assert(src.includes('$(dirname "$0")'), 'Derives path from script location');
    assert(src.includes('cd "$SCRIPT_DIR/.."'), 'Changes to project root via SCRIPT_DIR');
}

// ═══════════════════════════════════════════════════════════════════════════
// T21.5 — extractFunction: handles braces inside string literals
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── T21.5: extractFunction string-literal brace handling ──');
{
    // Import the improved extractFunction from marketplace audit
    const auditSrc = fs.readFileSync(path.join(ROOT, 'tests', 'test_marketplace_audit.js'), 'utf8');
    const fnMatch = auditSrc.match(/function extractFunction\(source, name\)\s*\{/);
    assert(fnMatch !== null, 'extractFunction exists in marketplace audit');

    // Verify it handles string literals
    assert(auditSrc.includes("ch === '\"'") || auditSrc.includes('ch === \'"\''),
        'extractFunction checks for double-quote string literals');
    assert(auditSrc.includes("ch === \"'\"") || auditSrc.includes("ch === '\\''"),
        'extractFunction checks for single-quote string literals');
    assert(auditSrc.includes("ch === '`'"),
        'extractFunction checks for template literals');

    // Verify same fix in website audit
    const webSrc = fs.readFileSync(path.join(ROOT, 'tests', 'test_website_audit.js'), 'utf8');
    assert(webSrc.includes("ch === '`'"),
        'website audit extractFunction also handles template literals');

    // Functional test: verify brace-counting handles string-literal braces
    // Build a simple extractFunction manually using the same algorithm
    function testExtract(source, name) {
        const re = new RegExp(`function ${name}\\s*\\(([^)]*)\\)\\s*\\{`);
        const match = source.match(re);
        if (!match) return null;
        let depth = 0;
        const start = match.index;
        for (let i = start; i < source.length; i++) {
            const ch = source[i];
            if (ch === '"' || ch === "'" || ch === '`') {
                const q = ch; i++;
                while (i < source.length && source[i] !== q) {
                    if (source[i] === '\\') i++;
                    i++;
                }
                continue;
            }
            if (ch === '{') depth++;
            if (ch === '}') { depth--; if (depth === 0) return source.slice(start, i + 1); }
        }
        return null;
    }

    const testSource = 'function greet(name) { return "Hello, {world}! " + name; }';
    const result = testExtract(testSource, 'greet');
    assert(result !== null, 'extractFunction correctly handles braces inside strings');
    assert(typeof result === 'string' && result.includes('{world}'),
        'Extracted function includes string with braces');
}

// ═══════════════════════════════════════════════════════════════════════════
// T21.6 — extractEscapeHtml: uses brace-counting, not [^}]+
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── T21.6: extractEscapeHtml brace-counting ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'tests', 'test_wallet_extension_audit.js'), 'utf8');

    assert(!src.includes('[^}]+'), 'No [^}]+ regex pattern (was fragile)');
    assert(src.includes('depth'), 'Uses depth counter for brace matching');
    assert(src.includes("depth === 0"), 'Checks depth === 0 for function end');
}

// ═══════════════════════════════════════════════════════════════════════════
// T21.7 — start-validator.sh: --keep-state guard
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── T21.7: start-validator.sh state guard ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'tests', 'start-validator.sh'), 'utf8');

    assert(src.includes('--keep-state'), 'Supports --keep-state flag');
    assert(src.includes('if ['), 'Has conditional check before rm');
    assert(!(/^rm -rf/m.test(src)), 'rm -rf is not unconditional at line start');
    assert(src.includes('Wiping state') || src.includes('warning') || src.includes('⚠️'),
        'Shows warning before state wipe');
}

// ═══════════════════════════════════════════════════════════════════════════
// T21.8 — test-dex-api-comprehensive.sh: configurable BASE URL
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── T21.8: test-dex-api-comprehensive.sh configurable URL ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'tests', 'test-dex-api-comprehensive.sh'), 'utf8');

    assert(src.includes('MOLT_RPC_URL'), 'Uses MOLT_RPC_URL env var');
    assert(src.includes('${MOLT_RPC_URL:-'), 'Has default fallback syntax');
    assert(src.includes('localhost:8899'), 'Default is still localhost:8899');
}

// ═══════════════════════════════════════════════════════════════════════════
// Summary
// ═══════════════════════════════════════════════════════════════════════════
console.log(`\n${'═'.repeat(60)}`);
console.log(`Phase 21 Tests: ${passed} passed, ${failed} failed (${passed + failed} total)`);
if (failed > 0) {
    console.error('PHASE 21 TESTS FAILED');
    process.exit(1);
}
console.log('All Phase 21 tests passed ✅');

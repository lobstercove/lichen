const assert = require('assert');
const {
    HELPER_FAMILIES,
    findDrift,
} = require('../sync_frontend_shared_helpers');

let passed = 0;
let failed = 0;

function test(name, fn) {
    try {
        fn();
        console.log(`  PASS ${name}`);
        passed += 1;
    } catch (error) {
        console.error(`  FAIL ${name}`);
        console.error(`    ${error.message}`);
        failed += 1;
    }
}

console.log('\nFrontend Shared Helper Drift Audit\n');

test('canonical helper families are defined', () => {
    assert(Array.isArray(HELPER_FAMILIES) && HELPER_FAMILIES.length >= 3, 'expected canonical helper families for the duplicate clusters');
});

test('shared helper families are drift-free', () => {
    const drift = findDrift();
    assert.strictEqual(
        drift.length,
        0,
        drift.map((entry) => `${entry.familyId}: ${entry.target} diverged from ${entry.source}`).join('\n') || 'unexpected shared helper drift detected'
    );
});

console.log(`\nFrontend Shared Helper Drift Audit: ${passed} passed, ${failed} failed (${passed + failed} total)`);

if (failed > 0) {
    process.exit(1);
}
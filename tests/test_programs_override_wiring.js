// Programs Playground override wiring audit
// Run: node tests/test_programs_override_wiring.js

const fs = require('fs');
const path = require('path');

let passed = 0;
let failed = 0;

function assert(condition, label) {
    if (condition) {
        passed++;
        console.log(`  ✅ ${label}`);
    } else {
        failed++;
        console.log(`  ❌ ${label}`);
    }
}

const root = path.join(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'programs', 'playground.html'), 'utf8');
const js = fs.readFileSync(path.join(root, 'programs', 'js', 'playground-complete.js'), 'utf8');

console.log('\n── Programs Override Wiring ──');

assert(html.includes('id="programSeedImportGroup"'), 'O-1 inline import container exists');
assert(html.includes('id="programSeedImportInput"'), 'O-2 inline seed input exists');
assert(html.includes('id="programSeedImportConfirmBtn"'), 'O-3 inline confirm button exists');
assert(html.includes('id="programSeedImportCancelBtn"'), 'O-4 inline cancel button exists');

assert(js.includes('toggleProgramSeedImportUI(show)'), 'O-5 program seed import UI toggle helper exists');
assert(js.includes("this.toggleProgramSeedImportUI(true);"), 'O-6 Import button opens inline import UI');
assert(js.includes("this.importProgramKeypair();"), 'O-7 Confirm import triggers program key import');

assert(!js.includes("prompt('Enter program seed (base58):')"), 'O-8 no system prompt for program key import');
assert(js.includes("preview.value = 'Connect wallet to preview';"), 'O-9 Program ID preview has connect-wallet state');
assert(js.includes("preview.value = 'Build to preview';"), 'O-10 Program ID preview has build state when wallet connected');
assert(js.includes('if (!overrideValue && this.programKeypair?.publicKey)'), 'O-11 override preview falls back to stored program keypair');

console.log(`\n${'═'.repeat(50)}`);
console.log(`Programs Override Wiring Audit: ${passed} passed, ${failed} failed (${passed + failed} total)`);
console.log(`${'═'.repeat(50)}`);
process.exit(failed > 0 ? 1 : 0);

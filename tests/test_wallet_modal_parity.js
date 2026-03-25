// Wallet modal parity checks across DEX, Marketplace, Programs
// Run: node tests/test_wallet_modal_parity.js

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

function normalizeHtml(html) {
    return html.replace(/\s+/g, ' ').trim();
}

function extractBetween(source, startMarker, endMarker) {
    const start = source.indexOf(startMarker);
    const end = source.indexOf(endMarker, start);
    if (start === -1 || end === -1 || end <= start) return null;
    return source.slice(start, end).trim();
}

const root = path.join(__dirname, '..');
const dexHtml = fs.readFileSync(path.join(root, 'dex', 'index.html'), 'utf8');
const programsHtml = fs.readFileSync(path.join(root, 'programs', 'playground.html'), 'utf8');
const programsJs = fs.readFileSync(path.join(root, 'programs', 'js', 'playground-complete.js'), 'utf8');
const marketplaceWalletJs = fs.readFileSync(path.join(root, 'marketplace', 'shared', 'wallet-connect.js'), 'utf8');
const dexSharedWalletJs = fs.readFileSync(path.join(root, 'dex', 'shared', 'wallet-connect.js'), 'utf8');
const programsSharedWalletJs = fs.readFileSync(path.join(root, 'programs', 'shared', 'wallet-connect.js'), 'utf8');

console.log('\n── Wallet Modal Markup Parity ──');

const dexModal = extractBetween(
    dexHtml,
    '<div class="wallet-modal-overlay hidden" id="walletModal">',
    '<!-- ═══════════════════ FOOTER ═══════════════════ -->'
);
const programsModal = extractBetween(
    programsHtml,
    '<div class="wallet-modal-overlay hidden" id="walletModal">',
    '<!-- Import Program Modal -->'
);

assert(Boolean(dexModal), 'P-1 DEX wallet modal block found');
assert(Boolean(programsModal), 'P-2 Programs wallet modal block found');
if (dexModal && programsModal) {
    assert(
        normalizeHtml(dexModal) === normalizeHtml(programsModal),
        'P-3 Programs wallet modal markup matches DEX exactly'
    );
}

console.log('\n── Programs Wallet Behavior Parity ──');

assert(
    programsJs.includes("document.getElementById('walletBtn')?.addEventListener('click', () => {\n            this.openWalletModal();\n        });"),
    'P-4 walletBtn always opens wallet modal'
);
assert(
    !programsJs.includes('this.toggleWalletDropdown();'),
    'P-5 walletBtn no longer routes to wallet dropdown'
);
assert(
    programsJs.includes("this.switchWmTab(this.wallets.length ? 'wallets' : 'extension');"),
    'P-6 openWalletModal default tab matches DEX (extension if none)'
);
assert(
    programsJs.includes('this.resetWalletModalInputs();') &&
    programsJs.includes('closeWalletModal()'),
    'P-7 closeWalletModal resets modal inputs'
);
assert(
    programsJs.includes("if (createBtn) createBtn.classList.add('hidden');"),
    'P-8 create flow hides Create button after generation'
);
assert(
    programsJs.includes("if (createBtn) createBtn.classList.remove('hidden');"),
    'P-9 modal reset restores Create button visibility'
);
assert(
    programsJs.includes('this.renderWalletList();\n        this.closeWalletModal();'),
    'P-10 import/extension paths refresh wallet list before closing'
);

console.log('\n── Marketplace Wallet Behavior Parity ──');

assert(
    marketplaceWalletJs.includes("this._switchTab(this.savedWallets.length ? 'wallets' : 'extension');"),
    'P-11 marketplace modal default tab matches DEX (extension if none)'
);
assert(
    marketplaceWalletJs.includes('await this._renderWalletList();'),
    'P-12 marketplace create/import/extension syncs wallet list immediately'
);
assert(
    (marketplaceWalletJs.includes("if (createBtn) createBtn.classList.add('hidden');") ||
        marketplaceWalletJs.includes("if (wmCreateBtn) wmCreateBtn.classList.add('hidden');")) &&
    marketplaceWalletJs.includes("if (createBtn) createBtn.classList.remove('hidden');"),
    'P-13 marketplace create button hidden after create and restored on reset'
);

console.log('\n── Shared Wallet Utility Parity ──');

assert(
    dexSharedWalletJs === programsSharedWalletJs,
    'P-14 programs shared wallet utility is byte-identical to DEX shared utility'
);

console.log(`\n${'═'.repeat(50)}`);
console.log(`Wallet Modal Parity Audit: ${passed} passed, ${failed} failed (${passed + failed} total)`);
console.log(`${'═'.repeat(50)}`);
process.exit(failed > 0 ? 1 : 0);

// ============================================================
// Phase 19 — Developer Portal Audit Tests
// 12 findings (D1–D12), 15 HTML files, 1 JS module
// ============================================================

const fs   = require('fs');
const path = require('path');

const DEV = path.join(__dirname, '..', 'developers');
const JS  = path.join(DEV, 'js', 'developers.js');
const CSS = path.join(DEV, 'css', 'developers.css');
const RPC_REFERENCE_MD = path.join(__dirname, '..', 'docs', 'guides', 'RPC_API_REFERENCE.md');

// All 15 HTML pages
const ALL_PAGES = fs.readdirSync(DEV).filter(f => f.endsWith('.html')).sort();

let pass = 0, fail = 0;
function ok(cond, msg) {
    if (cond) { pass++; }
    else { fail++; console.error(`  FAIL: ${msg}`); }
}

// Helper: read file contents
function read(file) {
    return fs.readFileSync(path.join(DEV, file), 'utf8');
}

console.log('Phase 19 — Developer Portal Audit Tests');
console.log('========================================\n');

// ------------------------------------------------------------------
// D7+D2: Inline CSS centralized into developers.css
// ------------------------------------------------------------------
console.log('[D7+D2] Inline CSS centralized');

const cssContent = fs.readFileSync(CSS, 'utf8');
ok(cssContent.includes('NAV BAR OVERRIDES'), 'developers.css has NAV BAR OVERRIDES section');
ok(cssContent.includes('.nav-menu a.active'), 'developers.css has .nav-menu a.active rule');
ok(cssContent.includes('.search-input'), 'developers.css has .search-input rule');
ok(cssContent.includes('.network-select'), 'developers.css has .network-select rule');

// 10 files that had inline styles removed (not index.html, not contract-reference.html)
const SHOULD_NOT_HAVE_INLINE_STYLE = [
    'getting-started.html', 'contracts.html', 'architecture.html',
    'validator.html', 'cli-reference.html', 'moltyid.html',
    'playground.html', 'changelog.html', 'rpc-reference.html', 'sdk-js.html'
];
SHOULD_NOT_HAVE_INLINE_STYLE.forEach(file => {
    const html = read(file);
    ok(!html.includes('<style>'), `${file} has no inline <style> block`);
});

// index.html and contract-reference.html keep their inline styles (page-specific)
ok(read('index.html').includes('<style>'), 'index.html retains page-specific inline styles');
ok(read('contract-reference.html').includes('<style>'), 'contract-reference.html retains page-specific inline styles');

// ------------------------------------------------------------------
// D1+D8: developers.js added to all pages
// ------------------------------------------------------------------
console.log('[D1+D8] developers.js on all pages');

ALL_PAGES.forEach(file => {
    const html = read(file);
    ok(html.includes('src="js/developers.js"'), `${file} includes developers.js`);
});

// 5 files should NOT have inline sidebar/search/copy scripts anymore
const CLEANED_INLINE = [
    'rpc-reference.html', 'ws-reference.html', 'sdk-js.html',
    'sdk-python.html', 'sdk-rust.html'
];
CLEANED_INLINE.forEach(file => {
    const html = read(file);
    ok(!html.includes("document.querySelectorAll('.sidebar-link').forEach"),
        `${file} no longer has inline sidebar-link handler`);
    ok(!html.includes("document.getElementById('searchOverlay')"),
        `${file} no longer has inline searchOverlay handler`);
});

// ------------------------------------------------------------------
// D12: initCodeCopy uses innerHTML (not textContent)
// ------------------------------------------------------------------
console.log('[D12] initCodeCopy uses innerHTML');

const jsContent = fs.readFileSync(JS, 'utf8');
const rpcReferenceMd = fs.readFileSync(RPC_REFERENCE_MD, 'utf8');
// Extract the initCodeCopy function body
const copyFnStart = jsContent.indexOf('function initCodeCopy()');
const copyFnEnd   = jsContent.indexOf('\n}\n', copyFnStart) + 3;
const copyFnBody  = jsContent.slice(copyFnStart, copyFnEnd);

ok(copyFnBody.includes('originalHTML = btn.innerHTML'), 'initCodeCopy saves btn.innerHTML');
ok(copyFnBody.includes("btn.querySelector('i')"), 'initCodeCopy detects icon buttons');
ok(copyFnBody.includes("'<i class=\"fas fa-check\"></i>'"), 'initCodeCopy shows fa-check icon for icon buttons');
ok(copyFnBody.includes("'Copied!'"), 'initCodeCopy shows Copied! text for text buttons');
ok(!copyFnBody.includes('btn.textContent ='), 'initCodeCopy does NOT use btn.textContent for restore');

// ------------------------------------------------------------------
// D3: Trust tiers in architecture.html match contract source
// ------------------------------------------------------------------
console.log('[D3] Trust tiers match contract source');

const arch = read('architecture.html');
// Canonical tier names from contracts/moltyid/src/lib.rs
ok(arch.includes('Newcomer'), 'architecture.html has Newcomer tier');
ok(arch.includes('Verified'), 'architecture.html has Verified tier');
ok(arch.includes('Established'), 'architecture.html has Established tier');
ok(arch.includes('Elite'), 'architecture.html has Elite tier');
ok(arch.includes('Legendary'), 'architecture.html has Legendary tier');
// Canonical score ranges
ok(arch.includes('0 – 99'), 'architecture.html has 0-99 range for Newcomer');
ok(arch.includes('100 – 499'), 'architecture.html has 100-499 range for Verified');
ok(arch.includes('500 – 999'), 'architecture.html has 500-999 range for Trusted');
ok(arch.includes('1,000 – 4,999'), 'architecture.html has 1000-4999 range for Established');
ok(arch.includes('5,000 – 9,999'), 'architecture.html has 5000-9999 range for Elite');
ok(arch.includes('10,000+'), 'architecture.html has 10000+ range for Legendary');
// Old wrong names must be gone
ok(!arch.includes('Unverified'), 'architecture.html removed Unverified');
ok(!arch.includes('Participant'), 'architecture.html removed Participant');
ok(!arch.includes('Contributor'), 'architecture.html removed Contributor');
ok(!arch.includes('>Authority<'), 'architecture.html removed Authority tier');

// ------------------------------------------------------------------
// D4: Copy buttons on contracts.html and moltyid.html
// ------------------------------------------------------------------
console.log('[D4] Copy buttons on code blocks');

const contractsHtml = read('contracts.html');
const moltyidHtml   = read('moltyid.html');
const contractsCopyCount = (contractsHtml.match(/code-copy-btn/g) || []).length;
const moltyidCopyCount   = (moltyidHtml.match(/code-copy-btn/g) || []).length;
ok(contractsCopyCount >= 12, `contracts.html has ${contractsCopyCount} copy buttons (>= 12)`);
ok(moltyidCopyCount >= 14, `moltyid.html has ${moltyidCopyCount} copy buttons (>= 14)`);

// ------------------------------------------------------------------
// D5: aria-labels on toggle buttons
// ------------------------------------------------------------------
console.log('[D5] aria-labels on toggle buttons');

// Every page with navToggle should have aria-label
ALL_PAGES.forEach(file => {
    const html = read(file);
    if (html.includes('id="navToggle"')) {
        ok(html.includes('aria-label="Toggle navigation menu"'),
            `${file} navToggle has aria-label`);
    }
});

// Pages with sidebarToggle should have aria-label
const SIDEBAR_PAGES = [
    'getting-started.html', 'contracts.html', 'architecture.html',
    'validator.html', 'cli-reference.html', 'moltyid.html', 'playground.html'
];
SIDEBAR_PAGES.forEach(file => {
    const html = read(file);
    ok(html.includes('aria-label="Toggle sidebar"'),
        `${file} sidebarToggle has aria-label`);
});

// ------------------------------------------------------------------
// D6: Breadcrumb separators standardized to ›
// ------------------------------------------------------------------
console.log('[D6] Breadcrumb separators');

const BREADCRUMB_PAGES = [
    'sdk-js.html', 'sdk-python.html', 'sdk-rust.html',
    'ws-reference.html', 'rpc-reference.html',
    'getting-started.html', 'architecture.html', 'contracts.html',
    'moltyid.html', 'validator.html', 'changelog.html',
    'cli-reference.html', 'playground.html'
];
BREADCRUMB_PAGES.forEach(file => {
    const html = read(file);
    if (html.includes('class="separator"')) {
        const separators = html.match(/<span class="separator">[^<]+<\/span>/g) || [];
        separators.forEach(sep => {
            ok(sep.includes('›'), `${file} separator is › (got: ${sep})`);
        });
    }
});

// ------------------------------------------------------------------
// D9: WebSocket section in sdk-rust.html
// ------------------------------------------------------------------
console.log('[D9] WebSocket section in sdk-rust.html');

const sdkRust = read('sdk-rust.html');
ok(sdkRust.includes('id="client-ws"'), 'sdk-rust.html has client-ws section');
ok(sdkRust.includes('WebSocket Subscriptions'), 'sdk-rust.html has WebSocket Subscriptions heading');
ok(sdkRust.includes('WsClient::connect'), 'sdk-rust.html documents WsClient::connect');
ok(sdkRust.includes('on_slot'), 'sdk-rust.html documents on_slot');
ok(sdkRust.includes('on_account_change'), 'sdk-rust.html documents on_account_change');
ok(sdkRust.includes('on_block'), 'sdk-rust.html documents on_block');
ok(sdkRust.includes('on_logs'), 'sdk-rust.html documents on_logs');
ok(sdkRust.includes('close()'), 'sdk-rust.html documents close()');
// Sidebar link
ok(sdkRust.includes('href="#client-ws"'), 'sdk-rust.html sidebar has link to WebSocket section');

// ------------------------------------------------------------------
// D10: contract-reference.html has portal nav and developers.js
// ------------------------------------------------------------------
console.log('[D10] contract-reference.html portal integration');

const contractRef = read('contract-reference.html');
ok(contractRef.includes('class="nav"'), 'contract-reference.html has portal nav bar');
ok(contractRef.includes('class="nav-menu"'), 'contract-reference.html has nav-menu');
ok(contractRef.includes('searchOverlay'), 'contract-reference.html has search overlay');
ok(contractRef.includes('src="js/developers.js"'), 'contract-reference.html includes developers.js');
ok(contractRef.includes('developers.css'), 'contract-reference.html links developers.css');
ok(contractRef.includes('id="navToggle"'), 'contract-reference.html has nav toggle');

// ------------------------------------------------------------------
// D11: Search index anchors are valid
// ------------------------------------------------------------------
console.log('[D11] Search index anchor validation');

// Extract SEARCH_INDEX entries with anchors
const indexMatch = jsContent.match(/SEARCH_INDEX\s*=\s*\[([\s\S]*?)\];/);
ok(indexMatch, 'SEARCH_INDEX found in developers.js');

if (indexMatch) {
    const indexBody = indexMatch[1];
    // Extract all url values
    const urlMatches = indexBody.match(/url:\s*'([^']+)'/g) || [];
    const urls = urlMatches.map(m => m.match(/url:\s*'([^']+)'/)[1]);

    let anchorUrls = urls.filter(u => u.includes('#'));
    ok(anchorUrls.length >= 20, `SEARCH_INDEX has ${anchorUrls.length} anchor URLs (>= 20)`);

    // Validate each anchor points to an existing id in the file
    let brokenAnchors = [];
    anchorUrls.forEach(url => {
        const [file, anchor] = url.split('#');
        const filePath = path.join(DEV, file);
        if (!fs.existsSync(filePath)) {
            brokenAnchors.push(`${url} — file not found`);
            return;
        }
        const html = fs.readFileSync(filePath, 'utf8');
        if (!html.includes(`id="${anchor}"`)) {
            brokenAnchors.push(`${url} — id="${anchor}" not found`);
        }
    });

    ok(brokenAnchors.length === 0,
        `All search index anchors are valid (broken: ${brokenAnchors.join(', ') || 'none'})`);
}

// ------------------------------------------------------------------
// Mobile nav handler in developers.js
// ------------------------------------------------------------------
console.log('[Mobile nav] initMobileNav in developers.js');

ok(jsContent.includes('function initMobileNav()'), 'developers.js has initMobileNav function');
ok(jsContent.includes('initMobileNav()'), 'developers.js calls initMobileNav on DOMContentLoaded');
ok(jsContent.includes("getElementById('navToggle')"), 'initMobileNav targets #navToggle');
ok(jsContent.includes("classList.toggle('active')"), 'initMobileNav toggles .active class');

// ------------------------------------------------------------------
// Structural integrity tests
// ------------------------------------------------------------------
console.log('[Structural] Portal-wide checks');

// All 15 HTML pages exist
ok(ALL_PAGES.length === 15, `15 HTML pages found (got ${ALL_PAGES.length})`);

// developers.js has all 9 init functions
const initFunctions = [
    'initSidebar', 'initScrollSpy', 'initCodeCopy', 'initLangTabs',
    'initSearch', 'initNetworkSelector', 'initNavHighlight', 'initMobileNav'
];
initFunctions.forEach(fn => {
    ok(jsContent.includes(`function ${fn}(`), `developers.js has ${fn} function`);
});

// DOMContentLoaded calls all init functions
const dclMatch = jsContent.match(/DOMContentLoaded.*?\{([\s\S]*?)\}/);
if (dclMatch) {
    initFunctions.forEach(fn => {
        ok(dclMatch[1].includes(`${fn}()`), `DOMContentLoaded calls ${fn}()`);
    });
}

// developers.css exists and has rules
ok(fs.existsSync(CSS), 'developers.css exists');
ok(cssContent.length > 100, 'developers.css has substantial content');

// Every page links to Font Awesome
ALL_PAGES.forEach(file => {
    const html = read(file);
    ok(html.includes('font-awesome'), `${file} links Font Awesome`);
});

// ------------------------------------------------------------------
// Summary
// ------------------------------------------------------------------

// ------------------------------------------------------------------
// D13: RPC endpoint parity with docs/guides/RPC_API_REFERENCE.md
// ------------------------------------------------------------------
console.log('[D13] RPC endpoint parity with RPC_API_REFERENCE');

const rpcReferenceHtml = read('rpc-reference.html');
const canonicalRpcMethods = [
    'getBalance', 'getAccount', 'getBlock', 'getLatestBlock', 'getSlot',
    'getTransaction', 'sendTransaction', 'getTotalBurned', 'getValidators',
    'getMetrics', 'health', 'getPeers', 'getNetworkInfo',
    'getValidatorInfo', 'getValidatorPerformance', 'getChainStatus',
    'stake', 'unstake'
];

canonicalRpcMethods.forEach(method => {
    ok(rpcReferenceMd.includes(`\`${method}\``), `RPC_API_REFERENCE documents ${method}`);
    ok(
        rpcReferenceHtml.includes(`id="${method}"`) ||
        rpcReferenceHtml.includes(`>${method}<`) ||
        rpcReferenceHtml.includes(`"method":"${method}"`),
        `developers/rpc-reference.html documents ${method}`
    );
});

console.log(`\n========================================`);
console.log(`Phase 19 Results: ${pass} passed, ${fail} failed out of ${pass + fail}`);
console.log(`========================================`);
process.exit(fail > 0 ? 1 : 0);

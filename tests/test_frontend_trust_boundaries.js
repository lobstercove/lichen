// Frontend trust-boundary checks for P4-2 high-value entrypoints
// Run: node tests/test_frontend_trust_boundaries.js

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
const dexHtml = fs.readFileSync(path.join(root, 'dex', 'index.html'), 'utf8');
const dexJs = fs.readFileSync(path.join(root, 'dex', 'dex.js'), 'utf8');
const dexHeaders = fs.readFileSync(path.join(root, 'dex', '_headers'), 'utf8');
const monitoringHtml = fs.readFileSync(path.join(root, 'monitoring', 'index.html'), 'utf8');
const monitoringJs = fs.readFileSync(path.join(root, 'monitoring', 'js', 'monitoring.js'), 'utf8');
const monitoringHeaders = fs.readFileSync(path.join(root, 'monitoring', '_headers'), 'utf8');
const playgroundHtml = fs.readFileSync(path.join(root, 'programs', 'playground.html'), 'utf8');
const playgroundJs = fs.readFileSync(path.join(root, 'programs', 'js', 'playground-complete.js'), 'utf8');
const programsIndexHtml = fs.readFileSync(path.join(root, 'programs', 'index.html'), 'utf8');
const programsLandingJs = fs.readFileSync(path.join(root, 'programs', 'js', 'landing.js'), 'utf8');
const programsHeaders = fs.readFileSync(path.join(root, 'programs', '_headers'), 'utf8');
const developersIndexHtml = fs.readFileSync(path.join(root, 'developers', 'index.html'), 'utf8');
const developersLichenIdHtml = fs.readFileSync(path.join(root, 'developers', 'lichenid.html'), 'utf8');
const developersPlaygroundHtml = fs.readFileSync(path.join(root, 'developers', 'playground.html'), 'utf8');
const developersArchitectureHtml = fs.readFileSync(path.join(root, 'developers', 'architecture.html'), 'utf8');
const developersGettingStartedHtml = fs.readFileSync(path.join(root, 'developers', 'getting-started.html'), 'utf8');
const developersChangelogHtml = fs.readFileSync(path.join(root, 'developers', 'changelog.html'), 'utf8');
const developersValidatorHtml = fs.readFileSync(path.join(root, 'developers', 'validator.html'), 'utf8');
const developersCliReferenceHtml = fs.readFileSync(path.join(root, 'developers', 'cli-reference.html'), 'utf8');
const developersRpcReferenceHtml = fs.readFileSync(path.join(root, 'developers', 'rpc-reference.html'), 'utf8');
const developersWsReferenceHtml = fs.readFileSync(path.join(root, 'developers', 'ws-reference.html'), 'utf8');
const developersSdkJsHtml = fs.readFileSync(path.join(root, 'developers', 'sdk-js.html'), 'utf8');
const developersSdkPythonHtml = fs.readFileSync(path.join(root, 'developers', 'sdk-python.html'), 'utf8');
const developersSdkRustHtml = fs.readFileSync(path.join(root, 'developers', 'sdk-rust.html'), 'utf8');
const developersContractsHtml = fs.readFileSync(path.join(root, 'developers', 'contracts.html'), 'utf8');
const developersContractReferenceHtml = fs.readFileSync(path.join(root, 'developers', 'contract-reference.html'), 'utf8');
const developersZkPrivacyHtml = fs.readFileSync(path.join(root, 'developers', 'zk-privacy.html'), 'utf8');
const developersJs = fs.readFileSync(path.join(root, 'developers', 'js', 'developers.js'), 'utf8');
const developersHeaders = fs.readFileSync(path.join(root, 'developers', '_headers'), 'utf8');
const marketplaceIndexHtml = fs.readFileSync(path.join(root, 'marketplace', 'index.html'), 'utf8');
const marketplaceIndexJs = fs.readFileSync(path.join(root, 'marketplace', 'js', 'marketplace.js'), 'utf8');
const marketplaceBrowseHtml = fs.readFileSync(path.join(root, 'marketplace', 'browse.html'), 'utf8');
const marketplaceBrowseJs = fs.readFileSync(path.join(root, 'marketplace', 'js', 'browse.js'), 'utf8');
const marketplaceCreateHtml = fs.readFileSync(path.join(root, 'marketplace', 'create.html'), 'utf8');
const marketplaceCreateJs = fs.readFileSync(path.join(root, 'marketplace', 'js', 'create.js'), 'utf8');
const marketplaceProfileHtml = fs.readFileSync(path.join(root, 'marketplace', 'profile.html'), 'utf8');
const marketplaceProfileJs = fs.readFileSync(path.join(root, 'marketplace', 'js', 'profile.js'), 'utf8');
const marketplaceItemHtml = fs.readFileSync(path.join(root, 'marketplace', 'item.html'), 'utf8');
const marketplaceItemJs = fs.readFileSync(path.join(root, 'marketplace', 'js', 'item.js'), 'utf8');
const marketplaceHeaders = fs.readFileSync(path.join(root, 'marketplace', '_headers'), 'utf8');
const websiteIndexHtml = fs.readFileSync(path.join(root, 'website', 'index.html'), 'utf8');
const websiteJs = fs.readFileSync(path.join(root, 'website', 'script.js'), 'utf8');
const websiteHeaders = fs.readFileSync(path.join(root, 'website', '_headers'), 'utf8');
const explorerAddressHtml = fs.readFileSync(path.join(root, 'explorer', 'address.html'), 'utf8');
const explorerAddressJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'address.js'), 'utf8');
const explorerTransactionHtml = fs.readFileSync(path.join(root, 'explorer', 'transaction.html'), 'utf8');
const explorerTransactionJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'transaction.js'), 'utf8');
const explorerContractHtml = fs.readFileSync(path.join(root, 'explorer', 'contract.html'), 'utf8');
const explorerContractJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'contract.js'), 'utf8');
const explorerDashboardHtml = fs.readFileSync(path.join(root, 'explorer', 'index.html'), 'utf8');
const explorerSharedJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'explorer.js'), 'utf8');
const explorerBlocksHtml = fs.readFileSync(path.join(root, 'explorer', 'blocks.html'), 'utf8');
const explorerBlocksJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'blocks.js'), 'utf8');
const explorerValidatorsHtml = fs.readFileSync(path.join(root, 'explorer', 'validators.html'), 'utf8');
const explorerValidatorsJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'validators.js'), 'utf8');
const explorerTransactionsHtml = fs.readFileSync(path.join(root, 'explorer', 'transactions.html'), 'utf8');
const explorerTransactionsJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'transactions.js'), 'utf8');
const explorerAgentsHtml = fs.readFileSync(path.join(root, 'explorer', 'agents.html'), 'utf8');
const explorerAgentsJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'agents.js'), 'utf8');
const explorerContractsHtml = fs.readFileSync(path.join(root, 'explorer', 'contracts.html'), 'utf8');
const explorerContractsJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'contracts.js'), 'utf8');
const explorerBlockHtml = fs.readFileSync(path.join(root, 'explorer', 'block.html'), 'utf8');
const explorerBlockJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'block.js'), 'utf8');
const explorerPrivacyHtml = fs.readFileSync(path.join(root, 'explorer', 'privacy.html'), 'utf8');
const explorerPrivacyJs = fs.readFileSync(path.join(root, 'explorer', 'js', 'privacy.js'), 'utf8');
const explorerHeaders = fs.readFileSync(path.join(root, 'explorer', '_headers'), 'utf8');
const walletHtml = fs.readFileSync(path.join(root, 'wallet', 'index.html'), 'utf8');
const walletJs = fs.readFileSync(path.join(root, 'wallet', 'js', 'wallet.js'), 'utf8');
const walletIdentityJs = fs.readFileSync(path.join(root, 'wallet', 'js', 'identity.js'), 'utf8');
const walletBase58Js = fs.readFileSync(path.join(root, 'wallet', 'js', 'base58.js'), 'utf8');
const walletBootstrapJs = fs.readFileSync(path.join(root, 'wallet', 'js', 'wallet-bootstrap.js'), 'utf8');
const walletHeaders = fs.readFileSync(path.join(root, 'wallet', '_headers'), 'utf8');

console.log('\n── DEX Trust Boundary ──');

assert(!dexHtml.includes("onclick=\"document.querySelector('[data-wm-tab=extension]').click()\""), 'P4-2 DEX no longer relies on inline wallet-tab onclick');
assert(dexHtml.includes('id="wmOpenExtensionTabBtn"'), 'P4-2 DEX wallet modal has a dedicated extension-tab button id');
assert(dexJs.includes("document.getElementById('wmOpenExtensionTabBtn')"), 'P4-2 DEX binds the extension-tab button via JavaScript');
assert(dexJs.includes("switchWmTab('extension')"), 'P4-2 DEX switches wallet tabs without inline handlers');
assert(dexHtml.includes('integrity="sha512-DTOQO9RWCH3ppGqcWaEA1BIZOC6xxalwEsw9c2QQeAIftl+Vegovlnee1c9QX4TctnWMn13TZye+giMm8e2LwA=="'), 'P4-2 DEX pins Font Awesome with SRI');
assert(dexHeaders.includes('/index.html'), 'P4-2 DEX has a page-specific CSP header rule');
assert(dexHeaders.includes("script-src 'self'"), 'P4-2 DEX CSP restricts script sources to self');
assert(!dexHeaders.includes("script-src 'self' 'unsafe-inline'"), 'P4-2 DEX CSP does not allow inline scripts');
assert(dexHeaders.includes("frame-src 'self' blob:"), 'P4-2 DEX CSP allows blob-backed chart frames required by the hosted TradingView bundle');
assert(dexHeaders.includes("worker-src 'self' blob:"), 'P4-2 DEX CSP allows blob-backed chart workers required by the hosted TradingView bundle');

console.log('\n── Monitoring Trust Boundary ──');

assert(!monitoringHtml.includes('onchange="switchNetwork(this.value)"'), 'P4-2 Monitoring no longer relies on inline network-switch handlers');
assert(!monitoringHtml.includes('onclick="dismissAlert()"'), 'P4-2 Monitoring no longer relies on inline alert-dismiss handlers');
assert(!monitoringHtml.includes('onclick="clearEvents()"'), 'P4-2 Monitoring no longer relies on inline event-feed clear handlers');
assert(!monitoringHtml.includes("onclick=\"setTPSRange('1m', event)\""), 'P4-2 Monitoring no longer relies on inline TPS-range handlers');
assert(!monitoringHtml.includes('onclick="clearThreats()"'), 'P4-2 Monitoring no longer relies on inline threat clear handlers');
assert(!monitoringHtml.includes('onclick="killswitchBanIP()"'), 'P4-2 Monitoring no longer relies on inline kill-switch handlers');
assert(monitoringHtml.includes('id="killswitchEmergencyShutdownBtn"'), 'P4-2 Monitoring exposes dedicated kill-switch button ids');
assert(monitoringHtml.includes('data-tps-range="15m"'), 'P4-2 Monitoring uses data attributes for TPS-range controls');
assert(monitoringHtml.includes('integrity="sha512-DTOQO9RWCH3ppGqcWaEA1BIZOC6xxalwEsw9c2QQeAIftl+Vegovlnee1c9QX4TctnWMn13TZye+giMm8e2LwA=="'), 'P4-2 Monitoring pins Font Awesome with SRI');
assert(monitoringJs.includes('function bindStaticControls()'), 'P4-2 Monitoring centralizes static control binding in JavaScript');
assert(monitoringJs.includes("document.getElementById('killswitchEmergencyShutdownBtn')?.addEventListener('click', killswitchEmergencyShutdown);"), 'P4-2 Monitoring binds emergency shutdown in JavaScript');
assert(monitoringJs.includes("document.querySelectorAll('[data-tps-range]').forEach((button) => {"), 'P4-2 Monitoring binds TPS controls in JavaScript');
assert(monitoringHeaders.includes('/index.html'), 'P4-2 Monitoring has a page-specific CSP header rule');
assert(monitoringHeaders.includes("script-src 'self'"), 'P4-2 Monitoring CSP restricts script sources to self');
assert(!monitoringHeaders.includes("script-src 'self' 'unsafe-inline'"), 'P4-2 Monitoring CSP does not allow inline scripts');

console.log('\n── Playground Trust Boundary ──');

assert(!playgroundHtml.includes('onclick="copyProgramId()"'), 'P4-2 Programs playground no longer uses inline copy handler');
assert(!playgroundHtml.includes("onclick=\"document.querySelector('[data-wm-tab=extension]').click()\""), 'P4-2 Programs playground no longer uses inline wallet-tab onclick');
assert(playgroundHtml.includes('id="copyProgramInfoIdBtn"'), 'P4-2 Programs playground exposes a dedicated copy button id');
assert(playgroundHtml.includes('id="wmOpenExtensionTabBtn"'), 'P4-2 Programs playground exposes a dedicated extension-tab button id');
assert(playgroundJs.includes("document.getElementById('copyProgramInfoIdBtn')?.addEventListener('click'"), 'P4-2 Programs playground binds program-id copy in JavaScript');
assert(playgroundJs.includes("document.getElementById('wmOpenExtensionTabBtn')?.addEventListener('click'"), 'P4-2 Programs playground binds wallet-tab switching in JavaScript');
assert(!playgroundJs.includes('window.copyProgramId = () =>'), 'P4-2 Programs playground no longer exports a global inline copy hook');
assert(playgroundHtml.includes('integrity="sha512-DTOQO9RWCH3ppGqcWaEA1BIZOC6xxalwEsw9c2QQeAIftl+Vegovlnee1c9QX4TctnWMn13TZye+giMm8e2LwA=="'), 'P4-2 Programs playground pins Font Awesome with SRI');
assert(playgroundHtml.includes('integrity="sha512-ZG31AN9z/CQD1YDDAK4RUAvogwbJHv6bHrumrnMLzdCrVu4HeAqrUX7Jsal/cbUwXGfaMUNmQU04tQ8XXl5Znw=="'), 'P4-2 Programs playground pins the Monaco loader with SRI');
assert(programsHeaders.includes('/playground.html'), 'P4-2 Programs playground has a page-specific CSP header rule');
assert(programsHeaders.includes("script-src 'self' https://cdnjs.cloudflare.com"), 'P4-2 Programs playground CSP pins scripts to self and cdnjs');
assert(!programsHeaders.includes("script-src 'self' 'unsafe-inline'"), 'P4-2 Programs playground CSP does not allow inline scripts');

console.log('\n── Programs Landing Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(programsIndexHtml), 'P4-2 Programs landing no longer relies on inline DOM event handlers');
assert(programsIndexHtml.includes('data-programs-action="copy-code"'), 'P4-2 Programs landing exposes data-driven code-copy controls');
assert(programsIndexHtml.includes('data-programs-action="view-code" data-example="token"'), 'P4-2 Programs landing exposes data-driven example view controls');
assert(programsIndexHtml.includes('integrity="sha512-DTOQO9RWCH3ppGqcWaEA1BIZOC6xxalwEsw9c2QQeAIftl+Vegovlnee1c9QX4TctnWMn13TZye+giMm8e2LwA=="'), 'P4-2 Programs landing pins Font Awesome with SRI');
assert(programsLandingJs.includes('function bindStaticControls()'), 'P4-2 Programs landing centralizes static control binding in JavaScript');
assert(programsLandingJs.includes("const control = event.target.closest('[data-programs-action]');"), 'P4-2 Programs landing dispatches controls through data attributes');
assert(programsLandingJs.includes("case 'copy-code':"), 'P4-2 Programs landing handles code-copy actions in JavaScript');
assert(programsLandingJs.includes("case 'view-code':"), 'P4-2 Programs landing handles example-view actions in JavaScript');
assert(!programsLandingJs.includes('onclick="'), 'P4-2 Programs landing runtime no longer emits inline onclick handlers');
assert(programsHeaders.includes('/index.html'), 'P4-2 Programs landing has a page-specific CSP header rule');
assert(programsHeaders.includes("/\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Programs landing protects the root route with strict CSP');
assert(programsHeaders.includes("/index.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Programs landing CSP restricts scripts to self');

console.log('\n── Developers LichenID Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(developersLichenIdHtml), 'P4-2 Developers LichenID no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(developersLichenIdHtml), 'P4-2 Developers LichenID no longer relies on inline script blocks');
assert(developersLichenIdHtml.includes('data-sdk-lang="js"'), 'P4-2 Developers LichenID exposes data-driven SDK tabs');
assert(developersJs.includes('function initSdkTabs()'), 'P4-2 Developers LichenID centralizes SDK tab binding in JavaScript');
assert(developersJs.includes("const tabs = document.querySelectorAll('.sdk-tab[data-sdk-lang]');"), 'P4-2 Developers LichenID queries SDK tabs in JavaScript');
assert(developersJs.includes('tabs.forEach((button) => {'), 'P4-2 Developers LichenID binds SDK tabs in JavaScript');
assert(developersHeaders.includes('/lichenid.html'), 'P4-2 Developers LichenID has a page-specific CSP header rule');
assert(developersHeaders.includes("/lichenid.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers LichenID CSP restricts scripts to self');

console.log('\n── Developers Home Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(developersIndexHtml), 'P4-2 Developers home no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(developersIndexHtml), 'P4-2 Developers home no longer relies on inline script blocks');
assert(developersJs.includes('function initDeveloperHomeStats()'), 'P4-2 Developers home centralizes homepage bootstrap in JavaScript');
assert(developersJs.includes("const statBlockHeight = document.getElementById('statBlockHeight');"), 'P4-2 Developers home targets homepage stats in JavaScript');
assert(developersJs.includes("developerHomeWs.onerror = () => { developerHomeWs.close(); };"), 'P4-2 Developers home keeps WebSocket error handling in JavaScript');
assert(developersHeaders.includes('/index.html'), 'P4-2 Developers home has a page-specific CSP header rule');
assert(developersHeaders.includes("/\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers home protects the root route with strict CSP');
assert(developersHeaders.includes("/index.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers home CSP restricts scripts to self');

console.log('\n── Developers Docs Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(developersPlaygroundHtml), 'P4-2 Developers playground no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(developersPlaygroundHtml), 'P4-2 Developers playground no longer relies on inline script blocks');
assert(developersPlaygroundHtml.includes('href="../programs/playground.html"'), 'P4-2 Developers playground uses a relative Programs fullscreen link that shared JavaScript can rewrite');
assert(developersPlaygroundHtml.includes('src="../programs/playground.html"'), 'P4-2 Developers playground uses a relative Programs iframe source that shared JavaScript can rewrite');
assert(!/\son[a-z]+="/i.test(developersArchitectureHtml), 'P4-2 Developers architecture no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(developersArchitectureHtml), 'P4-2 Developers architecture no longer relies on inline script blocks');
assert(!/\son[a-z]+="/i.test(developersGettingStartedHtml), 'P4-2 Developers getting-started no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(developersGettingStartedHtml), 'P4-2 Developers getting-started no longer relies on inline script blocks');
assert(!/\son[a-z]+="/i.test(developersChangelogHtml), 'P4-2 Developers changelog no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(developersChangelogHtml), 'P4-2 Developers changelog no longer relies on inline script blocks');
assert(!/\son[a-z]+="/i.test(developersValidatorHtml), 'P4-2 Developers validator no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(developersValidatorHtml), 'P4-2 Developers validator no longer relies on inline script blocks');
assert(!/\son[a-z]+="/i.test(developersCliReferenceHtml), 'P4-2 Developers CLI reference no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(developersCliReferenceHtml), 'P4-2 Developers CLI reference no longer relies on inline script blocks');
assert(developersJs.includes('function initChangelogFilters()'), 'P4-2 Developers changelog centralizes filter binding in JavaScript');
assert(developersJs.includes("const filterBtns = document.querySelectorAll('.changelog-filter-btn');"), 'P4-2 Developers changelog queries filter controls in JavaScript');
assert(developersJs.includes("const filter = button.dataset.filter || 'all';"), 'P4-2 Developers changelog resolves the requested filter in JavaScript');
assert(developersHeaders.includes('/architecture.html'), 'P4-2 Developers architecture has a page-specific CSP header rule');
assert(developersHeaders.includes("/architecture.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers architecture CSP restricts scripts to self');
assert(developersHeaders.includes('/getting-started.html'), 'P4-2 Developers getting-started has a page-specific CSP header rule');
assert(developersHeaders.includes("/getting-started.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers getting-started CSP restricts scripts to self');
assert(developersHeaders.includes('/changelog.html'), 'P4-2 Developers changelog has a page-specific CSP header rule');
assert(developersHeaders.includes("/changelog.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers changelog CSP restricts scripts to self');
assert(developersHeaders.includes('/validator.html'), 'P4-2 Developers validator has a page-specific CSP header rule');
assert(developersHeaders.includes("/validator.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers validator CSP restricts scripts to self');
assert(developersHeaders.includes('/cli-reference.html'), 'P4-2 Developers CLI reference has a page-specific CSP header rule');
assert(developersHeaders.includes("/cli-reference.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers CLI reference CSP restricts scripts to self');
assert(developersHeaders.includes('/playground.html'), 'P4-2 Developers playground has a page-specific CSP header rule');
assert(developersHeaders.includes("/playground.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Developers playground CSP restricts scripts to self');
assert(developersHeaders.includes("frame-src 'self' https://programs.lichen.network http://localhost:3012"), 'P4-2 Developers playground CSP only allows the Programs app frame origins');

[
    ['rpc reference', developersRpcReferenceHtml, '/rpc-reference.html'],
    ['WebSocket reference', developersWsReferenceHtml, '/ws-reference.html'],
    ['SDK JS', developersSdkJsHtml, '/sdk-js.html'],
    ['SDK Python', developersSdkPythonHtml, '/sdk-python.html'],
    ['SDK Rust', developersSdkRustHtml, '/sdk-rust.html'],
    ['contracts', developersContractsHtml, '/contracts.html'],
    ['contract reference', developersContractReferenceHtml, '/contract-reference.html'],
    ['zk-privacy', developersZkPrivacyHtml, '/zk-privacy.html'],
].forEach(([label, html, route]) => {
    assert(!/\son[a-z]+="/i.test(html), `P4-2 Developers ${label} no longer relies on inline DOM event handlers`);
    assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(html), `P4-2 Developers ${label} no longer relies on inline script blocks`);
    assert(developersHeaders.includes(route), `P4-2 Developers ${label} has a page-specific CSP header rule`);
    assert(developersHeaders.includes(`${route}\n  Content-Security-Policy: default-src 'self';`), `P4-2 Developers ${label} CSP restricts scripts to self`);
});

console.log('\n── Marketplace Browse/Create Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(marketplaceIndexHtml), 'P4-2 Marketplace home no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(marketplaceIndexHtml), 'P4-2 Marketplace home no longer relies on inline script blocks');
assert(marketplaceIndexJs.includes('data-marketplace-action="buy"'), 'P4-2 Marketplace home emits data-driven buy controls');
assert(marketplaceIndexJs.includes('data-marketplace-href="browse.html?collection='), 'P4-2 Marketplace home emits data-driven featured-collection navigation');
assert(marketplaceIndexJs.includes("var trendingNFTs = document.getElementById('trendingNFTs');"), 'P4-2 Marketplace home delegates trending NFT actions in JavaScript');
assert(marketplaceIndexJs.includes("var recentSales = document.getElementById('recentSales');"), 'P4-2 Marketplace home delegates recent-sales navigation in JavaScript');
assert(!marketplaceIndexJs.includes('onclick="'), 'P4-2 Marketplace home runtime no longer emits inline onclick handlers');
assert(marketplaceHeaders.includes('/index.html'), 'P4-2 Marketplace home has a page-specific CSP header rule');
assert(marketplaceHeaders.includes("/\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Marketplace home protects the root route with strict CSP');
assert(marketplaceHeaders.includes("/index.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Marketplace home CSP restricts scripts to self');

assert(!/\son[a-z]+="/i.test(marketplaceBrowseHtml), 'P4-2 Marketplace browse no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(marketplaceBrowseHtml), 'P4-2 Marketplace browse no longer relies on inline script blocks');
assert(marketplaceBrowseHtml.includes('id="clearFiltersBtn"'), 'P4-2 Marketplace browse exposes a dedicated clear-filters button id');
assert(marketplaceBrowseJs.includes('data-browse-action="buy"'), 'P4-2 Marketplace browse emits data-driven buy controls');
assert(marketplaceBrowseJs.includes('data-browse-page="'), 'P4-2 Marketplace browse emits data-driven pagination controls');
assert(marketplaceBrowseJs.includes('data-browse-collection="'), 'P4-2 Marketplace browse emits data-driven collection filters');
assert(marketplaceBrowseJs.includes("var clearFiltersBtn = document.getElementById('clearFiltersBtn');"), 'P4-2 Marketplace browse binds static clear-filters controls in JavaScript');
assert(marketplaceBrowseJs.includes("var nftsGrid = document.getElementById('nftsGrid');"), 'P4-2 Marketplace browse delegates NFT card actions in JavaScript');
assert(!marketplaceBrowseJs.includes('onclick="'), 'P4-2 Marketplace browse runtime no longer emits inline onclick handlers');
assert(!marketplaceBrowseJs.includes('onchange="'), 'P4-2 Marketplace browse runtime no longer emits inline onchange handlers');
assert(marketplaceHeaders.includes('/browse.html'), 'P4-2 Marketplace browse has a page-specific CSP header rule');
assert(marketplaceHeaders.includes("/browse.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Marketplace browse CSP restricts scripts to self');

assert(!/\son[a-z]+="/i.test(marketplaceCreateHtml), 'P4-2 Marketplace create no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(marketplaceCreateHtml), 'P4-2 Marketplace create no longer relies on inline script blocks');
assert(marketplaceCreateHtml.includes('id="addPropertyBtn"'), 'P4-2 Marketplace create exposes a dedicated add-property button id');
assert(marketplaceCreateJs.includes('data-create-action="remove-file"'), 'P4-2 Marketplace create emits a data-driven remove-file control');
assert(marketplaceCreateJs.includes('data-create-action="remove-property"'), 'P4-2 Marketplace create emits data-driven property removal controls');
assert(marketplaceCreateJs.includes('data-property-field="trait_type"'), 'P4-2 Marketplace create emits data-driven property inputs');
assert(marketplaceCreateJs.includes("var addPropertyBtn = document.getElementById('addPropertyBtn');"), 'P4-2 Marketplace create binds add-property controls in JavaScript');
assert(marketplaceCreateJs.includes("var propertiesList = document.getElementById('propertiesList');"), 'P4-2 Marketplace create delegates property editing in JavaScript');
assert(!marketplaceCreateJs.includes('onclick="'), 'P4-2 Marketplace create runtime no longer emits inline onclick handlers');
assert(!marketplaceCreateJs.includes('onchange="'), 'P4-2 Marketplace create runtime no longer emits inline onchange handlers');
assert(marketplaceHeaders.includes('/create.html'), 'P4-2 Marketplace create has a page-specific CSP header rule');
assert(marketplaceHeaders.includes("/create.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Marketplace create CSP restricts scripts to self');

assert(!/\son[a-z]+="/i.test(marketplaceProfileHtml), 'P4-2 Marketplace profile no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(marketplaceProfileHtml), 'P4-2 Marketplace profile no longer relies on inline script blocks');
assert(marketplaceProfileHtml.includes('id="copyProfileAddressBtn"'), 'P4-2 Marketplace profile exposes a dedicated copy-address button id');
assert(marketplaceProfileJs.includes('data-profile-action="update-price"'), 'P4-2 Marketplace profile emits data-driven collected-NFT actions');
assert(marketplaceProfileJs.includes('data-profile-action="collection-offer"'), 'P4-2 Marketplace profile emits data-driven created-NFT actions');
assert(marketplaceProfileJs.includes('data-profile-action="accept-offer"'), 'P4-2 Marketplace profile emits data-driven offer actions');
assert(marketplaceProfileJs.includes("var copyProfileAddressBtn = document.getElementById('copyProfileAddressBtn');"), 'P4-2 Marketplace profile binds static copy controls in JavaScript');
assert(marketplaceProfileJs.includes("bindGrid('collectedGrid'"), 'P4-2 Marketplace profile delegates collected-grid actions in JavaScript');
assert(marketplaceProfileJs.includes("var offersTable = document.getElementById('offersTable');"), 'P4-2 Marketplace profile delegates offer actions in JavaScript');
assert(!marketplaceProfileJs.includes('onclick="'), 'P4-2 Marketplace profile runtime no longer emits inline onclick handlers');
assert(marketplaceHeaders.includes('/profile.html'), 'P4-2 Marketplace profile has a page-specific CSP header rule');
assert(marketplaceHeaders.includes("/profile.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Marketplace profile CSP restricts scripts to self');

assert(!/\son[a-z]+="/i.test(marketplaceItemHtml), 'P4-2 Marketplace item no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(marketplaceItemHtml), 'P4-2 Marketplace item no longer relies on inline script blocks');
assert(marketplaceItemJs.includes('data-item-action="accept-offer"'), 'P4-2 Marketplace item emits data-driven offer actions');
assert(marketplaceItemJs.includes("var offersList = document.getElementById('offersList');"), 'P4-2 Marketplace item delegates offer actions in JavaScript');
assert(!marketplaceItemJs.includes('onclick="'), 'P4-2 Marketplace item runtime no longer emits inline onclick handlers');
assert(marketplaceHeaders.includes('/item.html'), 'P4-2 Marketplace item has a page-specific CSP header rule');
assert(marketplaceHeaders.includes("/item.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Marketplace item CSP restricts scripts to self');

console.log('\n── Website Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(websiteIndexHtml), 'P4-2 Website index no longer relies on inline DOM event handlers');
assert(websiteIndexHtml.includes('data-website-action="copy-code"'), 'P4-2 Website index exposes data-driven code-copy controls');
assert(websiteJs.includes('function bindStaticControls()'), 'P4-2 Website index centralizes static control binding in JavaScript');
assert(websiteJs.includes("document.querySelectorAll('.copy-btn[data-website-action=\"copy-code\"]').forEach((button) => {"), 'P4-2 Website index binds code-copy controls in JavaScript');
assert(!websiteJs.includes('onclick="'), 'P4-2 Website runtime no longer emits inline onclick handlers');
assert(websiteHeaders.includes('/index.html'), 'P4-2 Website index has a page-specific CSP header rule');
assert(websiteHeaders.includes("/\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Website protects the root route with strict CSP');
assert(websiteHeaders.includes("/index.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Website index CSP restricts scripts to self');

console.log('\n── Wallet Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(walletHtml), 'P4-2 Wallet entrypoint no longer relies on inline DOM event handlers');
assert(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(walletHtml), 'P4-2 Wallet extracts inline scripts into external files');
assert(walletHtml.includes('data-wallet-action="showCreateWallet"'), 'P4-2 Wallet welcome actions use data-driven dispatch');
assert(walletHtml.includes('data-wallet-trigger="importJsonFile"'), 'P4-2 Wallet keystore upload uses a dedicated JS trigger');
assert(walletHtml.includes('data-wallet-action="closeModal" data-wallet-arg="sendModal"'), 'P4-2 Wallet modals expose close actions via data attributes');
assert(walletHtml.includes('src="js/base58.js?v=20260403"'), 'P4-2 Wallet loads Base58 support from an external script');
assert(walletHtml.includes('src="js/wallet-bootstrap.js?v=20260403"'), 'P4-2 Wallet loads carousel and service-worker bootstrap from an external script');
assert(walletHtml.includes('integrity="sha512-DTOQO9RWCH3ppGqcWaEA1BIZOC6xxalwEsw9c2QQeAIftl+Vegovlnee1c9QX4TctnWMn13TZye+giMm8e2LwA=="'), 'P4-2 Wallet pins Font Awesome with SRI');
assert(walletHtml.includes('integrity="sha512-CNgIRecGo7nphbeZ04Sc13ka07paqdeTu0WR1IM4kNcpmBAUSHSQX0FslNhTDadL4O5SAGapGt4FodqL8My0mA=="'), 'P4-2 Wallet pins QRCode.js with SRI');
assert(walletHtml.includes('integrity="sha512-l8ZGwlcmmNhyMXRweX0SqrZHIdfK3UgOSJsdSK5ozqlXOsZDogZFYp+TUzzI7pFYGUmzTCKZjS1q0nHiWOvs0g=="'), 'P4-2 Wallet pins js-sha3 with SRI');
assert(walletJs.includes('function bindStaticControls()'), 'P4-2 Wallet centralizes entrypoint control binding in JavaScript');
assert(walletJs.includes("target.closest('[data-wallet-action], [data-wallet-trigger]')"), 'P4-2 Wallet dispatches static controls through data attributes');
assert(!walletJs.includes('onclick="'), 'P4-2 Wallet runtime no longer emits inline onclick handlers');
assert(!walletJs.includes('onkeypress="'), 'P4-2 Wallet runtime no longer emits inline keypress handlers');
assert(!walletIdentityJs.includes('onclick="'), 'P4-2 Wallet identity runtime no longer emits inline onclick handlers');
assert(walletIdentityJs.includes('data-wallet-action="showRegisterIdentityModal"'), 'P4-2 Wallet identity UI exposes data-driven actions');
assert(walletBase58Js.includes('var bs58 = window.bs58 ='), 'P4-2 Wallet externalizes Base58 support into a standalone script');
assert(walletBootstrapJs.includes("navigator.serviceWorker.register('./sw.js')"), 'P4-2 Wallet keeps service-worker bootstrap in an external script');
assert(walletHeaders.includes('/index.html'), 'P4-2 Wallet has a page-specific CSP header rule');
assert(walletHeaders.includes("script-src 'self' https://cdnjs.cloudflare.com"), 'P4-2 Wallet CSP pins scripts to self and cdnjs');
assert(walletHeaders.includes("worker-src 'self'"), 'P4-2 Wallet CSP restricts worker sources to self');
assert(!walletHeaders.includes("script-src 'self' 'unsafe-inline'"), 'P4-2 Wallet CSP does not allow inline scripts');

console.log('\n── Explorer Address Trust Boundary ──');

assert(!explorerAddressHtml.includes('onclick="copyAddressToClipboard('), 'P4-2 Explorer address no longer relies on inline copy handlers');
assert(explorerAddressHtml.includes('id="copyAddressBase58Btn"'), 'P4-2 Explorer address exposes a dedicated Base58 copy button id');
assert(explorerAddressHtml.includes('id="copyAddressEvmBtn"'), 'P4-2 Explorer address exposes a dedicated EVM copy button id');
assert(explorerAddressHtml.includes('id="copyAddressRawDataBtn"'), 'P4-2 Explorer address exposes a dedicated raw-data copy button id');
assert(explorerAddressHtml.includes('integrity="sha512-vPFW2/msTSDLL1vGc/5mQndcojozbjLV52ZAg0eb1awjc3AESwpR7O8mCE31ociBye1Wxosf2B7C6xw+AGsD6A=="'), 'P4-2 Explorer address pins js-sha3 with SRI');
assert(explorerAddressJs.includes('function bindStaticControls()'), 'P4-2 Explorer address centralizes static control binding in JavaScript');
assert(explorerAddressJs.includes("document.getElementById('copyAddressBase58Btn')?.addEventListener('click'"), 'P4-2 Explorer address binds copy buttons in JavaScript');
assert(explorerAddressJs.includes("document.getElementById('addressTxPrevBtn')?.addEventListener('click', prevTxPage);"), 'P4-2 Explorer address binds pagination controls in JavaScript');
assert(!explorerAddressJs.includes('onclick="'), 'P4-2 Explorer address runtime no longer emits inline onclick handlers');
assert(explorerHeaders.includes('/address.html'), 'P4-2 Explorer address has a page-specific CSP header rule');
assert(explorerHeaders.includes("script-src 'self' https://cdn.jsdelivr.net"), 'P4-2 Explorer address CSP pins scripts to self and jsDelivr');
assert(!explorerHeaders.includes("script-src 'self' 'unsafe-inline'"), 'P4-2 Explorer address CSP does not allow inline scripts');

console.log('\n── Explorer Transaction Trust Boundary ──');

assert(!explorerTransactionHtml.includes('onclick="copyToClipboard('), 'P4-2 Explorer transaction no longer relies on inline copy handlers');
assert(explorerTransactionHtml.includes('id="copyTxHashBtn"'), 'P4-2 Explorer transaction exposes a dedicated hash copy button id');
assert(explorerTransactionHtml.includes('id="copyProofRootBtn"'), 'P4-2 Explorer transaction exposes a dedicated proof-root copy button id');
assert(explorerTransactionHtml.includes('id="copyTxRawDataBtn"'), 'P4-2 Explorer transaction exposes a dedicated raw-data copy button id');
assert(explorerTransactionJs.includes('function bindStaticControls()'), 'P4-2 Explorer transaction centralizes static control binding in JavaScript');
assert(explorerTransactionJs.includes('function bindSignatureCopyButtons()'), 'P4-2 Explorer transaction centralizes signature copy binding in JavaScript');
assert(explorerTransactionJs.includes("document.getElementById('copyTxHashBtn')?.addEventListener('click'"), 'P4-2 Explorer transaction binds static copy controls in JavaScript');
assert(explorerTransactionJs.includes('bindSignatureCopyButtons();'), 'P4-2 Explorer transaction binds runtime signature copy controls after render');
assert(!explorerTransactionJs.includes('onclick="'), 'P4-2 Explorer transaction runtime no longer emits inline onclick handlers');
assert(explorerHeaders.includes('/transaction.html'), 'P4-2 Explorer transaction has a page-specific CSP header rule');
assert(explorerHeaders.includes("/transaction.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer transaction CSP restricts scripts to self');

console.log('\n── Explorer Contract Trust Boundary ──');

assert(!explorerContractHtml.includes('onclick="copyAddress()"'), 'P4-2 Explorer contract no longer relies on inline address-copy handlers');
assert(!explorerContractHtml.includes("onclick=\"switchTab('"), 'P4-2 Explorer contract no longer relies on inline tab handlers');
assert(explorerContractHtml.includes('id="copyContractAddressBtn"'), 'P4-2 Explorer contract exposes a dedicated copy button id');
assert(explorerContractHtml.includes('data-contract-tab="abi"'), 'P4-2 Explorer contract exposes data-driven tab controls');
assert(explorerContractJs.includes('function bindStaticControls()'), 'P4-2 Explorer contract centralizes static control binding in JavaScript');
assert(explorerContractJs.includes("document.getElementById('copyContractAddressBtn')?.addEventListener('click', copyAddress);"), 'P4-2 Explorer contract binds address copy in JavaScript');
assert(explorerContractJs.includes("document.querySelectorAll('.tab-btn[data-contract-tab]').forEach((button) => {"), 'P4-2 Explorer contract binds tab switching in JavaScript');
assert(explorerContractJs.includes('function bindProfileLogoFallback(profileEl)'), 'P4-2 Explorer contract binds token-logo fallback in JavaScript');
assert(explorerContractJs.includes('bindProfileLogoFallback(profileEl);'), 'P4-2 Explorer contract applies token-logo fallback after render');
assert(!explorerContractJs.includes('onerror="'), 'P4-2 Explorer contract runtime no longer emits inline onerror handlers');
assert(explorerHeaders.includes('/contract.html'), 'P4-2 Explorer contract has a page-specific CSP header rule');
assert(explorerHeaders.includes("/contract.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer contract CSP restricts scripts to self');

console.log('\n── Explorer Dashboard Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(explorerDashboardHtml), 'P4-2 Explorer dashboard no longer relies on inline DOM event handlers');
assert(explorerSharedJs.includes('function bindDashboardCopyControls()'), 'P4-2 Explorer dashboard centralizes runtime copy binding in JavaScript');
assert(explorerSharedJs.includes("document.getElementById('blocksTable')?.addEventListener('click', (event) => {"), 'P4-2 Explorer dashboard binds latest-block copy controls in JavaScript');
assert(explorerSharedJs.includes("document.getElementById('txsTable')?.addEventListener('click', (event) => {"), 'P4-2 Explorer dashboard binds latest-transaction copy controls in JavaScript');
assert(!explorerSharedJs.includes('onclick="safeCopy(this)"'), 'P4-2 Explorer dashboard runtime no longer emits inline safeCopy handlers');
assert(explorerHeaders.includes('/index.html'), 'P4-2 Explorer dashboard has a page-specific CSP header rule');
assert(explorerHeaders.includes("/\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer dashboard protects the root route with strict CSP');
assert(explorerHeaders.includes("/index.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer dashboard CSP restricts scripts to self');

console.log('\n── Explorer Blocks Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(explorerBlocksHtml), 'P4-2 Explorer blocks no longer relies on inline DOM event handlers');
assert(explorerBlocksHtml.includes('id="blocksApplyFiltersBtn"'), 'P4-2 Explorer blocks exposes a dedicated apply-filters button id');
assert(explorerBlocksHtml.includes('id="blocksClearFiltersBtn"'), 'P4-2 Explorer blocks exposes a dedicated clear-filters button id');
assert(explorerBlocksJs.includes('function bindStaticControls()'), 'P4-2 Explorer blocks centralizes static control binding in JavaScript');
assert(explorerBlocksJs.includes("document.getElementById('blocksApplyFiltersBtn')?.addEventListener('click', applyFilters);"), 'P4-2 Explorer blocks binds filter application in JavaScript');
assert(explorerBlocksJs.includes("document.getElementById('prevPage')?.addEventListener('click', previousPage);"), 'P4-2 Explorer blocks binds pagination in JavaScript');
assert(explorerBlocksJs.includes("document.getElementById('blocksTableFull')?.addEventListener('click', (event) => {"), 'P4-2 Explorer blocks binds runtime hash-copy controls in JavaScript');
assert(!explorerBlocksJs.includes('onclick="'), 'P4-2 Explorer blocks runtime no longer emits inline onclick handlers');
assert(explorerHeaders.includes('/blocks.html'), 'P4-2 Explorer blocks has a page-specific CSP header rule');
assert(explorerHeaders.includes("/blocks.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer blocks CSP restricts scripts to self');

console.log('\n── Explorer Validators Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(explorerValidatorsHtml), 'P4-2 Explorer validators no longer relies on inline DOM event handlers');
assert(explorerValidatorsJs.includes('function bindStaticControls()'), 'P4-2 Explorer validators centralizes static control binding in JavaScript');
assert(explorerValidatorsJs.includes("document.getElementById('prevPage')?.addEventListener('click', previousPage);"), 'P4-2 Explorer validators binds pagination in JavaScript');
assert(explorerValidatorsJs.includes("document.getElementById('validatorsTable')?.addEventListener('click', (event) => {"), 'P4-2 Explorer validators binds runtime address-copy controls in JavaScript');
assert(!explorerValidatorsJs.includes('onclick="'), 'P4-2 Explorer validators runtime no longer emits inline onclick handlers');
assert(explorerHeaders.includes('/validators.html'), 'P4-2 Explorer validators has a page-specific CSP header rule');
assert(explorerHeaders.includes("/validators.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer validators CSP restricts scripts to self');

console.log('\n── Explorer Transactions Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(explorerTransactionsHtml), 'P4-2 Explorer transactions no longer relies on inline DOM event handlers');
assert(explorerTransactionsHtml.includes('id="transactionsApplyFiltersBtn"'), 'P4-2 Explorer transactions exposes a dedicated apply-filters button id');
assert(explorerTransactionsHtml.includes('id="transactionsClearFiltersBtn"'), 'P4-2 Explorer transactions exposes a dedicated clear-filters button id');
assert(explorerTransactionsJs.includes('function bindStaticControls()'), 'P4-2 Explorer transactions centralizes static control binding in JavaScript');
assert(explorerTransactionsJs.includes("document.getElementById('transactionsApplyFiltersBtn')?.addEventListener('click', applyFilters);"), 'P4-2 Explorer transactions binds filter application in JavaScript');
assert(explorerTransactionsJs.includes("document.getElementById('prevPage')?.addEventListener('click', previousPage);"), 'P4-2 Explorer transactions binds pagination in JavaScript');
assert(explorerTransactionsJs.includes("document.getElementById('transactionsTable')?.addEventListener('click', (event) => {"), 'P4-2 Explorer transactions binds runtime signature-copy controls in JavaScript');
assert(!explorerTransactionsJs.includes('onclick="'), 'P4-2 Explorer transactions runtime no longer emits inline onclick handlers');
assert(explorerHeaders.includes('/transactions.html'), 'P4-2 Explorer transactions has a page-specific CSP header rule');
assert(explorerHeaders.includes("/transactions.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer transactions CSP restricts scripts to self');

console.log('\n── Explorer Agents Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(explorerAgentsHtml), 'P4-2 Explorer agents no longer relies on inline DOM event handlers');
assert(explorerAgentsHtml.includes('id="agentsApplyFiltersBtn"'), 'P4-2 Explorer agents exposes a dedicated apply-filters button id');
assert(explorerAgentsHtml.includes('id="agentsClearFiltersBtn"'), 'P4-2 Explorer agents exposes a dedicated clear-filters button id');
assert(explorerAgentsJs.includes('function bindStaticControls()'), 'P4-2 Explorer agents centralizes static control binding in JavaScript');
assert(explorerAgentsJs.includes("document.getElementById('agentsApplyFiltersBtn')?.addEventListener('click', applyFilters);"), 'P4-2 Explorer agents binds filter application in JavaScript');
assert(explorerAgentsJs.includes("document.getElementById('prevPage')?.addEventListener('click', previousPage);"), 'P4-2 Explorer agents binds pagination in JavaScript');
assert(explorerAgentsJs.includes("document.getElementById('agentsTable')?.addEventListener('click', (event) => {"), 'P4-2 Explorer agents binds runtime address-copy controls in JavaScript');
assert(!explorerAgentsJs.includes('onclick="'), 'P4-2 Explorer agents runtime no longer emits inline onclick handlers');
assert(explorerHeaders.includes('/agents.html'), 'P4-2 Explorer agents has a page-specific CSP header rule');
assert(explorerHeaders.includes("/agents.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer agents CSP restricts scripts to self');

console.log('\n── Explorer Contracts Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(explorerContractsHtml), 'P4-2 Explorer contracts no longer relies on inline DOM event handlers');
assert(explorerContractsHtml.includes('data-contract-filter="all"'), 'P4-2 Explorer contracts exposes data-driven category filters');
assert(explorerContractsJs.includes('function bindStaticControls()'), 'P4-2 Explorer contracts centralizes static control binding in JavaScript');
assert(explorerContractsJs.includes("document.querySelectorAll('.tab-btn[data-contract-filter]').forEach(function (button) {"), 'P4-2 Explorer contracts binds category tabs in JavaScript');
assert(explorerContractsJs.includes("document.getElementById('prevPage')?.addEventListener('click', previousPage);"), 'P4-2 Explorer contracts binds pagination in JavaScript');
assert(explorerContractsJs.includes("document.getElementById('contractsTableBody')?.addEventListener('click', function (event) {"), 'P4-2 Explorer contracts binds row navigation in JavaScript');
assert(!explorerContractsJs.includes('onclick="'), 'P4-2 Explorer contracts runtime no longer emits inline onclick handlers');
assert(explorerHeaders.includes('/contracts.html'), 'P4-2 Explorer contracts has a page-specific CSP header rule');
assert(explorerHeaders.includes("/contracts.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer contracts CSP restricts scripts to self');

console.log('\n── Explorer Block Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(explorerBlockHtml), 'P4-2 Explorer block no longer relies on inline DOM event handlers');
assert(explorerBlockHtml.includes('id="copyBlockHashBtn"'), 'P4-2 Explorer block exposes a dedicated block-hash copy button id');
assert(explorerBlockHtml.includes('id="copyStateRootBtn"'), 'P4-2 Explorer block exposes a dedicated state-root copy button id');
assert(explorerBlockHtml.includes('id="copyTxRootBtn"'), 'P4-2 Explorer block exposes a dedicated tx-root copy button id');
assert(explorerBlockHtml.includes('id="copyBlockRawDataBtn"'), 'P4-2 Explorer block exposes a dedicated raw-data copy button id');
assert(explorerBlockJs.includes('function bindStaticControls()'), 'P4-2 Explorer block centralizes static control binding in JavaScript');
assert(explorerBlockJs.includes("document.getElementById('copyBlockHashBtn')?.addEventListener('click', function () {"), 'P4-2 Explorer block binds block-hash copy in JavaScript');
assert(explorerBlockJs.includes("document.getElementById(id)?.addEventListener('click', function (event) {"), 'P4-2 Explorer block binds navigation controls in JavaScript');
assert(explorerBlockJs.includes('function setNavigationHref(buttonId, href) {'), 'P4-2 Explorer block centralizes navigation target state in JavaScript');
assert(!explorerBlockJs.includes('onclick ='), 'P4-2 Explorer block no longer assigns click handlers through DOM onclick properties');
assert(explorerHeaders.includes('/block.html'), 'P4-2 Explorer block has a page-specific CSP header rule');
assert(explorerHeaders.includes("/block.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer block CSP restricts scripts to self');

console.log('\n── Explorer Privacy Trust Boundary ──');

assert(!/\son[a-z]+="/i.test(explorerPrivacyHtml), 'P4-2 Explorer privacy no longer relies on inline DOM event handlers');
assert(explorerPrivacyHtml.includes('id="refreshShieldedTxsBtn"'), 'P4-2 Explorer privacy exposes a dedicated refresh button id');
assert(explorerPrivacyHtml.includes('id="lookupNullifierBtn"'), 'P4-2 Explorer privacy exposes a dedicated nullifier-lookup button id');
assert(explorerPrivacyJs.includes('function bindStaticControls()'), 'P4-2 Explorer privacy centralizes static control binding in JavaScript');
assert(explorerPrivacyJs.includes("document.querySelectorAll('.address-tab[data-tab]').forEach((tab) => {"), 'P4-2 Explorer privacy binds tab switching in JavaScript');
assert(explorerPrivacyJs.includes("document.getElementById('refreshShieldedTxsBtn')?.addEventListener('click', refreshShieldedTxs);"), 'P4-2 Explorer privacy binds refresh in JavaScript');
assert(explorerPrivacyJs.includes("document.getElementById('lookupNullifierBtn')?.addEventListener('click', lookupNullifier);"), 'P4-2 Explorer privacy binds nullifier lookup in JavaScript');
assert(!explorerPrivacyJs.includes('onclick="'), 'P4-2 Explorer privacy runtime no longer emits inline onclick handlers');
assert(explorerHeaders.includes('/privacy.html'), 'P4-2 Explorer privacy has a page-specific CSP header rule');
assert(explorerHeaders.includes("/privacy.html\n  Content-Security-Policy: default-src 'self';"), 'P4-2 Explorer privacy CSP restricts scripts to self');

console.log(`\n${'═'.repeat(50)}`);
console.log(`Frontend Trust Boundaries: ${passed} passed, ${failed} failed (${passed + failed} total)`);
console.log(`${'═'.repeat(50)}`);
process.exit(failed > 0 ? 1 : 0);
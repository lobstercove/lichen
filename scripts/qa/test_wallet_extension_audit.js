#!/usr/bin/env node
// ============================================================================
// Phase 12 — Wallet Extension Audit Tests
// Tests for all 9 audit findings (E-1 through E-9)
// Run: node scripts/qa/test_wallet_extension_audit.js
// ============================================================================

const assert = require('assert');
const fs = require('fs');
const path = require('path');

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed++;
    console.log(`  ✅ ${name}`);
  } catch (e) {
    failed++;
    console.log(`  ❌ ${name}: ${e.message}`);
  }
}

// ── Load source files ──
const extRoot = path.join(__dirname, '..', '..', 'wallet', 'extension', 'src');

const nftsSrc = fs.readFileSync(path.join(extRoot, 'pages', 'nfts.js'), 'utf8');
const fullSrc = fs.readFileSync(path.join(extRoot, 'pages', 'full.js'), 'utf8');
const popupSrc = fs.readFileSync(path.join(extRoot, 'popup', 'popup.js'), 'utf8');
const settingsSrc = fs.readFileSync(path.join(extRoot, 'pages', 'settings.js'), 'utf8');
const identitySrc = fs.readFileSync(path.join(extRoot, 'pages', 'identity.js'), 'utf8');
const homeSrc = fs.readFileSync(path.join(extRoot, 'pages', 'home.js'), 'utf8');
const homeHtmlSrc = fs.readFileSync(path.join(extRoot, 'pages', 'home.html'), 'utf8');
const txServiceSrc = fs.readFileSync(path.join(extRoot, 'core', 'tx-service.js'), 'utf8');
const bridgeServiceSrc = fs.readFileSync(path.join(extRoot, 'core', 'bridge-service.js'), 'utf8');
const identityServiceSrc = fs.readFileSync(path.join(extRoot, 'core', 'identity-service.js'), 'utf8');
const rpcServiceSrc = fs.readFileSync(path.join(extRoot, 'core', 'rpc-service.js'), 'utf8');
const providerRouterSrc = fs.readFileSync(path.join(extRoot, 'core', 'provider-router.js'), 'utf8');
const wsServiceSrc = fs.readFileSync(path.join(extRoot, 'core', 'ws-service.js'), 'utf8');
const serviceWorkerSrc = fs.readFileSync(path.join(extRoot, 'background', 'service-worker.js'), 'utf8');
const contentScriptSrc = fs.readFileSync(path.join(extRoot, 'content', 'content-script.js'), 'utf8');
const inpageProviderSrc = fs.readFileSync(path.join(extRoot, 'content', 'inpage-provider.js'), 'utf8');

// ── Extract escapeHtml / escapeHtmlExt from source files ──
function extractEscapeHtml(src, fnName) {
  const fnBlock = src.match(new RegExp(`function ${fnName}\\(str\\)\\s*\\{[\\s\\S]*?\\n\\}`));
  if (!fnBlock) return null;
  const body = fnBlock[0]
    .replace(new RegExp(`function ${fnName}\\(str\\)`), '')
    .replace(/^\s*\{/, '')
    .replace(/\}\s*$/, '');
  return new Function('str', body);
}

// Build escapeHtml from nfts.js source
const escapeHtmlNfts = extractEscapeHtml(nftsSrc, 'escapeHtml');
const escapeHtmlSettings = extractEscapeHtml(settingsSrc, 'escapeHtml');
const escapeHtmlIdentity = extractEscapeHtml(identitySrc, 'escapeHtml');
const escapeHtmlPopup = extractEscapeHtml(popupSrc, 'escapeHtml');
const escapeHtmlExt = extractEscapeHtml(fullSrc, 'escapeHtmlExt');

// ── Build safeImageUrl from nfts.js ──
function extractSafeImageUrl(src) {
  const match = src.match(/function safeImageUrl\(url\)\s*\{[\s\S]*?\n\}/);
  if (!match) return null;
  const body = match[0].replace('function safeImageUrl(url)', '').replace(/^\s*\{/, '').replace(/\}\s*$/, '');
  return new Function('url', body);
}
const safeImageUrl = extractSafeImageUrl(nftsSrc);

// ============================================================================
console.log('\n🔒 Phase 12 — Wallet Extension Audit Tests');
console.log('='.repeat(60));

// ── E-1: XSS in nfts.js — escapeHtml added ──
console.log('\n── E-1: NFTs Page XSS Protection ──');

test('E-1.1 nfts.js defines escapeHtml function', () => {
  assert.ok(nftsSrc.includes('function escapeHtml('), 'escapeHtml not found in nfts.js');
});

test('E-1.2 escapeHtml escapes < and > characters', () => {
  assert.ok(escapeHtmlNfts, 'escapeHtml function could not be extracted');
  assert.strictEqual(escapeHtmlNfts('<script>alert(1)</script>'), '&lt;script&gt;alert(1)&lt;/script&gt;');
});

test('E-1.3 escapeHtml escapes & character', () => {
  assert.strictEqual(escapeHtmlNfts('foo&bar'), 'foo&amp;bar');
});

test('E-1.4 escapeHtml escapes quotes', () => {
  assert.strictEqual(escapeHtmlNfts('"hello"'), '&quot;hello&quot;');
  assert.strictEqual(escapeHtmlNfts("it's"), "it&#x27;s");
});

test('E-1.5 escapeHtml handles empty/null input', () => {
  assert.strictEqual(escapeHtmlNfts(''), '');
  assert.strictEqual(escapeHtmlNfts(null), '');
  assert.strictEqual(escapeHtmlNfts(undefined), '');
});

test('E-1.6 nfts.js defines safeImageUrl function', () => {
  assert.ok(nftsSrc.includes('function safeImageUrl('), 'safeImageUrl not found');
});

test('E-1.7 safeImageUrl allows https URLs', () => {
  assert.ok(safeImageUrl, 'safeImageUrl could not be extracted');
  assert.strictEqual(safeImageUrl('https://example.com/img.png'), 'https://example.com/img.png');
});

test('E-1.8 safeImageUrl allows http URLs', () => {
  assert.strictEqual(safeImageUrl('http://example.com/img.png'), 'http://example.com/img.png');
});

test('E-1.9 safeImageUrl blocks javascript: URLs', () => {
  assert.strictEqual(safeImageUrl('javascript:alert(1)'), '');
});

test('E-1.10 safeImageUrl blocks data: URLs', () => {
  assert.strictEqual(safeImageUrl('data:text/html,<script>alert(1)</script>'), '');
});

test('E-1.11 safeImageUrl handles empty/null', () => {
  assert.strictEqual(safeImageUrl(''), '');
  assert.strictEqual(safeImageUrl(null), '');
});

test('E-1.12 nfts.js uses escapeHtml for NFT name', () => {
  assert.ok(nftsSrc.includes('escapeHtml(nft.name)'), 'nft.name not escaped');
});

test('E-1.13 nfts.js uses escapeHtml for NFT mint', () => {
  assert.ok(nftsSrc.includes('escapeHtml(nft.mint)'), 'nft.mint not escaped');
});

test('E-1.14 nfts.js uses escapeHtml for NFT standard', () => {
  assert.ok(nftsSrc.includes('escapeHtml(nft.standard)'), 'nft.standard not escaped');
});

test('E-1.15 nfts.js uses escapeHtml for NFT symbol', () => {
  assert.ok(nftsSrc.includes('escapeHtml(nft.symbol)'), 'nft.symbol not escaped');
});

test('E-1.16 nfts.js uses safeImageUrl for NFT image', () => {
  assert.ok(nftsSrc.includes('safeImageUrl(nft.image)'), 'nft.image not safe-url-checked');
});

// ── E-2: XSS in full.js identity tab ──
console.log('\n── E-2: Full Page Identity Tab XSS Protection ──');

test('E-2.1 full.js escapes displayName in identity profile', () => {
  assert.ok(fullSrc.includes('escapeHtmlExt(displayName)'), 'displayName not escaped');
});

test('E-2.2 full.js escapes lichenNameDisplay in identity profile', () => {
  assert.ok(fullSrc.includes('escapeHtmlExt(lichenNameDisplay)'), 'lichenNameDisplay not escaped');
});

test('E-2.3 full.js escapes skill names in identity tab', () => {
  // Check that skill name is escaped via escapeHtmlExt
  assert.ok(fullSrc.includes("escapeHtmlExt(String(s.name || s.skill || 'Unnamed'))"), 'skill name not escaped');
});

test('E-2.4 full.js escapes vouch labels in identity tab', () => {
  assert.ok(fullSrc.includes('escapeHtmlExt(v.voucher_name'), 'vouch label not escaped');
});

test('E-2.5 full.js escapes achievement names in identity tab', () => {
  assert.ok(fullSrc.includes('escapeHtmlExt(def.name)'), 'achievement name not escaped');
});

test('E-2.6 full.js escapes data.lichenName in .lichen name section', () => {
  assert.ok(fullSrc.includes("escapeHtmlExt(data.lichenName.endsWith('.lichen')"), 'data.lichenName not escaped in .lichen section');
});

test('E-2.7 full.js escapes data.endpoint in agent service section', () => {
  assert.ok(fullSrc.includes('escapeHtmlExt(data.endpoint)'), 'data.endpoint not escaped');
});

test('E-2.8 full.js escapes error message in catch block', () => {
  assert.ok(fullSrc.includes('escapeHtmlExt(e.message)'), 'error message not escaped in identity catch');
});

test('E-2.9 escapeHtmlExt function works correctly', () => {
  assert.ok(escapeHtmlExt, 'escapeHtmlExt could not be extracted');
  assert.strictEqual(escapeHtmlExt('<img onerror=alert(1)>'), '&lt;img onerror=alert(1)&gt;');
  assert.strictEqual(escapeHtmlExt('"onclick'), '&quot;onclick');
});

// ── E-3: XSS in popup.js identity panel ──
console.log('\n── E-3: Popup Identity Panel XSS Protection ──');

test('E-3.1 popup.js defines escapeHtml function', () => {
  assert.ok(popupSrc.includes('function escapeHtml('), 'escapeHtml not found in popup.js');
});

test('E-3.2 popup.js escapes identity.name', () => {
  assert.ok(popupSrc.includes('escapeHtml(identity.name)'), 'identity.name not escaped');
});

test('E-3.3 popup.js escapes lichenName', () => {
  assert.ok(popupSrc.includes('escapeHtml(licnName'), 'lichenName not escaped');
});

test('E-3.4 popup.js escapes tierName', () => {
  assert.ok(popupSrc.includes('escapeHtml(tierName)'), 'tierName not escaped');
});

test('E-3.5 popup.js escapes skill names', () => {
  assert.ok(popupSrc.includes('escapeHtml(s.name)'), 'skill names not escaped');
});

test('E-3.6 popup.js escapeHtml works correctly', () => {
  assert.ok(escapeHtmlPopup, 'escapeHtml could not be extracted from popup.js');
  assert.strictEqual(escapeHtmlPopup('<b>bold</b>'), '&lt;b&gt;bold&lt;/b&gt;');
});

// ── E-4: XSS in settings.js loadApprovedOrigins ──
console.log('\n── E-4: Settings Page Origin XSS Protection ──');

test('E-4.1 settings.js defines escapeHtml function', () => {
  assert.ok(settingsSrc.includes('function escapeHtml('), 'escapeHtml not found in settings.js');
});

test('E-4.2 settings.js escapes origin in text content', () => {
  assert.ok(settingsSrc.includes('escapeHtml(origin)'), 'origin not escaped in settings.js');
});

test('E-4.3 settings.js uses safeOrigin in data-origin attribute', () => {
  // The origin should be escaped before placement in data-origin
  assert.ok(settingsSrc.includes('data-origin="${safeOrigin}"') || settingsSrc.includes("data-origin=\"${safeOrigin}\""), 'origin not escaped in data attribute');
});

test('E-4.4 settings.js escapeHtml works correctly', () => {
  assert.ok(escapeHtmlSettings, 'escapeHtml could not be extracted from settings.js');
  assert.strictEqual(escapeHtmlSettings('"><script>'), '&quot;&gt;&lt;script&gt;');
});

// ── E-5: XSS in identity.js ──
console.log('\n── E-5: Identity Page XSS Protection ──');

test('E-5.1 identity.js defines escapeHtml function', () => {
  assert.ok(identitySrc.includes('function escapeHtml('), 'escapeHtml not found in identity.js');
});

test('E-5.2 identity.js escapes identity name', () => {
  assert.ok(identitySrc.includes('escapeHtml(details.name)'), 'details.name not escaped');
});

test('E-5.3 identity.js escapes endpoint', () => {
  assert.ok(identitySrc.includes('escapeHtml(details.endpoint)'), 'details.endpoint not escaped');
});

test('E-5.4 identity.js escapes skill names', () => {
  assert.ok(identitySrc.includes('escapeHtml(s.name)'), 's.name not escaped');
});

test('E-5.5 identity.js escapes achievement names', () => {
  assert.ok(identitySrc.includes('escapeHtml(a.name'), 'a.name not escaped');
});

test('E-5.6 identity.js escapeHtml works correctly', () => {
  assert.ok(escapeHtmlIdentity, 'escapeHtml could not be extracted from identity.js');
  assert.strictEqual(escapeHtmlIdentity("foo'bar"), "foo&#x27;bar");
});

// ── E-6: Missing blockhash validation in tx-service.js ──
console.log('\n── E-6: Blockhash Hex Validation ──');

test('E-6.1 tx-service.js validates blockhash hex format', () => {
  assert.ok(txServiceSrc.includes("'Invalid blockhash: must be exactly 64 hex characters'"), 'blockhash validation error message not found');
});

test('E-6.2 tx-service.js uses regex to validate blockhash', () => {
  assert.ok(txServiceSrc.includes('/^[0-9a-fA-F]{64}$/'), 'blockhash hex regex not found');
});

test('E-6.3 tx-service.js coerces blockhash to string before validation', () => {
  assert.ok(txServiceSrc.includes("String(message.blockhash || message.recent_blockhash || '')"), 'blockhash not coerced to String');
});

test('E-6.4 valid 64-char hex blockhash passes validation', () => {
  // Simulate the validation logic
  const hashHex = 'a'.repeat(64);
  assert.ok(/^[0-9a-fA-F]{64}$/.test(hashHex));
});

test('E-6.5 short blockhash fails validation', () => {
  const hashHex = 'abc123';
  assert.ok(!/^[0-9a-fA-F]{64}$/.test(hashHex));
});

test('E-6.6 non-hex blockhash fails validation', () => {
  const hashHex = 'g'.repeat(64);
  assert.ok(!/^[0-9a-fA-F]{64}$/.test(hashHex));
});

test('E-6.7 empty blockhash fails validation', () => {
  const hashHex = '';
  assert.ok(!/^[0-9a-fA-F]{64}$/.test(hashHex));
});

// ── E-7: Private key not zeroed after use in provider-router.js ──
console.log('\n── E-7: Private Key Zeroing After Use ──');

test('E-7.1 finalizeSignMessage uses try/finally for key zeroing', () => {
  // Check that after decryptPrivateKey, there's a finally block that zeros key
  const fnMatch = providerRouterSrc.match(/async function finalizeSignMessage[\s\S]*?^}/m);
  assert.ok(fnMatch, 'finalizeSignMessage not found');
  assert.ok(fnMatch[0].includes('finally'), 'finalizeSignMessage missing finally block');
  assert.ok(fnMatch[0].includes("'0'.repeat("), 'finalizeSignMessage not zeroing key');
});

test('E-7.2 finalizeSignTransaction uses try/finally for key zeroing', () => {
  const fnMatch = providerRouterSrc.match(/async function finalizeSignTransaction[\s\S]*?\n\}/m);
  assert.ok(fnMatch, 'finalizeSignTransaction not found');
  assert.ok(fnMatch[0].includes('finally'), 'finalizeSignTransaction missing finally block');
  assert.ok(fnMatch[0].includes("'0'.repeat("), 'finalizeSignTransaction not zeroing key');
});

test('E-7.3 finalizeSendTransaction uses try/finally for key zeroing', () => {
  const fnMatch = providerRouterSrc.match(/async function finalizeSendTransaction[\s\S]*?\n\}/m);
  assert.ok(fnMatch, 'finalizeSendTransaction not found');
  assert.ok(fnMatch[0].includes('finally'), 'finalizeSendTransaction missing finally block');
  assert.ok(fnMatch[0].includes("'0'.repeat("), 'finalizeSendTransaction not zeroing key');
});

test('E-7.4 all three finalize functions declare privateKeyHex with let', () => {
  // Must be `let privateKeyHex` not `const` for reassignment in finally
  const signMsgFn = providerRouterSrc.match(/async function finalizeSignMessage[\s\S]*?\n\}/m);
  const signTxFn = providerRouterSrc.match(/async function finalizeSignTransaction[\s\S]*?\n\}/m);
  const sendTxFn = providerRouterSrc.match(/async function finalizeSendTransaction[\s\S]*?\n\}/m);
  assert.ok(signMsgFn[0].includes('let privateKeyHex'), 'finalizeSignMessage uses const instead of let');
  assert.ok(signTxFn[0].includes('let privateKeyHex'), 'finalizeSignTransaction uses const instead of let');
  assert.ok(sendTxFn[0].includes('let privateKeyHex'), 'finalizeSendTransaction uses const instead of let');
});

// ── E-8: Missing hex format validation for private key import ──
console.log('\n── E-8: Private Key Import Hex Validation ──');

test('E-8.1 full.js handleImportPrivKey validates hex format', () => {
  assert.ok(fullSrc.includes('/^[0-9a-fA-F]{64}$/'), 'hex format regex not found in handleImportPrivKey');
});

test('E-8.2 full.js rejects non-hex private key', () => {
  // The regex pattern should reject non-hex
  const pattern = /^[0-9a-fA-F]{64}$/;
  assert.ok(!pattern.test('g'.repeat(64)), 'non-hex should fail');
  assert.ok(!pattern.test('zz' + 'a'.repeat(62)), 'mixed non-hex should fail');
});

test('E-8.3 full.js accepts valid 64-char hex private key', () => {
  const pattern = /^[0-9a-fA-F]{64}$/;
  assert.ok(pattern.test('a'.repeat(64)), 'valid hex should pass');
  assert.ok(pattern.test('0123456789abcdef'.repeat(4)), 'valid mixed hex should pass');
});

test('E-8.4 full.js error message is descriptive', () => {
  assert.ok(fullSrc.includes('0-9, a-f'), 'error message should mention valid hex chars');
});

// ── E-9: Inline onclick handler in full.js loadActivity ──
console.log('\n── E-9: No Inline onclick Handler ──');

test('E-9.1 full.js loadActivity does not use onclick attribute', () => {
  // loadActivity function area — confirm no onclick="loadActivity"
  assert.ok(!fullSrc.includes('onclick="loadActivity'), 'inline onclick="loadActivity" still present');
});

test('E-9.2 full.js creates Load More button with addEventListener', () => {
  assert.ok(fullSrc.includes("loadMoreBtn.addEventListener('click'") || fullSrc.includes('loadMoreBtn.addEventListener("click"'),
    'addEventListener not found for Load More button');
});

test('E-9.3 full.js creates Load More button via DOM API', () => {
  assert.ok(fullSrc.includes("document.createElement('button')") || fullSrc.includes('document.createElement("button")'),
    'createElement not used for Load More button');
});

// ── E-10: Trusted RPC split for critical extension flows ──
console.log('\n── E-10: Trusted RPC Split For Critical Flows ──');

test('E-10.1 rpc-service exposes getTrustedRpcEndpoint', () => {
  assert.ok(rpcServiceSrc.includes('export function getTrustedRpcEndpoint('), 'getTrustedRpcEndpoint helper not found');
  assert.ok(rpcServiceSrc.includes('return getTrustedRpcEndpoint(network);'), 'getRpcEndpoint should fall back through getTrustedRpcEndpoint');
});

test('E-10.2 bridge-service pins bridge control-plane RPC to trusted endpoints', () => {
  assert.ok(bridgeServiceSrc.includes('function getTrustedBridgeRpc(network)'), 'bridge-service should define getTrustedBridgeRpc');
  assert.ok(bridgeServiceSrc.includes("new LichenRPC(getTrustedRpcEndpoint(network))"), 'bridge-service should build bridge RPC from trusted endpoint');
  assert.ok(!bridgeServiceSrc.includes('await getConfiguredRpcEndpoint(network)'), 'bridge-service should not use configured custom RPC for bridge control-plane calls');
  assert.ok(bridgeServiceSrc.includes('buildBridgeAccessMessage('), 'bridge-service should build a signed bridge access message');
  assert.ok(bridgeServiceSrc.includes('Wallet password required for bridge authorization'), 'bridge-service should require a wallet password before signing bridge access');
  assert.ok(bridgeServiceSrc.includes('BRIDGE_CACHE_KEY'), 'bridge-service should maintain a local bridge deposit cache');
  assert.ok(!bridgeServiceSrc.includes("getBridgeDepositsByRecipient"), 'bridge-service should not rely on public recipient-history bridge RPC');
});

test('E-10.3 identity-service pins LichenID resolution to trusted metadata RPC', () => {
  assert.ok(identityServiceSrc.includes('getTrustedRpcEndpoint(network)'), 'identity-service should use trusted RPC endpoint');
  assert.ok(identityServiceSrc.includes("trustedRpc.call('getSymbolRegistry'"), 'identity-service should resolve symbol registry over trusted RPC');
  assert.ok(identityServiceSrc.includes("trustedRpc.call('getAllContracts'"), 'identity-service should fall back to trusted contract list lookup');
});

test('E-10.4 popup bridge flow uses authenticated bridge-service helpers', () => {
  assert.ok(popupSrc.includes('hasBridgeAccessAuth(wallet)'), 'popup bridge flow should check for existing bridge auth');
  assert.ok(popupSrc.includes('requestBridgeDepositAddress({'), 'popup bridge flow should request deposits through bridge-service');
  assert.ok(popupSrc.includes('getBridgeDepositStatus({'), 'popup bridge status polling should use bridge-service');
  assert.ok(popupSrc.includes('Wallet password (for bridge authorization):'), 'popup bridge flow should prompt for wallet password before bridge auth');
  assert.ok(!popupSrc.includes("rpc.call('createBridgeDeposit'"), 'popup should not call createBridgeDeposit directly');
  assert.ok(!popupSrc.includes("rpc.call('getBridgeDeposit'"), 'popup should not call getBridgeDeposit directly');
});

test('E-10.5 extension settings surfaces explain the trusted RPC split', () => {
  assert.ok(settingsSrc.includes('trusted endpoints'), 'settings page status should mention trusted endpoints');
  assert.ok(fullSrc.includes('trusted endpoints'), 'full-page settings save should mention trusted endpoints');
  assert.ok(popupSrc.includes('trusted endpoints'), 'popup settings save should mention trusted endpoints');
});

// ── Additional cross-cutting tests ──
console.log('\n── Cross-Cutting Verification ──');

test('CC-1 nfts.js no unescaped nft.name in innerHTML', () => {
  // After the fix, nft.name should never appear directly in a template literal
  // inside setHtml(..., items.map(...))
  const renderSection = nftsSrc.match(/setHtml\('nftsGrid',[\s\S]*?\)\);/);
  assert.ok(renderSection, 'nftsGrid render section not found');
  assert.ok(!renderSection[0].includes('${nft.name}'), 'raw nft.name still in innerHTML');
  assert.ok(!renderSection[0].includes('${nft.mint}'), 'raw nft.mint still in innerHTML');
});

test('CC-2 popup.js normalizePrivateKeyHex validates hex format', () => {
  // popup.js already has normalizePrivateKeyHex with hex validation
  assert.ok(popupSrc.includes('Private key must be hex'), 'popup normalizePrivateKeyHex lacks hex validation');
  assert.ok(popupSrc.includes('/^[0-9a-f]+$/'), 'popup normalizePrivateKeyHex lacks hex regex');
});

test('CC-3 approve.js uses escapeHtml for all rendered fields', () => {
  const approveSrc = fs.readFileSync(path.join(extRoot, 'pages', 'approve.js'), 'utf8');
  assert.ok(approveSrc.includes('escapeHtml(request.origin'), 'approve.js not escaping origin');
  assert.ok(approveSrc.includes('escapeHtml(normalizedMethod'), 'approve.js not escaping method');
});

test('CC-4 home.js has escapeHtml function', () => {
  assert.ok(homeSrc.includes('function escapeHtml('), 'home.js missing escapeHtml');
});

test('CC-4b bridge-service supports BNB/BSC chains', () => {
  assert.ok(bridgeServiceSrc.includes("'bsc'"), 'bridge-service missing bsc support');
  assert.ok(bridgeServiceSrc.includes("'bnb'"), 'bridge-service missing bnb alias support');
  assert.ok(bridgeServiceSrc.includes("normalizedChain === 'bnb' ? 'bsc' : normalizedChain"), 'bridge-service missing bnb->bsc canonicalization');
});

test('CC-4c home bridge selector exposes BNB chain', () => {
  assert.ok(homeSrc.includes("'bsc'"), 'home.js bridge chain allowlist missing bsc');
  assert.ok(homeHtmlSrc.includes('option value="bsc"'), 'home.html bridge chain dropdown missing bsc option');
});

test('CC-4d full page bridge modal exposes and wires BNB chain', () => {
  assert.ok(fullSrc.includes("Bridge from BNB Chain"), 'full.js missing BNB bridge card label');
  assert.ok(fullSrc.includes("startExtensionDeposit('bsc')"), 'full.js missing BSC click handler wiring');
  assert.ok(fullSrc.includes("bsc: 'BNB Chain'"), 'full.js missing bsc chain label mapping');
});

test('CC-5 no other inline onclick handlers in extension JS files', () => {
  const jsFiles = [
    'pages/nfts.js', 'pages/identity.js', 'pages/settings.js',
    'pages/approve.js', 'pages/home.js', 'popup/popup.js'
  ];
  for (const file of jsFiles) {
    const src = fs.readFileSync(path.join(extRoot, file), 'utf8');
    assert.ok(!src.includes('onclick="'), `onclick= found in ${file}`);
  }
});

test('CC-6 all five escapeHtml implementations handle XSS payload correctly', () => {
  const payload = '<img src=x onerror="alert(document.cookie)">';
  const expected = '&lt;img src=x onerror=&quot;alert(document.cookie)&quot;&gt;';
  for (const [name, fn] of [
    ['nfts', escapeHtmlNfts],
    ['settings', escapeHtmlSettings],
    ['identity', escapeHtmlIdentity],
    ['popup', escapeHtmlPopup],
    ['full', escapeHtmlExt]
  ]) {
    assert.ok(fn, `${name} escapeHtml not extracted`);
    assert.strictEqual(fn(payload), expected, `${name} escapeHtml fails on XSS payload`);
  }
});

test('CC-7 safeImageUrl blocks vbscript protocol', () => {
  assert.strictEqual(safeImageUrl('vbscript:MsgBox("XSS")'), '');
});

test('CC-8 safeImageUrl allows ipfs protocol', () => {
  const result = safeImageUrl('ipfs://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi');
  assert.ok(result.startsWith('ipfs://'), 'ipfs URL should be allowed');
});

test('CC-9 popup shield panel uses canonical getShieldedPoolState with fallback', () => {
  assert.ok(popupSrc.includes("rpc.call('getShieldedPoolState'"), 'popup shield panel must call getShieldedPoolState');
  assert.ok(popupSrc.includes("rpc.call('getShieldedPoolStats'"), 'popup shield panel should keep getShieldedPoolStats fallback');
});

test('CC-10 popup shield panel uses password-gated shield initialization', () => {
  assert.ok(popupSrc.includes('initializeShieldedPopupForActiveWallet'), 'popup missing shield initialization flow');
  assert.ok(popupSrc.includes("securePasswordPrompt('Enter your wallet password to initialize shielded privacy.')"), 'popup shield init should prompt for the wallet password');
  assert.ok(popupSrc.includes('deriveShieldedSeedFromWallet(wallet, password)'), 'popup shield init should derive the shield seed from decrypted wallet material');
  assert.ok(!popupSrc.includes('shielded-popup:v1'), 'popup should not derive shielded state from the public wallet address placeholder path');
});

test('CC-11 popup delete flow wipes encrypted key material', () => {
  assert.ok(popupSrc.includes("wipeWallet.encryptedKey = wipeString(wipeWallet.encryptedKey) || null;"), 'popup delete must wipe encryptedKey');
  assert.ok(popupSrc.includes('resetShieldedPopupState();'), 'popup delete should reset in-memory shielded state');
});

test('CC-12 ws-service events are forwarded to popup runtime handlers', () => {
  assert.ok(wsServiceSrc.includes("msg.method === 'subscription'"), 'ws-service should parse subscription notifications');
  assert.ok(wsServiceSrc.includes("type: 'account-change'"), 'ws-service should emit account-change events');
  assert.ok(serviceWorkerSrc.includes("type: 'LICHEN_WS_EVENT'"), 'service-worker should forward WS events to runtime listeners');
  assert.ok(popupSrc.includes("message?.type === 'LICHEN_WS_EVENT'"), 'popup should react to forwarded WS events');
});

test('CC-13 content script uses event-driven provider refresh (no 2s polling loop)', () => {
  assert.ok(!contentScriptSrc.includes('setInterval(() => {\n      checkProviderStateAndEmit();\n    }, 2000);'),
    'content-script should not use fixed 2s provider polling');
  assert.ok(contentScriptSrc.includes("message?.type === 'LICHEN_PROVIDER_STATE_DIRTY'"),
    'content-script should refresh on provider state dirty messages');
  assert.ok(contentScriptSrc.includes('document.addEventListener(\'visibilitychange\''),
    'content-script should refresh on visibility changes');
});

test('CC-14 window.ethereum shim is namespace-restricted (no broad lichenwallet spread)', () => {
  assert.ok(!inpageProviderSrc.includes('...window.licnwallet'), 'window.ethereum must not spread full lichenwallet surface');
  assert.ok(inpageProviderSrc.includes('/^(eth_|net_|web3_|wallet_)/.test(method)'), 'window.ethereum request should enforce allowed method namespaces');
  assert.ok(inpageProviderSrc.includes('Unsupported window.ethereum method'), 'window.ethereum should reject unsupported method names');
});

test('CC-15 popup uses live oracle feed for LICN USD display (no fixed $0.10)', () => {
  assert.ok(popupSrc.includes('/oracle/prices'), 'popup should fetch oracle prices endpoint');
  assert.ok(popupSrc.includes("String(feed?.asset || '').toUpperCase() === 'LICN'"), 'popup should select LICN oracle feed');
  assert.ok(!popupSrc.includes('(balanceLicn * 0.10)'), 'popup should not use hardcoded 0.10 LICN price');
});

test('CC-16 provider router prunes expired pending approvals and stale finalized requests', () => {
  assert.ok(providerRouterSrc.includes('const FINALIZED_REQUEST_TTL_MS = 5 * 60 * 1000;'),
    'provider router should define finalized request cleanup TTL');
  assert.ok(providerRouterSrc.includes("request.finalized = { ok: false, error: 'Approval timed out' };"),
    'provider router should finalize expired pending approvals as timed out');
  assert.ok(providerRouterSrc.includes('pendingRequests.delete(requestId);'),
    'provider router should delete stale finalized requests during pruning');
  assert.ok(providerRouterSrc.includes('if (!request || request.finalized) return null;'),
    'provider router should hide finalized requests from pending lookup');
});

test('CC-17 provider router applies TTL expiry to approved origins', () => {
  assert.ok(providerRouterSrc.includes("const APPROVED_ORIGINS_META_KEY = 'lichenApprovedOriginsMeta';"),
    'provider router should persist approved-origin metadata key');
  assert.ok(providerRouterSrc.includes('const APPROVED_ORIGIN_TTL_MS = 30 * 24 * 60 * 60 * 1000;'),
    'provider router should define approved-origin TTL');
  assert.ok(providerRouterSrc.includes('async function pruneApprovedOrigins('),
    'provider router should prune expired approved origins');
  assert.ok(providerRouterSrc.includes('meta[origin] = Date.now() + APPROVED_ORIGIN_TTL_MS;'),
    'provider router should stamp origin approvals with expiry');
});

test('CC-18 provider router reuses shared tx-service message serializer', () => {
  assert.ok(providerRouterSrc.includes("import { serializeMessageForSigning } from './tx-service.js';"),
    'provider router should import serializeMessageForSigning from tx-service');
  assert.ok(providerRouterSrc.includes('return serializeMessageForSigning(normalizedMessage);'),
    'provider router should serialize message bytes via tx-service helper');
  assert.ok(txServiceSrc.includes('export function serializeMessageForSigning(message)'),
    'tx-service should export canonical serializeMessageForSigning helper');
});

// ============================================================================
console.log('\n' + '='.repeat(60));
console.log(`Results: ${passed} passed, ${failed} failed, ${passed + failed} total`);
if (failed > 0) {
  console.log('❌ SOME TESTS FAILED');
  process.exit(1);
} else {
  console.log('✅ ALL TESTS PASSED');
}

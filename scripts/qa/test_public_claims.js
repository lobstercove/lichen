#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const claimFiles = [
    '.github/workflows/release.yml',
    'README.md',
    'website/index.html',
    'developers/index.html',
    'developers/architecture.html',
    'developers/cli-reference.html',
    'developers/contract-reference.html',
    'developers/contracts.html',
    'developers/exchange-integration.html',
    'developers/getting-started.html',
    'developers/validator.html',
    'developers/lichenid.html',
    'developers/playground.html',
    'developers/rpc-reference.html',
    'developers/ws-reference.html',
    'developers/zk-privacy.html',
    'developers/sdk-js.html',
    'developers/sdk-python.html',
    'developers/sdk-rust.html',
    'developers/shared/utils.js',
];

const forbidden = [
    [/\$0\.0001|\$2\.50|\$0\.05/g, 'fixed USD fee conversion without a protocol-defined exchange rate'],
    [/Mainnet\s*\+\s*Testnet Live/gi, 'mainnet-live claim'],
    [/--network\s+mainnet/g, 'runnable mainnet command before launch approval'],
    [/state-mainnet/g, 'runnable mainnet state path before launch approval'],
    [/join mainnet directly/gi, 'immediate mainnet-join instruction'],
    [/35,?000\+?\s*(?:tx\/s|\(single-thread\))/gi, 'unproven throughput headline'],
    [/~?10,?000\s*\/s/gi, 'unproven signature-throughput headline'],
    [/95%\s+(?:in|of nodes within)\s*(?:&lt;|<)?200ms/gi, 'unproven 1,000-node propagation headline'],
    [/priority mempool lanes?|priority transaction lanes?|express lane/gi, 'removed reputation priority lane'],
    [/reputation that unlocks fee discounts?|only influences fee discounts/gi, 'removed base-protocol reputation discount'],
    [/shift value through shielded|shielded flows when confidentiality/gi, 'active shielded-flow promise while scheme 0x01 is disabled'],
    [/current source candidate|Source-verified for candidate/gi, 'stale candidate wording on a public release surface'],
    [/[0-9.]+\s*KB\s+WASM|[0-9,]+\s+lines\s*[·|]\s*[0-9]+\s+tests|[0-9]+\+\s+(?:entry points|functions)/gi,
        'hand-entered contract artifact or source statistic on a public claim page'],
];

let failures = 0;

function read(relativePath) {
    return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function fail(message) {
    failures += 1;
    console.error(`FAIL ${message}`);
}

function pass(message) {
    console.log(`PASS ${message}`);
}

function lineFor(text, offset) {
    return text.slice(0, offset).split('\n').length;
}

for (const relativePath of claimFiles) {
    const text = read(relativePath);
    for (const [pattern, label] of forbidden) {
        pattern.lastIndex = 0;
        for (const match of text.matchAll(pattern)) {
            fail(`${relativePath}:${lineFor(text, match.index)} contains ${label}: ${JSON.stringify(match[0])}`);
        }
    }
}

function collectRustFiles(relativeDir) {
    const absoluteDir = path.join(repoRoot, relativeDir);
    const results = [];
    for (const entry of fs.readdirSync(absoluteDir, { withFileTypes: true })) {
        const relativePath = path.join(relativeDir, entry.name);
        if (entry.isDirectory()) {
            results.push(...collectRustFiles(relativePath));
        } else if (entry.isFile() && entry.name.endsWith('.rs')) {
            results.push(relativePath.split(path.sep).join('/'));
        }
    }
    return results;
}

for (const relativePath of [...collectRustFiles('core/src'), ...collectRustFiles('contracts')]) {
    const text = read(relativePath);
    const pattern = /(?:at \$0\.10(?:\/LICN)?|\$[0-9,.KkMm]+\s+at\s+\$0\.10)/g;
    for (const match of text.matchAll(pattern)) {
        fail(`${relativePath}:${lineFor(text, match.index)} contains a fixed LICN/USD assumption: ${JSON.stringify(match[0])}`);
    }
}

const requiredText = [
    ['core/src/genesis.rs', 'pub const DEFAULT_LICN_USD_8DEC: u64 = 15_000_000;', 'core defines the current $0.15 LICN genesis reference'],
    ['run-validator.sh', 'GENESIS_LICN_USD="${GENESIS_LICN_USD:-0.15}"', 'local testnet genesis defaults LICN to $0.15'],
    ['lichen-start.sh', 'GENESIS_LICN_USD="${GENESIS_LICN_USD:-0.15}"', 'interactive genesis defaults LICN to $0.15'],
    ['dex/dex.js', 'const LICHEN_GENESIS_PRICE = 0.15;', 'DEX fallback matches the $0.15 LICN reference'],
    ['wallet/js/wallet.js', 'const _OFFLINE_FALLBACK_PRICES = { LICN: 0.15,', 'web wallet fallback matches the $0.15 LICN reference'],
    ['wallet/extension/src/pages/full.js', "let _licnUsdPriceCache = { value: 0.15,", 'full wallet extension fallback matches the $0.15 LICN reference'],
    ['wallet/extension/src/popup/popup.js', "let _licnUsdPriceCache = { value: 0.15,", 'wallet extension popup fallback matches the $0.15 LICN reference'],
    ['deploy/mainnet-launch-runbook.md', '"licn_usd_8dec": 15000000', 'mainnet launch packet uses the $0.15 LICN reference'],
    ['.github/workflows/release.yml', 'Public deployment is testnet-only; mainnet has not launched.', 'release instructions preserve the testnet-only boundary'],
    ['.github/workflows/release.yml', '"$HOME/.lichen/state-testnet/seeds.json"', 'release instructions install testnet seeds into testnet state'],
    ['README.md', '**Candidate release line:** `v0.5.275`', 'README identifies the candidate source release line'],
    ['README.md', 'published\nGitHub release archives whose checksums', 'README binds installation to published signed artifacts'],
    ['README.md', 'Mainnet has not launched', 'README marks mainnet as not launched'],
    ['README.md', 'ML-KEM-768', 'README documents native PQ P2P key establishment'],
    ['website/index.html', '<option value="mainnet" disabled>Mainnet (not launched)</option>', 'website disables mainnet selection'],
    ['website/index.html', 'Historical shielded', 'website states the historical shielded boundary'],
    ['developers/index.html', 'Mainnet has not launched', 'developer hub states the mainnet boundary'],
    ['developers/cli-reference.html', 'Source-verified for release line <code>v0.5.275</code>', 'CLI reference identifies the source release line'],
    ['developers/cli-reference.html', 'published, checksum-signed, provenance-attested release artifacts', 'CLI reference binds installation to published signed artifacts'],
    ['developers/getting-started.html', 'lichen 0.5.275', 'getting-started guide matches the source release line'],
    ['developers/changelog.html', '<span class="changelog-version">v0.5.275</span>', 'developer changelog identifies the candidate release line'],
    ['docs/deployment/PRODUCTION_DEPLOYMENT.md', 'LICHEN_RELEASE_TAG=v0.5.275', 'production runbook uses the candidate release line'],
    ['deploy/mainnet-launch-runbook.md', 'export LICHEN_RELEASE_TAG=v0.5.275', 'launch runbook uses the candidate release line'],
    ['docs/deployment/ARCHIVE_V2_ACTIVATION_CADENCE_AND_VALIDATOR_LIVENESS_PLAN_2026-08-18.md', '`v0.5.275` is the only successor candidate', 'Archive V2 execution plan identifies the candidate release line'],
    ['developers/exchange-integration.html', 'exchange-testnet-v0.5.275', 'exchange portal identifies the candidate package line'],
    ['developers/architecture.html', 'ignored for ordering', 'architecture states reputation-neutral mempool ordering'],
    ['developers/architecture.html', 'application-layer\n                ML-DSA-65 and ML-KEM-768 handshake', 'architecture documents the native PQ P2P handshake'],
    ['developers/contract-reference.html', 'machine-checked Source Export Matrix is authoritative', 'contract reference separates source from live activation'],
    ['developers/contract-reference.html', 'base fee market and mempool remain reputation', 'contract reference preserves the reputation-neutral protocol boundary'],
    ['developers/zk-privacy.html', 'No shielded proof is accepted', 'privacy guide states fail-closed proof acceptance'],
];

for (const [relativePath, needle, label] of requiredText) {
    if (!read(relativePath).includes(needle)) {
        fail(`${relativePath} is missing required claim boundary: ${label}`);
    } else {
        pass(label);
    }
}

const sourceEvidence = [
    ['core/src/account.rs', 'ML-DSA-65 keypair for signing native Lichen transactions', 'native transaction ML-DSA evidence'],
    ['core/src/mempool.rs', 'intentionally ignored for ordering', 'reputation-neutral mempool evidence'],
    ['core/src/processor/fees.rs', 'reputation discounts removed', 'uniform base-fee evidence'],
    ['core/src/zk/mod.rs', 'proof scheme 0x01 lacks constrained private-witness verification', 'disabled shielded-scheme evidence'],
    ['p2p/src/peer.rs', 'application-layer ML-DSA + ML-KEM handshake before any P2P message is', 'native PQ P2P handshake evidence'],
    ['p2p/src/peer.rs', 'XChaCha20Poly1305', 'P2P frame encryption evidence'],
    ['website/shared-config.js', "const productionPrimaryNetwork = 'testnet';", 'production frontend testnet default evidence'],
];

for (const [relativePath, needle, label] of sourceEvidence) {
    if (!read(relativePath).includes(needle)) {
        fail(`${relativePath} is missing source evidence required by public claims: ${label}`);
    } else {
        pass(label);
    }
}

if (failures > 0) {
    console.error(`\nPublic claims QA: ${failures} failure(s)`);
    process.exit(1);
}

console.log('\nPublic claims QA: PASS');

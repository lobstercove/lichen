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
    ['.github/workflows/release.yml', 'Public deployment is testnet-only; mainnet has not launched.', 'release instructions preserve the testnet-only boundary'],
    ['.github/workflows/release.yml', '"$HOME/.lichen/state-testnet/seeds.json"', 'release instructions install testnet seeds into testnet state'],
    ['README.md', 'Current installed signed testnet release:** `v0.5.263`', 'README identifies the installed signed release'],
    ['README.md', '`v0.5.264` source becomes an installable release only when', 'README separates candidate qualification from release'],
    ['README.md', 'Mainnet has not launched', 'README marks mainnet as not launched'],
    ['README.md', 'ML-KEM-768', 'README documents native PQ P2P key establishment'],
    ['website/index.html', '<option value="mainnet" disabled>Mainnet (not launched)</option>', 'website disables mainnet selection'],
    ['website/index.html', 'Historical shielded', 'website states the historical shielded boundary'],
    ['developers/index.html', 'Mainnet has not launched', 'developer hub states the mainnet boundary'],
    ['developers/cli-reference.html', 'Source-verified for candidate <code>v0.5.264</code>', 'CLI reference identifies the current source candidate'],
    ['developers/cli-reference.html', 'currently signed testnet release is\n                <code>v0.5.263</code>', 'CLI reference separates source candidate from signed release'],
    ['developers/getting-started.html', 'currently signed\n                testnet release remains <code>v0.5.263</code>', 'getting-started guide separates source candidate from signed release'],
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

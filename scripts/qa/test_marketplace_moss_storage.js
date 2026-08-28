#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
let passed = 0;
let failed = 0;

function read(relativePath) {
    return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function assert(condition, label) {
    if (condition) {
        passed++;
        console.log(`  ✅ ${label}`);
    } else {
        failed++;
        console.log(`  ❌ ${label}`);
    }
}

console.log('\n── Marketplace Moss Storage ──');

const create = read('marketplace/js/create.js');
const walletBridge = read('marketplace/shared/wallet-connect.js');
const marketConfig = read('marketplace/js/marketplace-config.js');
const marketData = read('marketplace/js/marketplace-data.js');
const providerHttp = read('moss-provider/src/http.rs');
const providerMerkle = read('moss-provider/src/merkle.rs');
const contract = read('contracts/moss_storage/src/lib.rs');
const headers = read('marketplace/_headers');

assert(
    create.includes('var chunkSize = 65536;') &&
        create.includes("crypto.subtle.digest('SHA-256', bytes)") &&
        create.includes('var right = nodes[i + 1] || nodes[i];') &&
        providerMerkle.includes('pub const CHUNK_BYTES: usize = 65_536;') &&
        providerMerkle.includes('let right = if pair.len() == 2 { &pair[1] } else { &pair[0] };'),
    'browser and provider use the same 64 KiB SHA-256 Merkle commitment'
);

assert(
    create.includes("var message = 'lichen-moss-upload-v1\\n'") &&
        create.includes('window.lichenWallet.signMessage(message)') &&
        !create.includes('currentWallet.signMessage(message)') &&
        providerHttp.includes('format!("lichen-moss-upload-v1\\n{hash}\\n{size}\\n{content_type}")'),
    'Moss upload authorization uses the connected wallet and canonical provider message'
);

assert(
    walletBridge.includes("method: 'licn_signMessage'") &&
        walletBridge.includes('LichenWallet.prototype.signMessage = async function (message)') &&
        walletBridge.includes("typeof provider.signMessage !== 'function'"),
    'marketplace wallet bridge exposes fail-closed message signing'
);

assert(
    create.includes('var MOSS_PRICING_SCALE = 100000000n;') &&
        create.includes("buildContractCallData('store_data_v2'") &&
        create.includes('perReplicaNumerator + MOSS_PRICING_SCALE - 1n') &&
        contract.includes('const STORAGE_PRICING_V2_SCALE: u128 = 100_000_000;'),
    'marketplace storage quote matches the contract fixed-point ceil calculation'
);

const mintFlow = create.slice(create.indexOf('async function mintNFT()'));
const exactBalanceIndex = mintFlow.indexOf('if (userBalance < totalCost)');
const storageIndex = mintFlow.indexOf('storePreparedObjectsOnMoss([preparedMedia, preparedMetadata])');
const collectionIndex = mintFlow.indexOf("if (collection === 'new')", storageIndex);
assert(
    exactBalanceIndex !== -1 && storageIndex > exactBalanceIndex && collectionIndex > storageIndex &&
        mintFlow.includes('BigInt(preparedMedia.storageValue) + BigInt(preparedMetadata.storageValue)') &&
        mintFlow.includes('NFT metadata supports at most 64 populated properties'),
    'exact dual-object storage cost and metadata bounds are checked before chain-side mint actions'
);

assert(
    create.includes('storePreparedObjectsOnMoss([preparedMedia, preparedMetadata])') &&
        create.includes('preparedObjects.map(function (prepared)') &&
        create.includes("var mediaUri = 'moss://' + preparedMedia.objectHash") &&
        create.includes('var metadataUri = mossUris[1];'),
    'media and metadata are uploaded and committed together with content-addressed URIs'
);

assert(
    marketConfig.includes('mossGatewayUrl: config.moss') &&
        marketConfig.includes("value.indexOf('moss://') !== 0") &&
        marketData.includes("value.indexOf('moss://') === 0") &&
        marketData.includes('metadata exceeds 1 MiB'),
    'marketplace resolves bounded Moss metadata through the selected network gateway'
);

const sharedConfigs = [
    'marketplace/shared-config.js',
    'developers/shared-config.js',
    'dex/shared-config.js',
    'explorer/shared-config.js',
    'monitoring/shared-config.js',
    'programs/shared-config.js',
    'wallet/shared-config.js',
    'website/shared-config.js',
];
assert(
    sharedConfigs.every(relativePath => {
        const source = read(relativePath);
        return source.includes("moss: 'https://moss.lichen.network'") &&
            source.includes("moss: 'https://testnet-moss.lichen.network'") &&
            source.includes('function moss(networkKey)');
    }),
    'all shared network manifests expose identical mainnet and testnet Moss endpoints'
);

assert(
    headers.includes('https://moss.lichen.network') &&
        headers.includes('https://testnet-moss.lichen.network'),
    'marketplace CSP permits only the configured HTTPS Moss production gateways'
);

console.log(`\nMarketplace Moss storage: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);

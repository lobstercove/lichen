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
const providerContent = read('moss-provider/src/content.rs');
const providerReconcile = read('moss-provider/src/reconcile.rs');
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
    create.includes("var message = 'lichen-moss-upload-v2\\n'") &&
        create.includes("new TextEncoder().encode('lichen-moss-storage-id-v1')") &&
        create.includes('bs58encode(nonzeroRandomBytes(32))') &&
        create.includes("form.append('storage_id', storageId)") &&
        create.includes("form.append('request_nonce', requestNonce)") &&
        create.includes('window.lichenWallet.signMessage(message)') &&
        !create.includes('currentWallet.signMessage(message)') &&
        providerHttp.includes('"lichen-moss-upload-v2\\n{owner}\\n{storage_id}\\n{request_nonce}\\n{hash}\\n{size}\\n{content_type}"') &&
        providerHttp.includes('derive_storage_id(&owner, hash, request_nonce)?') &&
        providerHttp.includes('request_nonce == [0u8; 32]'),
    'Moss upload authorization binds the connected wallet, owner-scoped storage ID, and content commitment'
);

assert(
        create.includes('var MOSS_PROVIDER_URLS =') &&
        create.includes('entry.program || entry.program_id || entry.contract_id || entry.id') &&
        create.includes('var requiredReceipts = mossRequiredReplication();') &&
        create.includes('return localDevelopment ? 1 : MOSS_REPLICATION_FACTOR;') &&
        create.includes('providerIdentities.has(providerIdentity)') &&
        create.includes('providerPrice > MOSS_MAX_PRICE') &&
        create.includes('if (receipts.length >= requiredReceipts) break;') &&
        create.includes('receipts.length < requiredReceipts') &&
        create.includes("form.append('object', blob, objectHash)") &&
        create.includes("payload.state !== 'staged'") &&
        create.includes('window.LichenPQ.verifySignature(') &&
        create.includes("'lichen-moss-upload-receipt-v2\\n'") &&
        create.includes('payload.owner !== currentWallet.address') &&
        create.includes('payload.storage_id !== storageId') &&
        create.includes('payload.request_nonce !== requestNonce') &&
        providerHttp.includes('UploadReceiptCommitment {') &&
        providerHttp.includes('.signing_message()') &&
        providerHttp.includes('receipt_signature') &&
        providerHttp.includes('.add_assignment(') &&
        providerHttp.includes('provider_status.used') &&
        providerHttp.includes('provider_status.capacity') &&
        providerHttp.includes('refund_owner_charge(owner, size).await') &&
        providerHttp.includes('if !put.created {') &&
        providerHttp.includes('price_per_byte_per_slot: provider_price') &&
        marketConfig.includes('mossProviderUrls: Array.isArray(config.mossProviders)'),
    'public Moss uploads require exact-count, identity-distinct, provider-signed receipts before the on-chain storage call'
);

assert(
    contract.includes('storage_id_v3(&owner_arr, &supplied_hash, &request_nonce)') &&
        contract.includes('storage_content_hash(&hash_arr)') &&
        contract.includes('pub extern "C" fn get_storage_content_hash(') &&
        providerContent.includes('pub assignments: Vec<AssignmentRecord>') &&
        providerContent.includes('pending_assignment_bytes: AtomicU64') &&
        providerContent.includes('confirmed_used') &&
        providerContent.includes('Moss provider logical assignment capacity exceeded') &&
        providerContent.includes('directory_has_assignments(&assignment_directory)?') &&
        providerReconcile.includes('chain.storage_content_hash(storage_id).await?') &&
        providerReconcile.includes('store.remove_assignment(&record.hash, storage_id).await?'),
    'owner-scoped V3 IDs preserve raw challenge roots and shared objects until every durable assignment closes'
);

assert(
    walletBridge.includes("method: 'licn_signMessage'") &&
        walletBridge.includes('LichenWallet.prototype.signMessage = async function (message)') &&
        walletBridge.includes("typeof provider.signMessage !== 'function'"),
    'marketplace wallet bridge exposes fail-closed message signing'
);

assert(
    create.includes('var MOSS_PRICING_SCALE = 100000000n;') &&
        create.includes("buildContractCallData('store_data_v3'") &&
        create.includes('providerRoster.length !== prepared.replicationFactor * 32') &&
        create.includes('perReplicaNumerator + MOSS_PRICING_SCALE - 1n') &&
        contract.includes('const STORAGE_PRICING_V2_SCALE: u128 = 100_000_000;') &&
        contract.includes('pub extern "C" fn store_data_v3(') &&
        contract.includes('storage_assignment_allows_provider(&data_hash, &provider_arr)'),
    'marketplace quote and exact provider roster match the contract pricing-v3 assignment path'
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
        create.includes('await waitForSuccessfulTransaction(storageSignature') &&
        create.includes('await waitForMossReplication(mossProgram, preparedObjects') &&
        create.includes('prepared.uploadProviders = (uploadReceipt.replica_receipts || []).map') &&
        create.includes('var expectedProviders = new Set(preparedObjects[i].uploadProviders || []);') &&
        create.includes('matchedProviders < preparedObjects[i].replicationFactor') &&
        create.includes("'get_storage_info'") &&
        create.includes('var storageIdBytes = bs58decode(preparedObjects[i].storageId);') &&
        create.includes('var args = bytesToBase64(storageIdBytes);') &&
        create.includes('owner !== currentWallet.address') &&
        !create.includes('utf8ToBase64(JSON.stringify([preparedObjects[i].objectHash]))') &&
        create.includes("var mediaUri = 'moss://' + preparedMedia.objectHash") &&
        create.includes('var metadataUri = mossUris[1];'),
    'media and metadata wait for exact on-chain replication before minting with content-addressed URIs'
);

assert(
        marketConfig.includes('mossGatewayUrl: config.moss') &&
        marketConfig.includes("value.indexOf('moss://') !== 0") &&
        marketData.includes("value.indexOf('moss://') === 0") &&
        marketData.includes('response.body.getReader()') &&
        marketData.includes('total > maxBytes') &&
        marketData.includes('controller.abort()') &&
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
            source.includes("'https://moss-us.lichen.network'") &&
            source.includes("'https://testnet-moss-us.lichen.network'") &&
            source.includes("'https://testnet-moss-eu.lichen.network'") &&
            source.includes("'https://testnet-moss-sea.lichen.network'") &&
            source.includes("'https://testnet-moss-in.lichen.network'") &&
            source.includes('function moss(networkKey)') &&
            source.includes('function mossProviders(networkKey)');
    }),
    'all shared network manifests expose identical mainnet and testnet Moss endpoints'
);

assert(
    headers.includes('https://moss.lichen.network') &&
        headers.includes('https://moss-us.lichen.network') &&
        headers.includes('https://testnet-moss.lichen.network') &&
        headers.includes('https://testnet-moss-us.lichen.network') &&
        headers.includes('https://testnet-moss-eu.lichen.network') &&
        headers.includes('https://testnet-moss-sea.lichen.network') &&
        headers.includes('https://testnet-moss-in.lichen.network'),
    'marketplace CSP permits only the configured HTTPS Moss production gateways'
);

console.log(`\nMarketplace Moss storage: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);

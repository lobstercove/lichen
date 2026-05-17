#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');

const FILES = {
    packageJson: 'package.json',
    developerGuide: 'docs/guides/NEO_DEVELOPER_INTEGRATION.md',
    wrappedAssets: 'docs/defi/WRAPPED_ASSETS.md',
    custodyDeployment: 'docs/deployment/CUSTODY_DEPLOYMENT.md',
    productionDeployment: 'docs/deployment/PRODUCTION_DEPLOYMENT.md',
    rpcMarkdown: 'docs/guides/RPC_API_REFERENCE.md',
    jsSdkMarkdown: 'docs/api/JAVASCRIPT_SDK.md',
    pythonSdkMarkdown: 'docs/api/PYTHON_SDK.md',
    rustSdkMarkdown: 'docs/api/RUST_SDK.md',
    developerHome: 'developers/index.html',
    gettingStarted: 'developers/getting-started.html',
    rpcPortal: 'developers/rpc-reference.html',
    cliPortal: 'developers/cli-reference.html',
    contractPortal: 'developers/contract-reference.html',
    contractsPortal: 'developers/contracts.html',
    jsSdkPortal: 'developers/sdk-js.html',
    pythonSdkPortal: 'developers/sdk-python.html',
    rustSdkPortal: 'developers/sdk-rust.html',
    wsPortal: 'developers/ws-reference.html',
    searchIndex: 'developers/js/developers.js',
    expectedContracts: 'scripts/qa/expected-contracts.json',
};

const STALE_ACTIVE_PATTERNS = [
    'v0.5.10',
    '182 JSON-RPC dispatch names',
    '29 production smart contracts',
    'Full reference for all 28',
    'All four wrapped',
    'Genesis catalog total** | | **32**',
];

let passed = 0;
let failed = 0;

function read(relativePath) {
    return fs.readFileSync(path.join(ROOT, relativePath), 'utf8');
}

function readJson(relativePath) {
    return JSON.parse(read(relativePath));
}

function test(name, fn) {
    try {
        fn();
        passed += 1;
        process.stdout.write(`  PASS ${name}\n`);
    } catch (error) {
        failed += 1;
        process.stderr.write(`  FAIL ${name}: ${error.message}\n`);
    }
}

function assert(condition, message) {
    if (!condition) {
        throw new Error(message);
    }
}

function assertIncludes(source, needle, label) {
    assert(source.includes(needle), `${label} missing '${needle}'`);
}

function assertNotIncludes(source, needle, label) {
    assert(!source.includes(needle), `${label} still contains stale '${needle}'`);
}

function assertAllIncludes(source, needles, label) {
    needles.forEach((needle) => assertIncludes(source, needle, label));
}

function countLiteral(source, needle) {
    return source.split(needle).length - 1;
}

function main() {
    const docs = Object.fromEntries(
        Object.entries(FILES).map(([key, relativePath]) => [key, read(relativePath)])
    );
    const expectedContracts = readJson(FILES.expectedContracts).contracts;
    const packageJson = readJson(FILES.packageJson);

    process.stdout.write('\nNeo Developer Docs QA\n\n');

    test('npm doc QA entrypoint includes Neo developer docs QA', () => {
        assertIncludes(
            packageJson.scripts['test-deployment-docs'] || '',
            'test_neo_developer_docs.js',
            'package.json test-deployment-docs'
        );
    });

    test('active developer docs do not contain known stale Neo-era counts or versions', () => {
        [
            FILES.developerGuide,
            FILES.wrappedAssets,
            FILES.custodyDeployment,
            FILES.productionDeployment,
            FILES.rpcMarkdown,
            FILES.jsSdkMarkdown,
            FILES.pythonSdkMarkdown,
            FILES.rustSdkMarkdown,
            FILES.developerHome,
            FILES.gettingStarted,
            FILES.rpcPortal,
            FILES.cliPortal,
            FILES.contractPortal,
            FILES.contractsPortal,
            FILES.jsSdkPortal,
            FILES.pythonSdkPortal,
            FILES.rustSdkPortal,
            FILES.wsPortal,
            FILES.searchIndex,
        ].forEach((relativePath) => {
            const source = read(relativePath);
            STALE_ACTIVE_PATTERNS.forEach((needle) => assertNotIncludes(source, needle, relativePath));
        });
    });

    test('canonical Neo developer guide covers route, reserves, DEX, rewards, watchtower, and SDK examples', () => {
        assertAllIncludes(docs.developerGuide, [
            'getBridgeRouteRestrictionStatus',
            'getWgasStats',
            'getWneoStats',
            'getNeoGasRewardsStats',
            'getNeoGasRewardsPosition',
            'getNeoZkProofServiceStatus',
            'verifyNeoReserveLiabilityProof',
            'scripts/pq-evidence.js',
            'collectRouteHealthEvidence',
            'zk-prove reserve-liability',
            'dex_rewards.configure_lp_campaign',
            'neox/gas',
            'neox/neo',
            'wNEO/lUSD',
            'wNEO/LICN',
            'wGAS/lUSD',
            'wGAS/LICN',
            'BridgeChain::NeoX',
            'BridgeAsset::Gas',
        ], FILES.developerGuide);
    });

    test('developer portal overview and search index expose Neo route and rewards docs', () => {
        assertAllIncludes(docs.developerHome, [
            'Neo X Integration',
            'rpc-reference.html#neo-x-route-rewards',
            'wNEO/wGAS pairs',
        ], FILES.developerHome);
        assertAllIncludes(docs.searchIndex, [
            'Neo X Integration',
            'getNeoGasRewardsStats',
            'getNeoGasRewardsPosition',
            'getNeoZkProofServiceStatus',
            'verifyNeoReserveLiabilityProof',
            'Neo GAS Rewards Contract',
            'Wrapped Assets',
        ], FILES.searchIndex);
    });

    test('RPC portal and canonical RPC docs list Neo route, reserve, rewards, and DEX methods', () => {
        [
            [docs.rpcPortal, FILES.rpcPortal],
            [docs.rpcMarkdown, FILES.rpcMarkdown],
        ].forEach(([source, label]) => {
            assertAllIncludes(source, [
                'getBridgeRouteRestrictionStatus',
                'getWneoStats',
                'getWgasStats',
                'getNeoGasRewardsStats',
                'getNeoGasRewardsPosition',
                'getNeoZkProofServiceStatus',
                'verifyNeoReserveLiabilityProof',
                'neox/gas',
                'neox/neo',
                'wNEO/lUSD',
                'wNEO/LICN',
                'wGAS/lUSD',
                'wGAS/LICN',
            ], label);
        });
        assertIncludes(docs.rpcPortal, 'v0.5.43', FILES.rpcPortal);
        assertIncludes(docs.rpcPortal, 'neo-x-route-rewards', FILES.rpcPortal);
    });

    test('CLI docs expose route status, governed route payloads, and Neo symbol lookups', () => {
        assertAllIncludes(docs.cliPortal, [
            'v0.5.43',
            'lichen restriction status bridge-route neox gas',
            'lichen restriction status bridge-route neox neo',
            'lichen restriction build pause-bridge-route neox gas',
            'lichen restriction build resume-bridge-route neox gas',
            'lichen symbol lookup WNEO',
            'lichen symbol lookup WGAS',
            'lichen symbol lookup NEOGASRWD',
            'zk-prove reserve-liability',
            'zk-prove verify-reserve-liability',
        ], FILES.cliPortal);
    });

    test('contract docs and expected-contracts include Neo contracts with current counts', () => {
        assert(expectedContracts.length === 31, 'expected-contracts.json must list 31 genesis contracts');
        ['wgas_token', 'wneo_token', 'neo_gas_rewards'].forEach((contractName) => {
            assert(expectedContracts.includes(contractName), `expected-contracts.json missing ${contractName}`);
        });
        assertAllIncludes(docs.contractPortal, [
            '31 genesis-deployed smart contracts',
            '82,336',
            'Source Exports',
            '578',
            'Neo GAS Rewards Vault',
            'NEOGASRWD',
            'neo_gas_rewards',
            'wgas_token',
            'wneo_token',
            'dex_rewards.configure_lp_campaign',
            'submit_agent_job',
            'get_agent_compute_controls',
        ], FILES.contractPortal);
        ['wbnb_token', 'wgas_token', 'wneo_token', 'neo_gas_rewards'].forEach((contractName) => {
            assert(
                countLiteral(docs.contractPortal, `<td>${contractName}</td>`) === 1,
                `${FILES.contractPortal} must list ${contractName} exactly once in the live export matrix`
            );
        });
        assertAllIncludes(docs.contractsPortal, [
            'Genesis-Deployed Neo Contracts',
            'wNEO',
            'wGAS',
            'NEOGASRWD',
        ], FILES.contractsPortal);
    });

    test('SDK docs expose Neo route constants and rewards helpers', () => {
        assertAllIncludes(docs.jsSdkMarkdown, [
            'getNeoGasRewardsStats',
            'getNeoGasRewardsPosition',
            'BRIDGE_CHAINS.NEOX',
            'BRIDGE_ASSETS.GAS',
            'getNeoZkProofServiceStatus',
            'verifyNeoReserveLiabilityProof',
        ], FILES.jsSdkMarkdown);
        assertAllIncludes(docs.pythonSdkMarkdown, [
            'get_neo_gas_rewards_stats',
            'get_neo_gas_rewards_position',
            'BRIDGE_CHAIN_NEOX',
            'BRIDGE_ASSET_GAS',
            'get_neo_zk_proof_service_status',
            'verify_neo_reserve_liability_proof',
        ], FILES.pythonSdkMarkdown);
        assertAllIncludes(docs.rustSdkMarkdown, [
            'get_neo_gas_rewards_stats',
            'get_neo_gas_rewards_position',
            'BridgeChain::NeoX',
            'BridgeAsset::Gas',
            'get_neo_zk_proof_service_status',
            'verify_neo_reserve_liability_proof',
        ], FILES.rustSdkMarkdown);
        assertAllIncludes(docs.jsSdkPortal, ['getNeoGasRewardsStats', 'BRIDGE_CHAINS.NEOX', 'verifyNeoReserveLiabilityProof'], FILES.jsSdkPortal);
        assertAllIncludes(docs.pythonSdkPortal, ['get_neo_gas_rewards_stats', 'BRIDGE_CHAIN_NEOX', 'verify_neo_reserve_liability_proof'], FILES.pythonSdkPortal);
        assertAllIncludes(docs.rustSdkPortal, ['get_neo_gas_rewards_stats', 'BridgeChain::NeoX', 'verify_neo_reserve_liability_proof'], FILES.rustSdkPortal);
    });

    test('wrapped asset and custody docs cover wBNB, wGAS, wNEO, route env, and 31-contract catalog', () => {
        assertAllIncludes(docs.wrappedAssets, [
            'wBNB',
            'wGAS',
            'wNEO',
            'Neo X GAS',
            'Neo X NEO',
            'whole-NEO lots',
            'CUSTODY_WBNB_TOKEN_ADDR',
            'CUSTODY_WGAS_TOKEN_ADDR',
            'CUSTODY_WNEO_TOKEN_ADDR',
            '**Genesis catalog total** | | **31**',
        ], FILES.wrappedAssets);
        assertAllIncludes(docs.custodyDeployment, [
            'CUSTODY_WBNB_TOKEN_ADDR',
            'CUSTODY_WGAS_TOKEN_ADDR',
            'CUSTODY_WNEO_TOKEN_ADDR',
            'EVM Route Registry',
            'neox',
            'whole-NEO',
        ], FILES.custodyDeployment);
    });

    test('operator deployment docs link Neo developer guide and public gates', () => {
        assertAllIncludes(docs.productionDeployment, [
            'NEO_DEVELOPER_INTEGRATION.md',
            'NEO_PUBLIC_BETA_GATE_TEMPLATE.json',
            'NEO_LIQUIDITY_CORRIDOR_GATE_TEMPLATE.json',
            'NEO_ZK_PROOF_SERVICES_GATE_TEMPLATE.json',
            'check_neo_agent_compute_gate.js',
        ], FILES.productionDeployment);
    });

    test('wallet and WS developer pages describe read-only Neo route and rewards surfaces', () => {
        assertAllIncludes(docs.gettingStarted, [
            'Neo X Wallet Reads',
            'getBridgeRouteRestrictionStatus',
            'getNeoGasRewardsPosition',
        ], FILES.gettingStarted);
        assertAllIncludes(docs.wsPortal, [
            'Neo X Integration',
            'getBridgeRouteRestrictionStatus',
            'getNeoGasRewardsStats',
            'getDexPairs',
        ], FILES.wsPortal);
    });

    process.stdout.write(`\nNeo Developer Docs QA: ${passed} passed, ${failed} failed\n`);
    if (failed > 0) {
        process.exitCode = 1;
    }
}

main();

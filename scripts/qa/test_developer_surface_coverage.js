#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const read = (relative) => fs.readFileSync(path.join(ROOT, relative), 'utf8');

let passed = 0;
let failed = 0;

function check(condition, message) {
    if (condition) {
        passed += 1;
        process.stdout.write(`PASS ${message}\n`);
    } else {
        failed += 1;
        process.stderr.write(`FAIL ${message}\n`);
    }
}

function packageVersion(relative) {
    return read(relative).match(/^version = "([^"]+)"/m)?.[1];
}

const rpcSource = read('rpc/src/lib.rs');
const rpcDocs = read('developers/rpc-reference.html');
const nativeDispatcherStart = rpcSource.indexOf(
    'let result = match req.method.as_str() {',
    rpcSource.indexOf('pub async fn handle_rpc'),
);
const nativeDispatcherEnd = rpcSource.indexOf('\n    };', nativeDispatcherStart);
check(nativeDispatcherStart >= 0 && nativeDispatcherEnd > nativeDispatcherStart,
    'native RPC dispatcher is discoverable');

const nativeDispatcher = rpcSource.slice(nativeDispatcherStart, nativeDispatcherEnd);
const nativeMethods = [...new Set(
    [...nativeDispatcher.matchAll(/^\s*"([A-Za-z][A-Za-z0-9_]*)"(?:\s*\||\s*=>)/gm)]
        .map((match) => match[1]),
)];
const undocumentedRpc = nativeMethods.filter((method) => !rpcDocs.includes(method));
check(nativeMethods.length > 0, `native RPC dispatcher exposes ${nativeMethods.length} methods`);
check(undocumentedRpc.length === 0,
    `RPC reference covers every native dispatcher method${undocumentedRpc.length ? `: ${undocumentedRpc.join(', ')}` : ''}`);

const contractDocs = read('developers/contract-reference.html');
const contractRoot = path.join(ROOT, 'contracts');
const contractNames = fs.readdirSync(contractRoot)
    .filter((name) => fs.existsSync(path.join(contractRoot, name, 'abi.json')))
    .sort();
const undocumentedAbi = [];
let abiFunctionCount = 0;
for (const contractName of contractNames) {
    const abi = JSON.parse(read(`contracts/${contractName}/abi.json`));
    check(contractDocs.includes(contractName), `contract reference names ${contractName}`);
    for (const fn of abi.functions || []) {
        abiFunctionCount += 1;
        if (!contractDocs.includes(fn.name)) {
            undocumentedAbi.push(`${contractName}:${fn.name}`);
        }
    }
}
check(undocumentedAbi.length === 0,
    `contract reference covers all ${abiFunctionCount} ABI functions${undocumentedAbi.length ? `: ${undocumentedAbi.join(', ')}` : ''}`);

const cliDocs = read('developers/cli-reference.html');
for (const command of ['governed-transfer', 'validator fingerprint']) {
    check(cliDocs.includes(command), `CLI reference covers lichen ${command}`);
}
const cliArgs = read('cli/src/cli_args.rs');
const callSupport = read('cli/src/call_support.rs');
const writeCommandSupport = read('cli/src/write_command_support.rs');
check(
    cliArgs.includes('value_spores: u64') &&
        cliArgs.includes('default_value_t = 0') &&
        callSupport.includes('value_spores: u64') &&
        /\.call_contract\([\s\S]*?value_spores,\s*\)/.test(callSupport) &&
        /handle_call\([\s\S]*?value_spores,[\s\S]*?keypair/.test(writeCommandSupport) &&
        cliDocs.includes('--value-spores'),
    'payable lichen call value is wired from CLI argument through RPC submission and documentation',
);

const servicesDocs = read('developers/services.html');
for (const binary of [
    'lichen-validator',
    'lichen-genesis',
    'lichen',
    'lichen-archive-v2',
    'zk-prove',
    'lichen-custody',
    'lichen-faucet',
    'lichen-moss-provider',
]) {
    check(servicesDocs.includes(binary), `service reference covers ${binary}`);
}
for (const unit of [
    'lichen-validator.service',
    'lichen-custody.service',
    'lichen-faucet.service',
    'lichen-moss-provider.service',
]) {
    check(servicesDocs.includes(unit), `service reference links ${unit}`);
}

const jsVersion = JSON.parse(read('sdk/js/package.json')).version;
const pythonVersion = read('sdk/python/pyproject.toml').match(/^version = "([^"]+)"/m)?.[1];
const rustVersion = packageVersion('sdk/rust/Cargo.toml');
const runtimeVersion = packageVersion('validator/Cargo.toml');
check(read('developers/sdk-js.html').includes(`<code>${jsVersion}</code>`),
    `JavaScript SDK page matches source version ${jsVersion}`);
check(read('developers/sdk-python.html').includes(`<code>${pythonVersion}</code>`),
    `Python SDK page matches source version ${pythonVersion}`);
check(read('developers/sdk-rust.html').includes(`<code>${rustVersion}</code>`),
    `Rust SDK page matches source version ${rustVersion}`);
check(read('developers/getting-started.html').includes(`lichen ${runtimeVersion}`),
    `getting-started CLI output matches runtime version ${runtimeVersion}`);

const developerJs = read('developers/js/developers.js');
check(developerJs.includes("href: 'services.html'"), 'developer navigation exposes the service reference');
check(developerJs.includes("url: 'services.html'"), 'developer search indexes the service reference');

process.stdout.write(`\nDeveloper surface coverage: ${passed} passed, ${failed} failed\n`);
if (failed > 0) process.exit(1);

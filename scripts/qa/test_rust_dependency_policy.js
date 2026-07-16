#!/usr/bin/env node

const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '../..');
const workspaceManifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const coreManifest = fs.readFileSync(path.join(root, 'core/Cargo.toml'), 'utf8');
const custodyManifest = fs.readFileSync(path.join(root, 'custody/Cargo.toml'), 'utf8');
const rocksDbConsumerLocks = [
  'Cargo.lock',
  'compiler/Cargo.lock',
  'fuzz/Cargo.lock',
  'sdk/Cargo.lock',
  'sdk/rust/Cargo.lock',
].map((relativePath) => fs.readFileSync(path.join(root, relativePath), 'utf8'));
const contractBuilder = fs.readFileSync(path.join(root, 'scripts/build-all-contracts.sh'), 'utf8');

const checks = [
  [
    'PQ dependency graph pins the compatible PKCS#8 prerelease',
    /^pkcs8\s*=\s*"=0\.11\.0-rc\.11"\s*$/m.test(workspaceManifest),
  ],
  [
    'core anchors PKCS#8 for standalone path consumers',
    /^pkcs8\s*=\s*\{\s*workspace\s*=\s*true\s*\}\s*$/m.test(coreManifest),
  ],
  [
    'runtime stores use the audited RocksDB binding',
    /^rocksdb\s*=\s*"0\.24"\s*$/m.test(coreManifest)
      && /^rocksdb\s*=\s*"0\.24"\s*$/m.test(custodyManifest),
  ],
  [
    'all runtime-consumer lockfiles use the exact audited RocksDB 10.4.2 binding',
    rocksDbConsumerLocks.every((lockfile) => (
      /name = "rocksdb"\nversion = "0\.24\.0"/.test(lockfile)
      && /name = "librocksdb-sys"\nversion = "0\.17\.3\+10\.4\.2"/.test(lockfile)
    )),
  ],
  [
    'contract builds use the shared target directory',
    /CONTRACT_TARGET_DIR=.*target\/contract-build/.test(contractBuilder)
      && /--target-dir\s+"\$CONTRACT_TARGET_DIR"/.test(contractBuilder),
  ],
  [
    'contract Cargo cache does not shadow the runtime contracts directory',
    !/CONTRACT_TARGET_DIR=.*target\/contracts(?:[}"/]|$)/.test(contractBuilder),
  ],
  [
    'contract artifact lookup uses the shared target directory',
    /WASM_SOURCE="\$\{CONTRACT_TARGET_DIR\}\/wasm32-unknown-unknown\/release\//.test(contractBuilder),
  ],
];

let failures = 0;
for (const [name, passed] of checks) {
  if (passed) {
    console.log(`PASS ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL ${name}`);
  }
}

console.log(`\nRust dependency policy QA: ${checks.length - failures} passed, ${failures} failed`);
if (failures > 0) process.exit(1);

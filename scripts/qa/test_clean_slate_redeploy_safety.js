#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const scriptPath = path.join(repoRoot, 'scripts', 'clean-slate-redeploy.sh');
const script = fs.readFileSync(scriptPath, 'utf8');

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function indexOf(needle) {
  const index = script.indexOf(needle);
  assert(index >= 0, `missing '${needle}'`);
  return index;
}

const sigDownload = indexOf('-p SHA256SUMS.sig');
const sigVerifyCall = indexOf('verify_release_checksum_signature');
const sigVerifyLog = indexOf('SHA256SUMS PQ signature verified by');
const archiveChecksum = indexOf('actual=$(sha256_file "$RELEASE_ARTIFACT_DIR/$archive")');
assert(sigVerifyCall < sigDownload, 'signature verifier must be defined before the release download flow');
assert(sigVerifyLog < sigDownload, 'signature verifier body should stay before the release download flow');
assert(sigDownload < script.indexOf('verify_release_checksum_signature', sigDownload), 'SHA256SUMS.sig must be downloaded before signature verification is called');
assert(script.indexOf('verify_release_checksum_signature', sigDownload) < archiveChecksum, 'PQ signature must be verified before archive checksums are trusted');

assert(
  script.includes('RELEASE_SIGNING_ADDRESS="${LICHEN_RELEASE_SIGNING_ADDRESS:-8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk}"'),
  'clean-slate release verification must default to the pinned release signer'
);
assert(
  script.includes('generate_signed_metadata_manifest_for_seed()'),
  'clean-slate must generate signed metadata from the deployer side'
);
assert(
  script.includes('--rpc "http://127.0.0.1:${tunnel_port}"') &&
    script.includes('--keypair "$RELEASE_SIGNING_KEYPAIR"'),
  'signed metadata generation must use a local key over an SSH tunnel'
);
assert(
  script.includes('signed-metadata-manifest-${NETWORK}.json') &&
    script.includes('rsync -az'),
  'clean-slate must upload only the signed metadata artifact to seed'
);

assert(
  script.includes('--exclude keypairs/release-signing-key.json') &&
    script.includes("--exclude 'keypairs/*signing*'") &&
    script.includes('--exclude VPS_CREDENTIALS.md'),
  'repo sync must exclude local signing keys and VPS credentials'
);
assert(
  !script.includes('etc/lichen/secrets/release-signing-keypair-${NETWORK}.json'),
  'service secret bundle must not include release signing keypair'
);
assert(
  !script.includes('sudo install -m 640 -o root -g lichen \\\n  ~/lichen/keypairs/release-signing-key.json'),
  'clean-slate must not install the release signing key on VPSes'
);
assert(
  !script.includes('SIGNED_METADATA_KEYPAIR=$HOME/release-signing-keypair-$NETWORK.json'),
  'first-boot clean-slate must not use a copied remote signing key'
);

assert(
  script.includes('LICHEN_MAINNET_VPS_HOSTS') &&
    script.includes('Refusing to reuse testnet host defaults for a destructive mainnet operation.'),
  'mainnet clean-slate must require explicit mainnet hosts'
);
assert(
  script.includes('LICHEN_MAINNET_CLEAN_SLATE_INCIDENT_APPROVAL') &&
    script.includes('LICHEN_MAINNET_NON_DESTRUCTIVE_RECOVERY_IMPOSSIBLE') &&
    script.includes('Refusing destructive mainnet clean-slate redeploy.'),
  'mainnet clean-slate must require a separate destructive-operation approval'
);
assert(
  script.includes('Mainnet requires at least four unique bridge/oracle committee validators') &&
    script.includes('import os') &&
    script.includes('NETWORK=\'$NETWORK\'; $(declare -f verify_protocol_url); verify_protocol_url http://127.0.0.1:$RPC_PORT') &&
    script.includes('min_committee = 4 if os.environ.get("NETWORK") == "mainnet" else 2'),
  'mainnet clean-slate must enforce the four-validator bridge/oracle committee guard locally and remotely'
);

console.log('clean-slate redeploy safety QA passed');

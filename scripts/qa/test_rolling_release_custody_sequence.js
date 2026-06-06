#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const script = fs.readFileSync(path.join(ROOT, 'scripts/rolling-release-deploy.sh'), 'utf8');

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function indexOfOrThrow(needle) {
  const index = script.indexOf(needle);
  assert(index >= 0, `missing '${needle}'`);
  return index;
}

const installCall = indexOfOrThrow('install_host "$host"');
const healthCall = indexOfOrThrow('wait_healthy "$host"');
const custodyCall = indexOfOrThrow('restart_custody_if_local "$host"');
const signatureVerify = indexOfOrThrow('SHA256SUMS PQ signature verified by');
const checksumVerify = indexOfOrThrow('sha256sum -c SHA256SUMS --ignore-missing');

assert(signatureVerify < checksumVerify, 'release PQ signature must be verified before checksum verification');
assert(checksumVerify < installCall, 'release artifacts must be verified before validator install');
assert(installCall < healthCall, 'validator install must happen before health wait');
assert(healthCall < custodyCall, 'custody restart must happen only after validator health');
assert(script.includes('expected_custody_sha="$(require_archive_bin_sha "$archive" "$root" lichen-custody)"'),
  'custody release hash must be required before install');
assert(script.includes('require_archive_bin_sha "$archive" "$root" lichen-custody'),
  'custody release binary must be required before install');
assert(script.includes('require_archive_bin_sha "$archive" "$root" lichen-faucet'),
  'faucet release binary must be required before install');
assert(script.includes('validate_release_archive "$archive" "$(archive_root "$archive")"'),
  'release archive contents must be validated before deploy');
assert(script.includes('sudo mv -f "/usr/local/bin/$bin.new" "/usr/local/bin/$bin"'),
  'release binaries must be installed atomically with temp+rename');
assert(script.includes('install_optional_service_bin lichen-custody "$EXPECTED_CUSTODY_SHA"'),
  'custody binary must be installed when expected in the archive');
assert(script.includes('install_optional_service_bin lichen-faucet "$EXPECTED_FAUCET_SHA"'),
  'faucet binary must be installed when expected in the archive');
assert(script.includes('check_installed_bin_hash lichen-custody "$EXPECTED_CUSTODY_SHA"'),
  'custody binary hash must be verified immediately after install');
assert(script.includes('check_installed_bin_hash lichen-faucet "$EXPECTED_FAUCET_SHA"'),
  'faucet binary hash must be verified immediately after install');
assert(!script.includes('for bin in lichen-custody lichen-faucet; do\n  if [ -x "$root/$bin" ]; then'),
  'optional service install must not depend on temp extract executable checks');
assert(script.includes('systemctl list-unit-files --no-legend lichen-custody.service'), 'custody refresh must be conditional on service presence');
assert(script.includes('sudo systemctl stop lichen-custody.service || true'), 'custody service must be stopped before start');
assert(script.includes('sudo systemctl kill --kill-who=control-group -s SIGKILL lichen-custody.service || true'), 'custody service stale cgroup must be killed before start');
assert(script.includes('sudo systemctl start lichen-custody.service'), 'custody service must be started after RPC is healthy');
assert(script.includes('http://127.0.0.1:9105/health'), 'custody health must be verified after restart');

console.log('rolling release custody sequencing QA passed');

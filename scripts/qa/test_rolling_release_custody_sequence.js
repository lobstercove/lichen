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
const faucetCall = indexOfOrThrow('restart_faucet_if_local "$host"');
const signatureVerify = indexOfOrThrow('SHA256SUMS PQ signature verified by');
const checksumVerify = indexOfOrThrow('sha256sum -c SHA256SUMS --ignore-missing');

assert(signatureVerify < checksumVerify, 'release PQ signature must be verified before checksum verification');
assert(checksumVerify < installCall, 'release artifacts must be verified before validator install');
assert(installCall < healthCall, 'validator install must happen before health wait');
assert(healthCall < custodyCall, 'custody restart must happen only after validator health');
assert(custodyCall < faucetCall, 'faucet restart must happen after custody refresh');
assert(script.includes('expected_custody_sha="$(require_archive_bin_sha "$archive" "$root" lichen-custody)"'),
  'custody release hash must be required before install');
assert(script.includes('require_archive_bin_sha "$archive" "$root" lichen-custody'),
  'custody release binary must be required before install');
assert(script.includes('require_archive_bin_sha "$archive" "$root" lichen-faucet'),
  'faucet release binary must be required before install');
assert(script.includes('validate_release_archive "$archive" "$(archive_root "$archive")"'),
  'release archive contents must be validated before deploy');
assert(script.includes('REMOTE_RELEASE_DOWNLOAD="${LICHEN_REMOTE_RELEASE_DOWNLOAD:-auto}"'),
  'remote release download mode must default to auto');
assert(script.includes('Release ${RELEASE_TAG} is draft; using local SCP transfer for verified artifacts.'),
  'draft releases must use local SCP transfer instead of public tag URLs');
assert(script.includes('SSH_CONNECT_TIMEOUT="${LICHEN_SSH_CONNECT_TIMEOUT:-20}"'),
  'SSH connect timeout must be configurable for flaky recovery links');
assert(script.includes('-o ConnectionAttempts=3'),
  'SSH operations must retry connection establishment during rolling deploys');
assert(script.includes('-o ServerAliveInterval=10'),
  'SSH operations must use keepalives during rolling deploys');
assert(script.includes('bash -s; status=\\$?; exit \\$status'),
  'remote scripts must stream over the SSH session instead of relying on temporary SCP helpers');
assert(script.includes('testnet:37.59.97.61|testnet:eu-vps|testnet:vps-210edd4a'),
  'testnet EU validator aliases must map to the pinned validator identity');
assert(script.includes('testnet:148.113.43.247|testnet:seed-04'),
  'testnet seed validator alias must map to the pinned validator identity');
assert(script.includes('CUSTODY_SERVICE="lichen-custody.service"'),
  'testnet rolling deploy must target the testnet custody systemd unit');
assert(script.includes('CUSTODY_HEALTH_URL="http://127.0.0.1:9105/health"'),
  'testnet rolling deploy must verify the testnet custody health port');
assert(script.includes('CUSTODY_SERVICE="lichen-custody-mainnet.service"'),
  'mainnet rolling deploy must target the mainnet custody systemd unit');
assert(script.includes('CUSTODY_HEALTH_URL="http://127.0.0.1:9106/health"'),
  'mainnet rolling deploy must verify the mainnet custody health port');
assert(script.includes('ALLOW_UNHEALTHY_PREFLIGHT="${LICHEN_ALLOW_UNHEALTHY_PREFLIGHT:-0}"'),
  'unhealthy preflight bypass must be an explicit operator override');
assert(script.includes('preflight health: status='),
  'preflight must print parsed local validator health');
assert(script.includes('status == "ok" and age <= max_age and not disk_critical'),
  'preflight must reject stale or disk-critical validators by default');
assert(script.includes('LICHEN_ALLOW_UNHEALTHY_PREFLIGHT=1'),
  'preflight recovery override must be visible in operator output');
assert(script.includes('local validator RPC is unavailable; continuing because LICHEN_ALLOW_UNHEALTHY_PREFLIGHT=1.'),
  'preflight recovery override must allow a stopped local validator RPC for clean rejoin');
assert(script.includes('stage_release_bin()'),
  'release binaries must be staged before live install');
assert(script.includes('check_staged_bin_hash lichen-custody "$EXPECTED_CUSTODY_SHA"'),
  'custody staged binary hash must be verified before live install');
assert(script.includes('check_staged_bin_hash lichen-faucet "$EXPECTED_FAUCET_SHA"'),
  'faucet staged binary hash must be verified before live install');
assert(script.includes('sudo -n mv -f "/usr/local/bin/$bin.new" "/usr/local/bin/$bin"'),
  'release binaries must be committed atomically with temp+rename');
assert(script.includes('install_optional_service_bin lichen-custody "$EXPECTED_CUSTODY_SHA"'),
  'custody binary must be installed when expected in the archive');
assert(script.includes('install_optional_service_bin lichen-faucet "$EXPECTED_FAUCET_SHA"'),
  'faucet binary must be installed when expected in the archive');
assert(script.includes('install_staged_bin lichen-custody "$EXPECTED_CUSTODY_SHA"'),
  'custody live install must be gated by the expected release hash');
assert(script.includes('install_staged_bin lichen-faucet "$EXPECTED_FAUCET_SHA"'),
  'faucet live install must be gated by the expected release hash');
assert(!script.includes('for bin in lichen-custody lichen-faucet; do\n  if [ -x "$root/$bin" ]; then'),
  'optional service install must not depend on temp extract executable checks');
assert(script.includes('systemctl list-unit-files --no-legend "$CUSTODY_SERVICE"'), 'custody refresh must be conditional on network-aware service presence');
assert(script.includes('sudo -n systemctl stop "$CUSTODY_SERVICE" || true'), 'custody service must be stopped before start');
assert(script.includes('sudo -n systemctl kill --kill-who=control-group -s SIGKILL "$CUSTODY_SERVICE" || true'), 'custody service stale cgroup must be killed before start');
assert(script.includes('sudo -n systemctl start "$CUSTODY_SERVICE"'), 'custody service must be started after RPC is healthy');
assert(script.includes('curl -fsS "$CUSTODY_HEALTH_URL"'), 'custody health must be verified after restart through network-aware URL');
assert(script.includes('sudo -n systemctl start lichen-faucet.service'), 'faucet service must be started after RPC is healthy');
assert(script.includes('http://127.0.0.1:9100/health'), 'faucet health must be verified after restart');
assert(script.includes('unit is enabled but inactive'), 'release verification must fail enabled inactive optional services');

console.log('rolling release custody sequencing QA passed');

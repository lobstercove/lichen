#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const script = fs.readFileSync(path.join(ROOT, 'scripts/rolling-release-deploy.sh'), 'utf8');
const r2Script = fs.readFileSync(path.join(ROOT, 'scripts/archive-v2-r2-put.sh'), 'utf8');
const dualR2Script = fs.readFileSync(path.join(ROOT, 'scripts/archive-v2-r2-dual-publish.sh'), 'utf8');

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
const mossCall = indexOfOrThrow('restart_moss_if_local "$host"');
const signatureVerify = indexOfOrThrow('SHA256SUMS PQ signature verified by');
const checksumVerify = indexOfOrThrow('sha256sum -c SHA256SUMS --ignore-missing');

assert(signatureVerify < checksumVerify, 'release PQ signature must be verified before checksum verification');
assert(checksumVerify < installCall, 'release artifacts must be verified before validator install');
assert(installCall < healthCall, 'validator install must happen before health wait');
assert(healthCall < custodyCall, 'custody restart must happen only after validator health');
assert(custodyCall < faucetCall, 'faucet restart must happen after custody refresh');
assert(faucetCall < mossCall, 'Moss provider restart must happen after faucet refresh');
assert(script.includes('expected_custody_sha="$(require_archive_bin_sha "$archive" "$root" lichen-custody)"'),
  'custody release hash must be required before install');
assert(script.includes('require_archive_bin_sha "$archive" "$root" lichen-custody'),
  'custody release binary must be required before install');
assert(script.includes('require_archive_bin_sha "$archive" "$root" lichen-faucet'),
  'faucet release binary must be required before install');
assert(script.includes('require_archive_bin_sha "$archive" "$root" lichen-moss-provider'),
  'Moss provider release binary must be required before install');
assert(script.includes('require_archive_file_sha "$archive" "$root" deploy/lichen-moss-provider.service'),
  'Moss provider systemd unit must be required from the signed release archive');
assert(script.includes('validate_release_archive "$archive" "$(archive_root "$archive")"'),
  'release archive contents must be validated before deploy');
assert(script.includes('validate_archive_members()'),
  'release archives must reject unsafe local member paths and types');
assert(script.includes('validate_remote_archive_members()'),
  'release archives must be revalidated on the target before extraction');
assert(script.includes('member.isdir() or member.isreg()'),
  'release archives must reject symlinks, hardlinks, devices, and FIFOs');
assert(indexOfOrThrow('archive_root="$(validate_remote_archive_members)"') < indexOfOrThrow('tar xzf "$ARCHIVE" -C "$tmp"'),
  'target archive validation must complete before extraction');
assert(script.includes('REMOTE_RELEASE_DOWNLOAD="${LICHEN_REMOTE_RELEASE_DOWNLOAD:-auto}"'),
  'remote release download mode must default to auto');
assert(script.includes('Release ${RELEASE_TAG} is draft; using local SCP transfer for verified artifacts.'),
  'draft releases must use local SCP transfer instead of public tag URLs');
assert(script.includes('SSH_CONNECT_TIMEOUT="${LICHEN_SSH_CONNECT_TIMEOUT:-20}"'),
  'SSH connect timeout must be configurable for flaky recovery links');
assert(script.includes('-o ConnectionAttempts=3'),
  'SSH operations must retry connection establishment during rolling deploys');
assert((script.match(/-o ControlMaster=no/g) || []).length >= 2,
  'SSH and SCP must disable inherited multiplex masters to prevent cross-host socket reuse');
assert((script.match(/-o ControlPath=none/g) || []).length >= 2,
  'SSH and SCP must disable inherited multiplex control paths');
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
assert(script.includes('check_staged_bin_hash lichen-moss-provider "$EXPECTED_MOSS_SHA"'),
  'Moss provider staged binary hash must be verified before live install');
assert(script.includes('sudo -n mv -f "/usr/local/bin/$bin.new" "/usr/local/bin/$bin"'),
  'release binaries must be committed atomically with temp+rename');
assert(script.includes('install_optional_service_bin lichen-custody "$EXPECTED_CUSTODY_SHA"'),
  'custody binary must be installed when expected in the archive');
assert(script.includes('install_optional_service_bin lichen-faucet "$EXPECTED_FAUCET_SHA"'),
  'faucet binary must be installed when expected in the archive');
assert(script.includes('install_optional_service_bin lichen-moss-provider "$EXPECTED_MOSS_SHA"'),
  'Moss provider binary must be installed when expected in the archive');
assert(script.includes('install_staged_bin lichen-custody "$EXPECTED_CUSTODY_SHA"'),
  'custody live install must be gated by the expected release hash');
assert(script.includes('install_staged_bin lichen-faucet "$EXPECTED_FAUCET_SHA"'),
  'faucet live install must be gated by the expected release hash');
assert(script.includes('install_staged_bin lichen-moss-provider "$EXPECTED_MOSS_SHA"'),
  'Moss provider live install must be gated by the expected release hash');
assert(script.includes('sudo -n mv -f "/etc/systemd/system/${MOSS_SERVICE}.new" "/etc/systemd/system/$MOSS_SERVICE"'),
  'Moss provider unit must be installed atomically from the signed release archive');
assert(script.includes('check_regular_file_hash "/etc/systemd/system/$MOSS_SERVICE" "$EXPECTED_MOSS_UNIT_SHA"'),
  'installed Moss provider unit must match the signed release archive');
assert(script.includes('if [ -f /etc/lichen/moss-provider.env ]; then') &&
  script.includes('sudo -n systemctl enable "$MOSS_SERVICE"'),
  'Moss provider must remain disabled until host-local configuration exists');
assert(!script.includes('for bin in lichen-custody lichen-faucet; do\n  if [ -x "$root/$bin" ]; then'),
  'optional service install must not depend on temp extract executable checks');
assert(script.includes('systemctl list-unit-files --no-legend "$CUSTODY_SERVICE"'), 'custody refresh must be conditional on network-aware service presence');
assert(script.includes('sudo -n systemctl stop "$CUSTODY_SERVICE" || true'), 'custody service must be stopped before start');
assert(script.includes('sudo -n systemctl kill --kill-who=control-group -s SIGKILL "$CUSTODY_SERVICE" || true'), 'custody service stale cgroup must be killed before start');
assert(script.includes('sudo -n systemctl start "$CUSTODY_SERVICE"'), 'custody service must be started after RPC is healthy');
assert(script.includes('curl -fsS "$CUSTODY_HEALTH_URL"'), 'custody health must be verified after restart through network-aware URL');
assert(script.includes('sudo -n systemctl start lichen-faucet.service'), 'faucet service must be started after RPC is healthy');
assert(script.includes('http://127.0.0.1:9100/health'), 'faucet health must be verified after restart');
assert(script.includes('MOSS_HEALTH_URL="${LICHEN_MOSS_HEALTH_URL:-http://127.0.0.1:9120/readyz}"'),
  'Moss provider health endpoint must be configurable and fail closed');
assert(script.includes('check_service_tree_hash "$MOSS_SERVICE" "$EXPECTED_MOSS_SHA" "$MOSS_SERVICE"'),
  'Moss provider running process must match the signed release hash');
assert(script.includes('COORDINATED_RELEASE="${LICHEN_COORDINATED_RELEASE:-0}"'),
  'consensus-critical deployment must expose an explicit coordinated mode');
assert(script.includes('all hosts staged and stopped; starting the complete fleet'),
  'coordinated mode must finish the stopped install phase before any validator starts');
assert(indexOfOrThrow('stop_service_unit "$SERVICE"') < indexOfOrThrow('install_staged_bin "$bin"'),
  'validator service must stop before staged binaries replace live paths');
assert(script.includes('if [ "$DEFER_START" = "1" ]; then'),
  'coordinated install must defer validator start on every host');
assert(script.includes('unit is enabled but inactive'), 'release verification must fail enabled inactive optional services');
assert(r2Script.includes('"$ARCHIVE_V2_BINARY" verify'),
  'R2 publication must verify the exact catalog segment before upload');
assert(r2Script.includes('verified_object_hashes'),
  'R2 publication must bind the verified catalog index to the requested object hash');
assert(r2Script.includes('/dev/shm'),
  'R2 temporary credential configuration must prefer tmpfs');
assert(r2Script.includes('trap cleanup EXIT HUP INT TERM'),
  'R2 temporary credentials must be removed on every exit path');
assert(!r2Script.includes('--user "$AWS_ACCESS_KEY_ID'),
  'R2 secret credentials must not be exposed in curl process arguments');
assert(r2Script.includes('curl --config "$curl_config" "$endpoint/$R2_BUCKET/$prefix/$key"'),
  'R2 publication must read every object back through the authenticated endpoint');
assert(r2Script.includes('R2 read-after-write hash mismatch'),
  'R2 publication must fail closed on remote hash mismatch');
assert(dualR2Script.includes('R2_PRIMARY_ACCESS_KEY_ID') &&
  dualR2Script.includes('R2_PRIMARY_SECRET_ACCESS_KEY') &&
  dualR2Script.includes('R2_PRIMARY_SESSION_TOKEN'),
'dual R2 publication must require an independent primary temporary credential');
assert(dualR2Script.includes('R2_REPLICA_ACCESS_KEY_ID') &&
  dualR2Script.includes('R2_REPLICA_SECRET_ACCESS_KEY') &&
  dualR2Script.includes('R2_REPLICA_SESSION_TOKEN'),
'dual R2 publication must require an independent replica temporary credential');
assert(dualR2Script.includes('primary_curl_config=') &&
  dualR2Script.includes('replica_curl_config='),
'dual R2 publication must isolate bucket-scoped credentials in separate configs');
assert(dualR2Script.includes('remote_sha "$primary_curl_config" "$R2_PRIMARY_BUCKET"') &&
  dualR2Script.includes('remote_sha "$replica_curl_config" "$R2_REPLICA_BUCKET"'),
'dual R2 publication must read back each bucket with its own credential');
assert(dualR2Script.includes('remote_etag "$primary_curl_config" "$R2_PRIMARY_BUCKET"') &&
  dualR2Script.includes('remote_etag "$replica_curl_config" "$R2_REPLICA_BUCKET"') &&
  dualR2Script.includes('--header "If-Match: $expected_etag"'),
'dual R2 catalog replacement must fail closed on concurrent publication');
assert(!dualR2Script.includes('${AWS_ACCESS_KEY_ID') &&
  !dualR2Script.includes('${AWS_SECRET_ACCESS_KEY') &&
  !dualR2Script.includes('${AWS_SESSION_TOKEN'),
'dual R2 publication must not silently reuse one credential across both buckets');

console.log('rolling release custody sequencing QA passed');

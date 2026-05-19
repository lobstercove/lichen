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

assert(installCall < healthCall, 'validator install must happen before health wait');
assert(healthCall < custodyCall, 'custody restart must happen only after validator health');
assert(script.includes('systemctl list-unit-files lichen-custody.service'), 'custody refresh must be conditional on service presence');
assert(script.includes('sudo systemctl restart lichen-custody.service'), 'custody service must be restarted after RPC is healthy');
assert(script.includes('http://127.0.0.1:9105/health'), 'custody health must be verified after restart');

console.log('rolling release custody sequencing QA passed');

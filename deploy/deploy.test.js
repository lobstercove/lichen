/**
 * Infrastructure Tests — Phase 20 audit fixes
 * Run: node deploy/deploy.test.js
 *
 * Tests all findings fixed during Phase 20 Infrastructure audit:
 *  F20.1  — Docker port conflict (validator + faucet both on 9100)
 *  F20.2  — Dockerfile EXPOSE clarity + faucet port added
 *  F20.3  — Dockerfile missing curl for healthcheck
 *  F20.4  — deploy/setup.sh network validation
 *  F20.5  — Systemd service security hardening verification
 */
'use strict';

const fs = require('fs');
const path = require('path');

let passed = 0, failed = 0;
function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}
function assertEqual(a, b, msg) {
    const eq = typeof a === 'object' ? JSON.stringify(a) === JSON.stringify(b) : a === b;
    if (eq) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}\n`); }
}

const root = path.join(__dirname, '..');

// ═══════════════════════════════════════════════════════════════════════════
// F20.1 — Docker port conflict resolved
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F20.1: Docker port conflict resolved ──');

const compose = fs.readFileSync(path.join(root, 'docker-compose.yml'), 'utf-8');

// Extract all host port mappings (host:container format)
const portMappings = [];
const portRegex = /- "(\d+):(\d+)"/g;
let match;
while ((match = portRegex.exec(compose)) !== null) {
    portMappings.push({ host: match[1], container: match[2], raw: match[0] });
}

// Check no duplicate host ports
const hostPorts = portMappings.map(p => p.host);
const uniqueHostPorts = new Set(hostPorts);
assertEqual(hostPorts.length, uniqueHostPorts.size,
    'docker-compose: no duplicate host port mappings');

// Specifically verify validator and faucet don't collide
const validatorPorts = [];
const faucetPorts = [];
let currentService = '';
for (const line of compose.split('\n')) {
    if (/^\s+validator:/.test(line)) currentService = 'validator';
    else if (/^\s+faucet:/.test(line)) currentService = 'faucet';
    else if (/^\s+explorer:/.test(line)) currentService = 'explorer';
    const pm = line.match(/- "(\d+):(\d+)"/);
    if (pm) {
        if (currentService === 'validator') validatorPorts.push(pm[1]);
        if (currentService === 'faucet') faucetPorts.push(pm[1]);
    }
}

assert(validatorPorts.includes('9100'), 'validator: metrics on host port 9100');
assert(faucetPorts.includes('9101'), 'faucet: HTTP on host port 9101 (not 9100)');
assert(!faucetPorts.includes('9100'), 'faucet: does NOT use host port 9100');

// Verify faucet PORT env matches its published port
assert(compose.includes('PORT=9101'), 'faucet: PORT env var is 9101');

// ═══════════════════════════════════════════════════════════════════════════
// F20.2 — Dockerfile EXPOSE clarity
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F20.2: Dockerfile EXPOSE ports ──');

const dockerfile = fs.readFileSync(path.join(root, 'Dockerfile'), 'utf-8');

assert(dockerfile.includes('EXPOSE 7001'), 'Dockerfile: exposes P2P port 7001');
assert(dockerfile.includes('EXPOSE 8899'), 'Dockerfile: exposes RPC port 8899');
assert(dockerfile.includes('EXPOSE 8900'), 'Dockerfile: exposes WS port 8900');
assert(dockerfile.includes('EXPOSE 9100'), 'Dockerfile: exposes metrics port 9100');
assert(dockerfile.includes('EXPOSE 9101'), 'Dockerfile: exposes faucet port 9101');

// Verify comments distinguish metrics vs faucet
assert(dockerfile.includes('Validator Metrics'), 'Dockerfile: 9100 labeled as Validator Metrics');
assert(dockerfile.includes('Faucet port'), 'Dockerfile: 9101 labeled as Faucet port');

// ═══════════════════════════════════════════════════════════════════════════
// F20.3 — Dockerfile includes curl for healthcheck
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F20.3: Dockerfile curl for healthcheck ──');

assert(dockerfile.includes('curl'), 'Dockerfile: curl package in apt-get install');

// Verify healthcheck uses curl
assert(compose.includes('curl -sf http://localhost:8899'),
    'docker-compose: healthcheck uses curl');

// ═══════════════════════════════════════════════════════════════════════════
// F20.4 — deploy/setup.sh network validation
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F20.4: deploy/setup.sh validation ──');

const setup = fs.readFileSync(path.join(root, 'deploy', 'setup.sh'), 'utf-8');

// Verify script uses set -euo pipefail (strict mode)
assert(setup.includes('set -euo pipefail'),
    'setup.sh: strict mode enabled');

// Verify network argument validation
assert(setup.includes('testnet|mainnet)'),
    'setup.sh: only testnet and mainnet accepted');
assert(setup.includes('exit 1'),
    'setup.sh: exits on invalid input');

// Verify port assignments
assert(setup.includes('RPC_PORT=8899') && setup.includes('WS_PORT=8900'),
    'setup.sh: testnet ports correct');
assert(setup.includes('RPC_PORT=9899') && setup.includes('WS_PORT=9900'),
    'setup.sh: mainnet ports correct');

// Verify env file has restricted permissions
assert(setup.includes('chmod 600'),
    'setup.sh: env file has 600 permissions');

// ═══════════════════════════════════════════════════════════════════════════
// F20.5 — Systemd service security hardening
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F20.5: Systemd service hardening ──');

const service = fs.readFileSync(path.join(root, 'deploy', 'lichen-validator.service'), 'utf-8');

// Security directives
assert(service.includes('NoNewPrivileges=true'),
    'systemd: NoNewPrivileges enabled');
assert(service.includes('ProtectSystem=strict'),
    'systemd: ProtectSystem=strict');
assert(service.includes('ProtectHome=true'),
    'systemd: ProtectHome enabled');
assert(service.includes('PrivateTmp=true'),
    'systemd: PrivateTmp enabled');
assert(service.includes('ProtectKernelTunables=true'),
    'systemd: ProtectKernelTunables enabled');
assert(service.includes('ProtectKernelModules=true'),
    'systemd: ProtectKernelModules enabled');
assert(service.includes('ProtectControlGroups=true'),
    'systemd: ProtectControlGroups enabled');

// Resource limits
assert(service.includes('LimitNOFILE=65536'),
    'systemd: NOFILE limit set to 65536');

// Restart policy
assert(service.includes('Restart=on-failure'),
    'systemd: restart on failure');
assert(service.includes('RestartSec=10'),
    'systemd: 10s restart delay');

// Runs as lichen user
assert(service.includes('User=lichen'),
    'systemd: runs as lichen user');
assert(service.includes('Group=lichen'),
    'systemd: runs in lichen group');

// ═══════════════════════════════════════════════════════════════════════════
// Docker-compose structure validation
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── Docker-compose structure ──');

// Verify 3 services exist
assert(compose.includes('validator:'), 'docker-compose: validator service exists');
assert(compose.includes('faucet:'), 'docker-compose: faucet service exists');
assert(compose.includes('explorer:'), 'docker-compose: explorer service exists');

// Verify faucet depends on healthy validator
assert(compose.includes('condition: service_healthy'),
    'docker-compose: faucet waits for healthy validator');

// Verify faucet RPC_URL points to validator
assert(compose.includes('RPC_URL=http://validator:8899'),
    'docker-compose: faucet RPC_URL uses docker-internal validator address');

// Verify explorer serves static files
assert(compose.includes('nginx:alpine'),
    'docker-compose: explorer uses nginx:alpine');

// Verify named volume
assert(compose.includes('lichen-data:'),
    'docker-compose: named volume for validator state');

// Verify bridge network
assert(compose.includes('driver: bridge'),
    'docker-compose: bridge network for inter-service communication');

// ═══════════════════════════════════════════════════════════════════════════
// Dockerfile structure validation
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── Dockerfile structure ──');

// Multi-stage build
assert(dockerfile.includes('FROM rust:'), 'Dockerfile: Rust builder stage');
assert(dockerfile.includes('AS builder'), 'Dockerfile: builder stage named');
assert(dockerfile.includes('FROM debian:bookworm-slim'), 'Dockerfile: slim runtime image');

// Non-root user
assert(dockerfile.includes('USER lichen'), 'Dockerfile: runs as non-root lichen user');

// Dependency caching
assert(dockerfile.includes('Cargo.toml Cargo.lock'),
    'Dockerfile: copies manifests for dependency caching');

// Binary copies
assert(dockerfile.includes('lichen-validator'), 'Dockerfile: copies validator binary');
assert(dockerfile.includes('lichen /usr/local/bin'), 'Dockerfile: copies CLI binary');
assert(dockerfile.includes('lichen-faucet'), 'Dockerfile: copies faucet binary');

// Volume
assert(dockerfile.includes('VOLUME ["/var/lib/lichen"]'),
    'Dockerfile: data volume declared');

// ═══════════════════════════════════════════════════════════════════════════
// Summary
// ═══════════════════════════════════════════════════════════════════════════

console.log(`\n═══ Phase 20 Infrastructure: ${passed} passed, ${failed} failed ═══`);
process.exit(failed > 0 ? 1 : 0);

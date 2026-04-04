#!/usr/bin/env node
'use strict';

const assert = require('assert');
const fs = require('fs');
const http = require('http');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');
const {
  stableJsonStringify,
  verifySignedMetadataEnvelope,
} = require('../monitoring/shared/utils.js');
const { publicKeyToAddress, verifySignature } = require('../monitoring/shared/pq.js');

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed++;
    console.log(`  ✅ ${name}`);
  } catch (error) {
    failed++;
    console.log(`  ❌ ${name}: ${error.message}`);
  }
}

async function main() {
  console.log('\n🔐 Signed Metadata Manifest Tests');
  console.log('='.repeat(60));

  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'signed-metadata-manifest-'));
  const keypairPath = path.join(tmpDir, 'metadata-keypair.json');
  const manifestPath = path.join(tmpDir, 'signed-metadata-manifest.json');
  fs.writeFileSync(keypairPath, JSON.stringify({ privateKey: new Array(32).fill(7) }, null, 2) + '\n');

  const registryEntries = [
    {
      symbol: 'DEX',
      program: '11111111111111111111111111111112',
      owner: '11111111111111111111111111111111',
      name: 'SporeSwap Core',
      template: 'dex',
      metadata: { icon_class: 'fa-chart-line' },
      decimals: null,
    },
    {
      symbol: 'LUSD',
      program: '11111111111111111111111111111113',
      owner: '11111111111111111111111111111111',
      name: 'Licn USD',
      template: 'token',
      metadata: { logo_url: 'https://example.invalid/lusd.png' },
      decimals: 9,
    },
    {
      symbol: 'YID',
      program: '11111111111111111111111111111114',
      owner: '11111111111111111111111111111111',
      name: 'LichenID',
      template: 'identity',
      metadata: { icon_class: 'fa-id-card' },
      decimals: null,
    },
  ];

  function pageRegistry(limit, cursor) {
    const sorted = registryEntries.slice().sort((left, right) => left.symbol.localeCompare(right.symbol));
    let start = 0;
    if (cursor) {
      const index = sorted.findIndex((entry) => entry.symbol === cursor);
      start = index >= 0 ? index + 1 : sorted.length;
    }
    const slice = sorted.slice(start, start + limit + 1);
    const hasMore = slice.length > limit;
    const entries = hasMore ? slice.slice(0, limit) : slice;
    return {
      entries,
      count: entries.length,
      has_more: hasMore,
      next_cursor: hasMore && entries.length ? entries[entries.length - 1].symbol : null,
    };
  }

  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', (chunk) => {
      body += chunk;
    });
    req.on('end', () => {
      const rpc = JSON.parse(body || '{}');
      let result = null;

      if (rpc.method === 'getAllSymbolRegistry') {
        const firstParam = Array.isArray(rpc.params) ? rpc.params[0] : null;
        const limit = typeof firstParam === 'object' && firstParam && typeof firstParam.limit === 'number'
          ? firstParam.limit
          : (typeof firstParam === 'number' ? firstParam : 500);
        const cursor = typeof firstParam === 'object' && firstParam ? firstParam.cursor : null;
        result = pageRegistry(limit, cursor);
      } else {
        res.writeHead(500, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ jsonrpc: '2.0', id: rpc.id || 1, error: { message: `Unexpected method ${rpc.method}` } }));
        return;
      }

      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ jsonrpc: '2.0', id: rpc.id || 1, result }));
    });
  });

  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  const rpcUrl = `http://127.0.0.1:${address.port}`;
  const scriptPath = path.join(__dirname, '..', 'scripts', 'generate-signed-metadata-manifest.js');
  const result = await new Promise((resolve, reject) => {
    const child = spawn('node', [
      scriptPath,
      '--rpc', rpcUrl,
      '--network', 'local-testnet',
      '--keypair', keypairPath,
      '--out', manifestPath,
      '--page-limit', '2',
    ], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });
    child.on('error', reject);
    child.on('close', (code) => {
      resolve({ status: code, stdout, stderr });
    });
  });

  test('generator script exits cleanly', () => {
    assert.strictEqual(result.status, 0, result.stderr || result.stdout);
  });

  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));

  test('manifest envelope contains the signed metadata shape', () => {
    assert.strictEqual(manifest.schema_version, 1);
    assert.strictEqual(manifest.manifest_type, 'signed_metadata');
    assert.strictEqual(manifest.payload.network, 'local-testnet');
    assert.ok(typeof manifest.signer === 'string' && manifest.signer.length > 20);
    assert.strictEqual(manifest.payload.symbol_registry.length, 3);
    assert.ok(manifest.signature && typeof manifest.signature === 'object');
    assert.ok(manifest.signature.public_key && typeof manifest.signature.public_key.bytes === 'string');
    assert.ok(typeof manifest.signature.sig === 'string' && manifest.signature.sig.length > 100);
  });

  test('generator sorts and canonicalizes the signed registry payload', () => {
    const symbols = manifest.payload.symbol_registry.map((entry) => entry.symbol);
    assert.deepStrictEqual(symbols, ['DEX', 'LUSD', 'YID']);
    assert.strictEqual(Buffer.from(stableJsonStringify(manifest.payload), 'utf8').length > 0, true);
  });

  const verified = await verifySignedMetadataEnvelope(
    manifest,
    'local-testnet',
    verifySignature,
    manifest.signer
  );

  test('shared verifier rebuilds symbol indexes from the signed payload', () => {
    assert.strictEqual(verified.network, 'local-testnet');
    assert.strictEqual(verified.registryEntries.length, 3);
    assert.strictEqual(verified.registryBySymbol.DEX.program, '11111111111111111111111111111112');
    assert.strictEqual(verified.registryByProgram['11111111111111111111111111111114'].symbol, 'YID');
  });

  const derivedSigner = await publicKeyToAddress(
    Buffer.from(manifest.signature.public_key.bytes, 'hex'),
    manifest.signature.public_key.scheme_version
  );

  test('generated manifest signer matches the embedded verifying key', () => {
    assert.strictEqual(manifest.signer, derivedSigner);
  });

  await new Promise((resolve) => server.close(resolve));
  fs.rmSync(tmpDir, { recursive: true, force: true });

  console.log(`\n${'='.repeat(60)}`);
  console.log(`Signed Metadata Manifest: ${passed} passed, ${failed} failed (${passed + failed} total)`);
  console.log(`${'='.repeat(60)}`);
  process.exit(failed > 0 ? 1 : 0);
}

main().catch((error) => {
  console.error(error.stack || error.message || error);
  process.exit(1);
});
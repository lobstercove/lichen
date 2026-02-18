// ═══════════════════════════════════════════════════════════════════════════════
// MoltyDEX Load Test — Concurrent Users Scenario
// Tests: concurrent order placement, concurrent reads, mixed read/write
// ═══════════════════════════════════════════════════════════════════════════════

import { MoltDEX } from '@moltchain/dex-sdk';

interface ScenarioResult {
  name: string;
  totalRequests: number;
  successCount: number;
  failCount: number;
  durationMs: number;
  rps: number;
  avgLatencyMs: number;
  p99LatencyMs: number;
  errors: string[];
}

const ENDPOINT = process.env.DEX_ENDPOINT || 'http://localhost:8899';

async function measureLatency(fn: () => Promise<any>): Promise<{ ok: boolean; ms: number; error?: string }> {
  const start = performance.now();
  try {
    await fn();
    return { ok: true, ms: performance.now() - start };
  } catch (e: any) {
    return { ok: false, ms: performance.now() - start, error: e.message };
  }
}

/**
 * Scenario: Concurrent order placement from N traders
 * Simulates N traders placing orders simultaneously in batches
 */
export async function concurrentOrderPlacement(
  traders: number = 50,
  ordersPerTrader: number = 10,
): Promise<ScenarioResult> {
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;
  const total = traders * ordersPerTrader;

  const start = performance.now();

  // Create trader clients
  const clients = Array.from({ length: traders }, () =>
    new MoltDEX({ endpoint: ENDPOINT })
  );

  // Each trader places orders concurrently
  const traderPromises = clients.map(async (dex, traderIdx) => {
    for (let i = 0; i < ordersPerTrader; i++) {
      const result = await measureLatency(() =>
        dex.placeLimitOrder({
          pair: 0,
          side: (traderIdx + i) % 2 === 0 ? 'buy' : 'sell',
          price: 1.0 + (traderIdx % 20) * 0.01 + (i % 10) * 0.001,
          quantity: 1000 + traderIdx * 10,
        })
      );
      latencies.push(result.ms);
      if (result.ok) successCount++;
      else errors.push(result.error || 'unknown');
    }
  });

  await Promise.all(traderPromises);
  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: `concurrent_orders_${traders}_traders`,
    totalRequests: total,
    successCount,
    failCount: total - successCount,
    durationMs: duration,
    rps: (total / duration) * 1000,
    avgLatencyMs: latencies.reduce((a, b) => a + b, 0) / (latencies.length || 1),
    p99LatencyMs: latencies[Math.floor(latencies.length * 0.99)] || 0,
    errors: [...new Set(errors)],
  };
}

/**
 * Scenario: Concurrent read storm
 * N clients hammering read endpoints simultaneously
 */
export async function concurrentReadStorm(
  clients: number = 100,
  queriesPerClient: number = 20,
): Promise<ScenarioResult> {
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;
  const total = clients * queriesPerClient;

  const dexClients = Array.from({ length: clients }, () =>
    new MoltDEX({ endpoint: ENDPOINT })
  );

  const start = performance.now();

  const clientPromises = dexClients.map(async (dex, clientIdx) => {
    for (let i = 0; i < queriesPerClient; i++) {
      const endpoint = i % 4;
      const result = await measureLatency(() => {
        switch (endpoint) {
          case 0: return dex.getOrderBook(0, 20);
          case 1: return dex.getTrades(0, 50);
          case 2: return dex.getPairs();
          case 3: return dex.getPools();
          default: return dex.getPairs();
        }
      });
      latencies.push(result.ms);
      if (result.ok) successCount++;
      else errors.push(result.error || 'unknown');
    }
  });

  await Promise.all(clientPromises);
  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: `concurrent_reads_${clients}_clients`,
    totalRequests: total,
    successCount,
    failCount: total - successCount,
    durationMs: duration,
    rps: (total / duration) * 1000,
    avgLatencyMs: latencies.reduce((a, b) => a + b, 0) / (latencies.length || 1),
    p99LatencyMs: latencies[Math.floor(latencies.length * 0.99)] || 0,
    errors: [...new Set(errors)],
  };
}

/**
 * Scenario: Mixed read/write workload
 * 70% reads, 30% writes — realistic production mix
 */
export async function mixedWorkload(
  clients: number = 30,
  opsPerClient: number = 30,
): Promise<ScenarioResult> {
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;
  const total = clients * opsPerClient;

  const dexClients = Array.from({ length: clients }, () =>
    new MoltDEX({ endpoint: ENDPOINT })
  );

  const start = performance.now();

  const clientPromises = dexClients.map(async (dex, clientIdx) => {
    for (let i = 0; i < opsPerClient; i++) {
      const isWrite = (i + clientIdx) % 10 < 3; // 30% writes

      const result = await measureLatency(() => {
        if (isWrite) {
          return dex.placeLimitOrder({
            pair: 0,
            side: i % 2 === 0 ? 'buy' : 'sell',
            price: 1.0 + (i % 20) * 0.01,
            quantity: 1000,
          });
        } else {
          switch (i % 3) {
            case 0: return dex.getOrderBook(0, 20);
            case 1: return dex.getTrades(0, 50);
            default: return dex.getTicker(0);
          }
        }
      });

      latencies.push(result.ms);
      if (result.ok) successCount++;
      else errors.push(result.error || 'unknown');
    }
  });

  await Promise.all(clientPromises);
  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: `mixed_workload_${clients}_clients`,
    totalRequests: total,
    successCount,
    failCount: total - successCount,
    durationMs: duration,
    rps: (total / duration) * 1000,
    avgLatencyMs: latencies.reduce((a, b) => a + b, 0) / (latencies.length || 1),
    p99LatencyMs: latencies[Math.floor(latencies.length * 0.99)] || 0,
    errors: [...new Set(errors)],
  };
}

if (require.main === module) {
  (async () => {
    console.log('═══ MoltyDEX Load Test: Concurrent Scenarios ═══\n');
    console.log(`Endpoint: ${ENDPOINT}\n`);

    const scenarios = [
      () => concurrentOrderPlacement(50, 20),
      () => concurrentReadStorm(100, 50),
      () => mixedWorkload(30, 50),
    ];

    for (const scenario of scenarios) {
      const result = await scenario();
      console.log(`\n─── ${result.name} ───`);
      console.log(`  Total:   ${result.totalRequests} requests`);
      console.log(`  Success: ${result.successCount} (${((result.successCount / result.totalRequests) * 100).toFixed(1)}%)`);
      console.log(`  RPS:     ${result.rps.toFixed(1)}`);
      console.log(`  Avg:     ${result.avgLatencyMs.toFixed(1)}ms`);
      console.log(`  P99:     ${result.p99LatencyMs.toFixed(1)}ms`);
      if (result.errors.length > 0) {
        console.log(`  Errors:  ${result.errors.join(', ')}`);
      }
    }
  })();
}

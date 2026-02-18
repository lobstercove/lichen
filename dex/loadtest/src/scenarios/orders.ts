// ═══════════════════════════════════════════════════════════════════════════════
// MoltyDEX Load Test — Order Throughput Scenario
// Tests: order placement rate, cancellation rate, matching throughput
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
const PAIR_ID = 0; // MOLT/mUSD

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
 * Scenario 1: Sequential order placement
 * Places N orders sequentially, measures throughput and latency
 */
export async function orderPlacementSequential(count: number = 100): Promise<ScenarioResult> {
  const dex = new MoltDEX({ endpoint: ENDPOINT });
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;

  const start = performance.now();

  for (let i = 0; i < count; i++) {
    const price = 1.0 + (i % 100) * 0.01; // Vary prices
    const result = await measureLatency(() =>
      dex.placeLimitOrder({
        pair: PAIR_ID,
        side: i % 2 === 0 ? 'buy' : 'sell',
        price,
        quantity: 1000 + (i % 50) * 100,
      })
    );

    latencies.push(result.ms);
    if (result.ok) successCount++;
    else errors.push(result.error || 'unknown');
  }

  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: 'order_placement_sequential',
    totalRequests: count,
    successCount,
    failCount: count - successCount,
    durationMs: duration,
    rps: (count / duration) * 1000,
    avgLatencyMs: latencies.reduce((a, b) => a + b, 0) / latencies.length,
    p99LatencyMs: latencies[Math.floor(latencies.length * 0.99)] || 0,
    errors: [...new Set(errors)],
  };
}

/**
 * Scenario 2: Order cancel storm
 * Places orders then cancels them all as fast as possible
 */
export async function orderCancelStorm(count: number = 100): Promise<ScenarioResult> {
  const dex = new MoltDEX({ endpoint: ENDPOINT });
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;

  // First, place orders
  const orderIds: number[] = [];
  for (let i = 0; i < count; i++) {
    try {
      const resp = await dex.placeLimitOrder({
        pair: PAIR_ID,
        side: 'buy',
        price: 0.50 + i * 0.001, // Low prices to avoid matching
        quantity: 1000,
      });
      if (resp.data) orderIds.push(resp.data.orderId);
    } catch {
      // continue
    }
  }

  // Now cancel them all
  const start = performance.now();

  for (const orderId of orderIds) {
    const result = await measureLatency(() => dex.cancelOrder({ orderId }));
    latencies.push(result.ms);
    if (result.ok) successCount++;
    else errors.push(result.error || 'unknown');
  }

  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: 'order_cancel_storm',
    totalRequests: orderIds.length,
    successCount,
    failCount: orderIds.length - successCount,
    durationMs: duration,
    rps: (orderIds.length / duration) * 1000,
    avgLatencyMs: latencies.reduce((a, b) => a + b, 0) / (latencies.length || 1),
    p99LatencyMs: latencies[Math.floor(latencies.length * 0.99)] || 0,
    errors: [...new Set(errors)],
  };
}

/**
 * Scenario 3: Order book depth query under load
 * Hammers the orderbook endpoint while orders are being placed
 */
export async function orderbookQueryUnderLoad(
  queryCount: number = 200,
  orderCount: number = 50,
): Promise<ScenarioResult> {
  const dex = new MoltDEX({ endpoint: ENDPOINT });
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;

  // Background: place some orders
  const orderPromise = (async () => {
    for (let i = 0; i < orderCount; i++) {
      await dex.placeLimitOrder({
        pair: PAIR_ID,
        side: i % 2 === 0 ? 'buy' : 'sell',
        price: 1.0 + (i % 20) * 0.01,
        quantity: 1000,
      }).catch(() => {});
    }
  })();

  const start = performance.now();

  // Foreground: query orderbook repeatedly
  for (let i = 0; i < queryCount; i++) {
    const result = await measureLatency(() => dex.getOrderBook(PAIR_ID, 20));
    latencies.push(result.ms);
    if (result.ok) successCount++;
    else errors.push(result.error || 'unknown');
  }

  await orderPromise;
  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: 'orderbook_query_under_load',
    totalRequests: queryCount,
    successCount,
    failCount: queryCount - successCount,
    durationMs: duration,
    rps: (queryCount / duration) * 1000,
    avgLatencyMs: latencies.reduce((a, b) => a + b, 0) / latencies.length,
    p99LatencyMs: latencies[Math.floor(latencies.length * 0.99)] || 0,
    errors: [...new Set(errors)],
  };
}

// Run if executed directly
if (require.main === module) {
  (async () => {
    console.log('═══ MoltyDEX Load Test: Order Scenarios ═══\n');
    console.log(`Endpoint: ${ENDPOINT}\n`);

    const scenarios = [
      () => orderPlacementSequential(500),
      () => orderCancelStorm(200),
      () => orderbookQueryUnderLoad(500, 100),
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

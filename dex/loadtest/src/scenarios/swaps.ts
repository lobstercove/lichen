// ═══════════════════════════════════════════════════════════════════════════════
// MoltyDEX Load Test — Swap Throughput Scenario
// Tests: router swap rate, AMM pool swap rate, quote endpoint performance
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
 * Scenario: Router swap throughput
 * Executes smart-routed swaps as fast as possible
 */
export async function routerSwapThroughput(count: number = 200): Promise<ScenarioResult> {
  const dex = new MoltDEX({ endpoint: ENDPOINT });
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;

  const start = performance.now();

  for (let i = 0; i < count; i++) {
    const amountIn = 1000 + (i % 100) * 100;
    const result = await measureLatency(() =>
      dex.swap({
        tokenIn: 'MOLT',
        tokenOut: 'mUSD',
        amountIn,
        slippage: 0.5,
      })
    );

    latencies.push(result.ms);
    if (result.ok) successCount++;
    else errors.push(result.error || 'unknown');
  }

  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: 'router_swap_throughput',
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
 * Scenario: Quote endpoint performance
 * Rapid-fire swap quotes (read-only, no state change)
 */
export async function quotePerformance(count: number = 500): Promise<ScenarioResult> {
  const dex = new MoltDEX({ endpoint: ENDPOINT });
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;

  const start = performance.now();

  for (let i = 0; i < count; i++) {
    const result = await measureLatency(() =>
      dex.getSwapQuote({
        tokenIn: 'MOLT',
        tokenOut: 'mUSD',
        amountIn: 10_000 + i * 100,
        slippage: 1.0,
      })
    );

    latencies.push(result.ms);
    if (result.ok) successCount++;
    else errors.push(result.error || 'unknown');
  }

  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: 'quote_performance',
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
 * Scenario: Multi-pair swap rotation
 * Swaps across different token pairs in sequence
 */
export async function multiPairSwapRotation(count: number = 300): Promise<ScenarioResult> {
  const dex = new MoltDEX({ endpoint: ENDPOINT });
  const latencies: number[] = [];
  const errors: string[] = [];
  let successCount = 0;

  const pairs = [
    { in: 'MOLT', out: 'mUSD' },
    { in: 'wSOL', out: 'mUSD' },
    { in: 'wETH', out: 'mUSD' },
    { in: 'mUSD', out: 'MOLT' },
  ];

  const start = performance.now();

  for (let i = 0; i < count; i++) {
    const pair = pairs[i % pairs.length];
    const result = await measureLatency(() =>
      dex.swap({
        tokenIn: pair.in,
        tokenOut: pair.out,
        amountIn: 5000,
        slippage: 1.0,
      })
    );

    latencies.push(result.ms);
    if (result.ok) successCount++;
    else errors.push(result.error || 'unknown');
  }

  const duration = performance.now() - start;
  latencies.sort((a, b) => a - b);

  return {
    name: 'multi_pair_swap_rotation',
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

if (require.main === module) {
  (async () => {
    console.log('═══ MoltyDEX Load Test: Swap Scenarios ═══\n');
    console.log(`Endpoint: ${ENDPOINT}\n`);

    const scenarios = [
      () => routerSwapThroughput(500),
      () => quotePerformance(1000),
      () => multiPairSwapRotation(500),
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

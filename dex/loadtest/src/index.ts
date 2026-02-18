// ═══════════════════════════════════════════════════════════════════════════════
// MoltyDEX Load Test — Main Runner
// Runs all load test scenarios and produces a summary report
// ═══════════════════════════════════════════════════════════════════════════════

import { orderPlacementSequential, orderCancelStorm, orderbookQueryUnderLoad } from './scenarios/orders';
import { routerSwapThroughput, quotePerformance, multiPairSwapRotation } from './scenarios/swaps';
import { concurrentOrderPlacement, concurrentReadStorm, mixedWorkload } from './scenarios/concurrent';

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

function printResult(result: ScenarioResult): void {
  const successRate = ((result.successCount / result.totalRequests) * 100).toFixed(1);
  const status = result.failCount === 0 ? '✅' : result.failCount < result.totalRequests * 0.05 ? '⚠️' : '❌';

  console.log(`  ${status} ${result.name.padEnd(40)} ${String(result.totalRequests).padStart(6)} reqs  ${result.rps.toFixed(0).padStart(6)} rps  ${result.avgLatencyMs.toFixed(1).padStart(8)}ms avg  ${result.p99LatencyMs.toFixed(1).padStart(8)}ms p99  ${successRate}%`);
  if (result.errors.length > 0) {
    console.log(`     Errors: ${result.errors.slice(0, 3).join(', ')}`);
  }
}

async function main(): Promise<void> {
  console.log('╔════════════════════════════════════════════════════════════════╗');
  console.log('║            MoltyDEX Load Test Suite                           ║');
  console.log(`║  Endpoint: ${ENDPOINT.padEnd(51)}║`);
  console.log(`║  Time:     ${new Date().toISOString().padEnd(51)}║`);
  console.log('╚════════════════════════════════════════════════════════════════╝');
  console.log('');

  const results: ScenarioResult[] = [];

  // ── Order Scenarios ──
  console.log('─── Order Scenarios ───');
  results.push(await orderPlacementSequential(500));
  printResult(results[results.length - 1]);

  results.push(await orderCancelStorm(200));
  printResult(results[results.length - 1]);

  results.push(await orderbookQueryUnderLoad(500, 100));
  printResult(results[results.length - 1]);

  // ── Swap Scenarios ──
  console.log('\n─── Swap Scenarios ───');
  results.push(await routerSwapThroughput(500));
  printResult(results[results.length - 1]);

  results.push(await quotePerformance(1000));
  printResult(results[results.length - 1]);

  results.push(await multiPairSwapRotation(500));
  printResult(results[results.length - 1]);

  // ── Concurrent Scenarios ──
  console.log('\n─── Concurrent Scenarios ───');
  results.push(await concurrentOrderPlacement(50, 20));
  printResult(results[results.length - 1]);

  results.push(await concurrentReadStorm(100, 50));
  printResult(results[results.length - 1]);

  results.push(await mixedWorkload(30, 50));
  printResult(results[results.length - 1]);

  // ── Summary ──
  console.log('\n╔════════════════════════════════════════════════════════════════╗');
  console.log('║  SUMMARY                                                       ║');
  console.log('╚════════════════════════════════════════════════════════════════╝');

  const totalReqs = results.reduce((s, r) => s + r.totalRequests, 0);
  const totalSuccess = results.reduce((s, r) => s + r.successCount, 0);
  const totalFail = results.reduce((s, r) => s + r.failCount, 0);
  const avgRps = results.reduce((s, r) => s + r.rps, 0) / results.length;
  const maxP99 = Math.max(...results.map(r => r.p99LatencyMs));

  console.log(`  Total requests:  ${totalReqs}`);
  console.log(`  Success:         ${totalSuccess} (${((totalSuccess / totalReqs) * 100).toFixed(1)}%)`);
  console.log(`  Failed:          ${totalFail}`);
  console.log(`  Avg RPS:         ${avgRps.toFixed(0)}`);
  console.log(`  Max P99 Latency: ${maxP99.toFixed(1)}ms`);
  console.log(`  Scenarios:       ${results.length} total, ${results.filter(r => r.failCount === 0).length} passed`);

  // Targets
  console.log('\n  Target Thresholds:');
  console.log(`  ${avgRps >= 100 ? '✅' : '❌'} Avg RPS ≥ 100:        ${avgRps.toFixed(0)}`);
  console.log(`  ${maxP99 <= 500 ? '✅' : '❌'} Max P99 ≤ 500ms:     ${maxP99.toFixed(1)}ms`);
  console.log(`  ${totalFail === 0 ? '✅' : '❌'} Zero failures:       ${totalFail}`);
}

main().catch(console.error);

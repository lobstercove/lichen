// ═══════════════════════════════════════════════════════════════════════════════
// Market Maker Configuration
// ═══════════════════════════════════════════════════════════════════════════════

export interface SpreadConfig {
  /** Half-spread in basis points (e.g., 10 = 0.1% each side) */
  halfSpreadBps: number;
  /** Number of price levels per side */
  levels: number;
  /** Size per level (in base token units, unscaled) */
  sizePerLevel: number;
  /** Price step between levels (in bps) */
  levelStepBps: number;
  /** Refresh interval in ms */
  refreshMs: number;
  /** Max position skew before hedging (in base units) */
  maxSkew: number;
}

export interface GridConfig {
  /** Lower price bound */
  priceLow: number;
  /** Upper price bound */
  priceHigh: number;
  /** Number of grid levels */
  gridLevels: number;
  /** Size per grid order (in base units) */
  sizePerOrder: number;
  /** Refresh interval in ms */
  refreshMs: number;
}

export interface BotConfig {
  /** DEX API endpoint */
  endpoint: string;
  /** WebSocket endpoint */
  wsEndpoint: string;
  /** Trading pair ID */
  pairId: number;
  /** Strategy type */
  strategy: 'spread' | 'grid';
  /** Strategy-specific config */
  spread?: SpreadConfig;
  grid?: GridConfig;
  /** Max total orders to maintain */
  maxOrders: number;
  /** Dry run mode (log only, no orders) */
  dryRun: boolean;
  /** Log level */
  logLevel: 'debug' | 'info' | 'warn' | 'error';
}

export const DEFAULT_SPREAD_CONFIG: SpreadConfig = {
  halfSpreadBps: 15,
  levels: 5,
  sizePerLevel: 1000,
  levelStepBps: 5,
  refreshMs: 2000,
  maxSkew: 10000,
};

export const DEFAULT_GRID_CONFIG: GridConfig = {
  priceLow: 0.80,
  priceHigh: 1.20,
  gridLevels: 20,
  sizePerOrder: 500,
  refreshMs: 5000,
};

export function loadConfig(): BotConfig {
  const strategy = (process.env.MM_STRATEGY || 'spread') as 'spread' | 'grid';

  return {
    endpoint: process.env.DEX_ENDPOINT || 'http://localhost:8899',
    wsEndpoint: process.env.DEX_WS_ENDPOINT || 'ws://localhost:8900/ws',
    pairId: parseInt(process.env.MM_PAIR_ID || '0', 10),
    strategy,
    spread: {
      halfSpreadBps: parseInt(process.env.MM_HALF_SPREAD_BPS || '15', 10),
      levels: parseInt(process.env.MM_LEVELS || '5', 10),
      sizePerLevel: parseFloat(process.env.MM_SIZE_PER_LEVEL || '1000'),
      levelStepBps: parseInt(process.env.MM_LEVEL_STEP_BPS || '5', 10),
      refreshMs: parseInt(process.env.MM_REFRESH_MS || '2000', 10),
      maxSkew: parseFloat(process.env.MM_MAX_SKEW || '10000'),
    },
    grid: {
      priceLow: parseFloat(process.env.MM_GRID_LOW || '0.80'),
      priceHigh: parseFloat(process.env.MM_GRID_HIGH || '1.20'),
      gridLevels: parseInt(process.env.MM_GRID_LEVELS || '20', 10),
      sizePerOrder: parseFloat(process.env.MM_GRID_SIZE || '500'),
      refreshMs: parseInt(process.env.MM_GRID_REFRESH_MS || '5000', 10),
    },
    maxOrders: parseInt(process.env.MM_MAX_ORDERS || '50', 10),
    dryRun: process.env.MM_DRY_RUN === 'true',
    logLevel: (process.env.MM_LOG_LEVEL || 'info') as any,
  };
}

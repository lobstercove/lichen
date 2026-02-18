// ═══════════════════════════════════════════════════════════════════════════════
// @moltchain/dex-sdk — Main Entry Point
// Re-exports all modules for convenient access
// ═══════════════════════════════════════════════════════════════════════════════

// Client (primary export)
export { MoltDEX } from './client';

// WebSocket
export { DexWebSocket } from './websocket';

// Order book utilities
export {
  decodeOrder,
  encodePlaceOrder,
  encodeCancelOrder,
  buildOrderBook,
  midPrice,
  spreadBps,
} from './orderbook';

// AMM utilities
export {
  decodePool,
  decodeLPPosition,
  encodeCreatePool,
  encodeAddLiquidity,
  encodeRemoveLiquidity,
  encodeSwap,
  priceToSqrtPrice,
  sqrtPriceToPrice,
  priceToTick,
  tickToPrice,
  feeTierBps,
  estimateSwapOutput,
} from './amm';

// Router utilities
export {
  decodeRoute,
  encodeRouterSwap,
  decodeSwapRecord,
  calculateMinOutput,
  calculatePriceImpact,
  suggestRouteType,
} from './router';

// Margin utilities
export {
  decodeMarginPosition,
  encodeOpenPosition,
  encodeClosePosition,
  encodeAddMargin,
  unrealizedPnl,
  marginRatio,
  isLiquidatable,
  liquidationPrice,
  effectiveLeverage,
} from './margin';

// All types
export type {
  // Core primitives
  Address,
  ScaledPrice,
  Side,
  PositionSide,
  OrderType,
  TimeInForce,
  OrderStatus,
  PositionStatus,
  PairStatus,
  ProposalType,
  ProposalStatus,
  RouteType,
  FeeTier,
  CandleInterval,
  // Data structures
  TradingPair,
  Order,
  OrderBookLevel,
  OrderBook,
  Trade,
  Pool,
  LPPosition,
  Route,
  SwapResult,
  MarginPosition,
  MarginInfo,
  Candle,
  Stats24h,
  TraderStats,
  LeaderboardEntry,
  Proposal,
  RewardInfo,
  Ticker,
  // Params
  PlaceOrderParams,
  CancelOrderParams,
  SwapParams,
  OpenPositionParams,
  ClosePositionParams,
  AddLiquidityParams,
  RemoveLiquidityParams,
  CreatePoolParams,
  CreateProposalParams,
  // Config
  MoltDEXConfig,
  ApiResponse,
  // WebSocket events
  WSOrderBookUpdate,
  WSTradeEvent,
  WSTickerEvent,
  WSCandleEvent,
  WSOrderEvent,
  WSPositionEvent,
} from './types';

/** SDK Version */
export const VERSION = '1.0.0';

/** Default API endpoint */
export const DEFAULT_ENDPOINT = 'http://localhost:8899';

/** Default WebSocket endpoint */
export const DEFAULT_WS_ENDPOINT = 'ws://localhost:8900';

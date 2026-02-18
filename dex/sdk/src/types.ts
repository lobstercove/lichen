// ═══════════════════════════════════════════════════════════════════════════════
// @moltchain/dex-sdk — Shared TypeScript Types
// MoltyDEX: Hybrid CLOB + Concentrated Liquidity AMM
// ═══════════════════════════════════════════════════════════════════════════════

// ---------------------------------------------------------------------------
// Core Primitives
// ---------------------------------------------------------------------------

/** 32-byte address as hex string */
export type Address = string;

/** Price scaled by 1e9 (1.0 = 1_000_000_000) */
export type ScaledPrice = bigint;

/** Side of an order / position */
export type Side = 'buy' | 'sell';

/** Position direction */
export type PositionSide = 'long' | 'short';

/** Order type */
export type OrderType = 'limit' | 'market' | 'stop-limit' | 'post-only';

/** Time in force */
export type TimeInForce = 'GTC' | 'IOC' | 'FOK';

/** Order status */
export type OrderStatus = 'open' | 'partial' | 'filled' | 'cancelled' | 'expired';

/** Margin position status */
export type PositionStatus = 'open' | 'closed' | 'liquidated';

/** Pair status */
export type PairStatus = 'active' | 'paused' | 'delisted';

/** Proposal types */
export type ProposalType = 'new_pair' | 'fee_change' | 'delist' | 'param_change';

/** Proposal status */
export type ProposalStatus = 'active' | 'passed' | 'rejected' | 'executed' | 'cancelled';

/** Route type */
export type RouteType = 'clob' | 'amm' | 'split' | 'multi_hop' | 'legacy';

/** AMM fee tier */
export type FeeTier = '1bps' | '5bps' | '30bps' | '100bps';

/** OHLCV candle interval (seconds) */
export type CandleInterval = 60 | 300 | 900 | 3600 | 14400 | 86400;

// ---------------------------------------------------------------------------
// Trading Pair
// ---------------------------------------------------------------------------

export interface TradingPair {
  pairId: number;
  baseToken: Address;
  quoteToken: Address;
  tickSize: bigint;
  lotSize: bigint;
  minOrder: bigint;
  status: PairStatus;
  makerFeeBps: number;
  takerFeeBps: number;
  dailyVolume: bigint;
  /** Human-readable symbol e.g. "MOLT/mUSD" */
  symbol?: string;
}

// ---------------------------------------------------------------------------
// Orders
// ---------------------------------------------------------------------------

export interface Order {
  orderId: number;
  trader: Address;
  pairId: number;
  side: Side;
  orderType: OrderType;
  price: bigint;
  quantity: bigint;
  filled: bigint;
  status: OrderStatus;
  createdSlot: number;
  expirySlot: number;
}

export interface PlaceOrderParams {
  pair: string | number;
  side: Side;
  price: number;
  quantity: number;
  orderType?: OrderType;
  timeInForce?: TimeInForce;
  /** Expiry in slots (0 = GTC) */
  expiry?: number;
}

export interface CancelOrderParams {
  orderId: number;
}

// ---------------------------------------------------------------------------
// Order Book
// ---------------------------------------------------------------------------

export interface OrderBookLevel {
  price: number;
  quantity: number;
  orders: number;
}

export interface OrderBook {
  pairId: number;
  bids: OrderBookLevel[];
  asks: OrderBookLevel[];
  lastUpdate: number;
}

// ---------------------------------------------------------------------------
// Trades
// ---------------------------------------------------------------------------

export interface Trade {
  tradeId: number;
  pairId: number;
  price: number;
  quantity: number;
  taker: Address;
  makerOrderId: number;
  slot: number;
  side: Side;
  timestamp?: number;
}

// ---------------------------------------------------------------------------
// AMM Pool
// ---------------------------------------------------------------------------

export interface Pool {
  poolId: number;
  tokenA: Address;
  tokenB: Address;
  sqrtPrice: bigint;
  tick: number;
  liquidity: bigint;
  feeTier: FeeTier;
  protocolFee: number;
}

export interface LPPosition {
  positionId: number;
  owner: Address;
  poolId: number;
  lowerTick: number;
  upperTick: number;
  liquidity: bigint;
  feeAOwed: bigint;
  feeBOwed: bigint;
  createdSlot: number;
}

export interface AddLiquidityParams {
  poolId: number;
  lowerTick: number;
  upperTick: number;
  amount: number;
}

export interface RemoveLiquidityParams {
  positionId: number;
}

export interface CreatePoolParams {
  tokenA: Address;
  tokenB: Address;
  sqrtPrice: number;
  feeTier: FeeTier;
}

// ---------------------------------------------------------------------------
// Router (Smart Order Routing)
// ---------------------------------------------------------------------------

export interface Route {
  routeId: number;
  tokenIn: Address;
  tokenOut: Address;
  routeType: RouteType;
  poolOrPairId: number;
  secondaryId: number;
  splitPercent: number;
  enabled: boolean;
}

export interface SwapParams {
  tokenIn: string | Address;
  tokenOut: string | Address;
  amountIn: number;
  /** Max slippage in percent (e.g. 0.5 = 0.5%) */
  slippage: number;
}

export interface SwapResult {
  amountIn: bigint;
  amountOut: bigint;
  routeType: RouteType;
  routeId: number;
  priceImpact: number;
  slot: number;
}

// ---------------------------------------------------------------------------
// Margin Trading
// ---------------------------------------------------------------------------

export interface MarginPosition {
  positionId: number;
  trader: Address;
  pairId: number;
  side: PositionSide;
  status: PositionStatus;
  size: bigint;
  margin: bigint;
  entryPrice: bigint;
  leverage: number;
  createdSlot: number;
  realizedPnl: bigint;
  accumulatedFunding: bigint;
}

export interface OpenPositionParams {
  pair: string | number;
  side: PositionSide;
  margin: number;
  leverage: number;
}

export interface ClosePositionParams {
  positionId: number;
}

export interface MarginInfo {
  insuranceFund: bigint;
  lastFundingSlot: number;
  maintenanceBps: number;
}

// ---------------------------------------------------------------------------
// Analytics
// ---------------------------------------------------------------------------

export interface Candle {
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
  slot: number;
  timestamp?: number;
}

export interface Stats24h {
  volume: number;
  high: number;
  low: number;
  open: number;
  close: number;
  tradeCount: number;
  change: number;
  changePercent: number;
}

export interface TraderStats {
  address: Address;
  totalVolume: bigint;
  tradeCount: number;
  totalPnl: bigint;
  lastTradeSlot: number;
}

export interface LeaderboardEntry {
  rank: number;
  address: Address;
  volume: bigint;
  tradeCount: number;
  pnl: bigint;
}

// ---------------------------------------------------------------------------
// Governance
// ---------------------------------------------------------------------------

export interface Proposal {
  proposalId: number;
  proposer: Address;
  proposalType: ProposalType;
  status: ProposalStatus;
  createdSlot: number;
  endSlot: number;
  yesVotes: bigint;
  noVotes: bigint;
  pairId: number;
  data: Uint8Array;
  newMakerFee: number;
  newTakerFee: number;
}

export interface CreateProposalParams {
  proposalType: ProposalType;
  pairId: number;
  data?: Uint8Array;
  makerFee?: number;
  takerFee?: number;
}

// ---------------------------------------------------------------------------
// Rewards
// ---------------------------------------------------------------------------

export interface RewardInfo {
  pending: bigint;
  claimed: bigint;
  totalVolume: bigint;
  tier: number;
  referrer?: Address;
  referralCount: number;
  referralEarnings: bigint;
}

// ---------------------------------------------------------------------------
// WebSocket Events
// ---------------------------------------------------------------------------

export interface WSOrderBookUpdate {
  pairId: number;
  bids: OrderBookLevel[];
  asks: OrderBookLevel[];
  slot: number;
}

export interface WSTradeEvent {
  tradeId: number;
  pairId: number;
  price: number;
  quantity: number;
  side: Side;
  slot: number;
}

export interface WSTickerEvent {
  pairId: number;
  lastPrice: number;
  bid: number;
  ask: number;
  volume24h: number;
  change24h: number;
}

export interface WSCandleEvent {
  pairId: number;
  interval: CandleInterval;
  candle: Candle;
}

export interface WSOrderEvent {
  orderId: number;
  status: OrderStatus;
  filled: bigint;
  remaining: bigint;
  slot: number;
}

export interface WSPositionEvent {
  positionId: number;
  status: PositionStatus;
  unrealizedPnl: bigint;
  marginRatio: number;
  slot: number;
}

// ---------------------------------------------------------------------------
// Client Configuration
// ---------------------------------------------------------------------------

export interface MoltDEXConfig {
  /** REST API endpoint (default: http://localhost:8899) */
  endpoint?: string;
  /** WebSocket endpoint (default: ws://localhost:8900) */
  wsEndpoint?: string;
  /** Wallet keypair for signing transactions */
  wallet?: any; // Keypair from @moltchain/sdk
  /** MoltyID identity string (e.g. "alice.molt") */
  moltyId?: string;
  /** API key for rate limit bypass */
  apiKey?: string;
  /** Request timeout in ms (default: 30000) */
  timeout?: number;
}

// ---------------------------------------------------------------------------
// API Response Wrapper
// ---------------------------------------------------------------------------

export interface ApiResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
  slot?: number;
}

/** Ticker summary for a pair */
export interface Ticker {
  pairId: number;
  symbol: string;
  lastPrice: number;
  bid: number;
  ask: number;
  high24h: number;
  low24h: number;
  volume24h: number;
  change24h: number;
  changePercent24h: number;
}

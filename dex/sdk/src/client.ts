// ═══════════════════════════════════════════════════════════════════════════════
// @moltchain/dex-sdk — MoltDEX Client
// Main entry point for interacting with MoltyDEX
// ═══════════════════════════════════════════════════════════════════════════════

import type {
  MoltDEXConfig,
  ApiResponse,
  TradingPair,
  Order,
  OrderBook,
  Trade,
  Pool,
  LPPosition,
  MarginPosition,
  MarginInfo,
  Route,
  SwapResult,
  Candle,
  Stats24h,
  LeaderboardEntry,
  RewardInfo,
  Proposal,
  Ticker,
  PlaceOrderParams,
  CancelOrderParams,
  SwapParams,
  OpenPositionParams,
  ClosePositionParams,
  AddLiquidityParams,
  RemoveLiquidityParams,
  CreatePoolParams,
  CreateProposalParams,
  CandleInterval,
  Address,
} from './types';
import { DexWebSocket } from './websocket';

const DEFAULT_ENDPOINT = 'http://localhost:8899';
const DEFAULT_WS_ENDPOINT = 'ws://localhost:8900/ws';
const DEFAULT_TIMEOUT = 30_000;
const PRICE_SCALE = 1_000_000_000;

/**
 * MoltDEX SDK Client
 *
 * @example
 * ```typescript
 * import { MoltDEX } from '@moltchain/dex-sdk';
 *
 * const dex = new MoltDEX({
 *   endpoint: 'https://dex.moltchain.io',
 *   wallet: myKeypair,
 *   moltyId: 'alice.molt',
 * });
 *
 * // Place a limit order
 * const order = await dex.placeLimitOrder({
 *   pair: 'MOLT/mUSD', side: 'buy', price: 1.50, quantity: 1000, timeInForce: 'GTC'
 * });
 *
 * // Smart-routed swap
 * const result = await dex.swap({
 *   tokenIn: 'MOLT', tokenOut: 'mUSD', amountIn: 1_000_000, slippage: 0.5
 * });
 * ```
 */
export class MoltDEX {
  private endpoint: string;
  private wsEndpoint: string;
  private wallet: any;
  private moltyId?: string;
  private apiKey?: string;
  private timeout: number;
  private ws: DexWebSocket | null = null;

  constructor(config: MoltDEXConfig = {}) {
    this.endpoint = (config.endpoint || DEFAULT_ENDPOINT).replace(/\/$/, '');
    this.wsEndpoint = (config.wsEndpoint || DEFAULT_WS_ENDPOINT).replace(/\/$/, '');
    this.wallet = config.wallet;
    this.moltyId = config.moltyId;
    this.apiKey = config.apiKey;
    this.timeout = config.timeout || DEFAULT_TIMEOUT;
  }

  // -------------------------------------------------------------------------
  // HTTP helpers
  // -------------------------------------------------------------------------

  private async request<T>(method: string, path: string, body?: any): Promise<T> {
    const url = `${this.endpoint}${path}`;
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    if (this.apiKey) headers['X-API-Key'] = this.apiKey;
    if (this.moltyId) headers['X-MoltyID'] = this.moltyId;

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeout);

    try {
      const res = await fetch(url, {
        method,
        headers,
        body: body ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });
      if (!res.ok) {
        const text = await res.text().catch(() => '');
        throw new Error(`HTTP ${res.status}: ${text}`);
      }
      const json = await res.json();
      // API returns { success, data, error, slot } envelope — unwrap it
      if (json && typeof json === 'object' && 'success' in json) {
        if (!json.success) {
          throw new Error(json.error || 'API request failed');
        }
        return json.data as T;
      }
      return json as T;
    } finally {
      clearTimeout(timer);
    }
  }

  private get<T>(path: string): Promise<T> {
    return this.request('GET', path);
  }

  private post<T>(path: string, body: any): Promise<T> {
    return this.request('POST', path, body);
  }

  private del<T>(path: string): Promise<T> {
    return this.request('DELETE', path);
  }

  /** Call the JSON-RPC endpoint for lower-level operations */
  private async rpc(method: string, params: any = {}): Promise<any> {
    const body = { jsonrpc: '2.0', id: Date.now(), method, params };
    const res = await this.post<{ result?: any; error?: any }>('/', body);
    if (res.error) throw new Error(`RPC error ${res.error.code}: ${res.error.message}`);
    return res.result;
  }

  // =========================================================================
  // PAIRS
  // =========================================================================

  /** Get all trading pairs */
  async getPairs(): Promise<TradingPair[]> {
    return this.get('/api/v1/pairs');
  }

  /** Get a specific trading pair */
  async getPair(pairId: number): Promise<TradingPair> {
    return this.get(`/api/v1/pairs/${pairId}`);
  }

  /** Get ticker data for a pair */
  async getTicker(pairId: number): Promise<Ticker> {
    return this.get(`/api/v1/pairs/${pairId}/ticker`);
  }

  /** Get all tickers */
  async getTickers(): Promise<Ticker[]> {
    return this.get('/api/v1/tickers');
  }

  // =========================================================================
  // ORDER BOOK
  // =========================================================================

  /** Get L2 order book (aggregated price levels) */
  async getOrderBook(pairId: number, depth: number = 20): Promise<OrderBook> {
    return this.get(`/api/v1/pairs/${pairId}/orderbook?depth=${depth}`);
  }

  /** Get recent trades for a pair */
  async getTrades(pairId: number, limit: number = 50): Promise<Trade[]> {
    return this.get(`/api/v1/pairs/${pairId}/trades?limit=${limit}`);
  }

  // =========================================================================
  // ORDERS
  // =========================================================================

  /** Place a limit order */
  async placeLimitOrder(params: PlaceOrderParams): Promise<ApiResponse<Order>> {
    return this.post('/api/v1/orders', {
      ...params,
      orderType: params.orderType || 'limit',
    });
  }

  /** Place a market order */
  async placeMarketOrder(params: Omit<PlaceOrderParams, 'price'>): Promise<ApiResponse<Order>> {
    return this.post('/api/v1/orders', {
      ...params,
      orderType: 'market',
      price: 0,
    });
  }

  /** Cancel an open order */
  async cancelOrder(params: CancelOrderParams): Promise<ApiResponse<{ cancelled: boolean }>> {
    return this.del(`/api/v1/orders/${params.orderId}`);
  }

  /** Cancel all open orders for a pair (or all pairs if pairId omitted) */
  async cancelAllOrders(pairId?: number): Promise<ApiResponse<{ count: number }>> {
    const q = pairId !== undefined ? `?pairId=${pairId}` : '';
    return this.del(`/api/v1/orders${q}`);
  }

  /** Get all orders for the connected wallet */
  async getMyOrders(
    status?: 'open' | 'all',
    pairId?: number,
  ): Promise<Order[]> {
    if (!this.wallet) throw new Error('Wallet required to query own orders');
    const addr = this.getAddress();
    const params = new URLSearchParams({ trader: addr });
    if (status) params.set('status', status);
    if (pairId !== undefined) params.set('pairId', String(pairId));
    return this.get(`/api/v1/orders?${params}`);
  }

  /** Get a specific order by ID */
  async getOrder(orderId: number): Promise<Order> {
    return this.get(`/api/v1/orders/${orderId}`);
  }

  // =========================================================================
  // SMART ROUTING & SWAPS
  // =========================================================================

  /** Execute a smart-routed swap */
  async swap(params: SwapParams): Promise<ApiResponse<SwapResult>> {
    return this.post('/api/v1/router/swap', params);
  }

  /** Get swap quote without executing */
  async getSwapQuote(params: SwapParams): Promise<ApiResponse<SwapResult>> {
    return this.post('/api/v1/router/quote', params);
  }

  /** Get all registered routes */
  async getRoutes(): Promise<Route[]> {
    return this.get('/api/v1/routes');
  }

  // =========================================================================
  // AMM POOLS
  // =========================================================================

  /** Get all AMM pools */
  async getPools(): Promise<Pool[]> {
    return this.get('/api/v1/pools');
  }

  /** Get a specific pool */
  async getPool(poolId: number): Promise<Pool> {
    return this.get(`/api/v1/pools/${poolId}`);
  }

  /** Create a new AMM pool */
  async createPool(params: CreatePoolParams): Promise<ApiResponse<Pool>> {
    return this.post('/api/v1/pools', params);
  }

  /** Add concentrated liquidity to a pool */
  async addLiquidity(params: AddLiquidityParams): Promise<ApiResponse<LPPosition>> {
    return this.post(`/api/v1/pools/${params.poolId}/liquidity`, params);
  }

  /** Remove liquidity (close LP position) */
  async removeLiquidity(params: RemoveLiquidityParams): Promise<ApiResponse<{ removed: boolean }>> {
    return this.del(`/api/v1/pools/positions/${params.positionId}`);
  }

  /** Get LP positions for the connected wallet */
  async getMyPositions(): Promise<LPPosition[]> {
    if (!this.wallet) throw new Error('Wallet required');
    return this.get(`/api/v1/pools/positions?owner=${this.getAddress()}`);
  }

  /** Execute a direct pool swap */
  async poolSwap(
    poolId: number,
    amountIn: number,
    zeroForOne: boolean,
    minOut: number,
  ): Promise<ApiResponse<SwapResult>> {
    return this.post(`/api/v1/pools/${poolId}/swap`, { amountIn, zeroForOne, minOut });
  }

  // =========================================================================
  // MARGIN TRADING
  // =========================================================================

  /** Open a margin position */
  async openPosition(params: OpenPositionParams): Promise<ApiResponse<MarginPosition>> {
    return this.post('/api/v1/margin/open', params);
  }

  /** Close a margin position */
  async closePosition(params: ClosePositionParams): Promise<ApiResponse<MarginPosition>> {
    return this.post('/api/v1/margin/close', params);
  }

  /** Add margin to an existing position */
  async addMargin(positionId: number, amount: number): Promise<ApiResponse<MarginPosition>> {
    return this.post(`/api/v1/margin/positions/${positionId}/add`, { amount });
  }

  /** Get a margin position */
  async getMarginPosition(positionId: number): Promise<MarginPosition> {
    return this.get(`/api/v1/margin/positions/${positionId}`);
  }

  /** Get all margin positions for connected wallet */
  async getMyMarginPositions(): Promise<MarginPosition[]> {
    if (!this.wallet) throw new Error('Wallet required');
    return this.get(`/api/v1/margin/positions?trader=${this.getAddress()}`);
  }

  /** Get margin system info (insurance fund, funding rate, etc.) */
  async getMarginInfo(): Promise<MarginInfo> {
    return this.get('/api/v1/margin/info');
  }

  // =========================================================================
  // ANALYTICS
  // =========================================================================

  /** Get OHLCV candles for a pair */
  async getCandles(
    pairId: number,
    interval: CandleInterval = 3600,
    limit: number = 100,
  ): Promise<Candle[]> {
    return this.get(`/api/v1/pairs/${pairId}/candles?interval=${interval}&limit=${limit}`);
  }

  /** Get 24h rolling stats for a pair */
  async get24hStats(pairId: number): Promise<Stats24h> {
    return this.get(`/api/v1/pairs/${pairId}/stats`);
  }

  /** Get trader leaderboard */
  async getLeaderboard(limit: number = 20): Promise<LeaderboardEntry[]> {
    return this.get(`/api/v1/leaderboard?limit=${limit}`);
  }

  /** Get stats for a specific trader */
  async getTraderStats(address: Address): Promise<LeaderboardEntry> {
    return this.get(`/api/v1/traders/${address}/stats`);
  }

  // =========================================================================
  // REWARDS
  // =========================================================================

  /** Get pending rewards for connected wallet */
  async getMyRewards(): Promise<RewardInfo> {
    if (!this.wallet) throw new Error('Wallet required');
    return this.get(`/api/v1/rewards/${this.getAddress()}`);
  }

  /** Claim pending rewards */
  async claimRewards(): Promise<ApiResponse<{ claimed: bigint }>> {
    return this.post('/api/v1/rewards/claim', {});
  }

  /** Set referrer (one-time) */
  async setReferrer(referrerAddress: Address): Promise<ApiResponse<{ set: boolean }>> {
    return this.post('/api/v1/rewards/referrer', { referrer: referrerAddress });
  }

  // =========================================================================
  // GOVERNANCE
  // =========================================================================

  /** Get all proposals */
  async getProposals(status?: string): Promise<Proposal[]> {
    const q = status ? `?status=${status}` : '';
    return this.get(`/api/v1/governance/proposals${q}`);
  }

  /** Get a specific proposal */
  async getProposal(proposalId: number): Promise<Proposal> {
    return this.get(`/api/v1/governance/proposals/${proposalId}`);
  }

  /** Create a governance proposal */
  async createProposal(params: CreateProposalParams): Promise<ApiResponse<Proposal>> {
    return this.post('/api/v1/governance/proposals', params);
  }

  /** Cast a vote */
  async vote(proposalId: number, support: boolean, amount: number): Promise<ApiResponse<{ voted: boolean }>> {
    return this.post(`/api/v1/governance/proposals/${proposalId}/vote`, { support, amount });
  }

  // =========================================================================
  // WEBSOCKET SUBSCRIPTIONS
  // =========================================================================

  /** Get or create the WebSocket connection */
  private getWs(): DexWebSocket {
    if (!this.ws) {
      this.ws = new DexWebSocket(this.wsEndpoint, this.apiKey);
    }
    return this.ws;
  }

  /** Subscribe to real-time order book updates */
  subscribeTrades(pairId: number, callback: (trade: any) => void): () => void {
    return this.getWs().subscribe(`trades:${pairId}`, callback);
  }

  /** Subscribe to order book updates */
  subscribeOrderBook(pairId: number, callback: (book: any) => void): () => void {
    return this.getWs().subscribe(`orderbook:${pairId}`, callback);
  }

  /** Subscribe to ticker updates */
  subscribeTicker(pairId: number, callback: (ticker: any) => void): () => void {
    return this.getWs().subscribe(`ticker:${pairId}`, callback);
  }

  /** Subscribe to candle updates */
  subscribeCandles(pairId: number, interval: CandleInterval, callback: (candle: any) => void): () => void {
    return this.getWs().subscribe(`candles:${pairId}:${interval}`, callback);
  }

  /** Subscribe to user order updates (requires wallet) */
  subscribeMyOrders(callback: (order: any) => void): () => void {
    if (!this.wallet) throw new Error('Wallet required');
    return this.getWs().subscribe(`orders:${this.getAddress()}`, callback);
  }

  /** Subscribe to user position updates (requires wallet) */
  subscribeMyPositions(callback: (pos: any) => void): () => void {
    if (!this.wallet) throw new Error('Wallet required');
    return this.getWs().subscribe(`positions:${this.getAddress()}`, callback);
  }

  /** Close WebSocket connection */
  disconnect(): void {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  // =========================================================================
  // UTILITY
  // =========================================================================

  /** Get the wallet address as hex string */
  getAddress(): string {
    if (!this.wallet) throw new Error('No wallet configured');
    const pk = typeof this.wallet.pubkey === 'function' ? this.wallet.pubkey() : this.wallet.pubkey;
    // Support both PublicKey objects and raw bytes
    if (pk && typeof pk.toBase58 === 'function') return pk.toBase58();
    if (pk instanceof Uint8Array) return Buffer.from(pk).toString('hex');
    return String(pk);
  }

  /** Convert a human-readable price (e.g. 1.50) to contract scale (1_500_000_000) */
  static priceToScaled(price: number): bigint {
    return BigInt(Math.round(price * PRICE_SCALE));
  }

  /** Convert a contract-scale price back to human-readable */
  static scaledToPrice(scaled: bigint): number {
    return Number(scaled) / PRICE_SCALE;
  }

  /** Convert a fee tier string to the contract u8 value */
  static feeTierToU8(tier: string): number {
    const map: Record<string, number> = { '1bps': 0, '5bps': 1, '30bps': 2, '100bps': 3 };
    return map[tier] ?? 2;
  }
}

// Lichen JavaScript/TypeScript SDK
// Official SDK for interacting with Lichen blockchain

export { PublicKey } from './publickey';
export { Keypair } from './keypair';
export { Connection } from './connection';
export {
  Transaction,
  TransactionBuilder,
  Instruction,
  Message,
} from './transaction';

export type {
  Balance,
  Account,
  Block,
  Validator,
  NetworkInfo,
  ChainStatus,
  Metrics,
} from './connection';

/**
 * SDK version
 */
export const VERSION = '0.1.0';

/**
 * Default RPC URL (override with LICHEN_RPC_URL env var)
 */
export const DEFAULT_RPC_URL = (typeof process !== 'undefined' && process.env?.LICHEN_RPC_URL) || 'http://localhost:8899';

/**
 * Default WebSocket URL (override with LICHEN_WS_URL env var)
 */
export const DEFAULT_WS_URL = (typeof process !== 'undefined' && process.env?.LICHEN_WS_URL) || 'ws://localhost:8900';

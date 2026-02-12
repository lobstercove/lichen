// MoltChain JavaScript/TypeScript SDK
// Official SDK for interacting with MoltChain blockchain

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
 * Default RPC URL
 */
export const DEFAULT_RPC_URL = 'http://localhost:8899';

/**
 * Default WebSocket URL
 */
export const DEFAULT_WS_URL = 'ws://localhost:8900';

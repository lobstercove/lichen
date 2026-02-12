// MoltChain SDK - Connection Class

import WebSocket from 'ws';
import { PublicKey } from './publickey';
import { Keypair } from './keypair';
import { Transaction, TransactionBuilder } from './transaction';
import { encodeTransaction } from './bincode';

/**
 * Balance information
 */
export interface Balance {
  shells: number;
  molt: number;
}

/**
 * Account information
 */
export interface Account {
  shells: number;
  owner: string;
  executable: boolean;
  data: string;
}

/**
 * Block information
 */
export interface Block {
  slot: number;
  hash: string;
  parentHash: string;
  transactions: number;
  timestamp: number;
}

/**
 * Validator information
 */
export interface Validator {
  pubkey: string;
  stake: number;
  reputation: number;
  blocksProposed: number;
  votesCast: number;
  correctVotes: number;
  lastActiveSlot: number;
}

/**
 * Network information
 */
export interface NetworkInfo {
  chainId: string;
  networkId: string;
  version: string;
  currentSlot: number;
  validatorCount: number;
  peerCount: number;
}

/**
 * Chain status
 */
export interface ChainStatus {
  currentSlot: number;
  validatorCount: number;
  totalStake: number;
  tps: number;
  totalTransactions: number;
  totalBlocks: number;
  averageBlockTime: number;
  isHealthy: boolean;
}

/**
 * Performance metrics
 */
export interface Metrics {
  tps: number;
  totalTransactions: number;
  totalBlocks: number;
  averageBlockTime: number;
}

/**
 * RPC/WebSocket connection to MoltChain
 */
export class Connection {
  private rpcUrl: string;
  private wsUrl?: string;
  private ws?: WebSocket;
  private subscriptions = new Map<number, (data: any) => void>();
  private nextId = 1;

  constructor(rpcUrl: string, wsUrl?: string) {
    this.rpcUrl = rpcUrl;
    this.wsUrl = wsUrl;
  }

  /**
   * Make an RPC call
   */
  private async rpc(method: string, params: any[] = []): Promise<any> {
    const response = await fetch(this.rpcUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: this.nextId++,
        method,
        params,
      }),
    });

    const data: any = await response.json();
    
    if (data.error) {
      throw new Error(`RPC Error: ${data.error.message}`);
    }
    
    return data.result;
  }

  // ============================================================================
  // BASIC QUERIES
  // ============================================================================

  /**
   * Get account balance
   */
  async getBalance(pubkey: PublicKey): Promise<Balance> {
    return this.rpc('getBalance', [pubkey.toBase58()]);
  }

  /**
   * Get account information
   */
  async getAccount(pubkey: PublicKey): Promise<Account> {
    return this.rpc('getAccount', [pubkey.toBase58()]);
  }

  /**
   * Get block by slot number
   */
  async getBlock(slot: number): Promise<Block> {
    return this.rpc('getBlock', [slot]);
  }

  /**
   * Get latest block
   */
  async getLatestBlock(): Promise<Block> {
    return this.rpc('getLatestBlock');
  }

  /**
   * Get current slot
   */
  async getSlot(): Promise<number> {
    const result = await this.rpc('getSlot');
    return typeof result === 'number' ? result : result.slot;
  }

  /**
   * Get recent blockhash for transactions
   */
  async getRecentBlockhash(): Promise<string> {
    const result = await this.rpc('getRecentBlockhash');
    return typeof result === 'string' ? result : result.blockhash;
  }

  /**
   * Get transaction by signature
   */
  async getTransaction(signature: string): Promise<any> {
    return this.rpc('getTransaction', [signature]);
  }

  /**
   * Send transaction
   */
  async sendTransaction(transaction: Transaction): Promise<string> {
    const txBytes = encodeTransaction(transaction);
    const txBase64 = Buffer.from(txBytes).toString('base64');
    const result = await this.rpc('sendTransaction', [txBase64]);
    return typeof result === 'string' ? result : result.signature;
  }

  /**
   * Get total burned MOLT
   */
  async getTotalBurned(): Promise<Balance> {
    return this.rpc('getTotalBurned');
  }

  /**
   * Get all validators
   */
  async getValidators(): Promise<Validator[]> {
    const result = await this.rpc('getValidators');
    return result.validators;
  }

  /**
   * Get performance metrics
   */
  async getMetrics(): Promise<Metrics> {
    return this.rpc('getMetrics');
  }

  /**
   * Health check
   */
  async health(): Promise<{ status: string }> {
    return this.rpc('health');
  }

  // ============================================================================
  // NETWORK ENDPOINTS
  // ============================================================================

  /**
   * Get connected peers
   */
  async getPeers(): Promise<any[]> {
    const result = await this.rpc('getPeers');
    return result.peers;
  }

  /**
   * Get network information
   */
  async getNetworkInfo(): Promise<NetworkInfo> {
    return this.rpc('getNetworkInfo');
  }

  // ============================================================================
  // VALIDATOR ENDPOINTS
  // ============================================================================

  /**
   * Get detailed validator information
   */
  async getValidatorInfo(pubkey: PublicKey): Promise<Validator> {
    return this.rpc('getValidatorInfo', [pubkey.toBase58()]);
  }

  /**
   * Get validator performance metrics
   */
  async getValidatorPerformance(pubkey: PublicKey): Promise<any> {
    return this.rpc('getValidatorPerformance', [pubkey.toBase58()]);
  }

  /**
   * Get comprehensive chain status
   */
  async getChainStatus(): Promise<ChainStatus> {
    return this.rpc('getChainStatus');
  }

  // ============================================================================
  // STAKING ENDPOINTS
  // ============================================================================

  /**
   * Create stake transaction
   */
  async stake(from: Keypair, validator: PublicKey, amount: number): Promise<string> {
    const blockhash = await this.getRecentBlockhash();
    const instruction = TransactionBuilder.stake(from.pubkey(), validator, amount);
    const transaction = new TransactionBuilder()
      .add(instruction)
      .setRecentBlockhash(blockhash)
      .buildAndSign(from);
    return this.sendTransaction(transaction);
  }

  /**
   * Create unstake transaction
   */
  async unstake(from: Keypair, validator: PublicKey, amount: number): Promise<string> {
    const blockhash = await this.getRecentBlockhash();
    const instruction = TransactionBuilder.unstake(from.pubkey(), validator, amount);
    const transaction = new TransactionBuilder()
      .add(instruction)
      .setRecentBlockhash(blockhash)
      .buildAndSign(from);
    return this.sendTransaction(transaction);
  }

  /**
   * Get staking status
   */
  async getStakingStatus(pubkey: PublicKey): Promise<any> {
    return this.rpc('getStakingStatus', [pubkey.toBase58()]);
  }

  /**
   * Get staking rewards
   */
  async getStakingRewards(pubkey: PublicKey): Promise<any> {
    return this.rpc('getStakingRewards', [pubkey.toBase58()]);
  }

  // ============================================================================
  // ACCOUNT ENDPOINTS
  // ============================================================================

  /**
   * Get enhanced account information
   */
  async getAccountInfo(pubkey: PublicKey): Promise<any> {
    return this.rpc('getAccountInfo', [pubkey.toBase58()]);
  }

  /**
   * Get transaction history
   */
  async getTransactionHistory(pubkey: PublicKey, limit: number = 10): Promise<any> {
    return this.rpc('getTransactionHistory', [pubkey.toBase58(), limit]);
  }

  /**
   * Get program accounts
   */
  async getProgramAccounts(programId: PublicKey): Promise<any[]> {
    const result = await this.rpc('getProgramAccounts', [programId.toBase58()]);
    return result.accounts || [];
  }

  /**
   * Simulate transaction (dry run)
   */
  async simulateTransaction(transaction: Transaction): Promise<any> {
    const txBytes = encodeTransaction(transaction);
    const txBase64 = Buffer.from(txBytes).toString('base64');
    return this.rpc('simulateTransaction', [txBase64]);
  }

  // ============================================================================
  // CONTRACT ENDPOINTS
  // ============================================================================

  /**
   * Get contract information
   */
  async getContractInfo(contractId: PublicKey): Promise<any> {
    return this.rpc('getContractInfo', [contractId.toBase58()]);
  }

  /**
   * Get contract logs
   */
  async getContractLogs(contractId: PublicKey): Promise<any> {
    return this.rpc('getContractLogs', [contractId.toBase58()]);
  }

  /**
   * Get contract ABI/IDL (machine-readable function and event interface)
   */
  async getContractAbi(contractId: PublicKey): Promise<any> {
    return this.rpc('getContractAbi', [contractId.toBase58()]);
  }

  /**
   * Set/update contract ABI (owner only)
   */
  async setContractAbi(contractId: PublicKey, abi: any): Promise<any> {
    return this.rpc('setContractAbi', [contractId.toBase58(), abi]);
  }

  /**
   * Get all deployed contracts
   */
  async getAllContracts(): Promise<any> {
    return this.rpc('getAllContracts');
  }

  // ==========================================================================
  // PROGRAM ENDPOINTS (DRAFT)
  // ==========================================================================

  async getProgram(programId: PublicKey): Promise<any> {
    return this.rpc('getProgram', [programId.toBase58()]);
  }

  async getProgramStats(programId: PublicKey): Promise<any> {
    return this.rpc('getProgramStats', [programId.toBase58()]);
  }

  async getPrograms(): Promise<any> {
    return this.rpc('getPrograms');
  }

  async getProgramCalls(programId: PublicKey): Promise<any> {
    return this.rpc('getProgramCalls', [programId.toBase58()]);
  }

  async getProgramStorage(programId: PublicKey): Promise<any> {
    return this.rpc('getProgramStorage', [programId.toBase58()]);
  }

  // ==========================================================================
  // NFT ENDPOINTS (DRAFT)
  // ==========================================================================

  async getCollection(collectionId: PublicKey): Promise<any> {
    return this.rpc('getCollection', [collectionId.toBase58()]);
  }

  async getNFT(collectionId: PublicKey, tokenId: number): Promise<any> {
    return this.rpc('getNFT', [collectionId.toBase58(), tokenId]);
  }

  async getNFTsByOwner(owner: PublicKey): Promise<any> {
    return this.rpc('getNFTsByOwner', [owner.toBase58()]);
  }

  async getNFTsByCollection(collectionId: PublicKey): Promise<any> {
    return this.rpc('getNFTsByCollection', [collectionId.toBase58()]);
  }

  async getNFTActivity(collectionId: PublicKey, tokenId: number): Promise<any> {
    return this.rpc('getNFTActivity', [collectionId.toBase58(), tokenId]);
  }

  // ============================================================================
  // WEBSOCKET SUBSCRIPTIONS
  // ============================================================================

  /**
   * Connect WebSocket
   */
  private async connectWs(): Promise<void> {
    if (!this.wsUrl) {
      throw new Error('WebSocket URL not provided');
    }
    
    if (this.ws?.readyState === WebSocket.OPEN) {
      return;
    }

    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(this.wsUrl!);
      
      this.ws.on('open', () => {
        resolve();
      });
      
      this.ws.on('message', (data: WebSocket.Data) => {
        const msg = JSON.parse(data.toString());
        
        if (msg.method === 'subscription') {
          const { subscription, result } = msg.params;
          const handler = this.subscriptions.get(subscription);
          if (handler) {
            handler(result);
          }
        }
      });

      this.ws.on('error', (error) => {
        console.error('WebSocket error:', error);
        reject(error);
      });
    });
  }

  /**
   * Subscribe to method
   */
  private async subscribe(method: string, params: any = null): Promise<number> {
    await this.connectWs();
    
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      const timeout = setTimeout(() => reject(new Error('Subscription timeout')), 5000);
      
      const messageHandler = (data: WebSocket.Data) => {
        const msg = JSON.parse(data.toString());
        if (msg.id === id) {
          clearTimeout(timeout);
          this.ws!.off('message', messageHandler);
          if (msg.error) {
            reject(new Error(msg.error.message));
          } else {
            resolve(msg.result);
          }
        }
      };
      
      this.ws!.on('message', messageHandler);
      this.ws!.send(JSON.stringify({
        jsonrpc: '2.0',
        id,
        method,
        params,
      }));
    });
  }

  /**
   * Unsubscribe from subscription
   */
  private async unsubscribe(method: string, subscriptionId: number): Promise<boolean> {
    const id = this.nextId++;
    
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('Unsubscribe timeout')), 5000);
      
      const messageHandler = (data: WebSocket.Data) => {
        const msg = JSON.parse(data.toString());
        if (msg.id === id) {
          clearTimeout(timeout);
          this.ws!.off('message', messageHandler);
          this.subscriptions.delete(subscriptionId);
          if (msg.error) {
            reject(new Error(msg.error.message));
          } else {
            resolve(msg.result as boolean);
          }
        }
      };
      
      this.ws!.on('message', messageHandler);
      this.ws!.send(JSON.stringify({
        jsonrpc: '2.0',
        id,
        method,
        params: subscriptionId,
      }));
    });
  }

  /**
   * Subscribe to slot updates
   */
  async onSlot(callback: (slot: number) => void): Promise<number> {
    const subId = await this.subscribe('subscribeSlots');
    this.subscriptions.set(subId, (data) => callback(data.slot));
    return subId;
  }

  /**
   * Unsubscribe from slots
   */
  async offSlot(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeSlots', subscriptionId);
  }

  /**
   * Subscribe to block updates
   */
  async onBlock(callback: (block: Block) => void): Promise<number> {
    const subId = await this.subscribe('subscribeBlocks');
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from blocks
   */
  async offBlock(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeBlocks', subscriptionId);
  }

  /**
   * Subscribe to transaction updates
   */
  async onTransaction(callback: (transaction: any) => void): Promise<number> {
    const subId = await this.subscribe('subscribeTransactions');
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from transactions
   */
  async offTransaction(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeTransactions', subscriptionId);
  }

  /**
   * Subscribe to account changes
   */
  async onAccountChange(pubkey: PublicKey, callback: (account: any) => void): Promise<number> {
    const subId = await this.subscribe('subscribeAccount', pubkey.toBase58());
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from account changes
   */
  async offAccountChange(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeAccount', subscriptionId);
  }

  /**
   * Subscribe to contract logs
   */
  async onLogs(callback: (log: any) => void, contractId?: PublicKey): Promise<number> {
    const params = contractId ? contractId.toBase58() : null;
    const subId = await this.subscribe('subscribeLogs', params);
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from logs
   */
  async offLogs(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeLogs', subscriptionId);
  }

  /**
   * Subscribe to program updates
   */
  async onProgramUpdates(callback: (event: any) => void): Promise<number> {
    const subId = await this.subscribe('subscribeProgramUpdates');
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from program updates
   */
  async offProgramUpdates(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeProgramUpdates', subscriptionId);
  }

  /**
   * Subscribe to program calls
   */
  async onProgramCalls(callback: (event: any) => void, programId?: PublicKey): Promise<number> {
    const params = programId ? programId.toBase58() : null;
    const subId = await this.subscribe('subscribeProgramCalls', params);
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from program calls
   */
  async offProgramCalls(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeProgramCalls', subscriptionId);
  }

  /**
   * Subscribe to NFT mints
   */
  async onNftMints(callback: (event: any) => void, collectionId?: PublicKey): Promise<number> {
    const params = collectionId ? collectionId.toBase58() : null;
    const subId = await this.subscribe('subscribeNftMints', params);
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from NFT mints
   */
  async offNftMints(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeNftMints', subscriptionId);
  }

  /**
   * Subscribe to NFT transfers
   */
  async onNftTransfers(callback: (event: any) => void, collectionId?: PublicKey): Promise<number> {
    const params = collectionId ? collectionId.toBase58() : null;
    const subId = await this.subscribe('subscribeNftTransfers', params);
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from NFT transfers
   */
  async offNftTransfers(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeNftTransfers', subscriptionId);
  }

  /**
   * Subscribe to marketplace listings
   */
  async onMarketListings(callback: (event: any) => void): Promise<number> {
    const subId = await this.subscribe('subscribeMarketListings');
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from marketplace listings
   */
  async offMarketListings(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeMarketListings', subscriptionId);
  }

  /**
   * Subscribe to marketplace sales
   */
  async onMarketSales(callback: (event: any) => void): Promise<number> {
    const subId = await this.subscribe('subscribeMarketSales');
    this.subscriptions.set(subId, callback);
    return subId;
  }

  /**
   * Unsubscribe from marketplace sales
   */
  async offMarketSales(subscriptionId: number): Promise<boolean> {
    return this.unsubscribe('unsubscribeMarketSales', subscriptionId);
  }

  /**
   * Close connection
   */
  close(): void {
    if (this.ws) {
      this.ws.close();
      this.ws = undefined;
    }
    this.subscriptions.clear();
  }
}

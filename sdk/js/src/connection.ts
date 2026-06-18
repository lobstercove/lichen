// Lichen SDK - Connection Class

import WebSocket from 'ws';
import { createHash } from 'crypto';
import { PublicKey } from './publickey.js';
import { Keypair } from './keypair.js';
import { Transaction, TransactionBuilder } from './transaction.js';
import { encodeTransaction, hexToBytes, bytesToHex } from './bincode.js';

/** SHA-256 hash as Uint8Array */
function sha256(data: Uint8Array): Uint8Array {
  return new Uint8Array(createHash('sha256').update(data).digest());
}

function concatBytes(parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function u64Le(value: number | bigint): Uint8Array {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, BigInt(value), true);
  return out;
}

/**
 * Balance information
 */
export interface Balance {
  spores: number;
  licn: string;
  spendable?: number;
  spendable_licn?: string;
  staked?: number;
  staked_licn?: string;
  locked?: number;
  locked_licn?: string;
  moss_staked?: number;
  moss_staked_licn?: string;
  moss_value?: number;
  moss_value_licn?: string;
}

/**
 * Account information
 */
export interface Account {
  spores: number;
  licn: string;
  spendable?: number;
  spendable_licn?: string;
  staked?: number;
  staked_licn?: string;
  locked?: number;
  locked_licn?: string;
  owner: string;
  executable: boolean;
  data_len: number;
  pubkey?: string;
  evm_address?: string;
}

/**
 * Block information
 */
export interface Block {
  slot: number;
  hash: string;
  commit_round?: number;
  parent_hash: string;
  state_root: string;
  tx_root?: string;
  timestamp: number;
  validator: string;
  transaction_count: number;
  transactions?: any[];
  block_reward?: number;
  commit_signatures?: string[];
  commit_validator_count?: number;
}

/**
 * Validator information
 */
export interface Validator {
  pubkey: string;
  stake: number;
  reputation: number;
  blocks_proposed: number;
  transactions_processed?: number;
  votes_cast: number;
  correct_votes: number;
  last_active_slot: number;
  last_vote_slot?: number;
  bootstrap_debt?: number;
  vesting_status?: string;
  earned_amount?: number;
  graduation_slot?: number | null;
}

/**
 * Network information
 */
export interface NetworkInfo {
  chain_id: string;
  network_id: string;
  version: string;
  current_slot: number;
  validator_count: number;
  peer_count: number;
}

/**
 * Chain status
 */
export interface ChainStatus {
  slot?: number;
  epoch?: number;
  block_height?: number;
  current_slot: number;
  latest_block?: number;
  validator_count: number;
  validators?: number;
  total_stake?: number;
  total_staked: number;
  tps: number;
  peak_tps?: number;
  total_transactions: number;
  total_blocks: number;
  average_block_time?: number;
  block_time_ms: number;
  total_supply: number;
  projected_supply?: number;
  total_burned: number;
  total_minted?: number;
  peer_count: number;
  chain_id: string;
  network: string;
  is_healthy?: boolean;
  inflation_rate_bps?: number;
}

/**
 * Performance metrics
 */
export interface Metrics {
  tps: number;
  peak_tps: number;
  total_transactions: number;
  daily_transactions: number;
  total_blocks: number;
  average_block_time: number;
  avg_block_time_ms: number;
  avg_txs_per_block: number;
  total_accounts: number;
  active_accounts: number;
  total_supply: number;
  projected_supply: number;
  circulating_supply: number;
  total_burned: number;
  total_minted: number;
  total_staked: number;
  treasury_balance: number;
  total_contracts: number;
  validator_count: number;
  slot_duration_ms: number;
  fee_burn_percent: number;
  current_epoch: number;
  slots_into_epoch: number;
  inflation_rate_bps: number;
}

/**
 * Total burned LICN.
 */
export interface BurnedInfo {
  spores: number;
  licn: number;
}

/**
 * Address transaction history.
 */
export interface TransactionHistoryResponse {
  transactions: any[];
  has_more: boolean;
  next_before_slot?: number | null;
}

/**
 * A single step in a Merkle inclusion proof
 */
export interface ProofStep {
  hash: string;
  direction: 'left' | 'right';
}

/**
 * Merkle inclusion proof for a transaction
 */
export interface TransactionProof {
  slot: number;
  tx_index: number;
  tx_hash: string;
  root: string;
  proof: ProofStep[];
}

/**
 * Read-only contract call result.
 */
export interface ReadonlyContractResult {
  success: boolean;
  returnData?: string | null;
  returnCode?: number | null;
  logs?: string[];
  error?: string | null;
  computeUsed?: number;
}

/**
 * Contract list entry returned by getAllContracts.
 */
export interface ContractSummary {
  program_id: string;
  symbol?: string | null;
  name?: string | null;
  owner?: string | null;
  registry_owner?: string | null;
  template?: string | null;
  metadata?: Record<string, any> | null;
  code_size: number;
  is_executable: boolean;
  has_abi: boolean;
  abi_functions: number;
  code_hash: string;
  version: number;
  lifecycle_status: string;
  lifecycle_updated_slot: number;
  lifecycle_restriction_id?: number | null;
  lifecycle_effective_at_slot: number;
  previous_code_hash?: string;
}

/**
 * Current getAllContracts RPC envelope.
 */
export interface ContractListResponse {
  contracts: ContractSummary[];
  count: number;
  has_more: boolean;
  next_cursor?: string | null;
}

export interface ContractListOptions {
  limit?: number;
  cursor?: string | null;
}

export interface DeployContractResult {
  signature: string;
  contractAddress: PublicKey;
  contractAddressBase58: string;
}

/**
 * RPC/WebSocket connection to Lichen
 */
export class Connection {
  private rpcUrl: string;
  private wsUrl?: string;
  private ws?: WebSocket;
  private subscriptions = new Map<number, (data: any) => void>();
  private nextId = 1;
  private timeoutMs: number;

  constructor(rpcUrl: string, wsUrl?: string, options?: { timeoutMs?: number }) {
    this.rpcUrl = rpcUrl;
    this.wsUrl = wsUrl;
    this.timeoutMs = options?.timeoutMs ?? 30_000;
  }

  /**
   * Make an RPC call
   */
  private async rpc(method: string, params: any[] = []): Promise<any> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);

    let response: Response;
    try {
      response = await fetch(this.rpcUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          id: this.nextId++,
          method,
          params,
        }),
        signal: controller.signal,
      });
    } catch (err: any) {
      clearTimeout(timer);
      if (err.name === 'AbortError') {
        throw new Error(`RPC request timed out after ${this.timeoutMs}ms: ${method}`);
      }
      throw err;
    } finally {
      clearTimeout(timer);
    }

    if (!response.ok) {
      const text = await response.text().catch(() => '');
      throw new Error(`RPC HTTP ${response.status}: ${text}`);
    }

    const data: any = await response.json();

    if (data.error) {
      throw new Error(`RPC Error: ${data.error.message}`);
    }

    return data.result;
  }

  /**
   * Make a raw JSON-RPC request.
   *
   * SDK feature clients use this to expose typed wrappers without duplicating
   * timeout, HTTP, and JSON-RPC error handling.
   */
  async rpcRequest<T = any>(method: string, params: any[] = []): Promise<T> {
    return this.rpc(method, params) as Promise<T>;
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
   * Get transaction by signature.
   * For contract calls, response includes: return_code (u32), return_data (base64), contract_logs (string[]).
   */
  async getTransaction(signature: string): Promise<any> {
    return this.rpc('getTransaction', [signature]);
  }

  /**
   * Get a Merkle inclusion proof for a transaction by its signature.
   */
  async getTransactionProof(signature: string): Promise<TransactionProof> {
    return this.rpc('getTransactionProof', [signature]);
  }

  /**
   * Verify a Merkle inclusion proof for a transaction against a root.
   * Uses SHA-256 with domain-separated leaf (0x00) and internal (0x01) nodes.
   *
   * This is a pure static function — no RPC call needed.
   */
  static verifyTransactionProof(root: string, txHash: string, proof: ProofStep[]): boolean {
    // Domain-separated leaf: SHA256(0x00 || tx_hash_bytes)
    const leafData = new Uint8Array(33);
    leafData[0] = 0x00;
    leafData.set(hexToBytes(txHash), 1);
    let current = sha256(leafData);

    for (const step of proof) {
      const sibling = hexToBytes(step.hash);
      const nodeData = new Uint8Array(65);
      nodeData[0] = 0x01; // internal node domain tag
      if (step.direction === 'left') {
        nodeData.set(sibling, 1);
        nodeData.set(current, 33);
      } else {
        nodeData.set(current, 1);
        nodeData.set(sibling, 33);
      }
      current = sha256(nodeData);
    }

    return bytesToHex(current) === root;
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
   * Get total burned LICN
   */
  async getTotalBurned(): Promise<BurnedInfo> {
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

  private async getSigningChainId(): Promise<string> {
    const info = (await this.getNetworkInfo()) as any;
    return info.chain_id ?? info.chainId ?? '';
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
  async stake(from: Keypair, validator: PublicKey, amount: number | bigint): Promise<string> {
    const blockhash = await this.getRecentBlockhash();
    const chainId = await this.getSigningChainId();
    const instruction = TransactionBuilder.stake(from.pubkey(), validator, amount);
    const transaction = new TransactionBuilder()
      .add(instruction)
      .setRecentBlockhash(blockhash)
      .buildAndSignForChainId(from, chainId);
    return this.sendTransaction(transaction);
  }

  /**
   * Create unstake transaction
   */
  async unstake(from: Keypair, validator: PublicKey, amount: number | bigint): Promise<string> {
    const blockhash = await this.getRecentBlockhash();
    const chainId = await this.getSigningChainId();
    const instruction = TransactionBuilder.unstake(from.pubkey(), validator, amount);
    const transaction = new TransactionBuilder()
      .add(instruction)
      .setRecentBlockhash(blockhash)
      .buildAndSignForChainId(from, chainId);
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
  // TRANSFER & CONTRACT ENDPOINTS
  // ============================================================================

  /**
   * Transfer native LICN (spores) from one account to another.
   *
   * @param from - Sender keypair (signer)
   * @param to - Recipient public key
   * @param amount - Amount in spores (1 LICN = 1,000,000,000 spores)
   * @returns Transaction signature
   */
  async transfer(from: Keypair, to: PublicKey, amount: number | bigint): Promise<string> {
    const blockhash = await this.getRecentBlockhash();
    const chainId = await this.getSigningChainId();
    const instruction = TransactionBuilder.transfer(from.pubkey(), to, amount);
    const transaction = new TransactionBuilder()
      .add(instruction)
      .setRecentBlockhash(blockhash)
      .buildAndSignForChainId(from, chainId);
    return this.sendTransaction(transaction);
  }

  /**
   * Derive the SDK/CLI deploy address convention for a contract transaction.
   */
  static deriveContractAddress(deployer: PublicKey, code: Uint8Array, slot: number | bigint): PublicKey {
    const codeHash = sha256(code);
    const digest = sha256(concatBytes([deployer.toBytes(), codeHash, u64Le(slot)]));
    return PublicKey.fromBytes(digest);
  }

  /**
   * Deploy a WASM smart contract.
   *
   * @param deployer - Deployer keypair (signer, pays deploy fee)
   * @param code - WASM bytecode (must start with \\0asm magic, max 512 KB)
   * @param initData - Optional initialization data passed to contract init
   * @param contractAddress - Optional explicit contract account address
   * @returns Transaction signature and deployed contract address
   */
  async deployContract(
    deployer: Keypair,
    code: Uint8Array,
    initData: Uint8Array = new Uint8Array(0),
    contractAddress?: PublicKey,
  ): Promise<DeployContractResult> {
    const address = contractAddress ?? Connection.deriveContractAddress(deployer.pubkey(), code, await this.getSlot());
    const blockhash = await this.getRecentBlockhash();
    const chainId = await this.getSigningChainId();
    const instruction = TransactionBuilder.deployContract(deployer.pubkey(), address, code, initData);
    const transaction = new TransactionBuilder()
      .add(instruction)
      .setRecentBlockhash(blockhash)
      .buildAndSignForChainId(deployer, chainId);
    const signature = await this.sendTransaction(transaction);
    return {
      signature,
      contractAddress: address,
      contractAddressBase58: address.toBase58(),
    };
  }

  /**
   * Call a function on a deployed WASM smart contract.
   *
   * @param caller - Caller keypair (signer)
   * @param contract - Contract account public key
   * @param functionName - Name of the contract function to invoke
   * @param args - Serialized function arguments (default: empty)
   * @param value - Native LICN to send with the call in spores (default: 0)
   * @returns Transaction signature
   */
  async callContract(
    caller: Keypair,
    contract: PublicKey,
    functionName: string,
    args: Uint8Array = new Uint8Array(0),
    value: number | bigint = 0,
  ): Promise<string> {
    const blockhash = await this.getRecentBlockhash();
    const chainId = await this.getSigningChainId();
    const instruction = TransactionBuilder.callContract(caller.pubkey(), contract, functionName, args, value);
    const transaction = new TransactionBuilder()
      .add(instruction)
      .setRecentBlockhash(blockhash)
      .buildAndSignForChainId(caller, chainId);
    return this.sendTransaction(transaction);
  }

  /**
   * Upgrade a deployed WASM smart contract (owner only).
   *
   * @param owner - Contract owner keypair (signer)
   * @param contract - Contract account public key
   * @param code - New WASM bytecode
   * @returns Transaction signature
   */
  async upgradeContract(
    owner: Keypair,
    contract: PublicKey,
    code: Uint8Array,
  ): Promise<string> {
    const blockhash = await this.getRecentBlockhash();
    const chainId = await this.getSigningChainId();
    const instruction = TransactionBuilder.upgradeContract(owner.pubkey(), contract, code);
    const transaction = new TransactionBuilder()
      .add(instruction)
      .setRecentBlockhash(blockhash)
      .buildAndSignForChainId(owner, chainId);
    return this.sendTransaction(transaction);
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
  async getTransactionHistory(
    pubkey: PublicKey,
    limit: number = 10,
    beforeSlot?: number,
  ): Promise<TransactionHistoryResponse> {
    const options: { limit: number; before_slot?: number } = { limit };
    if (beforeSlot !== undefined) {
      options.before_slot = beforeSlot;
    }
    return this.rpc('getTransactionHistory', [pubkey.toBase58(), options]);
  }

  /**
   * Simulate transaction (dry run)
   */
  async simulateTransaction(transaction: Transaction): Promise<any> {
    const txBytes = encodeTransaction(transaction);
    const txBase64 = Buffer.from(txBytes).toString('base64');
    return this.rpc('simulateTransaction', [txBase64]);
  }

  /**
   * Execute a read-only contract call without submitting a transaction.
   */
  async callReadonlyContract(
    contractId: PublicKey,
    functionName: string,
    args: Uint8Array = new Uint8Array(),
    from?: PublicKey,
  ): Promise<ReadonlyContractResult> {
    const params: string[] = [contractId.toBase58(), functionName, Buffer.from(args).toString('base64')];
    if (from) {
      params.push(from.toBase58());
    }
    return this.rpc('callContract', params);
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
  async getAllContracts(options: ContractListOptions = {}): Promise<ContractListResponse> {
    const params = Object.keys(options).length > 0 ? [options] : [];
    return this.rpc('getAllContracts', params);
  }

  /**
   * Fetch every getAllContracts page. Use this for discovery/catalog tasks.
   */
  async getAllContractsAll(limit = 1000): Promise<ContractSummary[]> {
    const contracts: ContractSummary[] = [];
    let cursor: string | null | undefined = null;

    do {
      const page = await this.getAllContracts({ limit, ...(cursor ? { cursor } : {}) });
      contracts.push(...(page.contracts || []));
      cursor = page.has_more ? page.next_cursor : null;
    } while (cursor);

    return contracts;
  }

  /**
   * Get a symbol-registry entry.
   */
  async getSymbolRegistry(symbol: string): Promise<any> {
    return this.rpc('getSymbolRegistry', [symbol]);
  }

  /**
   * Get the complete LichenID profile for an address.
   */
  async getLichenIdProfile(pubkey: PublicKey): Promise<any> {
    return this.rpc('getLichenIdProfile', [pubkey.toBase58()]);
  }

  /**
   * Get the LichenID reputation summary for an address.
   */
  async getLichenIdReputation(pubkey: PublicKey): Promise<any> {
    return this.rpc('getLichenIdReputation', [pubkey.toBase58()]);
  }

  /**
   * Get LichenID skills for an address.
   */
  async getLichenIdSkills(pubkey: PublicKey): Promise<any> {
    return this.rpc('getLichenIdSkills', [pubkey.toBase58()]);
  }

  /**
   * Get LichenID vouches for an address.
   */
  async getLichenIdVouches(pubkey: PublicKey): Promise<any> {
    return this.rpc('getLichenIdVouches', [pubkey.toBase58()]);
  }

  /**
   * Resolve a .lichen name to its owner.
   */
  async resolveLichenName(name: string): Promise<any> {
    return this.rpc('resolveLichenName', [name]);
  }

  /**
   * Get premium-name auction state for a .lichen label.
   */
  async getNameAuction(name: string): Promise<any> {
    return this.rpc('getNameAuction', [name]);
  }

  /**
   * Get the LichenID agent directory.
   */
  async getLichenIdAgentDirectory(options: {
    type?: number;
    available?: boolean;
    min_reputation?: number;
    limit?: number;
    offset?: number;
  } = {}): Promise<any> {
    return this.rpc('getLichenIdAgentDirectory', [options]);
  }

  /**
   * Get aggregated LichenID statistics.
   */
  async getLichenIdStats(): Promise<any> {
    return this.rpc('getLichenIdStats');
  }

  /**
   * Get aggregated SporePay streaming statistics.
   */
  async getSporePayStats(): Promise<any> {
    return this.rpc('getSporePayStats');
  }

  /**
   * Get aggregated LichenSwap AMM statistics.
   */
  async getLichenSwapStats(): Promise<any> {
    return this.rpc('getLichenSwapStats');
  }

  /**
   * Get aggregated ThallLend lending statistics.
   */
  async getThallLendStats(): Promise<any> {
    return this.rpc('getThallLendStats');
  }

  /**
   * Get aggregated SporeVault yield-vault statistics.
   */
  async getSporeVaultStats(): Promise<any> {
    return this.rpc('getSporeVaultStats');
  }

  /**
   * Get aggregated Neo GAS rewards vault statistics.
   */
  async getNeoGasRewardsStats(): Promise<any> {
    return this.rpc('getNeoGasRewardsStats');
  }

  /**
   * Get per-wallet Neo GAS rewards vault accounting.
   */
  async getNeoGasRewardsPosition(address: PublicKey | string): Promise<any> {
    const value = typeof address === 'string' ? address : address.toBase58();
    return this.rpc('getNeoGasRewardsPosition', [value]);
  }

  /**
   * Get Neo reserve/liability proof-service verifier metadata.
   */
  async getNeoZkProofServiceStatus(): Promise<any> {
    return this.rpc('getNeoZkProofServiceStatus');
  }

  /**
   * Verify a CLI-produced Neo reserve/liability proof envelope.
   */
  async verifyNeoReserveLiabilityProof(proofEnvelope: any): Promise<any> {
    return this.rpc('verifyNeoReserveLiabilityProof', [proofEnvelope]);
  }

  /**
   * Get aggregated BountyBoard marketplace statistics.
   */
  async getBountyBoardStats(): Promise<any> {
    return this.rpc('getBountyBoardStats');
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

  async getNFTActivity(collectionId: PublicKey, options: { limit?: number } | number = {}): Promise<any> {
    const activityOptions = typeof options === 'number' ? { limit: options } : options;
    return this.rpc('getNFTActivity', [collectionId.toBase58(), activityOptions]);
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
        // AUDIT-FIX J-1: Guard against malformed WebSocket messages
        let msg: any;
        try {
          msg = JSON.parse(data.toString());
        } catch {
          console.warn('Lichen WS: ignoring non-JSON message');
          return;
        }

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
      const messageHandler = (data: WebSocket.Data) => {
        // AUDIT-FIX J-1: Guard against malformed WebSocket messages
        let msg: any;
        try {
          msg = JSON.parse(data.toString());
        } catch {
          return; // skip non-JSON frames
        }
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

      const timeout = setTimeout(() => {
        this.ws?.off('message', messageHandler);
        reject(new Error('Subscription timeout'));
      }, 5000);

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
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      this.subscriptions.delete(subscriptionId);
      return false;
    }

    const id = this.nextId++;

    return new Promise((resolve, reject) => {
      const messageHandler = (data: WebSocket.Data) => {
        // AUDIT-FIX J-1: Guard against malformed WebSocket messages
        let msg: any;
        try {
          msg = JSON.parse(data.toString());
        } catch {
          return; // skip non-JSON frames
        }
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

      const timeout = setTimeout(() => {
        this.ws?.off('message', messageHandler);
        reject(new Error('Unsubscribe timeout'));
      }, 5000);

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

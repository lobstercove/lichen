"""Connection class for MoltChain RPC and WebSocket"""

import asyncio
import json
import base64
import logging
import os
from typing import Any, Callable, Dict, List, Optional
import httpx
import websockets
from .keypair import Keypair
from .publickey import PublicKey
from .transaction import Transaction, TransactionBuilder

logger = logging.getLogger(__name__)

RPC_NO_BLOCKS_RETRIES = max(1, int(os.getenv("MOLT_RPC_NO_BLOCKS_RETRIES", "20")))
RPC_NO_BLOCKS_DELAY_SECS = max(0.05, float(os.getenv("MOLT_RPC_NO_BLOCKS_DELAY_SECS", "0.5")))


class Connection:
    """
    RPC and WebSocket connection to MoltChain
    
    Provides async methods for all 24 RPC endpoints and 10 WebSocket subscriptions.
    """
    
    def __init__(self, rpc_url: str, ws_url: Optional[str] = None):
        """
        Create a connection to MoltChain
        
        Args:
            rpc_url: HTTP RPC endpoint (e.g., 'http://localhost:8899')
            ws_url: WebSocket endpoint (e.g., 'ws://localhost:8900')
        """
        self.rpc_url = rpc_url
        self.ws_url = ws_url
        self._ws: Optional[websockets.WebSocketClientProtocol] = None
        self._subscriptions: Dict[int, Callable] = {}
        self._next_id = 1
        self._ws_task: Optional[asyncio.Task] = None
        self._pending_responses: Dict[int, asyncio.Future] = {}
        self._client: Optional[httpx.AsyncClient] = None
    
    async def _get_client(self) -> httpx.AsyncClient:
        """Get or create a persistent HTTP client with connection pooling"""
        if self._client is None or self._client.is_closed:
            self._client = httpx.AsyncClient(timeout=30.0)
        return self._client

    async def _rpc(self, method: str, params: List[Any] = None) -> Any:
        """Make an RPC call"""
        if params is None:
            params = []

        attempts = RPC_NO_BLOCKS_RETRIES
        for attempt in range(attempts):
            client = await self._get_client()
            response = await client.post(
                self.rpc_url,
                json={
                    "jsonrpc": "2.0",
                    "id": self._next_id,
                    "method": method,
                    "params": params
                },
            )
            self._next_id += 1

            # J-2: Check HTTP status before parsing JSON
            response.raise_for_status()
            data = response.json()

            if "error" in data:
                message = str(data["error"].get("message", "RPC error"))
                if "No blocks yet" in message and attempt < attempts - 1:
                    await asyncio.sleep(RPC_NO_BLOCKS_DELAY_SECS)
                    continue
                raise Exception(f"RPC Error: {message}")

            return data.get("result")

        raise Exception("RPC Error: exceeded retry budget while waiting for blocks")
    
    # ============================================================================
    # BASIC QUERIES
    # ============================================================================
    
    async def get_balance(self, pubkey: PublicKey) -> Dict[str, int]:
        """Get account balance"""
        return await self._rpc("getBalance", [str(pubkey)])
    
    async def get_account(self, pubkey: PublicKey) -> Dict[str, Any]:
        """Get account information"""
        return await self._rpc("getAccount", [str(pubkey)])
    
    async def get_block(self, slot: int) -> Dict[str, Any]:
        """Get block by slot number"""
        return await self._rpc("getBlock", [slot])
    
    async def get_latest_block(self) -> Dict[str, Any]:
        """Get latest block"""
        return await self._rpc("getLatestBlock")
    
    async def get_slot(self) -> int:
        """Get current slot"""
        result = await self._rpc("getSlot")
        # RPC returns int directly, not wrapped
        if isinstance(result, int):
            return result
        if isinstance(result, dict):
            return result.get("slot", 0)
        return int(result) if result is not None else 0
    
    async def get_transaction(self, signature: str) -> Dict[str, Any]:
        """Get transaction by signature"""
        return await self._rpc("getTransaction", [signature])
    
    async def send_transaction(self, transaction: Transaction) -> str:
        """Send transaction"""
        tx_bytes = TransactionBuilder.transaction_to_bincode(transaction)
        tx_base64 = base64.b64encode(tx_bytes).decode("ascii")
        result = await self._rpc("sendTransaction", [tx_base64])
        return result
    
    async def get_total_burned(self) -> Dict[str, int]:
        """Get total burned MOLT"""
        return await self._rpc("getTotalBurned")
    
    async def get_validators(self) -> List[Dict[str, Any]]:
        """Get all validators"""
        result = await self._rpc("getValidators")
        if isinstance(result, dict):
            return result.get("validators", [])
        return []
    
    async def get_metrics(self) -> Dict[str, Any]:
        """Get performance metrics"""
        return await self._rpc("getMetrics")
    
    async def get_recent_blockhash(self) -> str:
        """Get recent blockhash for transaction building"""
        result = await self._rpc("getRecentBlockhash")
        # Handle both string and object response
        if isinstance(result, str):
            return result
        return result.get("blockhash", result)
    
    async def health(self) -> Dict[str, str]:
        """Health check"""
        return await self._rpc("health")
    
    # ============================================================================
    # NETWORK ENDPOINTS
    # ============================================================================
    
    async def get_peers(self) -> List[Dict[str, Any]]:
        """Get connected peers"""
        result = await self._rpc("getPeers")
        return result["peers"]
    
    async def get_network_info(self) -> Dict[str, Any]:
        """Get network information"""
        return await self._rpc("getNetworkInfo")
    
    # ============================================================================
    # VALIDATOR ENDPOINTS
    # ============================================================================
    
    async def get_validator_info(self, pubkey: PublicKey) -> Dict[str, Any]:
        """Get detailed validator information"""
        return await self._rpc("getValidatorInfo", [str(pubkey)])
    
    async def get_validator_performance(self, pubkey: PublicKey) -> Dict[str, Any]:
        """Get validator performance metrics"""
        return await self._rpc("getValidatorPerformance", [str(pubkey)])
    
    async def get_chain_status(self) -> Dict[str, Any]:
        """Get comprehensive chain status"""
        return await self._rpc("getChainStatus")
    
    # ============================================================================
    # STAKING ENDPOINTS
    # ============================================================================
    
    async def stake(self, from_keypair: Keypair, validator: PublicKey, amount: int) -> str:
        """Create and send a stake transaction"""
        blockhash = await self.get_recent_blockhash()
        instruction = TransactionBuilder.stake(from_keypair.public_key(), validator, amount)
        transaction = (TransactionBuilder()
            .add(instruction)
            .set_recent_blockhash(blockhash)
            .build_and_sign(from_keypair))
        return await self.send_transaction(transaction)
    
    async def unstake(self, from_keypair: Keypair, validator: PublicKey, amount: int) -> str:
        """Create and send an unstake request transaction"""
        blockhash = await self.get_recent_blockhash()
        instruction = TransactionBuilder.unstake(from_keypair.public_key(), validator, amount)
        transaction = (TransactionBuilder()
            .add(instruction)
            .set_recent_blockhash(blockhash)
            .build_and_sign(from_keypair))
        return await self.send_transaction(transaction)
    
    async def get_staking_status(self, pubkey: PublicKey) -> Dict[str, Any]:
        """Get staking status"""
        return await self._rpc("getStakingStatus", [str(pubkey)])
    
    async def get_staking_rewards(self, pubkey: PublicKey) -> Dict[str, Any]:
        """Get staking rewards"""
        return await self._rpc("getStakingRewards", [str(pubkey)])
    
    # ============================================================================
    # ACCOUNT ENDPOINTS
    # ============================================================================
    
    async def get_account_info(self, pubkey: PublicKey) -> Dict[str, Any]:
        """Get enhanced account information"""
        return await self._rpc("getAccountInfo", [str(pubkey)])
    
    async def get_transaction_history(self, pubkey: PublicKey, limit: int = 10) -> Dict[str, Any]:
        """Get transaction history"""
        return await self._rpc("getTransactionHistory", [str(pubkey), limit])
    
    # ============================================================================
    # CONTRACT ENDPOINTS
    # ============================================================================
    
    async def get_contract_info(self, contract_id: PublicKey) -> Dict[str, Any]:
        """Get contract information"""
        return await self._rpc("getContractInfo", [str(contract_id)])
    
    async def get_contract_logs(self, contract_id: PublicKey) -> Dict[str, Any]:
        """Get contract logs"""
        return await self._rpc("getContractLogs", [str(contract_id)])
    
    async def get_all_contracts(self) -> Dict[str, Any]:
        """Get all deployed contracts"""
        return await self._rpc("getAllContracts")

    # ============================================================================
    # PROGRAM ENDPOINTS (DRAFT)
    # ============================================================================

    async def get_program(self, program_id: PublicKey) -> Dict[str, Any]:
        """Get program information"""
        return await self._rpc("getProgram", [str(program_id)])

    async def get_program_stats(self, program_id: PublicKey) -> Dict[str, Any]:
        """Get program statistics"""
        return await self._rpc("getProgramStats", [str(program_id)])

    async def get_programs(self) -> Dict[str, Any]:
        """Get list of programs"""
        return await self._rpc("getPrograms")

    async def get_program_calls(self, program_id: PublicKey) -> Dict[str, Any]:
        """Get program call history"""
        return await self._rpc("getProgramCalls", [str(program_id)])

    async def get_program_storage(self, program_id: PublicKey) -> Dict[str, Any]:
        """Get program storage summary"""
        return await self._rpc("getProgramStorage", [str(program_id)])

    # ============================================================================
    # NFT ENDPOINTS (DRAFT)
    # ============================================================================

    async def get_collection(self, collection_id: PublicKey) -> Dict[str, Any]:
        """Get NFT collection"""
        return await self._rpc("getCollection", [str(collection_id)])

    async def get_nft(self, collection_id: PublicKey, token_id: int) -> Dict[str, Any]:
        """Get NFT details"""
        return await self._rpc("getNFT", [str(collection_id), token_id])

    async def get_nfts_by_owner(self, owner: PublicKey) -> Dict[str, Any]:
        """Get NFTs by owner"""
        return await self._rpc("getNFTsByOwner", [str(owner)])

    async def get_nfts_by_collection(self, collection_id: PublicKey) -> Dict[str, Any]:
        """Get NFTs by collection"""
        return await self._rpc("getNFTsByCollection", [str(collection_id)])

    async def get_nft_activity(self, collection_id: PublicKey, token_id: int) -> Dict[str, Any]:
        """Get NFT activity"""
        return await self._rpc("getNFTActivity", [str(collection_id), token_id])
    
    # ============================================================================
    # WEBSOCKET SUBSCRIPTIONS
    # ============================================================================
    
    async def _connect_ws(self):
        """Connect to WebSocket"""
        if not self.ws_url:
            raise ValueError("WebSocket URL not provided")
        
        if self._ws and not self._ws.closed:
            return
        
        self._ws = await websockets.connect(self.ws_url)
        
        # Start message handler
        if not self._ws_task or self._ws_task.done():
            self._ws_task = asyncio.create_task(self._handle_ws_messages())
    
    async def _handle_ws_messages(self):
        """Handle incoming WebSocket messages"""
        try:
            async for message in self._ws:
                try:
                    data = json.loads(message)
                except json.JSONDecodeError:
                    continue
                
                try:
                    # Check if this is a response to a pending request
                    if "id" in data and data["id"] in self._pending_responses:
                        future = self._pending_responses.pop(data["id"])
                        future.set_result(data)
                    # Check if this is a subscription notification
                    elif data.get("method") == "subscription":
                        params = data.get("params", {})
                        sub_id = params.get("subscription")
                        result = params.get("result")
                        if sub_id is None:
                            continue
                        
                        callback = self._subscriptions.get(sub_id)
                        if callback:
                            try:
                                if asyncio.iscoroutinefunction(callback):
                                    await callback(result)
                                else:
                                    callback(result)
                            except Exception as e:
                                # AUDIT-FIX J-7: Log callback errors instead of silently swallowing
                                logger.warning(
                                    "WS subscription callback error (sub %s): %s",
                                    sub_id, e, exc_info=True
                                )
                except (KeyError, TypeError):
                    continue
        except websockets.exceptions.ConnectionClosed:
            pass
    
    async def _subscribe(self, method: str, params: Any = None) -> int:
        """Subscribe to a WebSocket method"""
        await self._connect_ws()
        
        request_id = self._next_id
        self._next_id += 1
        
        # Create future for this request
        future = asyncio.get_event_loop().create_future()
        self._pending_responses[request_id] = future
        
        await self._ws.send(json.dumps({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params
        }))
        
        # Wait for response from the background task
        try:
            data = await asyncio.wait_for(future, timeout=5.0)
            if "error" in data:
                raise Exception(f"Subscription error: {data['error']['message']}")
            return data["result"]
        except asyncio.TimeoutError:
            self._pending_responses.pop(request_id, None)
            raise Exception("Subscription request timed out")
    
    async def _unsubscribe(self, method: str, sub_id: int) -> bool:
        """Unsubscribe from a subscription"""
        request_id = self._next_id
        self._next_id += 1
        
        # Create future for this request
        future = asyncio.get_event_loop().create_future()
        self._pending_responses[request_id] = future
        
        await self._ws.send(json.dumps({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": sub_id
        }))
        
        # Wait for response from the background task
        try:
            data = await asyncio.wait_for(future, timeout=5.0)
            self._subscriptions.pop(sub_id, None)
            return data.get("result", False)
        except asyncio.TimeoutError:
            self._pending_responses.pop(request_id, None)
            return False
    
    async def on_slot(self, callback: Callable[[int], None]) -> int:
        """Subscribe to slot updates"""
        sub_id = await self._subscribe("subscribeSlots")
        self._subscriptions[sub_id] = lambda data: callback(data["slot"])
        return sub_id
    
    async def off_slot(self, sub_id: int) -> bool:
        """Unsubscribe from slots"""
        return await self._unsubscribe("unsubscribeSlots", sub_id)
    
    async def on_block(self, callback: Callable[[Dict[str, Any]], None]) -> int:
        """Subscribe to block updates"""
        sub_id = await self._subscribe("subscribeBlocks")
        self._subscriptions[sub_id] = callback
        return sub_id
    
    async def off_block(self, sub_id: int) -> bool:
        """Unsubscribe from blocks"""
        return await self._unsubscribe("unsubscribeBlocks", sub_id)
    
    async def on_transaction(self, callback: Callable[[Dict[str, Any]], None]) -> int:
        """Subscribe to transaction updates"""
        sub_id = await self._subscribe("subscribeTransactions")
        self._subscriptions[sub_id] = callback
        return sub_id
    
    async def off_transaction(self, sub_id: int) -> bool:
        """Unsubscribe from transactions"""
        return await self._unsubscribe("unsubscribeTransactions", sub_id)
    
    async def on_account_change(self, pubkey: PublicKey, callback: Callable[[Dict[str, Any]], None]) -> int:
        """Subscribe to account changes"""
        sub_id = await self._subscribe("subscribeAccount", str(pubkey))
        self._subscriptions[sub_id] = callback
        return sub_id
    
    async def off_account_change(self, sub_id: int) -> bool:
        """Unsubscribe from account changes"""
        return await self._unsubscribe("unsubscribeAccount", sub_id)
    
    async def on_logs(self, callback: Callable[[Dict[str, Any]], None], contract_id: Optional[PublicKey] = None) -> int:
        """Subscribe to contract logs"""
        params = str(contract_id) if contract_id else None
        sub_id = await self._subscribe("subscribeLogs", params)
        self._subscriptions[sub_id] = callback
        return sub_id
    
    async def off_logs(self, sub_id: int) -> bool:
        """Unsubscribe from logs"""
        return await self._unsubscribe("unsubscribeLogs", sub_id)

    async def on_program_updates(self, callback: Callable[[Dict[str, Any]], None]) -> int:
        """Subscribe to program updates"""
        sub_id = await self._subscribe("subscribeProgramUpdates")
        self._subscriptions[sub_id] = callback
        return sub_id

    async def off_program_updates(self, sub_id: int) -> bool:
        """Unsubscribe from program updates"""
        return await self._unsubscribe("unsubscribeProgramUpdates", sub_id)

    async def on_program_calls(self, callback: Callable[[Dict[str, Any]], None], program_id: Optional[PublicKey] = None) -> int:
        """Subscribe to program calls"""
        params = str(program_id) if program_id else None
        sub_id = await self._subscribe("subscribeProgramCalls", params)
        self._subscriptions[sub_id] = callback
        return sub_id

    async def off_program_calls(self, sub_id: int) -> bool:
        """Unsubscribe from program calls"""
        return await self._unsubscribe("unsubscribeProgramCalls", sub_id)

    async def on_nft_mints(self, callback: Callable[[Dict[str, Any]], None], collection_id: Optional[PublicKey] = None) -> int:
        """Subscribe to NFT mints"""
        params = str(collection_id) if collection_id else None
        sub_id = await self._subscribe("subscribeNftMints", params)
        self._subscriptions[sub_id] = callback
        return sub_id

    async def off_nft_mints(self, sub_id: int) -> bool:
        """Unsubscribe from NFT mints"""
        return await self._unsubscribe("unsubscribeNftMints", sub_id)

    async def on_nft_transfers(self, callback: Callable[[Dict[str, Any]], None], collection_id: Optional[PublicKey] = None) -> int:
        """Subscribe to NFT transfers"""
        params = str(collection_id) if collection_id else None
        sub_id = await self._subscribe("subscribeNftTransfers", params)
        self._subscriptions[sub_id] = callback
        return sub_id

    async def off_nft_transfers(self, sub_id: int) -> bool:
        """Unsubscribe from NFT transfers"""
        return await self._unsubscribe("unsubscribeNftTransfers", sub_id)

    async def on_market_listings(self, callback: Callable[[Dict[str, Any]], None]) -> int:
        """Subscribe to marketplace listings"""
        sub_id = await self._subscribe("subscribeMarketListings")
        self._subscriptions[sub_id] = callback
        return sub_id

    async def off_market_listings(self, sub_id: int) -> bool:
        """Unsubscribe from marketplace listings"""
        return await self._unsubscribe("unsubscribeMarketListings", sub_id)

    async def on_market_sales(self, callback: Callable[[Dict[str, Any]], None]) -> int:
        """Subscribe to marketplace sales"""
        sub_id = await self._subscribe("subscribeMarketSales")
        self._subscriptions[sub_id] = callback
        return sub_id

    async def off_market_sales(self, sub_id: int) -> bool:
        """Unsubscribe from marketplace sales"""
        return await self._unsubscribe("unsubscribeMarketSales", sub_id)
    
    async def close(self):
        """Close connection and clean up all resources"""
        # Cancel all pending response futures
        for future in self._pending_responses.values():
            if not future.done():
                future.cancel()
        self._pending_responses.clear()

        if self._ws_task:
            self._ws_task.cancel()
            try:
                await self._ws_task
            except asyncio.CancelledError:
                pass
        
        if self._ws:
            await self._ws.close()
        
        if self._client:
            await self._client.aclose()
            self._client = None
        
        self._subscriptions.clear()

    async def __aenter__(self):
        """Async context manager entry"""
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit — ensures resources are released"""
        await self.close()
        return False

"""Connection class for Lichen RPC and WebSocket"""

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

RPC_NO_BLOCKS_RETRIES = max(1, int(os.getenv("LICHEN_RPC_NO_BLOCKS_RETRIES", "20")))
RPC_NO_BLOCKS_DELAY_SECS = max(0.05, float(os.getenv("LICHEN_RPC_NO_BLOCKS_DELAY_SECS", "0.5")))
RPC_TRANSPORT_RETRIES = max(1, int(os.getenv("LICHEN_RPC_TRANSPORT_RETRIES", "3")))
RPC_TRANSPORT_DELAY_SECS = max(0.05, float(os.getenv("LICHEN_RPC_TRANSPORT_DELAY_SECS", "0.25")))


class Connection:
    """
    RPC and WebSocket connection to Lichen
    
    Provides async methods for all 24 RPC endpoints and 10 WebSocket subscriptions.
    """
    
    def __init__(self, rpc_url: str, ws_url: Optional[str] = None):
        """
        Create a connection to Lichen
        
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

    async def _rpc(
        self,
        method: str,
        params: List[Any] = None,
        headers: Optional[Dict[str, str]] = None,
    ) -> Any:
        """Make an RPC call"""
        if params is None:
            params = []

        attempts = RPC_NO_BLOCKS_RETRIES
        transport_failures = 0
        for attempt in range(attempts):
            client = await self._get_client()
            request_id = self._next_id
            self._next_id += 1
            try:
                response = await client.post(
                    self.rpc_url,
                    json={
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "method": method,
                        "params": params
                    },
                    headers=headers,
                )
                # J-2: Check HTTP status before parsing JSON
                response.raise_for_status()
                data = response.json()
            except httpx.TransportError as exc:
                transport_failures += 1
                if transport_failures < RPC_TRANSPORT_RETRIES:
                    await asyncio.sleep(RPC_TRANSPORT_DELAY_SECS)
                    continue
                raise Exception(f"RPC transport error: {exc}") from exc

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
        """Get transaction by signature.
        For contract calls, response includes: return_code (u32), return_data (base64), contract_logs (list[str]).
        """
        return await self._rpc("getTransaction", [signature])
    
    async def send_transaction(self, transaction: Transaction) -> str:
        """Send transaction"""
        tx_bytes = TransactionBuilder.transaction_to_bincode(transaction)
        tx_base64 = base64.b64encode(tx_bytes).decode("ascii")
        result = await self._rpc("sendTransaction", [tx_base64])
        return result
    
    async def get_total_burned(self) -> Dict[str, int]:
        """Get total burned LICN"""
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
        instruction = TransactionBuilder.stake(from_keypair.pubkey(), validator, amount)
        transaction = (TransactionBuilder()
            .add(instruction)
            .set_recent_blockhash(blockhash)
            .build_and_sign(from_keypair))
        return await self.send_transaction(transaction)
    
    async def unstake(self, from_keypair: Keypair, validator: PublicKey, amount: int) -> str:
        """Create and send an unstake request transaction"""
        blockhash = await self.get_recent_blockhash()
        instruction = TransactionBuilder.unstake(from_keypair.pubkey(), validator, amount)
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
    # TRANSFER & CONTRACT TRANSACTION ENDPOINTS
    # ============================================================================

    async def transfer(self, from_keypair: Keypair, to: PublicKey, amount: int) -> str:
        """
        Transfer native LICN (spores) from one account to another.

        Args:
            from_keypair: Sender keypair (signer)
            to: Recipient public key
            amount: Amount in spores (1 LICN = 1_000_000_000 spores)

        Returns:
            Transaction signature
        """
        blockhash = await self.get_recent_blockhash()
        instruction = TransactionBuilder.transfer(from_keypair.pubkey(), to, amount)
        transaction = (TransactionBuilder()
            .add(instruction)
            .set_recent_blockhash(blockhash)
            .build_and_sign(from_keypair))
        return await self.send_transaction(transaction)

    async def deploy_contract(
        self,
        deployer: Keypair,
        code: bytes,
        init_data: bytes = b"",
    ) -> str:
        """
        Deploy a WASM smart contract.

        Args:
            deployer: Deployer keypair (signer, pays deploy fee)
            code: WASM bytecode (must start with \\0asm magic, max 512 KB)
            init_data: Optional initialization data passed to contract init

        Returns:
            Transaction signature
        """
        blockhash = await self.get_recent_blockhash()
        instruction = TransactionBuilder.deploy_contract(deployer.pubkey(), code, init_data)
        transaction = (TransactionBuilder()
            .add(instruction)
            .set_recent_blockhash(blockhash)
            .build_and_sign(deployer))
        return await self.send_transaction(transaction)

    async def call_contract(
        self,
        caller: Keypair,
        contract: PublicKey,
        function_name: str,
        args: bytes = b"",
        value: int = 0,
    ) -> str:
        """
        Call a function on a deployed WASM smart contract.

        Args:
            caller: Caller keypair (signer)
            contract: Contract account public key
            function_name: Name of the contract function to invoke
            args: Serialized function arguments (default: empty)
            value: Native LICN to send with the call in spores (default: 0)

        Returns:
            Transaction signature
        """
        blockhash = await self.get_recent_blockhash()
        instruction = TransactionBuilder.call_contract(
            caller.pubkey(), contract, function_name, args, value
        )
        transaction = (TransactionBuilder()
            .add(instruction)
            .set_recent_blockhash(blockhash)
            .build_and_sign(caller))
        return await self.send_transaction(transaction)

    async def upgrade_contract(
        self,
        owner: Keypair,
        contract: PublicKey,
        code: bytes,
    ) -> str:
        """
        Upgrade a deployed WASM smart contract (owner only).

        Args:
            owner: Contract owner keypair (signer)
            contract: Contract account public key
            code: New WASM bytecode

        Returns:
            Transaction signature
        """
        blockhash = await self.get_recent_blockhash()
        instruction = TransactionBuilder.upgrade_contract(owner.pubkey(), contract, code)
        transaction = (TransactionBuilder()
            .add(instruction)
            .set_recent_blockhash(blockhash)
            .build_and_sign(owner))
        return await self.send_transaction(transaction)

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

    async def confirm_transaction(self, signature: str, timeout: float = 30.0) -> Optional[Dict[str, Any]]:
        """Wait for a transaction signature to be confirmed via WebSocket.

        Uses ``signatureSubscribe`` for a push-based one-shot notification
        instead of polling ``getTransaction`` in a loop.  Falls back to RPC
        polling when the WS URL is not available or the WS connection fails.

        Returns the signature-status notification dict on success, or *None*
        if the timeout expires.
        """

        # ── Auto-derive WS URL when not explicitly set ──
        if not self.ws_url:
            self.ws_url = self._derive_ws_url(self.rpc_url)

        # ── Try WS-based confirmation first ──
        if self.ws_url:
            try:
                return await self._confirm_via_ws(signature, timeout)
            except Exception:
                pass  # fall through to RPC polling

        # ── Fallback: RPC polling (same as old wait_tx) ──
        return await self._confirm_via_rpc(signature, timeout)

    async def _confirm_via_ws(self, signature: str, timeout: float) -> Optional[Dict[str, Any]]:
        """Subscribe to signatureStatus and wait for the one-shot notification.

        After the WS notification fires, fetches the full transaction via RPC
        so callers get the same data shape as the polling fallback.
        """
        await self._connect_ws()

        result_future: asyncio.Future = asyncio.get_event_loop().create_future()

        def _on_status(data: Any) -> None:
            if not result_future.done():
                result_future.set_result(data)

        sub_id = await self._subscribe("signatureSubscribe", [signature])
        self._subscriptions[sub_id] = _on_status

        try:
            await asyncio.wait_for(result_future, timeout=timeout)
            # WS confirmed — now fetch full tx info so callers get return_code etc.
            try:
                tx_info = await self.get_transaction(signature)
                if tx_info:
                    return tx_info
            except Exception:
                pass
            # If fetch fails, return a minimal confirmation dict
            return {"signature": signature, "confirmed": True}
        except asyncio.TimeoutError:
            return None
        finally:
            self._subscriptions.pop(sub_id, None)
            try:
                await self._unsubscribe("signatureUnsubscribe", sub_id)
            except Exception:
                pass

    async def _confirm_via_rpc(self, signature: str, timeout: float) -> Optional[Dict[str, Any]]:
        """Fall back to polling getTransaction."""
        t0 = asyncio.get_event_loop().time()
        while asyncio.get_event_loop().time() - t0 < timeout:
            try:
                tx = await self.get_transaction(signature)
                if tx:
                    return tx
            except Exception:
                pass
            await asyncio.sleep(0.5)
        return None

    @staticmethod
    def _derive_ws_url(rpc_url: str) -> Optional[str]:
        """Best-effort derivation of a WS URL from the RPC HTTP URL."""
        if not rpc_url:
            return None
        url = rpc_url.rstrip("/")
        if "://localhost" in url or "://127.0.0.1" in url:
            # localhost: port 8899 → 8900
            return url.replace("http://", "ws://").replace(":8899", ":8900")
        if url.startswith("https://"):
            # Production: assume /ws path on same host
            return url.replace("https://", "wss://") + "/ws"
        if url.startswith("http://"):
            return url.replace("http://", "ws://") + "/ws"
        return None

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

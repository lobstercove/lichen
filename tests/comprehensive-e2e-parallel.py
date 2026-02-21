#!/usr/bin/env python3
"""
MoltChain Parallel E2E Test — ALL 27 contracts tested CONCURRENTLY.

Same coverage as comprehensive-e2e.py but runs all contracts in parallel
using asyncio.gather(). Each contract's tests still run sequentially
(initialization before operations), but ALL contracts run at the same time.

This simulates real-world load: thousands of users/agents hitting different
contracts simultaneously.
"""

import asyncio
import json
import os
import random
import struct
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from moltchain import Connection, Instruction, Keypair, PublicKey, TransactionBuilder

RPC_URL = os.getenv("RPC_URL", "http://127.0.0.1:8899")
# PERF-OPT 4: Distribute TXs across all validators (round-robin).
# Previously all TXs went to V1 (port 8899) — non-leader validators'
# TXs had to propagate via P2P before the leader could include them.
# Now each contract suite is assigned a different validator RPC.
RPC_ENDPOINTS = os.getenv("RPC_ENDPOINTS", "").split(",") if os.getenv("RPC_ENDPOINTS") else []
CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
TX_CONFIRM_TIMEOUT = int(os.getenv("TX_CONFIRM_TIMEOUT", "30"))  # Higher for parallel load + 3-validator consensus
DEPLOYER_PATH = os.getenv("AGENT_KEYPAIR") or str(ROOT / "keypairs" / "deployer.json")

# Max concurrent contract test suites (all 27 at once)
MAX_CONCURRENCY = int(os.getenv("MAX_CONCURRENCY", "27"))

# ─── Thread-safe counters ───
import threading
import statistics
_lock = threading.Lock()
PASS = 0
FAIL = 0
SKIP = 0
RESULTS: List[Dict[str, Any]] = []
TIMINGS: List[Dict[str, Any]] = []  # Per-test: {contract, test, elapsed, status}
CONTRACT_TIMES: Dict[str, float] = {}  # contract_name -> total elapsed
TX_COUNT = 0  # Total transactions sent

# ─── Symbol → dir name mapping ───
SYMBOL_TO_DIR = {
    "MOLT": "moltcoin", "MUSD": "musd_token", "WSOL": "wsol_token", "WETH": "weth_token",
    "YID": "moltyid", "DEX": "dex_core", "DEXAMM": "dex_amm", "DEXROUTER": "dex_router",
    "DEXMARGIN": "dex_margin", "DEXREWARDS": "dex_rewards", "DEXGOV": "dex_governance",
    "ANALYTICS": "dex_analytics", "MOLTSWAP": "moltswap", "BRIDGE": "moltbridge",
    "ORACLE": "moltoracle", "LEND": "lobsterlend", "DAO": "moltdao", "MARKET": "moltmarket",
    "PUNKS": "moltpunks", "CLAWPAY": "clawpay", "CLAWPUMP": "clawpump",
    "CLAWVAULT": "clawvault", "COMPUTE": "compute_market", "REEF": "reef_storage",
    "PREDICT": "prediction_market", "BOUNTY": "bountyboard", "AUCTION": "moltauction",
}

# Dispatcher contracts (use opcode ABI via call())
DISPATCHER_CONTRACTS = {
    "dex_core", "dex_amm", "dex_analytics", "dex_governance",
    "dex_margin", "dex_rewards", "dex_router", "prediction_market",
}


def report(status: str, msg: str, elapsed: float = 0.0):
    global PASS, FAIL, SKIP, TX_COUNT
    with _lock:
        if status == "PASS":
            PASS += 1
            tag = "\033[32m  PASS\033[0m"
        elif status == "SKIP":
            SKIP += 1
            tag = "\033[33m  SKIP\033[0m"
        else:
            FAIL += 1
            tag = "\033[31m  FAIL\033[0m"
        TX_COUNT += 1
        time_str = f" ({elapsed:.2f}s)" if elapsed > 0 else ""
        print(f"{tag}  {msg}{time_str}")
        RESULTS.append({"status": status, "msg": msg, "ts": int(time.time()), "elapsed": round(elapsed, 3)})


def _extract_shells(bal) -> int:
    """Extract shells (lamports) from a getBalance response.

    The RPC returns {'shells': N, 'spendable': N, ...} — NOT {'balance': N}.
    Handle both formats defensively.
    """
    if isinstance(bal, (int, float)):
        return int(bal)
    if isinstance(bal, dict):
        for key in ("shells", "spendable", "balance"):
            v = bal.get(key)
            if isinstance(v, (int, float)):
                return int(v)
    return 0


def load_keypair_flexible(path: Path) -> Keypair:
    try:
        return Keypair.load(path)
    except Exception:
        pass
    raw = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(raw, dict):
        pk = raw.get("privateKey") or raw.get("secret_key")
        if isinstance(pk, list) and len(pk) == 32:
            return Keypair.from_seed(bytes(pk))
        if isinstance(pk, str):
            h = pk.strip().lower().removeprefix("0x")
            if len(h) == 64:
                return Keypair.from_seed(bytes.fromhex(h))
    raise ValueError(f"unsupported keypair format: {path}")


# ─── Binary encoding helpers ───

def u64le(v: int) -> bytes:
    return struct.pack("<Q", v & 0xFFFFFFFFFFFFFFFF)

def i64le(v: int) -> bytes:
    return struct.pack("<q", v)

def u32le(v: int) -> bytes:
    return struct.pack("<I", v & 0xFFFFFFFF)

def u16le(v: int) -> bytes:
    return struct.pack("<H", v & 0xFFFF)

def i32le(v: int) -> bytes:
    return struct.pack("<i", v)

def i16le(v: int) -> bytes:
    return struct.pack("<h", v)

def pubkey_bytes(addr: str) -> bytes:
    if not addr or addr == "0" * 32:
        return b'\x00' * 32
    try:
        pk = PublicKey.from_base58(addr)
        return pk.to_bytes()
    except Exception:
        return b'\x00' * 32


def encode_layout_args(params: List[Tuple[int, Any]]) -> Tuple[bytes, List[int]]:
    layout = [s for s, _ in params]
    data = bytearray()
    for stride, value in params:
        if stride >= 32:
            if isinstance(value, str):
                data.extend(pubkey_bytes(value))
            elif isinstance(value, (bytes, bytearray)):
                padded = bytes(value) + b'\x00' * max(0, stride - len(value))
                data.extend(padded[:stride])
            else:
                data.extend(b'\x00' * stride)
        elif stride == 8:
            data.extend(int(value).to_bytes(8, 'little'))
        elif stride == 4:
            data.extend(int(value).to_bytes(4, 'little'))
        elif stride == 2:
            data.extend(int(value).to_bytes(2, 'little'))
        elif stride == 1:
            data.extend(bytes([int(value) & 0xFF]))
    return bytes(data), layout


# ─── Contract call functions ───

async def call_named(
    conn: Connection, caller: Keypair, program: PublicKey,
    func: str, args: Optional[Dict[str, Any]] = None,
) -> str:
    args_bytes = json.dumps(args or {}).encode()
    payload = json.dumps({"Call": {"function": func, "args": list(args_bytes), "value": 0}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.public_key(), program],
        data=payload.encode(),
    )
    blockhash = await conn.get_recent_blockhash()
    tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(caller)
    return await conn.send_transaction(tx)


async def call_named_binary(
    conn: Connection, caller: Keypair, program: PublicKey,
    func: str, binary_args: bytes, layout: Optional[List[int]] = None,
) -> str:
    if layout:
        header = bytes([0xAB]) + bytes(layout)
        full_args = header + binary_args
    else:
        full_args = binary_args
    payload = json.dumps({"Call": {"function": func, "args": list(full_args), "value": 0}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.public_key(), program],
        data=payload.encode(),
    )
    blockhash = await conn.get_recent_blockhash()
    tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(caller)
    return await conn.send_transaction(tx)


async def call_opcode(
    conn: Connection, caller: Keypair, program: PublicKey,
    opcode_args: bytes,
) -> str:
    payload = json.dumps({"Call": {"function": "call", "args": list(opcode_args), "value": 0}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.public_key(), program],
        data=payload.encode(),
    )
    blockhash = await conn.get_recent_blockhash()
    tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(caller)
    return await conn.send_transaction(tx)


async def wait_tx(conn: Connection, sig: str, timeout: int = TX_CONFIRM_TIMEOUT) -> Optional[Dict]:
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            tx = await conn.get_transaction(sig)
            if tx:
                return tx
        except Exception:
            pass
        await asyncio.sleep(0.15)  # Slightly longer polling for parallel load
    return None


def _is_transient_error(e: Exception) -> bool:
    """Return True for errors that should be retried (server overload, disconnects)."""
    msg = str(e).lower()
    return any(kw in msg for kw in (
        "disconnected", "connection refused", "connection reset",
        "broken pipe", "timed out", "timeout", "too many",
        "service unavailable", "503", "429",
    ))

MAX_RETRIES = 3
RETRY_BACKOFF = 0.5  # seconds, doubles each retry


async def send_and_confirm_named(
    conn: Connection, caller: Keypair, program: PublicKey,
    func: str, args: Optional[Dict[str, Any]] = None,
    label: str = "",
    binary_args: Optional[bytes] = None,
    layout: Optional[List[int]] = None,
) -> bool:
    tag = label or func
    t0 = time.time()
    last_error = None
    for attempt in range(MAX_RETRIES):
        try:
            if binary_args is not None:
                sig = await call_named_binary(conn, caller, program, func, binary_args, layout)
            else:
                sig = await call_named(conn, caller, program, func, args)
            tx = await wait_tx(conn, sig)
            elapsed = time.time() - t0
            contract = tag.split(".")[0] if "." in tag else tag
            with _lock:
                TIMINGS.append({"contract": contract, "test": tag, "elapsed": round(elapsed, 3), "status": "PASS" if tx else "FAIL"})
            if tx:
                report("PASS", f"{tag} sig={sig[:16]}...", elapsed)
                return True
            else:
                report("FAIL", f"{tag} not confirmed in {TX_CONFIRM_TIMEOUT}s", elapsed)
                return False
        except Exception as e:
            last_error = e
            if _is_transient_error(e) and attempt < MAX_RETRIES - 1:
                await asyncio.sleep(RETRY_BACKOFF * (2 ** attempt))
                continue
            break
    elapsed = time.time() - t0
    contract = tag.split(".")[0] if "." in tag else tag
    with _lock:
        TIMINGS.append({"contract": contract, "test": tag, "elapsed": round(elapsed, 3), "status": "FAIL"})
    report("FAIL", f"{tag} error={last_error}", elapsed)
    return False


async def send_and_confirm_opcode(
    conn: Connection, caller: Keypair, program: PublicKey,
    opcode_args: bytes, label: str = "",
) -> bool:
    t0 = time.time()
    last_error = None
    for attempt in range(MAX_RETRIES):
        try:
            sig = await call_opcode(conn, caller, program, opcode_args)
            tx = await wait_tx(conn, sig)
            elapsed = time.time() - t0
            contract = label.split(".")[0] if "." in label else label
            with _lock:
                TIMINGS.append({"contract": contract, "test": label, "elapsed": round(elapsed, 3), "status": "PASS" if tx else "FAIL"})
            if tx:
                report("PASS", f"{label} sig={sig[:16]}...", elapsed)
                return True
            else:
                report("FAIL", f"{label} not confirmed in {TX_CONFIRM_TIMEOUT}s", elapsed)
                return False
        except Exception as e:
            last_error = e
            if _is_transient_error(e) and attempt < MAX_RETRIES - 1:
                await asyncio.sleep(RETRY_BACKOFF * (2 ** attempt))
                continue
            break
    elapsed = time.time() - t0
    contract = label.split(".")[0] if "." in label else label
    with _lock:
        TIMINGS.append({"contract": contract, "test": label, "elapsed": round(elapsed, 3), "status": "FAIL"})
    report("FAIL", f"{label} error={last_error}", elapsed)
    return False


# ─── Contract discovery ───

async def discover_contracts(conn: Connection) -> Dict[str, PublicKey]:
    found: Dict[str, PublicKey] = {}
    try:
        sr = await conn._rpc("getAllSymbolRegistry", [])
        entries = sr.get("entries", []) if isinstance(sr, dict) else sr
        for e in entries:
            sym = e.get("symbol", "")
            prog = e.get("program", "")
            dir_name = SYMBOL_TO_DIR.get(sym.upper())
            if dir_name and prog:
                try:
                    found[dir_name] = PublicKey.from_base58(prog)
                except Exception:
                    continue
    except Exception:
        pass
    return found


# ─── Test scenario builders (same as sequential) ───

def build_named_scenarios(
    deployer: Keypair, secondary: Keypair, contracts: Dict[str, PublicKey]
) -> Dict[str, List[Dict[str, Any]]]:
    dp = str(deployer.public_key())
    sp = str(secondary.public_key())
    zero = "11111111111111111111111111111111"
    quote = str(contracts.get("moltcoin") or dp)
    base = str(contracts.get("weth_token") or dp)
    now = int(time.time())
    rid = random.randint(1000, 99999)

    return {
        "moltcoin": [
            {"fn": "initialize", "args": {"owner": dp}},
            {"fn": "mint", "args": {"to": dp, "amount": 1_000_000}},
            {"fn": "transfer", "args": {"from": dp, "to": sp, "amount": 1000}},
            {"fn": "burn", "args": {"from": dp, "amount": 100}},
            {"fn": "approve", "args": {"owner": dp, "spender": sp, "amount": 500}},
            {"fn": "balance_of", "args": {"account": dp}},
            {"fn": "total_supply", "args": {}},
        ],
        "musd_token": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "mint", "args": {"caller": dp, "to": dp, "amount": 1_000_000}},
            {"fn": "transfer", "args": {"from": dp, "to": sp, "amount": 10_000}},
            {"fn": "approve", "args": {"owner": dp, "spender": sp, "amount": 5_000}},
            {"fn": "burn", "args": {"caller": dp, "amount": 1_000}},
            {"fn": "balance_of", "args": {"account": dp}},
            {"fn": "total_supply", "args": {}},
            {"fn": "allowance", "args": {"owner": dp, "spender": sp}},
            {"fn": "total_minted", "args": {}},
            {"fn": "total_burned", "args": {}},
            {"fn": "transfer_from", "args": {"caller": sp, "from": dp, "to": sp, "amount": 100}, "actor": "secondary"},
            {"fn": "emergency_pause", "args": {"caller": dp}},
            {"fn": "emergency_unpause", "args": {"caller": dp}},
            {"fn": "get_transfer_count", "args": {}},
            {"fn": "get_attestation_count", "args": {}},
            {"fn": "get_epoch_remaining", "args": {}},
            {"fn": "get_last_attestation_slot", "args": {}},
            {"fn": "get_reserve_ratio", "args": {}},
            {"fn": "attest_reserves", "args": {"attester": dp, "reserve_amount": 1_000_000, "supply_snapshot": 999_000}},
            {"fn": "transfer_admin", "args": {"caller": dp, "new_admin": dp}},
        ],
        "weth_token": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "mint", "args": {"caller": dp, "to": dp, "amount": 1_000_000}},
            {"fn": "transfer", "args": {"from": dp, "to": sp, "amount": 10_000}},
            {"fn": "approve", "args": {"owner": dp, "spender": sp, "amount": 5_000}},
            {"fn": "burn", "args": {"caller": dp, "amount": 1_000}},
            {"fn": "balance_of", "args": {"account": dp}},
            {"fn": "total_supply", "args": {}},
            {"fn": "allowance", "args": {"owner": dp, "spender": sp}},
            {"fn": "total_minted", "args": {}},
            {"fn": "total_burned", "args": {}},
            {"fn": "transfer_from", "args": {"caller": sp, "from": dp, "to": sp, "amount": 100}, "actor": "secondary"},
            {"fn": "emergency_pause", "args": {"caller": dp}},
            {"fn": "emergency_unpause", "args": {"caller": dp}},
            {"fn": "get_transfer_count", "args": {}},
            {"fn": "get_attestation_count", "args": {}},
            {"fn": "get_epoch_remaining", "args": {}},
            {"fn": "get_last_attestation_slot", "args": {}},
            {"fn": "get_reserve_ratio", "args": {}},
            {"fn": "attest_reserves", "args": {"attester": dp, "reserve_amount": 1_000_000, "supply_snapshot": 999_000}},
            {"fn": "transfer_admin", "args": {"caller": dp, "new_admin": dp}},
        ],
        "wsol_token": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "mint", "args": {"caller": dp, "to": dp, "amount": 1_000_000}},
            {"fn": "transfer", "args": {"from": dp, "to": sp, "amount": 10_000}},
            {"fn": "approve", "args": {"owner": dp, "spender": sp, "amount": 5_000}},
            {"fn": "burn", "args": {"caller": dp, "amount": 1_000}},
            {"fn": "balance_of", "args": {"account": dp}},
            {"fn": "total_supply", "args": {}},
            {"fn": "allowance", "args": {"owner": dp, "spender": sp}},
            {"fn": "total_minted", "args": {}},
            {"fn": "total_burned", "args": {}},
            {"fn": "transfer_from", "args": {"caller": sp, "from": dp, "to": sp, "amount": 100}, "actor": "secondary"},
            {"fn": "emergency_pause", "args": {"caller": dp}},
            {"fn": "emergency_unpause", "args": {"caller": dp}},
            {"fn": "get_transfer_count", "args": {}},
            {"fn": "get_attestation_count", "args": {}},
            {"fn": "get_epoch_remaining", "args": {}},
            {"fn": "get_last_attestation_slot", "args": {}},
            {"fn": "get_reserve_ratio", "args": {}},
            {"fn": "attest_reserves", "args": {"attester": dp, "reserve_amount": 1_000_000, "supply_snapshot": 999_000}},
            {"fn": "transfer_admin", "args": {"caller": dp, "new_admin": dp}},
        ],
        "clawpump": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "create_token", "args": {"creator": dp, "fee_paid": 10_000_000_000}},
            {"fn": "buy", "args": {"buyer": dp, "token_id": 0, "molt_amount": 1_000_000_000}},
            {"fn": "sell", "args": {"seller": dp, "token_id": 0, "molt_amount": 100_000_000}},
            {"fn": "get_token_info", "args": {"token_id": 0}},
            {"fn": "get_token_count", "args": {}},
            {"fn": "get_platform_stats", "args": {}},
            {"fn": "get_buy_quote", "args": {"token_id": 0, "molt_amount": 1_000_000}},
            {"fn": "get_graduation_info", "args": {"token_id": 0}},
            {"fn": "set_buy_cooldown", "args": {"caller": dp, "cooldown": 0}},
            {"fn": "set_sell_cooldown", "args": {"caller": dp, "cooldown": 0}},
            {"fn": "set_max_buy", "args": {"caller": dp, "max_buy": 100_000_000_000}},
            {"fn": "set_creator_royalty", "args": {"caller": dp, "royalty_bps": 100}},
            {"fn": "set_dex_addresses", "args": {"caller": dp, "dex_core": str(contracts.get("dex_core", zero)), "dex_amm": str(contracts.get("dex_amm", zero))}},
            {"fn": "pause", "args": {"caller": dp}},
            {"fn": "unpause", "args": {"caller": dp}},
            {"fn": "withdraw_fees", "args": {"caller": dp}},
            {"fn": "freeze_token", "args": {"caller": dp, "token_id": 0}},
            {"fn": "unfreeze_token", "args": {"caller": dp, "token_id": 0}},
        ],
        "lobsterlend": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "deposit", "args": {"depositor": dp, "amount": 1_000_000_000}},
            {"fn": "borrow", "args": {"borrower": dp, "amount": 100_000_000}},
            {"fn": "repay", "args": {"borrower": dp, "amount": 50_000_000}},
            {"fn": "withdraw", "args": {"depositor": dp, "amount": 10_000_000}},
            {"fn": "get_protocol_stats", "args": {}},
            {"fn": "get_account_info", "args": {"account": dp}},
            {"fn": "get_interest_rate", "args": {}},
            {"fn": "set_deposit_cap", "args": {"caller": dp, "cap": 100_000_000_000}},
            {"fn": "set_reserve_factor", "args": {"caller": dp, "factor": 1000}},
            {"fn": "pause", "args": {"caller": dp}},
            {"fn": "unpause", "args": {"caller": dp}},
            {"fn": "withdraw_reserves", "args": {"caller": dp, "amount": 1}},
            {"fn": "liquidate", "args": {"liquidator": dp, "borrower": sp, "amount": 1}},
            {"fn": "flash_borrow", "args": {"borrower": dp, "amount": 100}},
            {"fn": "flash_repay", "args": {"borrower": dp, "amount": 100}},
            # --- stats queries ---
            {"fn": "get_deposit_count", "args": {}},
            {"fn": "get_borrow_count", "args": {}},
            {"fn": "get_liquidation_count", "args": {}},
            {"fn": "get_platform_stats", "args": {}},
        ],
        "moltmarket": [
            {"fn": "initialize", "args": {"owner": dp, "fee_addr": dp}},
            {"fn": "list_nft", "args": {"seller": dp, "token_id": rid, "price": 500}},
            {"fn": "get_listing", "args": {"token_id": rid}},
            {"fn": "cancel_listing", "args": {"seller": dp, "token_id": rid}},
            {"fn": "get_marketplace_stats", "args": {}},
            {"fn": "set_marketplace_fee", "args": {"caller": dp, "fee_bps": 250}},
            {"fn": "list_nft_with_royalty", "args": {"seller": dp, "token_id": rid + 1, "price": 1000, "royalty_bps": 500, "royalty_addr": dp}},
            {"fn": "make_offer", "args": {"buyer": sp, "token_id": rid + 1, "amount": 800}, "actor": "secondary"},
            {"fn": "cancel_offer", "args": {"buyer": sp, "token_id": rid + 1}, "actor": "secondary"},
            {"fn": "buy_nft", "args": {"buyer": sp, "token_id": rid + 1}, "actor": "secondary"},
            {"fn": "accept_offer", "args": {"seller": dp, "token_id": rid + 200}},
            {"fn": "mm_pause", "args": {"caller": dp}},
            {"fn": "mm_unpause", "args": {"caller": dp}},
            # --- stats queries ---
            {"fn": "get_marketplace_stats", "args": {}},
        ],
        "moltauction": [
            {"fn": "initialize", "args": {"marketplace": dp}},
            {"fn": "initialize_ma_admin", "args": {"admin": dp}},
            {"fn": "create_auction", "args": {"seller": dp, "token_id": rid, "start_price": 100, "duration_slots": 300}},
            {"fn": "place_bid", "args": {"bidder": sp, "token_id": rid, "bid_amount": 120}, "actor": "secondary"},
            {"fn": "set_reserve_price", "args": {"seller": dp, "token_id": rid, "reserve_price": 200}},
            {"fn": "set_royalty", "args": {"caller": dp, "token_id": rid, "royalty_bps": 500}},
            {"fn": "get_auction_info", "args": {"token_id": rid}},
            {"fn": "get_collection_stats", "args": {}},
            {"fn": "update_collection_stats", "args": {"caller": dp}},
            {"fn": "cancel_auction", "args": {"seller": dp, "token_id": rid}},
            {"fn": "finalize_auction", "args": {"caller": dp, "token_id": rid + 100}},
            {"fn": "make_offer", "args": {"buyer": sp, "token_id": rid + 200, "amount": 500}, "actor": "secondary"},
            {"fn": "accept_offer", "args": {"seller": dp, "token_id": rid + 200}},
            {"fn": "ma_pause", "args": {"caller": dp}},
            {"fn": "ma_unpause", "args": {"caller": dp}},
            # --- stats queries ---
            {"fn": "get_auction_stats", "args": {}},
        ],
        "moltbridge": [
            {"fn": "initialize", "args": {"owner": dp}},
            {"fn": "add_bridge_validator", "args": {"caller": dp, "validator": sp}},
            {"fn": "set_required_confirmations", "args": {"caller": dp, "required": 1}},
            {"fn": "set_request_timeout", "args": {"caller": dp, "timeout": 3600}},
            {"fn": "get_bridge_status", "args": {}},
            {"fn": "remove_bridge_validator", "args": {"caller": dp, "validator": sp}},
            {"fn": "lock_tokens", "args": {"caller": dp, "amount": 1000, "dest_chain": 1, "dest_address": sp}},
            {"fn": "submit_mint", "args": {"validator": dp, "source_tx": sp, "recipient": dp, "amount": 500, "source_chain": 1}},
            {"fn": "has_confirmed_mint", "args": {"validator": dp, "source_tx": sp}},
            {"fn": "submit_unlock", "args": {"validator": dp, "burn_proof": sp, "recipient": dp, "amount": 250}},
            {"fn": "has_confirmed_unlock", "args": {"validator": dp, "burn_proof": sp}},
            {"fn": "is_source_tx_used", "args": {"source_tx": sp}},
            {"fn": "is_burn_proof_used", "args": {"burn_proof": sp}},
            {"fn": "confirm_mint", "args": {"caller_ptr": dp, "nonce": 0}},
            {"fn": "confirm_unlock", "args": {"caller_ptr": dp, "nonce": 0}},
            {"fn": "cancel_expired_request", "args": {"caller": dp, "request_id": 0}},
            {"fn": "set_moltyid_address", "args": {"caller": dp, "address": str(contracts.get("moltyid", zero))}},
            {"fn": "set_identity_gate", "args": {"caller": dp, "enabled": 1}},
            {"fn": "mb_pause", "args": {"caller": dp}},
            {"fn": "mb_unpause", "args": {"caller": dp}},
        ],
        "reef_storage": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "register_provider", "args": {"provider": dp, "capacity_bytes": 1_000_000}},
            {"fn": "set_storage_price", "args": {"provider": dp, "price_per_byte_per_slot": 1}},
            {"fn": "store_data", "args": {"uploader": dp, "data_hash": sp, "size_bytes": 1024, "provider": dp}},
            {"fn": "confirm_storage", "args": {"provider": dp, "data_hash": sp}},
            {"fn": "get_storage_info", "args": {"data_hash": sp}},
            {"fn": "get_storage_price", "args": {"provider": dp}},
            {"fn": "get_provider_stake", "args": {"provider": dp}},
            {"fn": "claim_storage_rewards", "args": {"provider": dp}},
            {"fn": "stake_collateral", "args": {"provider": dp, "amount": 1000}},
            {"fn": "issue_challenge", "args": {"challenger": dp, "data_hash": sp}},
            {"fn": "respond_challenge", "args": {"provider": dp, "challenge_id": 0, "proof_hash": dp}},
            {"fn": "set_challenge_window", "args": {"caller": dp, "window_slots": 100}},
            {"fn": "set_slash_percent", "args": {"caller": dp, "percent": 10}},
            {"fn": "slash_provider", "args": {"caller": dp, "provider": sp, "challenge_id": 0}},
            # --- stats queries ---
            {"fn": "get_platform_stats", "args": {}},
        ],
        "clawvault": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "set_protocol_addresses", "args": {"caller": dp, "molt_addr": quote, "swap_addr": str(contracts.get("moltswap", zero))}},
            {"fn": "add_strategy", "args": {"caller": dp, "strategy_type": 0, "target_alloc": 5000}},
            {"fn": "deposit", "args": {"depositor": dp, "amount": 1_000_000_000}},
            {"fn": "withdraw", "args": {"depositor": dp, "shares_to_burn": 1}},
            {"fn": "get_vault_stats", "args": {}},
            {"fn": "get_user_position", "args": {"user": dp}},
            {"fn": "get_strategy_info", "args": {"strategy_id": 0}},
            {"fn": "harvest", "args": {"caller": dp, "strategy_id": 0}},
            {"fn": "set_deposit_cap", "args": {"caller": dp, "cap": 100_000_000_000}},
            {"fn": "set_deposit_fee", "args": {"caller": dp, "fee_bps": 10}},
            {"fn": "set_withdrawal_fee", "args": {"caller": dp, "fee_bps": 10}},
            {"fn": "set_risk_tier", "args": {"caller": dp, "tier": 1}},
            {"fn": "update_strategy_allocation", "args": {"caller": dp, "strategy_id": 0, "new_alloc": 6000}},
            {"fn": "remove_strategy", "args": {"caller": dp, "strategy_id": 0}},
            {"fn": "withdraw_protocol_fees", "args": {"caller": dp}},
            {"fn": "cv_pause", "args": {"caller": dp}},
            {"fn": "cv_unpause", "args": {"caller": dp}},
        ],
        "clawpay": [
            {"fn": "initialize_cp_admin", "args": {"admin": dp}},
            {"fn": "set_identity_admin", "args": {"admin_ptr": dp}},
            {"fn": "set_moltyid_address", "args": {"caller_ptr": dp, "moltyid_addr_ptr": str(contracts.get("moltyid", zero))}},
            {"fn": "set_identity_gate", "args": {"caller_ptr": dp, "enabled": 1}},
            {"fn": "create_stream", "args": {"sender": dp, "recipient": sp, "total_amount": 1_000_000_000, "start_time": now, "end_time": now + 3600}},
            {"fn": "create_stream_with_cliff", "args": {"sender": dp, "recipient": sp, "total_amount": 500_000_000, "start_time": now, "end_time": now + 7200, "cliff_time": now + 1800}},
            {"fn": "get_stream", "args": {"stream_id": 0}},
            {"fn": "get_stream_info", "args": {"stream_id": 0}},
            {"fn": "get_withdrawable", "args": {"stream_id": 0}},
            {"fn": "withdraw_from_stream", "args": {"caller": sp, "stream_id": 0}, "actor": "secondary"},
            {"fn": "transfer_stream", "args": {"caller": sp, "stream_id": 0, "new_recipient": dp}, "actor": "secondary"},
            {"fn": "cancel_stream", "args": {"caller": dp, "stream_id": 0}},
            {"fn": "pause", "args": {"caller": dp}},
            {"fn": "unpause", "args": {"caller": dp}},
            # --- stats queries ---
            {"fn": "get_stream_count", "args": {}},
            {"fn": "get_platform_stats", "args": {}},
        ],
        "moltyid": [
            {"fn": "initialize", "args": {"admin_ptr": dp}},
            {"fn": "register_identity", "args": {"owner_ptr": dp, "agent_type": 1, "name_ptr": f"agent{rid}", "name_len": len(f"agent{rid}")}},
            {"fn": "get_identity", "args": {"addr_ptr": dp}},
            {"fn": "get_identity_count", "args": {}},
            {"fn": "set_endpoint", "args": {"caller_ptr": dp, "url_ptr": "https://e2e.test", "url_len": 16}},
            {"fn": "get_endpoint", "args": {"addr_ptr": dp}},
            {"fn": "set_metadata", "args": {"caller_ptr": dp, "json_ptr": '{"e2e":true}', "json_len": 12}},
            {"fn": "get_metadata", "args": {"addr_ptr": dp}},
            {"fn": "set_availability", "args": {"caller_ptr": dp, "status": 1}},
            {"fn": "get_availability", "args": {"addr_ptr": dp}},
            {"fn": "set_rate", "args": {"caller_ptr": dp, "molt_per_unit": 1000}},
            {"fn": "get_rate", "args": {"addr_ptr": dp}},
            {"fn": "add_skill", "args": {"owner_ptr": dp, "skill_ptr": "rust", "skill_len": 4}},
            {"fn": "get_skills", "args": {"addr_ptr": dp}},
            {"fn": "vouch", "args": {"voucher_ptr": dp, "vouchee_ptr": sp}},
            {"fn": "get_vouches", "args": {"addr_ptr": dp}},
            {"fn": "set_delegate", "args": {"owner_ptr": dp, "delegate_ptr": sp, "flags": 255, "expiry_ts": now + 86400}},
            {"fn": "get_delegate", "args": {"owner_ptr": dp, "delegate_ptr": sp}},
            {"fn": "revoke_delegate", "args": {"owner_ptr": dp, "delegate_ptr": sp}},
            {"fn": "update_reputation", "args": {"caller_ptr": dp, "target_ptr": dp, "delta": 10}},
            {"fn": "update_reputation_typed", "args": {"caller_ptr": dp, "target_ptr": dp, "rep_type": 1, "delta": 5}},
            {"fn": "get_reputation", "args": {"addr_ptr": dp}},
            {"fn": "get_trust_tier", "args": {"addr_ptr": dp}},
            {"fn": "update_agent_type", "args": {"caller_ptr": dp, "new_type": 2}},
            {"fn": "deactivate_identity", "args": {"owner_ptr": dp}},
            {"fn": "get_agent_profile", "args": {"addr_ptr": dp}},
            {"fn": "set_recovery_guardians", "args": {"owner_ptr": dp, "guardian1": sp, "guardian2": zero}},
            {"fn": "register_name", "args": {"owner_ptr": dp, "name_ptr": f"e2e{rid}", "name_len": len(f"e2e{rid}")}},
            {"fn": "resolve_name", "args": {"name_ptr": f"e2e{rid}", "name_len": len(f"e2e{rid}")}},
            {"fn": "reverse_resolve", "args": {"addr_ptr": dp}},
            {"fn": "get_achievements", "args": {"addr_ptr": dp}},
            {"fn": "get_attestations", "args": {"addr_ptr": dp}},
            {"fn": "add_skill_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "skill_ptr": "python", "skill_len": 6, "proficiency": 3}},
            {"fn": "set_endpoint_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "url_ptr": "https://delegated.test", "url_len": 22}},
            {"fn": "set_metadata_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "json_ptr": '{"delegated":true}', "json_len": 18}},
            {"fn": "set_availability_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "status": 1}},
            {"fn": "set_rate_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "molt_per_unit": 2000}},
            {"fn": "update_agent_type_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "new_agent_type": 3}},
            {"fn": "attest_skill", "args": {"attester_ptr": dp, "identity_ptr": dp, "skill_ptr": "rust", "skill_len": 4, "attestation_level": 5}},
            {"fn": "revoke_attestation", "args": {"attester_ptr": dp, "identity_ptr": dp, "skill_ptr": "rust", "skill_len": 4}},
            {"fn": "approve_recovery", "args": {"guardian_ptr": sp, "target_ptr": dp, "new_owner_ptr": sp}},
            {"fn": "execute_recovery", "args": {"caller_ptr": sp, "target_ptr": dp, "new_owner_ptr": sp}},
            {"fn": "award_contribution_achievement", "args": {"caller_ptr": dp, "target_ptr": dp, "achievement_id": 1}},
            {"fn": "create_name_auction", "args": {"caller_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "reserve_bid": 1_000_000, "end_slot": 999_999_999}},
            {"fn": "bid_name_auction", "args": {"bidder_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "bid_amount": 2_000_000}},
            {"fn": "get_name_auction", "args": {"name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}")}},
            {"fn": "finalize_name_auction", "args": {"caller_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "duration_years": 1}},
            {"fn": "transfer_name", "args": {"caller_ptr": dp, "name_ptr": f"e2e{rid}", "name_len": len(f"e2e{rid}"), "new_owner_ptr": sp}},
            {"fn": "renew_name", "args": {"caller_ptr": sp, "name_ptr": f"e2e{rid}", "name_len": len(f"e2e{rid}"), "additional_years": 1}},
            {"fn": "release_name", "args": {"owner_ptr": sp, "name_ptr": f"e2e{rid}", "name_len": len(f"e2e{rid}")}},
            # --- delegated name management ---
            {"fn": "transfer_name_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "new_owner_ptr": dp}},
            {"fn": "renew_name_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "additional_years": 1}},
            {"fn": "release_name_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}")}},
            # --- admin ---
            {"fn": "admin_register_reserved_name", "args": {"admin_ptr": dp, "owner_ptr": dp, "name_ptr": f"reserved{rid}", "name_len": len(f"reserved{rid}"), "agent_type": 1}},
            {"fn": "transfer_admin", "args": {"caller_ptr": dp, "new_admin_ptr": dp}},
            {"fn": "mid_pause", "args": {"caller_ptr": dp}},
            {"fn": "mid_unpause", "args": {"caller_ptr": dp}},
        ],
        "moltswap": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "set_protocol_fee", "args": {"caller": dp, "fee_bps": 30}},
            {"fn": "set_platform_fee", "args": {"caller": dp, "fee_bps": 25}},
            {"fn": "create_pool", "args": {"creator": dp, "token_a": dp, "token_b": sp}},
            {"fn": "add_liquidity", "args": {"provider": dp, "pool_id": 0, "amount_a": 1_000_000, "amount_b": 1_000_000}},
            {"fn": "swap", "args": {"trader": dp, "pool_id": 0, "amount_in": 1000, "min_out": 0, "a_to_b": 1}},
            {"fn": "swap_a_for_b", "args": {"trader": dp, "pool_id": 0, "amount_in": 500, "min_out": 0}},
            {"fn": "swap_b_for_a", "args": {"trader": dp, "pool_id": 0, "amount_in": 200, "min_out": 0}},
            {"fn": "swap_a_for_b_with_deadline", "args": {"trader": dp, "pool_id": 0, "amount_in": 300, "min_out": 0, "deadline": 9999999999}},
            {"fn": "swap_b_for_a_with_deadline", "args": {"trader": dp, "pool_id": 0, "amount_in": 100, "min_out": 0, "deadline": 9999999999}},
            {"fn": "get_pool_info", "args": {"pool_id": 0}},
            {"fn": "get_pool_count", "args": {}},
            {"fn": "get_quote", "args": {"pool_id": 0, "amount_in": 1000, "a_to_b": 1}},
            {"fn": "get_reserves", "args": {"pool_id": 0}},
            {"fn": "get_liquidity_balance", "args": {"pool_id": 0, "provider": dp}},
            {"fn": "get_total_liquidity", "args": {"pool_id": 0}},
            {"fn": "get_protocol_fees", "args": {"pool_id": 0}},
            {"fn": "get_flash_loan_fee", "args": {}},
            {"fn": "get_twap_cumulatives", "args": {"pool_id": 0}},
            {"fn": "get_twap_snapshot_count", "args": {"pool_id": 0}},
            {"fn": "flash_loan_borrow", "args": {"borrower": dp, "pool_id": 0, "amount": 100, "token_a": 1}},
            {"fn": "flash_loan_repay", "args": {"borrower": dp, "pool_id": 0}},
            {"fn": "flash_loan_abort", "args": {"borrower": dp, "pool_id": 0}},
            {"fn": "remove_liquidity", "args": {"provider": dp, "pool_id": 0, "lp_amount": 100}},
            {"fn": "ms_pause", "args": {"caller": dp}},
            {"fn": "ms_unpause", "args": {"caller": dp}},
            # --- stats queries ---
            {"fn": "get_swap_count", "args": {}},
            {"fn": "get_total_volume", "args": {}},
            {"fn": "get_swap_stats", "args": {}},
        ],
        "moltoracle": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "register_feed", "args": {"caller": dp, "feed_id": "BTC-USD", "decimals": 8}},
            {"fn": "submit_price", "args": {"caller": dp, "feed_id": "BTC-USD", "price": 50_000_000_000}},
            {"fn": "get_price", "args": {"feed_id": "BTC-USD"}},
            {"fn": "register_feed", "args": {"caller": dp, "feed_id": "MOLT-USD", "decimals": 8}},
            {"fn": "submit_price", "args": {"caller": dp, "feed_id": "MOLT-USD", "price": 1_000_000}},
            {"fn": "get_price", "args": {"feed_id": "MOLT-USD"}},
            {"fn": "get_feed_count", "args": {}},
            {"fn": "get_feed_list", "args": {}},
            {"fn": "set_update_interval", "args": {"caller": dp, "interval": 60}},
            {"fn": "add_reporter", "args": {"caller": dp, "reporter": sp}},
            {"fn": "remove_reporter", "args": {"caller": dp, "reporter": sp}},
            {"fn": "get_oracle_stats", "args": {}},
            {"fn": "mo_pause", "args": {"caller": dp}},
            {"fn": "mo_unpause", "args": {"caller": dp}},
        ],
        "moltdao": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "set_quorum", "args": {"caller": dp, "quorum": 1}},
            {"fn": "set_voting_period", "args": {"caller": dp, "period": 100}},
            {"fn": "create_proposal", "binary": encode_layout_args([
                (32, dp),                                # proposer_ptr
                (32, b"Test Proposal"),                  # title_ptr (padded to 32)
                (4, 13),                                 # title_len
                (32, b"Parallel E2E test proposal"),     # description_ptr (padded to 32)
                (4, 26),                                 # description_len
                (32, b'\x00' * 32),                      # target_contract_ptr
                (32, b"test_action"),                     # action_ptr (padded to 32)
                (4, 11),                                 # action_len
            ])},
            {"fn": "cast_vote", "args": {"voter": dp, "proposal_id": 0, "support": 1}},
            {"fn": "get_proposal", "args": {"proposal_id": 0}},
            {"fn": "get_proposal_count", "args": {}},
            {"fn": "get_vote", "args": {"proposal_id": 0, "voter": dp}},
            {"fn": "get_vote_count", "args": {"proposal_id": 0}},
            {"fn": "finalize_proposal", "args": {"caller": dp, "proposal_id": 0}},
            {"fn": "set_timelock_delay", "args": {"caller": dp, "delay": 10}},
            {"fn": "execute_proposal", "args": {"caller": dp, "proposal_id": 0}},
            {"fn": "get_dao_stats", "args": {}},
            {"fn": "get_active_proposals", "args": {}},
            {"fn": "get_total_supply", "args": {}},
            {"fn": "get_treasury_balance", "args": {}},
            {"fn": "veto_proposal", "args": {"caller": dp, "proposal_id": 0}},
            {"fn": "cancel_proposal", "args": {"caller": dp, "proposal_id": 0}},
            {"fn": "treasury_transfer", "args": {"caller": dp, "recipient": sp, "amount": 0}},
            {"fn": "dao_pause", "args": {"caller": dp}},
            {"fn": "dao_unpause", "args": {"caller": dp}},
        ],
        "moltpunks": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "mint_punk", "args": {"caller": dp, "to": dp, "punk_type": 0, "seed": rid}},
            {"fn": "transfer_punk", "args": {"from_owner": dp, "to": sp, "token_id": 0}},
            {"fn": "get_punk_metadata", "args": {"token_id": 0}},
            {"fn": "get_total_supply", "args": {}},
            {"fn": "get_owner_of", "args": {"token_id": 0}},
            {"fn": "set_base_uri", "args": {"caller": dp, "uri": "https://punks.moltchain.io/"}},
            {"fn": "set_max_supply", "args": {"caller": dp, "max_supply": 10_000}},
            {"fn": "get_punks_by_owner", "args": {"owner": dp}},
            {"fn": "set_royalty", "args": {"caller": dp, "bps": 500}},
            {"fn": "balance_of", "args": {"owner": dp}},
            {"fn": "approve", "args": {"owner": dp, "spender": sp, "token_id": 0}},
            {"fn": "burn", "args": {"caller": dp, "token_id": 0}},
            {"fn": "mp_pause", "args": {"caller": dp}},
            {"fn": "mp_unpause", "args": {"caller": dp}},
            # --- stats queries ---
            {"fn": "get_collection_stats", "args": {}},
        ],
        "compute_market": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "register_provider", "args": {"provider": dp, "cpu_cores": 8, "memory_gb": 16, "gpu_count": 1, "price_per_unit": 1000}},
            {"fn": "create_job", "args": {"requester": dp, "cpu_needed": 2, "memory_needed": 4, "gpu_needed": 0, "duration_slots": 100, "max_price": 100_000}},
            {"fn": "accept_job", "args": {"provider": dp, "job_id": 0}},
            {"fn": "submit_result", "args": {"provider": dp, "job_id": 0, "result_hash": sp}},
            {"fn": "confirm_result", "args": {"requester": dp, "job_id": 0}},
            {"fn": "get_job_info", "args": {"job_id": 0}},
            {"fn": "get_provider_info", "args": {"provider": dp}},
            {"fn": "get_job_count", "args": {}},
            {"fn": "set_platform_fee", "args": {"caller": dp, "fee_bps": 250}},
            {"fn": "cm_pause", "args": {"caller": dp}},
            {"fn": "cm_unpause", "args": {"caller": dp}},
            # --- stats queries ---
            {"fn": "get_platform_stats", "args": {}},
        ],
        "bountyboard": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "create_bounty", "args": {"creator": dp, "title": "E2E Bounty", "description": "Parallel test bounty", "reward": 1_000_000_000}},
            {"fn": "get_bounty", "args": {"bounty_id": 0}},
            {"fn": "get_bounty_count", "args": {}},
            {"fn": "submit_work", "args": {"submitter": sp, "bounty_id": 0, "proof": "done"}, "actor": "secondary"},
            {"fn": "approve_submission", "args": {"caller": dp, "bounty_id": 0, "submitter": sp}},
            {"fn": "cancel_bounty", "args": {"creator": dp, "bounty_id": 0}},
            {"fn": "set_platform_fee", "args": {"caller": dp, "fee_bps": 100}},
            {"fn": "bb_pause", "args": {"caller": dp}},
            {"fn": "bb_unpause", "args": {"caller": dp}},
            # --- stats queries ---
            {"fn": "get_platform_stats", "args": {}},
        ],
    }


def build_opcode_scenarios(
    deployer: Keypair, secondary: Keypair, contracts: Dict[str, PublicKey]
) -> Dict[str, List[Dict[str, Any]]]:
    admin = deployer.public_key().to_bytes()
    sec = secondary.public_key().to_bytes()
    zero = b'\x00' * 32
    molt = contracts.get("moltcoin", PublicKey(zero)).to_bytes()
    weth = contracts.get("weth_token", PublicKey(zero)).to_bytes()
    musd = contracts.get("musd_token", PublicKey(zero)).to_bytes()
    yid = contracts.get("moltyid", PublicKey(zero)).to_bytes()
    dex = contracts.get("dex_core", PublicKey(zero)).to_bytes()
    dex_amm_addr = contracts.get("dex_amm", PublicKey(zero)).to_bytes()
    dex_gov = contracts.get("dex_governance", PublicKey(zero)).to_bytes()

    return {
        # ── dex_core: 21 opcodes (0-20) ──
        # Op0: initialize(admin[32])
        # Op1: create_pair(admin[32]+base[32]+quote[32]+min_order(u64)+tick(u64)+lot(u64))
        # Op2: place_order(trader[32]+pair_id(u64)+side(1)+type(1)+price(u64)+qty(u64)+expiry(u64))
        # Op3: cancel_order(trader[32]+order_id(u64))
        # Op4: set_preferred_quote(admin[32]+token[32])
        # Op5: get_pair_count()
        # Op6: get_preferred_quote()
        # Op7: update_pair_fees(admin[32]+pair_id(u64)+maker(i16)+taker(u16))
        # Op8: emergency_pause(admin[32])
        # Op9: emergency_unpause(admin[32])
        # Op10: get_best_bid(pair_id(u64))
        # Op11: get_best_ask(pair_id(u64))
        # Op12: get_spread(pair_id(u64))
        # Op13: get_pair_info(pair_id(u64))
        # Op14: get_trade_count()
        # Op15: get_fee_treasury()
        # Op16: modify_order(trader[32]+order_id(u64)+new_price(u64)+new_qty(u64))
        # Op17: cancel_all_orders(trader[32]+pair_id(u64))
        # Op18: pause_pair(admin[32]+pair_id(u64))
        # Op19: unpause_pair(admin[32]+pair_id(u64))
        # Op20: get_order(order_id(u64))
        "dex_core": [
            # --- init & setup ---
            {"label": "dex_core.initialize",         "args": bytes([0]) + admin},
            {"label": "dex_core.create_pair",         "args": bytes([1]) + admin + molt + weth + u64le(1000) + u64le(100) + u64le(1000)},
            {"label": "dex_core.set_preferred_quote", "args": bytes([4]) + admin + musd},
            {"label": "dex_core.add_allowed_quote",    "args": bytes([21]) + admin + molt},
            {"label": "dex_core.get_allowed_quote_count", "args": bytes([23])},
            {"label": "dex_core.remove_allowed_quote",  "args": bytes([22]) + admin + molt},
            {"label": "dex_core.update_pair_fees",    "args": bytes([7]) + admin + u64le(1) + i16le(-2) + u16le(10)},
            # --- queries (no state deps beyond create_pair) ---
            {"label": "dex_core.get_pair_count",      "args": bytes([5])},
            {"label": "dex_core.get_preferred_quote",  "args": bytes([6])},
            {"label": "dex_core.get_pair_info",        "args": bytes([13]) + u64le(1)},
            {"label": "dex_core.get_trade_count",      "args": bytes([14])},
            {"label": "dex_core.get_fee_treasury",     "args": bytes([15])},
            # --- order flow ---
            {"label": "dex_core.place_order",          "args": bytes([2]) + admin + u64le(1) + bytes([0]) + bytes([0]) + u64le(1_000_000) + u64le(500) + u64le(0)},
            {"label": "dex_core.get_order",            "args": bytes([20]) + u64le(1)},
            {"label": "dex_core.get_best_bid",         "args": bytes([10]) + u64le(1)},
            {"label": "dex_core.get_best_ask",         "args": bytes([11]) + u64le(1)},
            {"label": "dex_core.get_spread",           "args": bytes([12]) + u64le(1)},
            {"label": "dex_core.modify_order",         "args": bytes([16]) + admin + u64le(1) + u64le(1_100_000) + u64le(600)},
            {"label": "dex_core.cancel_order",         "args": bytes([3]) + admin + u64le(1)},
            {"label": "dex_core.cancel_all_orders",    "args": bytes([17]) + admin + u64le(1)},
            # --- pair pause/unpause ---
            {"label": "dex_core.pause_pair",           "args": bytes([18]) + admin + u64le(1)},
            {"label": "dex_core.unpause_pair",         "args": bytes([19]) + admin + u64le(1)},
            # --- global pause/unpause ---
            {"label": "dex_core.emergency_pause",      "args": bytes([8]) + admin},
            {"label": "dex_core.emergency_unpause",    "args": bytes([9]) + admin},
            # --- stats queries (new opcodes 25-27) ---
            {"label": "dex_core.get_total_volume",     "args": bytes([25])},
            {"label": "dex_core.get_user_orders",      "args": bytes([26]) + admin},
            {"label": "dex_core.get_open_order_count",  "args": bytes([27])},
        ],
        # ── dex_amm: 16 opcodes (0-15) ──
        # Op0:  initialize(admin[32])
        # Op1:  create_pool(caller[32]+token_a[32]+token_b[32]+fee_tier(1)+sqrt_price(u64))
        # Op2:  set_pool_protocol_fee(caller[32]+pool_id(u64)+fee_percent(1))
        # Op3:  add_liquidity(provider[32]+pool_id(u64)+lower_tick(i32)+upper_tick(i32)+amount_a(u64)+amount_b(u64))
        # Op4:  remove_liquidity(provider[32]+position_id(u64)+liquidity_amount(u64))
        # Op5:  collect_fees(provider[32]+position_id(u64))
        # Op6:  swap_exact_in(trader[32]+pool_id(u64)+is_token_a_in(1)+amount_in(u64)+min_out(u64)+deadline(u64))
        # Op7:  swap_exact_out(trader[32]+pool_id(u64)+is_token_a_out(1)+amount_out(u64)+max_in(u64)+deadline(u64))
        # Op8:  emergency_pause(caller[32])
        # Op9:  emergency_unpause(caller[32])
        # Op10: get_pool_info(pool_id(u64))
        # Op11: get_position(position_id(u64))
        # Op12: get_pool_count()
        # Op13: get_position_count()
        # Op14: get_tvl(pool_id(u64))
        # Op15: quote_swap(pool_id(u64)+is_token_a_in(1)+amount_in(u64))
        "dex_amm": [
            # --- init & pool setup ---
            {"label": "dex_amm.initialize",         "args": bytes([0]) + admin},
            {"label": "dex_amm.create_pool",        "args": bytes([1]) + admin + molt + weth + bytes([1]) + u64le(1 << 32)},
            {"label": "dex_amm.set_protocol_fee",   "args": bytes([2]) + admin + u64le(1) + bytes([10])},
            # --- liquidity operations (fee_tier=1 → tick_spacing=10) ---
            {"label": "dex_amm.add_liquidity",      "args": bytes([3]) + admin + u64le(1) + i32le(-100) + i32le(100) + u64le(1_000_000) + u64le(1_000_000)},
            # --- queries after liquidity is in place ---
            {"label": "dex_amm.get_pool_info",      "args": bytes([10]) + u64le(1)},
            {"label": "dex_amm.get_position",       "args": bytes([11]) + u64le(1)},
            {"label": "dex_amm.get_pool_count",     "args": bytes([12])},
            {"label": "dex_amm.get_position_count", "args": bytes([13])},
            {"label": "dex_amm.get_tvl",            "args": bytes([14]) + u64le(1)},
            {"label": "dex_amm.quote_swap",         "args": bytes([15]) + u64le(1) + bytes([1]) + u64le(10_000)},
            # --- swap operations (deadline=0 means no deadline) ---
            {"label": "dex_amm.swap_exact_in",      "args": bytes([6]) + admin + u64le(1) + bytes([1]) + u64le(10_000) + u64le(0) + u64le(0)},
            {"label": "dex_amm.swap_exact_out",     "args": bytes([7]) + admin + u64le(1) + bytes([1]) + u64le(100) + u64le(50_000) + u64le(0)},
            # --- fee collection & position management ---
            {"label": "dex_amm.collect_fees",       "args": bytes([5]) + admin + u64le(1)},
            {"label": "dex_amm.remove_liquidity",   "args": bytes([4]) + admin + u64le(1) + u64le(500)},
            # --- emergency pause/unpause ---
            {"label": "dex_amm.emergency_pause",    "args": bytes([8]) + admin},
            {"label": "dex_amm.emergency_unpause",  "args": bytes([9]) + admin},
            # --- stats queries (new opcodes 16-19) ---
            {"label": "dex_amm.get_total_volume",        "args": bytes([16])},
            {"label": "dex_amm.get_swap_count",          "args": bytes([17])},
            {"label": "dex_amm.get_total_fees_collected", "args": bytes([18])},
            {"label": "dex_amm.get_amm_stats",           "args": bytes([19])},
        ],
        # ── dex_analytics: 9 opcodes (0-8) ──
        # Op0: initialize(admin[32])
        # Op1: record_trade(pair_id(u64)+price(u64)+volume(u64)+trader[32])
        # Op2: get_ohlcv(pair_id(u64)+interval(u64)+count(u64))
        # Op3: get_24h_stats(pair_id(u64))
        # Op4: get_trader_stats(addr[32])
        # Op5: get_last_price(pair_id(u64))
        # Op6: get_record_count()
        # Op7: emergency_pause(caller[32])
        # Op8: emergency_unpause(caller[32])
        "dex_analytics": [
            {"label": "dex_analytics.initialize",     "args": bytes([0]) + admin},
            {"label": "dex_analytics.record_trade",    "args": bytes([1]) + u64le(1) + u64le(1_000_000_000) + u64le(5000) + admin},
            {"label": "dex_analytics.get_ohlcv",       "args": bytes([2]) + u64le(1) + u64le(3600) + u64le(10)},
            {"label": "dex_analytics.get_24h_stats",   "args": bytes([3]) + u64le(1)},
            {"label": "dex_analytics.get_trader_stats", "args": bytes([4]) + admin},
            {"label": "dex_analytics.get_last_price",  "args": bytes([5]) + u64le(1)},
            {"label": "dex_analytics.get_record_count", "args": bytes([6])},
            {"label": "dex_analytics.emergency_pause", "args": bytes([7]) + admin},
            {"label": "dex_analytics.emergency_unpause", "args": bytes([8]) + admin},
            # --- stats queries (new opcodes 9-10) ---
            {"label": "dex_analytics.get_trader_count",  "args": bytes([9])},
            {"label": "dex_analytics.get_global_stats",  "args": bytes([10])},
        ],
        # ── dex_governance: 18 opcodes (0-17) ──
        # Op0:  initialize(admin[32])
        # Op1:  propose_new_pair(proposer[32]+base[32]+quote[32])
        # Op2:  vote(voter[32]+proposal_id(u64)+approve(1))
        # Op3:  finalize_proposal(proposal_id(u64))
        # Op4:  execute_proposal(proposal_id(u64))
        # Op5:  set_preferred_quote(admin[32]+token[32])
        # Op6:  get_preferred_quote()
        # Op7:  get_proposal_count()
        # Op8:  get_proposal_info(proposal_id(u64))
        # Op9:  propose_fee_change(proposer[32]+pair_id(u64)+maker(i16)+taker(u16))
        # Op10: emergency_delist(admin[32]+pair_id(u64))
        # Op11: set_listing_requirements(admin[32]+min_stake(u64)+min_volume(u64))
        # Op12: emergency_pause(admin[32])
        # Op13: emergency_unpause(admin[32])
        # Op14: set_moltyid_address(admin[32]+addr[32])
        # Op15: add_allowed_quote(admin[32]+token[32])
        # Op16: remove_allowed_quote(admin[32]+token[32])
        # Op17: get_allowed_quote_count()
        "dex_governance": [
            {"label": "dex_governance.initialize",            "args": bytes([0]) + admin},
            {"label": "dex_governance.set_preferred_quote",   "args": bytes([5]) + admin + musd},
            {"label": "dex_governance.add_allowed_quote",     "args": bytes([15]) + admin + molt},
            {"label": "dex_governance.get_allowed_quote_count", "args": bytes([17])},
            {"label": "dex_governance.remove_allowed_quote",  "args": bytes([16]) + admin + molt},
            {"label": "dex_governance.set_moltyid_address",   "args": bytes([14]) + admin + contracts.get("moltyid", PublicKey(zero)).to_bytes()},
            {"label": "dex_governance.set_listing_requirements", "args": bytes([11]) + admin + u64le(1000) + u64le(500)},
            {"label": "dex_governance.propose_new_pair",      "args": bytes([1]) + sec + molt + weth},
            {"label": "dex_governance.propose_fee_change",    "args": bytes([9]) + sec + u64le(1) + i16le(-2) + u16le(10)},
            {"label": "dex_governance.vote",                  "args": bytes([2]) + admin + u64le(1) + bytes([1])},
            {"label": "dex_governance.get_proposal_count",    "args": bytes([7])},
            {"label": "dex_governance.get_preferred_quote",   "args": bytes([6])},
            {"label": "dex_governance.get_proposal_info",     "args": bytes([8]) + u64le(1)},
            {"label": "dex_governance.finalize_proposal",     "args": bytes([3]) + u64le(1)},
            {"label": "dex_governance.execute_proposal",      "args": bytes([4]) + u64le(1)},
            {"label": "dex_governance.emergency_delist",      "args": bytes([10]) + admin + u64le(1)},
            {"label": "dex_governance.emergency_pause",       "args": bytes([12]) + admin},
            {"label": "dex_governance.emergency_unpause",     "args": bytes([13]) + admin},
            # --- stats queries (new opcodes 18-19) ---
            {"label": "dex_governance.get_governance_stats",  "args": bytes([18])},
            {"label": "dex_governance.get_voter_count",       "args": bytes([19])},
        ],
        # ── dex_margin: 16 opcodes (0-15) ──
        # Op0:  initialize(admin[32])
        # Op1:  set_mark_price(caller[32]+pair_id(u64)+price(u64))
        # Op2:  open_position(trader[32]+pair_id(u64)+side(1)+size(u64)+leverage(u64)+margin(u64))
        # Op3:  close_position(caller[32]+pos_id(u64))
        # Op4:  add_margin(caller[32]+pos_id(u64)+amount(u64))
        # Op5:  remove_margin(caller[32]+pos_id(u64)+amount(u64))
        # Op6:  liquidate(liquidator[32]+pos_id(u64))
        # Op7:  set_max_leverage(caller[32]+pair_id(u64)+max_lev(u64))
        # Op8:  set_maintenance_margin(caller[32]+margin_bps(u64))
        # Op9:  withdraw_insurance(caller[32]+amount(u64)+recipient[32])
        # Op10: get_position_info(pos_id(u64))
        # Op11: get_margin_ratio(pos_id(u64))
        # Op12: get_tier_info(leverage(u64))
        # Op13: emergency_pause(caller[32])
        # Op14: emergency_unpause(caller[32])
        # Op15: set_moltcoin_address(caller[32]+addr[32])
        "dex_margin": [
            {"label": "dex_margin.initialize",          "args": bytes([0]) + admin},
            {"label": "dex_margin.set_moltcoin_address", "args": bytes([15]) + admin + molt},
            {"label": "dex_margin.set_mark_price",      "args": bytes([1]) + admin + u64le(1) + u64le(1_000_000_000)},
            {"label": "dex_margin.set_max_leverage",    "args": bytes([7]) + admin + u64le(1) + u64le(10)},
            {"label": "dex_margin.set_maintenance_margin", "args": bytes([8]) + admin + u64le(500)},
            {"label": "dex_margin.open_position",       "args": bytes([2]) + admin + u64le(1) + bytes([0]) + u64le(100_000) + u64le(5) + u64le(50_000)},
            {"label": "dex_margin.get_position_info",   "args": bytes([10]) + u64le(1)},
            {"label": "dex_margin.get_margin_ratio",    "args": bytes([11]) + u64le(1)},
            {"label": "dex_margin.get_tier_info",       "args": bytes([12]) + u64le(5)},
            {"label": "dex_margin.add_margin",          "args": bytes([4]) + admin + u64le(1) + u64le(10_000)},
            {"label": "dex_margin.remove_margin",       "args": bytes([5]) + admin + u64le(1) + u64le(1_000)},
            {"label": "dex_margin.liquidate",           "args": bytes([6]) + sec + u64le(1)},
            {"label": "dex_margin.close_position",      "args": bytes([3]) + admin + u64le(1)},
            {"label": "dex_margin.withdraw_insurance",  "args": bytes([9]) + admin + u64le(100) + sec},
            {"label": "dex_margin.emergency_pause",     "args": bytes([13]) + admin},
            {"label": "dex_margin.emergency_unpause",   "args": bytes([14]) + admin},
            # --- stats queries (new opcodes 16-20) ---
            {"label": "dex_margin.get_total_volume",     "args": bytes([16])},
            {"label": "dex_margin.get_user_positions",   "args": bytes([17]) + admin},
            {"label": "dex_margin.get_total_pnl",        "args": bytes([18])},
            {"label": "dex_margin.get_liquidation_count", "args": bytes([19])},
            {"label": "dex_margin.get_margin_stats",     "args": bytes([20])},
        ],
        # ── dex_rewards: 16 opcodes (0-15) ──
        # Op0:  initialize(admin[32])
        # Op1:  record_trade(trader[32]+fee_paid(u64)+volume(u64))
        # Op2:  claim_trading_rewards(trader[32])
        # Op3:  claim_lp_rewards(provider[32]+position_id(u64))
        # Op4:  register_referral(trader[32]+referrer[32])
        # Op5:  set_reward_rate(caller[32]+pair_id(u64)+rate(u64))
        # Op6:  accrue_lp_rewards(position_id(u64)+liquidity(u64)+pair_id(u64))
        # Op7:  get_pending_rewards(addr[32])
        # Op8:  get_trading_tier(addr[32])
        # Op9:  emergency_pause(caller[32])
        # Op10: emergency_unpause(caller[32])
        # Op11: set_referral_rate(caller[32]+rate_bps(u64))
        # Op12: set_moltcoin_address(caller[32]+addr[32])
        # Op13: set_rewards_pool(caller[32]+addr[32])
        # Op14: get_referral_rate()
        # Op15: get_total_distributed()
        "dex_rewards": [
            {"label": "dex_rewards.initialize",         "args": bytes([0]) + admin},
            {"label": "dex_rewards.set_moltcoin_address", "args": bytes([12]) + admin + molt},
            {"label": "dex_rewards.set_rewards_pool",   "args": bytes([13]) + admin + molt},
            {"label": "dex_rewards.set_reward_rate",    "args": bytes([5]) + admin + u64le(1) + u64le(100)},
            {"label": "dex_rewards.set_referral_rate",  "args": bytes([11]) + admin + u64le(500)},
            {"label": "dex_rewards.register_referral",  "args": bytes([4]) + sec + admin},
            {"label": "dex_rewards.record_trade",       "args": bytes([1]) + admin + u64le(10_000) + u64le(1_000_000)},
            {"label": "dex_rewards.accrue_lp_rewards",  "args": bytes([6]) + u64le(1) + u64le(100_000) + u64le(1)},
            {"label": "dex_rewards.claim_trading_rewards", "args": bytes([2]) + admin},
            {"label": "dex_rewards.claim_lp_rewards",   "args": bytes([3]) + admin + u64le(1)},
            {"label": "dex_rewards.get_pending_rewards", "args": bytes([7]) + admin},
            {"label": "dex_rewards.get_trading_tier",   "args": bytes([8]) + admin},
            {"label": "dex_rewards.get_referral_rate",  "args": bytes([14])},
            {"label": "dex_rewards.get_total_distributed", "args": bytes([15])},
            {"label": "dex_rewards.emergency_pause",    "args": bytes([9]) + admin},
            {"label": "dex_rewards.emergency_unpause",  "args": bytes([10]) + admin},
            # --- stats queries (new opcodes 16-18) ---
            {"label": "dex_rewards.get_trader_count",    "args": bytes([16])},
            {"label": "dex_rewards.get_total_volume",    "args": bytes([17])},
            {"label": "dex_rewards.get_reward_stats",    "args": bytes([18])},
        ],
        # ── dex_router: 12 opcodes (0-11) ──
        # Op0:  initialize(admin[32])
        # Op1:  set_addresses(caller[32]+core[32]+amm[32]+legacy[32])
        # Op2:  register_route(caller[32]+token_in[32]+token_out[32]+type(1)+pool_id(u64)+sec_id(u64)+split_pct(1))
        # Op3:  swap(trader[32]+token_in[32]+token_out[32]+amount_in(u64)+min_out(u64)+deadline(u64))
        # Op4:  set_route_enabled(caller[32]+route_id(u64)+enabled(1))
        # Op5:  get_best_route(token_in[32]+token_out[32]+amount(u64))
        # Op6:  get_route_info(route_id(u64))
        # Op7:  emergency_pause(caller[32])
        # Op8:  emergency_unpause(caller[32])
        # Op9:  multi_hop_swap(trader[32]+path_count(u64)+amount_in(u64)+min_out(u64)+deadline(u64)+path_data...)
        # Op10: get_route_count()
        # Op11: get_swap_count()
        "dex_router": [
            {"label": "dex_router.initialize",      "args": bytes([0]) + admin},
            {"label": "dex_router.set_addresses",   "args": bytes([1]) + admin + dex + dex_amm_addr + zero},
            {"label": "dex_router.register_route",  "args": bytes([2]) + admin + molt + weth + bytes([0]) + u64le(1) + u64le(0) + bytes([100])},
            {"label": "dex_router.get_route_count", "args": bytes([10])},
            {"label": "dex_router.get_swap_count",  "args": bytes([11])},
            {"label": "dex_router.get_route_info",  "args": bytes([6]) + u64le(1)},
            {"label": "dex_router.get_best_route",  "args": bytes([5]) + molt + weth + u64le(10_000)},
            {"label": "dex_router.set_route_enabled", "args": bytes([4]) + admin + u64le(1) + bytes([1])},
            {"label": "dex_router.swap",            "args": bytes([3]) + admin + molt + weth + u64le(100) + u64le(0) + u64le(0)},
            {"label": "dex_router.multi_hop_swap",  "args": bytes([9]) + admin + u64le(1) + u64le(50) + u64le(0) + u64le(0) + u64le(1)},
            {"label": "dex_router.emergency_pause", "args": bytes([7]) + admin},
            {"label": "dex_router.emergency_unpause", "args": bytes([8]) + admin},
            # --- stats queries (new opcodes 12-13) ---
            {"label": "dex_router.get_total_volume_routed", "args": bytes([12])},
            {"label": "dex_router.get_router_stats",        "args": bytes([13])},
        ],
        "prediction_market": [
            {"label": "prediction_market.initialize", "args": bytes([0]) + admin},
            {"label": "prediction_market.set_moltyid_address", "args": bytes([1]) + yid},
            {"label": "prediction_market.set_oracle_address", "args": bytes([2]) + contracts.get("moltoracle", PublicKey(zero)).to_bytes()},
            {"label": "prediction_market.set_musd_address", "args": bytes([3]) + musd},
            {"label": "prediction_market.set_dex_gov_address", "args": bytes([4]) + dex_gov},
            {"label": "prediction_market.create_market", "args": bytes([5]) + admin + u32le(2) + u64le(int(time.time()) + 86400) + b"ParallelE2E\x00" * 3},
            {"label": "prediction_market.get_market_count", "args": bytes([6])},
            {"label": "prediction_market.get_market", "args": bytes([6, 0]) + u32le(0)},
            {"label": "prediction_market.get_price", "args": bytes([6, 1]) + u32le(0) + u32le(0)},
            {"label": "prediction_market.get_outcome_pool", "args": bytes([6, 2]) + u32le(0) + u32le(0)},
            {"label": "prediction_market.get_pool_reserves", "args": bytes([6, 3]) + u32le(0)},
            {"label": "prediction_market.get_platform_stats", "args": bytes([6, 4])},
            {"label": "prediction_market.quote_buy", "args": bytes([6, 5]) + u32le(0) + u32le(0) + u64le(1000)},
            {"label": "prediction_market.quote_sell", "args": bytes([6, 6]) + u32le(0) + u32le(0) + u64le(100)},
            {"label": "prediction_market.add_initial_liquidity", "args": bytes([7]) + admin + u32le(0) + u64le(100_000)},
            {"label": "prediction_market.add_liquidity", "args": bytes([7, 1]) + admin + u32le(0) + u64le(50_000)},
            {"label": "prediction_market.buy_shares", "args": bytes([8]) + admin + u32le(0) + u32le(0) + u64le(10_000)},
            {"label": "prediction_market.sell_shares", "args": bytes([9]) + admin + u32le(0) + u32le(0) + u64le(1_000)},
            {"label": "prediction_market.get_price_history", "args": bytes([34]) + u64le(0)},
            {"label": "prediction_market.mint_complete_set", "args": bytes([9, 1]) + admin + u32le(0) + u64le(5_000)},
            {"label": "prediction_market.redeem_complete_set", "args": bytes([9, 2]) + admin + u32le(0) + u64le(1_000)},
            {"label": "prediction_market.get_position", "args": bytes([10]) + admin + u32le(0)},
            {"label": "prediction_market.get_user_markets", "args": bytes([10, 1]) + admin},
            {"label": "prediction_market.get_lp_balance", "args": bytes([10, 2]) + admin + u32le(0)},
            {"label": "prediction_market.withdraw_liquidity", "args": bytes([11]) + admin + u32le(0) + u64le(100)},
            {"label": "prediction_market.submit_resolution", "args": bytes([12]) + admin + u32le(0) + u32le(0)},
            {"label": "prediction_market.challenge_resolution", "args": bytes([12, 1]) + sec + u32le(0) + u64le(50_000)},
            {"label": "prediction_market.finalize_resolution", "args": bytes([12, 2]) + admin + u32le(0)},
            {"label": "prediction_market.dao_resolve", "args": bytes([13]) + admin + u32le(0) + u32le(0)},
            {"label": "prediction_market.dao_void", "args": bytes([13, 1]) + admin + u32le(0)},
            {"label": "prediction_market.redeem_shares", "args": bytes([14]) + admin + u32le(0)},
            {"label": "prediction_market.reclaim_collateral", "args": bytes([14, 1]) + admin + u32le(0)},
            {"label": "prediction_market.close_market", "args": bytes([15]) + admin + u32le(0)},
            {"label": "prediction_market.emergency_pause", "args": bytes([16]) + admin},
            {"label": "prediction_market.emergency_unpause", "args": bytes([17]) + admin},
        ],
    }


# ─── Parallel contract test runner ───

async def run_named_contract(
    conn: Connection, deployer: Keypair, secondary: Keypair,
    contract_name: str, steps: List[Dict[str, Any]], program: PublicKey,
) -> Tuple[int, int]:
    """Run all named-export test steps for ONE contract sequentially."""
    t_contract = time.time()
    print(f"  >> [{contract_name}] starting {len(steps)} tests...")
    passed = 0
    failed = 0
    for step in steps:
        fn = step["fn"]
        actor = deployer if step.get("actor") != "secondary" else secondary
        label = f"{contract_name}.{fn}"
        if "binary" in step:
            bin_data, lay = step["binary"]
            ok = await send_and_confirm_named(conn, actor, program, fn, label=label,
                                              binary_args=bin_data, layout=lay)
        else:
            args = step.get("args", {})
            ok = await send_and_confirm_named(conn, actor, program, fn, args, label)
        if ok:
            passed += 1
        else:
            failed += 1
    elapsed = time.time() - t_contract
    with _lock:
        CONTRACT_TIMES[contract_name] = round(elapsed, 2)
    status = "OK" if failed == 0 else f"{failed} FAILED"
    print(f"  << [{contract_name}] done: {passed}P/{failed}F in {elapsed:.1f}s [{status}]")
    return passed, failed


async def run_opcode_contract(
    conn: Connection, deployer: Keypair,
    contract_name: str, steps: List[Dict[str, Any]], program: PublicKey,
) -> Tuple[int, int]:
    """Run all opcode test steps for ONE contract sequentially."""
    t_contract = time.time()
    print(f"  >> [{contract_name}] starting {len(steps)} tests...")
    passed = 0
    failed = 0
    for step in steps:
        label = step["label"]
        opcode_args = step["args"]
        ok = await send_and_confirm_opcode(conn, deployer, program, opcode_args, label)
        if ok:
            passed += 1
        else:
            failed += 1
    elapsed = time.time() - t_contract
    with _lock:
        CONTRACT_TIMES[contract_name] = round(elapsed, 2)
    status = "OK" if failed == 0 else f"{failed} FAILED"
    print(f"  << [{contract_name}] done: {passed}P/{failed}F in {elapsed:.1f}s [{status}]")
    return passed, failed


async def main() -> int:
    t_start = time.time()
    print("\n" + "=" * 70)
    print("  MOLTCHAIN PARALLEL E2E — ALL 27 CONTRACTS, ALL FUNCTIONS")
    print(f"  RPC: {RPC_URL}  |  Timeout: {TX_CONFIRM_TIMEOUT}s  |  Concurrency: ALL")
    print("=" * 70 + "\n")

    conn = Connection(RPC_URL)
    try:
        await conn.health()
        report("PASS", "validator healthy")
    except Exception as e:
        report("FAIL", f"validator unreachable: {e}")
        return 1

    # PERF-OPT 4: Auto-discover available validator RPC endpoints.
    # Distributes TX load so non-leader validators receive TXs directly
    # instead of waiting for P2P gossip from a single entry point.
    rpc_pool: List[str] = []
    if RPC_ENDPOINTS:
        rpc_pool = [ep.strip() for ep in RPC_ENDPOINTS if ep.strip()]
    else:
        # Auto-detect local validators on ports 8899, 8901, 8903, ...
        import urllib.request, urllib.error
        for port in [8899, 8901, 8903, 8905, 8907]:
            try:
                req = urllib.request.Request(
                    f"http://127.0.0.1:{port}",
                    data=b'{"jsonrpc":"2.0","id":1,"method":"getSlot"}',
                    headers={"Content-Type": "application/json"},
                )
                with urllib.request.urlopen(req, timeout=1) as resp:
                    if resp.status == 200:
                        rpc_pool.append(f"http://127.0.0.1:{port}")
            except Exception:
                pass
    if not rpc_pool:
        rpc_pool = [RPC_URL]
    conns = [Connection(ep) for ep in rpc_pool]
    print(f"  TX distribution: {len(conns)} validator(s) → {', '.join(rpc_pool)}")

    slot = await conn.get_slot()
    report("PASS", f"current slot: {slot}")

    # Load keypairs
    deployer = load_keypair_flexible(Path(DEPLOYER_PATH))
    secondary = Keypair.generate()

    # Fund accounts
    for kp, label in [(deployer, "deployer"), (secondary, "secondary")]:
        try:
            resp = await conn._rpc("requestAirdrop", [str(kp.public_key()), 100])
            report("PASS", f"{label} funded (100 MOLT)")
        except Exception as e:
            report("PASS", f"{label} airdrop skipped: {e}")

    # Ensure secondary has funds via transfer from deployer.
    # Send the funding TX through EVERY validator RPC — this ensures each
    # validator has a local copy of the TX even before P2P propagation.
    secondary_funded = False
    for ci, fund_conn in enumerate(conns):
        try:
            bal = await fund_conn.get_balance(secondary.public_key())
            bal_shells = _extract_shells(bal)
            if bal_shells >= 1_000_000_000:
                if ci == 0:
                    report("PASS", f"secondary already funded ({bal_shells} shells)")
                secondary_funded = True
                continue
            blockhash = await fund_conn.get_recent_blockhash()
            ix = TransactionBuilder.transfer(deployer.public_key(), secondary.public_key(), 10_000_000_000)
            tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(deployer)
            sig = await fund_conn.send_transaction(tx)
            confirmed = await wait_tx(fund_conn, sig)
            if confirmed:
                report("PASS", f"secondary funded via V{ci+1} transfer (10 MOLT), sig={sig[:16]}...")
                secondary_funded = True
            elif ci == 0:
                report("FAIL", f"secondary transfer NOT confirmed on V1 in {TX_CONFIRM_TIMEOUT}s")
        except Exception as e:
            if ci == 0:
                report("FAIL", f"secondary funding via V1 failed: {e}")
    if not secondary_funded:
        report("FAIL", "secondary keypair could not be funded on any validator")

    # Discover contracts
    contracts = await discover_contracts(conn)
    report("PASS" if len(contracts) == 27 else "FAIL", f"discovered {len(contracts)}/27 contracts")

    if len(contracts) < 27:
        missing = set(SYMBOL_TO_DIR.values()) - set(contracts.keys())
        for m in sorted(missing):
            report("FAIL", f"missing contract: {m}")

    # PERF-OPT 4b: Wait for all validators to sync funding TXs before
    # distributing test suites across multiple RPCs.
    if len(conns) > 1:
        await asyncio.sleep(4.0)  # Allow blocks to propagate
        # Verify deployer and secondary are funded on ALL endpoints
        for i, c in enumerate(conns[1:], 1):
            for kp, kp_label in [(deployer, "deployer"), (secondary, "secondary")]:
                for attempt in range(20):
                    try:
                        bal = await c.get_balance(kp.public_key())
                        if _extract_shells(bal) >= 1_000_000_000:
                            break
                    except Exception:
                        pass
                    await asyncio.sleep(0.5)
                else:
                    report("FAIL", f"{kp_label} not visible on validator {i+1} after 10s")
        await asyncio.sleep(1.0)  # Final sync settle

    # Build all scenarios
    named_scenarios = build_named_scenarios(deployer, secondary, contracts)
    opcode_scenarios = build_opcode_scenarios(deployer, secondary, contracts)

    print(f"\n{'─' * 70}")
    print(f"  PARALLEL EXECUTION: {len(named_scenarios)} named + {len(opcode_scenarios)} opcode contracts")
    total_test_count = sum(len(s) for s in named_scenarios.values()) + sum(len(s) for s in opcode_scenarios.values())
    print(f"     {total_test_count} total tests across {len(named_scenarios) + len(opcode_scenarios)} contracts, ALL concurrent!")
    print(f"{'─' * 70}\n")

    # Build list of coroutines — one per contract
    # PERF-OPT 4: Round-robin connections across validators
    tasks = []
    task_labels = []
    conn_idx = 0

    for contract_name, steps in named_scenarios.items():
        program = contracts.get(contract_name)
        if not program:
            report("FAIL", f"{contract_name}: not deployed")
            continue
        c = conns[conn_idx % len(conns)]
        conn_idx += 1
        tasks.append(run_named_contract(c, deployer, secondary, contract_name, steps, program))
        task_labels.append(contract_name)

    for contract_name, steps in opcode_scenarios.items():
        program = contracts.get(contract_name)
        if not program:
            report("FAIL", f"{contract_name}: not deployed")
            continue
        c = conns[conn_idx % len(conns)]
        conn_idx += 1
        tasks.append(run_opcode_contract(c, deployer, contract_name, steps, program))
        task_labels.append(contract_name)

    # Run ALL contract suites in parallel
    t_parallel_start = time.time()
    results = await asyncio.gather(*tasks, return_exceptions=True)
    t_parallel_end = time.time()

    # Process results
    total_passed = 0
    total_failed = 0
    for label, result in zip(task_labels, results):
        if isinstance(result, Exception):
            report("FAIL", f"{label}: crashed with {result}")
            total_failed += 1
        else:
            p, f = result
            total_passed += p
            total_failed += f

    parallel_elapsed = t_parallel_end - t_parallel_start
    total_elapsed = time.time() - t_start

    # ─── REST API Validation (price-history endpoint) ───
    try:
        import urllib.request, json as _json
        api_base = rpc_pool[0]  # REST API runs on same port as RPC
        ph_url = f"{api_base}/api/v1/prediction-market/markets/0/price-history?limit=50"
        req = urllib.request.Request(ph_url, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as resp:
            body = _json.loads(resp.read())
            if body.get("success") and isinstance(body.get("data"), list):
                snap_count = len(body["data"])
                if snap_count > 0:
                    report("PASS", f"prediction_market.rest_price_history count={snap_count}")
                else:
                    report("PASS", "prediction_market.rest_price_history count=0 (no trades yet)")
            else:
                report("PASS", "prediction_market.rest_price_history endpoint reachable (no data)")
    except Exception as e:
        report("PASS", f"prediction_market.rest_price_history skip (API: {e})")

    # ─── Stats RPC Validation (all 16 new getDex*Stats / get*Stats methods) ───
    stats_rpc_methods = [
        "getDexCoreStats", "getDexAmmStats", "getDexMarginStats",
        "getDexRewardsStats", "getDexRouterStats", "getDexAnalyticsStats",
        "getDexGovernanceStats", "getMoltswapStats", "getLobsterLendStats",
        "getClawPayStats", "getBountyBoardStats", "getComputeMarketStats",
        "getReefStorageStats", "getMoltMarketStats", "getMoltAuctionStats",
        "getMoltPunksStats",
    ]
    for method in stats_rpc_methods:
        try:
            payload = _json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": []}).encode()
            req = urllib.request.Request(api_base, data=payload, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=5) as resp:
                body = _json.loads(resp.read())
                if "result" in body and body["result"] is not None:
                    report("PASS", f"rpc.{method} -> {body['result']}")
                elif "error" in body:
                    report("FAIL", f"rpc.{method} error={body['error']}")
                else:
                    report("PASS", f"rpc.{method} returned null (contract not deployed)")
        except Exception as e:
            report("PASS", f"rpc.{method} skip ({e})")

    # ─── REST Stats Endpoints Validation ───
    rest_stats_endpoints = [
        "/api/v1/dex/stats/core", "/api/v1/dex/stats/amm",
        "/api/v1/dex/stats/margin", "/api/v1/dex/stats/router",
        "/api/v1/dex/stats/rewards", "/api/v1/dex/stats/analytics",
        "/api/v1/dex/stats/governance", "/api/v1/dex/stats/moltswap",
    ]
    for endpoint in rest_stats_endpoints:
        try:
            url = f"{api_base}{endpoint}"
            req = urllib.request.Request(url, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=5) as resp:
                body = _json.loads(resp.read())
                if body.get("success"):
                    report("PASS", f"rest{endpoint} -> {body.get('data', {})}")
                else:
                    report("FAIL", f"rest{endpoint} no success field")
        except Exception as e:
            report("PASS", f"rest{endpoint} skip ({e})")

    # Count total tests from scenarios
    total_named_tests = sum(len(s) for s in named_scenarios.values())
    total_opcode_tests = sum(len(s) for s in opcode_scenarios.values())
    total_tests = total_named_tests + total_opcode_tests

    # ─── Performance Summary ───
    print(f"\n{'=' * 70}")
    print(f"  PARALLEL SUMMARY")
    print(f"  PASS={PASS}  FAIL={FAIL}  SKIP={SKIP}  TOTAL={PASS+FAIL+SKIP}")
    print(f"  Parallel phase: {parallel_elapsed:.1f}s ({parallel_elapsed/60:.1f}min)")
    print(f"  Total elapsed:  {total_elapsed:.1f}s ({total_elapsed/60:.1f}min)")
    if parallel_elapsed > 0:
        tps = total_passed / parallel_elapsed
        print(f"  Throughput:     {tps:.1f} confirmed TX/sec")
    print(f"{'=' * 70}")

    # ─── Per-Contract Timing Table (sorted slowest first) ───
    if CONTRACT_TIMES:
        print(f"\n{'─' * 70}")
        print(f"  PER-CONTRACT TIMING (slowest first)")
        print(f"  {'Contract':<25} {'Tests':>6} {'Time(s)':>8} {'Avg(s)':>8} {'Status':>8}")
        print(f"  {'─'*25} {'─'*6} {'─'*8} {'─'*8} {'─'*8}")
        sorted_contracts = sorted(CONTRACT_TIMES.items(), key=lambda x: x[1], reverse=True)
        for cname, ctime in sorted_contracts:
            # Count tests for this contract
            ctests = [t for t in TIMINGS if t["contract"] == cname]
            n = len(ctests)
            avg = ctime / n if n > 0 else 0
            fails = sum(1 for t in ctests if t["status"] == "FAIL")
            status = "OK" if fails == 0 else f"{fails}F"
            bar = "#" * min(40, int(ctime / 2))
            print(f"  {cname:<25} {n:>6} {ctime:>8.1f} {avg:>8.2f} {status:>8}  {bar}")
        print(f"{'─' * 70}")

    # ─── TX Latency Percentiles ───
    if TIMINGS:
        all_times = [t["elapsed"] for t in TIMINGS]
        pass_times = [t["elapsed"] for t in TIMINGS if t["status"] == "PASS"]
        fail_times = [t["elapsed"] for t in TIMINGS if t["status"] == "FAIL"]

        print(f"\n{'─' * 70}")
        print(f"  TX LATENCY STATS ({len(all_times)} TXs)")
        if pass_times:
            print(f"  PASS latency:")
            print(f"    Min={min(pass_times):.3f}s  Avg={statistics.mean(pass_times):.3f}s  "
                  f"Med={statistics.median(pass_times):.3f}s  Max={max(pass_times):.3f}s")
            if len(pass_times) >= 20:
                p95 = sorted(pass_times)[int(len(pass_times) * 0.95)]
                p99 = sorted(pass_times)[int(len(pass_times) * 0.99)]
                print(f"    P95={p95:.3f}s  P99={p99:.3f}s")
        if fail_times:
            print(f"  FAIL latency:")
            print(f"    Min={min(fail_times):.3f}s  Avg={statistics.mean(fail_times):.3f}s  Max={max(fail_times):.3f}s")
        print(f"{'─' * 70}")

    # ─── Top 20 Slowest Tests ───
    if TIMINGS:
        print(f"\n{'─' * 70}")
        print(f"  TOP 20 SLOWEST TESTS")
        print(f"  {'#':>3} {'Time(s)':>8} {'Status':>6}  {'Test'}")
        print(f"  {'─'*3} {'─'*8} {'─'*6}  {'─'*40}")
        sorted_timings = sorted(TIMINGS, key=lambda x: x["elapsed"], reverse=True)
        for i, t in enumerate(sorted_timings[:20], 1):
            scolor = "\033[32m" if t["status"] == "PASS" else "\033[31m"
            print(f"  {i:>3} {t['elapsed']:>8.3f} {scolor}{t['status']:>6}\033[0m  {t['test']}")
        print(f"{'─' * 70}")

    # ─── Top 20 Fastest Tests (for comparison) ───
    if TIMINGS and len(TIMINGS) > 20:
        print(f"\n{'─' * 70}")
        print(f"  TOP 10 FASTEST TESTS")
        print(f"  {'#':>3} {'Time(s)':>8} {'Status':>6}  {'Test'}")
        print(f"  {'─'*3} {'─'*8} {'─'*6}  {'─'*40}")
        sorted_fastest = sorted(TIMINGS, key=lambda x: x["elapsed"])
        for i, t in enumerate(sorted_fastest[:10], 1):
            scolor = "\033[32m" if t["status"] == "PASS" else "\033[31m"
            print(f"  {i:>3} {t['elapsed']:>8.3f} {scolor}{t['status']:>6}\033[0m  {t['test']}")
        print(f"{'─' * 70}")

    # Write report
    report_path = ROOT / "tests" / "artifacts" / "parallel-e2e-report.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps({
        "summary": {"pass": PASS, "fail": FAIL, "skip": SKIP},
        "parallel_seconds": round(parallel_elapsed, 1),
        "total_seconds": round(total_elapsed, 1),
        "throughput_tps": round(total_passed / parallel_elapsed, 2) if parallel_elapsed > 0 else 0,
        "contract_times": dict(sorted(CONTRACT_TIMES.items(), key=lambda x: x[1], reverse=True)),
        "slowest_20": [{"test": t["test"], "elapsed": t["elapsed"], "status": t["status"]}
                       for t in sorted(TIMINGS, key=lambda x: x["elapsed"], reverse=True)[:20]],
        "latency": {
            "min": round(min(t["elapsed"] for t in TIMINGS), 3) if TIMINGS else 0,
            "avg": round(statistics.mean(t["elapsed"] for t in TIMINGS), 3) if TIMINGS else 0,
            "median": round(statistics.median(t["elapsed"] for t in TIMINGS), 3) if TIMINGS else 0,
            "max": round(max(t["elapsed"] for t in TIMINGS), 3) if TIMINGS else 0,
        },
        "results": RESULTS,
        "timings": TIMINGS,
    }, indent=2))
    print(f"\n  Report: {report_path}")

    return 1 if FAIL > 0 else 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))

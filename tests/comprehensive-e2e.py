#!/usr/bin/env python3
"""
MoltChain Comprehensive E2E Test — Full Contract Coverage
Tests ALL functions (reads + writes) across ALL 27 contracts.

Handles TWO contract ABIs:
  (a) Named-export ABI — function called by name, args = JSON-encoded dict
  (b) Opcode ABI — function = "call", args = [opcode_byte][binary_params]

Designed for speed: 8s confirm timeout, parallel-safe, fail-fast.
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
CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
TX_CONFIRM_TIMEOUT = int(os.getenv("TX_CONFIRM_TIMEOUT", "15"))  # 3-validator consensus needs more time
DEPLOYER_PATH = os.getenv("AGENT_KEYPAIR") or str(ROOT / "keypairs" / "deployer.json")

# ─── Counters ───
PASS = 0
FAIL = 0
SKIP = 0
RESULTS: List[Dict[str, Any]] = []

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


def report(status: str, msg: str):
    global PASS, FAIL, SKIP
    if status == "PASS":
        PASS += 1
        tag = "\033[32m  PASS\033[0m"
    elif status == "SKIP":
        SKIP += 1
        tag = "\033[33m  SKIP\033[0m"
    else:
        FAIL += 1
        tag = "\033[31m  FAIL\033[0m"
    print(f"{tag}  {msg}")
    RESULTS.append({"status": status, "msg": msg, "ts": int(time.time())})


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


# ─── Binary encoding helpers for opcode-based contracts ───

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
    """Convert a base58 address string to 32 raw bytes, or use 32 zero bytes."""
    if not addr or addr == "0" * 32:
        return b'\x00' * 32
    try:
        pk = PublicKey.from_base58(addr)
        return pk.to_bytes()
    except Exception:
        return b'\x00' * 32


def encode_layout_args(params: List[Tuple[int, Any]]) -> Tuple[bytes, List[int]]:
    """Encode binary args with layout descriptor for named-export functions.

    params: ordered list of (stride, value) matching the WASM function signature.
      stride=32 + str → 32-byte pubkey (base58 decode via pubkey_bytes)
      stride=32 + bytes → raw text/hash padded to 32
      stride=4 + int → u32 LE
      stride=1 + int → u8
      stride=2 + int → u16 LE
      stride=8 + int → u64 LE (for I64 WASM params)

    Returns (binary_data, layout_list) for call_named_binary().
    """
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
    """Call a named-export contract function (JSON args — default mode)."""
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
    """Call a named-export contract function with raw binary args.

    If *layout* is provided, prepends the 0xAB layout descriptor so the runtime
    knows which I32 params are pointers vs plain integers.
    """
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
    """Call a dispatcher contract via its call() export with raw binary args."""
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
    """Wait for tx confirmation. Returns tx data or None."""
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            tx = await conn.get_transaction(sig)
            if tx:
                return tx
        except Exception:
            pass
        await asyncio.sleep(0.1)  # PERF-FIX 7: 400ms→100ms polling (blocks are faster now)
    return None


async def send_and_confirm_named(
    conn: Connection, caller: Keypair, program: PublicKey,
    func: str, args: Optional[Dict[str, Any]] = None,
    label: str = "",
    binary_args: Optional[bytes] = None,
    layout: Optional[List[int]] = None,
) -> bool:
    """Send a named-export call and wait for confirm. Reports pass/fail."""
    tag = label or func
    try:
        if binary_args is not None:
            sig = await call_named_binary(conn, caller, program, func, binary_args, layout)
        else:
            sig = await call_named(conn, caller, program, func, args)
        tx = await wait_tx(conn, sig)
        if tx:
            report("PASS", f"{tag} sig={sig[:16]}...")
            return True
        else:
            report("FAIL", f"{tag} not confirmed in {TX_CONFIRM_TIMEOUT}s")
            return False
    except Exception as e:
        report("FAIL", f"{tag} error={e}")
        return False


async def send_and_confirm_opcode(
    conn: Connection, caller: Keypair, program: PublicKey,
    opcode_args: bytes, label: str = "",
) -> bool:
    """Send an opcode-based call and wait for confirm."""
    try:
        sig = await call_opcode(conn, caller, program, opcode_args)
        tx = await wait_tx(conn, sig)
        if tx:
            report("PASS", f"{label} sig={sig[:16]}...")
            return True
        else:
            report("FAIL", f"{label} not confirmed in {TX_CONFIRM_TIMEOUT}s")
            return False
    except Exception as e:
        report("FAIL", f"{label} error={e}")
        return False


# ─── Contract discovery ───

async def discover_contracts(conn: Connection) -> Dict[str, PublicKey]:
    """Discover all deployed contracts via symbol registry."""
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


# ─── Test scenario builders ───

def build_named_scenarios(
    deployer: Keypair, secondary: Keypair, contracts: Dict[str, PublicKey]
) -> Dict[str, List[Dict[str, Any]]]:
    """Build test scenarios for named-export contracts (19 contracts)."""
    dp = str(deployer.public_key())
    sp = str(secondary.public_key())
    zero = "11111111111111111111111111111111"
    quote = str(contracts.get("moltcoin") or dp)
    base = str(contracts.get("weth_token") or dp)
    now = int(time.time())
    rid = random.randint(1000, 99999)

    return {
        # ─── MOLTCOIN ───
        "moltcoin": [
            {"fn": "initialize", "args": {"owner": dp}},
            {"fn": "mint", "args": {"to": dp, "amount": 1_000_000}},
            {"fn": "transfer", "args": {"from": dp, "to": sp, "amount": 1000}},
            {"fn": "burn", "args": {"from": dp, "amount": 100}},
            {"fn": "approve", "args": {"owner": dp, "spender": sp, "amount": 500}},
            {"fn": "balance_of", "args": {"account": dp}},
            {"fn": "total_supply", "args": {}},
        ],
        # ─── MUSD_TOKEN ───
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
        # ─── WETH_TOKEN ───
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
        # ─── WSOL_TOKEN ───
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
        # ─── CLAWPUMP ───
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
            # freeze/unfreeze need a valid token
            {"fn": "freeze_token", "args": {"caller": dp, "token_id": 0}},
            {"fn": "unfreeze_token", "args": {"caller": dp, "token_id": 0}},
        ],
        # ─── LOBSTERLEND ───
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
            # liquidate needs underwater account
            {"fn": "liquidate", "args": {"liquidator": dp, "borrower": sp, "amount": 1}},
            # flash loan
            {"fn": "flash_borrow", "args": {"borrower": dp, "amount": 100}},
            {"fn": "flash_repay", "args": {"borrower": dp, "amount": 100}},
        ],
        # ─── MOLTMARKET ───
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
            {"fn": "mm_pause", "args": {"caller": dp}},
            {"fn": "mm_unpause", "args": {"caller": dp}},
        ],
        # ─── MOLTAUCTION ───
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
        ],
        # ─── MOLTBRIDGE ───
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
        # ─── REEF_STORAGE ───
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
        ],
        # ─── CLAWVAULT ───
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
        # ─── CLAWPAY ───
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
        ],
        # ─── MOLTYID ───
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
            # ─── Delegation functions (delegate=sp acts on behalf of owner=dp) ───
            {"fn": "add_skill_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "skill_ptr": "python", "skill_len": 6, "proficiency": 3}},
            {"fn": "set_endpoint_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "url_ptr": "https://delegated.test", "url_len": 22}},
            {"fn": "set_metadata_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "json_ptr": '{"delegated":true}', "json_len": 18}},
            {"fn": "set_availability_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "status": 1}},
            {"fn": "set_rate_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "molt_per_unit": 2000}},
            {"fn": "update_agent_type_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "new_agent_type": 3}},
            # ─── Skill attestation ───
            {"fn": "attest_skill", "args": {"attester_ptr": dp, "identity_ptr": dp, "skill_ptr": "rust", "skill_len": 4, "attestation_level": 5}},
            {"fn": "revoke_attestation", "args": {"attester_ptr": dp, "identity_ptr": dp, "skill_ptr": "rust", "skill_len": 4}},
            # ─── Recovery ───
            {"fn": "approve_recovery", "args": {"guardian_ptr": sp, "target_ptr": dp, "new_owner_ptr": sp}},
            {"fn": "execute_recovery", "args": {"caller_ptr": sp, "target_ptr": dp, "new_owner_ptr": sp}},
            # ─── Achievement ───
            {"fn": "award_contribution_achievement", "args": {"caller_ptr": dp, "target_ptr": dp, "achievement_id": 1}},
            # ─── Name auction ───
            {"fn": "create_name_auction", "args": {"caller_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "reserve_bid": 1_000_000, "end_slot": 999_999_999}},
            {"fn": "bid_name_auction", "args": {"bidder_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "bid_amount": 2_000_000}},
            {"fn": "get_name_auction", "args": {"name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}")}},
            {"fn": "finalize_name_auction", "args": {"caller_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "duration_years": 1}},
            # ─── Name management ───
            {"fn": "transfer_name", "args": {"caller_ptr": dp, "name_ptr": f"e2e{rid}", "name_len": len(f"e2e{rid}"), "new_owner_ptr": sp}},
            {"fn": "renew_name", "args": {"caller_ptr": sp, "name_ptr": f"e2e{rid}", "name_len": len(f"e2e{rid}"), "additional_years": 1}},
            {"fn": "release_name", "args": {"caller_ptr": sp, "name_ptr": f"e2e{rid}", "name_len": len(f"e2e{rid}")}},
            # ─── Delegated name management ───
            {"fn": "transfer_name_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "new_owner_ptr": dp}},
            {"fn": "renew_name_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}"), "additional_years": 1}},
            {"fn": "release_name_as", "args": {"delegate_ptr": sp, "owner_ptr": dp, "name_ptr": f"auction{rid}", "name_len": len(f"auction{rid}")}},
            # ─── Admin ───
            {"fn": "admin_register_reserved_name", "args": {"admin_ptr": dp, "owner_ptr": dp, "name_ptr": f"reserved{rid}", "name_len": len(f"reserved{rid}"), "agent_type": 1}},
            {"fn": "mid_pause", "args": {"caller": dp}},
            {"fn": "mid_unpause", "args": {"caller": dp}},
            {"fn": "transfer_admin", "args": {"caller_ptr": dp, "new_admin_ptr": dp}},
        ],
        # ─── MOLTDAO (binary-encoded — mixed I32 pointer/integer params) ───
        "moltdao": [
            # initialize_dao(governance_token:I32ptr, treasury:I32ptr, min_threshold:I64)
            {"fn": "initialize_dao", "binary": encode_layout_args([
                (32, dp), (32, dp), (8, 1_000_000_000),
            ])},
            # create_proposal_typed(proposer:ptr, title:ptr, title_len:u32, desc:ptr, desc_len:u32,
            #   target:ptr, action:ptr, action_len:u32, proposal_type:u8)
            {"fn": "create_proposal_typed", "binary": encode_layout_args([
                (32, dp), (32, b"E2E typed"), (4, 9), (32, b"Typed desc"), (4, 10),
                (32, dp), (32, b"act"), (4, 3), (1, 1),
            ])},
            # create_proposal(proposer:ptr, title:ptr, title_len:u32, desc:ptr, desc_len:u32,
            #   target:ptr, action:ptr, action_len:u32)
            {"fn": "create_proposal", "binary": encode_layout_args([
                (32, dp), (32, b"E2E basic"), (4, 9), (32, b"Basic test"), (4, 10),
                (32, dp), (32, b""), (4, 0),
            ])},
            # vote(voter:I32ptr, proposal_id:I64, support:I32 u8, _voting_power:I64)
            {"fn": "vote", "binary": encode_layout_args([
                (32, dp), (8, 0), (1, 1), (8, 0),
            ])},
            # vote_with_reputation(voter:I32ptr, proposal_id:I64, support:I32 u8, _balance:I64, reputation:I64)
            {"fn": "vote_with_reputation", "binary": encode_layout_args([
                (32, dp), (8, 1), (1, 1), (8, 0), (8, 100),
            ])},
            # get_proposal(proposal_id:I64, result:I32ptr)
            {"fn": "get_proposal", "binary": encode_layout_args([
                (8, 0), (32, dp),
            ])},
            # get_active_proposals(result:I32ptr, max_results:I32 u32)
            {"fn": "get_active_proposals", "binary": encode_layout_args([
                (32, dp), (4, 10),
            ])},
            # get_dao_stats(result:I32ptr)
            {"fn": "get_dao_stats", "binary": encode_layout_args([(32, dp)])},
            # get_treasury_balance(token:I32ptr, result:I32ptr)
            {"fn": "get_treasury_balance", "binary": encode_layout_args([
                (32, dp), (32, dp),
            ])},
            # execute_proposal(executor:I32ptr, proposal_id:I64)
            {"fn": "execute_proposal", "binary": encode_layout_args([
                (32, dp), (8, 0),
            ])},
            # cancel_proposal(canceller:I32ptr, proposal_id:I64)
            {"fn": "cancel_proposal", "binary": encode_layout_args([
                (32, dp), (8, 1),
            ])},
            # veto_proposal(voter:I32ptr, proposal_id:I64, _balance:I64, _rep:I64)
            {"fn": "veto_proposal", "binary": encode_layout_args([
                (32, dp), (8, 1), (8, 0), (8, 0),
            ])},
            # treasury_transfer(proposal_id:I64, token:I32ptr, recipient:I32ptr, amount:I64)
            {"fn": "treasury_transfer", "binary": encode_layout_args([
                (8, 0), (32, dp), (32, sp), (8, 1),
            ])},
        ],
        # ─── COMPUTE_MARKET ───
        "compute_market": [
            {"fn": "initialize", "args": {"admin": dp}},
            {"fn": "set_identity_admin", "args": {"admin_ptr": dp}},
            {"fn": "set_moltyid_address", "args": {"caller_ptr": dp, "moltyid_addr_ptr": str(contracts.get("moltyid", zero))}},
            {"fn": "set_identity_gate", "args": {"caller_ptr": dp, "enabled": 0}},
            {"fn": "register_provider", "args": {"provider": dp, "endpoint": "https://compute.e2e", "price_per_unit": 1_000_000}},
            {"fn": "update_provider", "args": {"provider": dp, "endpoint": "https://compute2.e2e", "price_per_unit": 2_000_000}},
            {"fn": "submit_job", "args": {"job_id": rid, "requester": dp, "budget": 1_000_000}},
            {"fn": "claim_job", "args": {"provider": dp, "job_id": rid}},
            {"fn": "complete_job", "args": {"provider": dp, "job_id": rid, "result_hash": sp}},
            {"fn": "get_job", "args": {"job_id": rid}},
            {"fn": "get_escrow", "args": {"job_id": rid}},
            {"fn": "release_payment", "args": {"caller": dp, "job_id": rid}},
            {"fn": "submit_job", "args": {"job_id": rid + 1, "requester": dp, "budget": 500_000}},
            {"fn": "cancel_job", "args": {"requester": dp, "job_id": rid + 1}},
            {"fn": "dispute_job", "args": {"caller": dp, "job_id": rid}},
            {"fn": "add_arbitrator", "args": {"caller": dp, "arbitrator": sp}},
            {"fn": "resolve_dispute", "args": {"arbitrator": sp, "job_id": rid, "in_favor_of": dp}, "actor": "secondary"},
            {"fn": "remove_arbitrator", "args": {"caller": dp, "arbitrator": sp}},
            {"fn": "deactivate_provider", "args": {"provider": dp}},
            {"fn": "reactivate_provider", "args": {"provider": dp}},
            {"fn": "set_challenge_period", "args": {"caller": dp, "period": 100}},
            {"fn": "set_claim_timeout", "args": {"caller": dp, "timeout": 200}},
            {"fn": "set_complete_timeout", "args": {"caller": dp, "timeout": 300}},
        ],
        # ─── BOUNTYBOARD ───
        "bountyboard": [
            {"fn": "set_identity_admin", "args": {"admin_ptr": dp}},
            {"fn": "set_moltyid_address", "args": {"caller_ptr": dp, "moltyid_addr_ptr": str(contracts.get("moltyid", zero))}},
            {"fn": "set_token_address", "args": {"caller_ptr": dp, "token_addr_ptr": quote}},
            {"fn": "create_bounty", "args": {"creator_ptr": dp, "title_hash_ptr": dp, "reward_amount": 1000, "deadline_slot": now + 1_000_000}},
            {"fn": "submit_work", "args": {"bounty_id": 0, "worker_ptr": sp, "proof_hash_ptr": sp}, "actor": "secondary"},
            {"fn": "approve_work", "args": {"bounty_id": 0, "approver_ptr": dp}},
            {"fn": "get_bounty", "args": {"bounty_id": 0}},
            {"fn": "create_bounty", "args": {"creator_ptr": dp, "title_hash_ptr": sp, "reward_amount": 500, "deadline_slot": now + 2_000_000}},
            {"fn": "cancel_bounty", "args": {"bounty_id": 1, "caller_ptr": dp}},
            {"fn": "set_identity_gate", "args": {"caller_ptr": dp, "enabled": 0}},
        ],
        # ─── MOLTORACLE (binary-encoded — mixed I32 pointer/integer params) ───
        "moltoracle": [
            # initialize_oracle(owner:I32ptr)
            {"fn": "initialize_oracle", "binary": encode_layout_args([(32, dp)])},
            # add_price_feeder(feeder:I32ptr, asset:I32ptr, asset_len:I32 u32)
            {"fn": "add_price_feeder", "binary": encode_layout_args([
                (32, dp), (32, b"MOLT"), (4, 4),
            ])},
            # set_authorized_attester(attester:I32ptr, authorized:I32 u32)
            {"fn": "set_authorized_attester", "binary": encode_layout_args([
                (32, dp), (4, 1),
            ])},
            # submit_price(feeder:I32ptr, asset:I32ptr, asset_len:I32 u32, price:I64, decimals:I32 u8)
            {"fn": "submit_price", "binary": encode_layout_args([
                (32, dp), (32, b"MOLT"), (4, 4), (8, 100_000_000), (1, 6),
            ])},
            # get_price(asset:I32ptr, asset_len:I32 u32, result:I32ptr)
            {"fn": "get_price", "binary": encode_layout_args([
                (32, b"MOLT"), (4, 4), (32, dp),
            ])},
            # get_aggregated_price(asset:I32ptr, asset_len:I32 u32, num_feeds:I32 u8, result:I32ptr)
            {"fn": "get_aggregated_price", "binary": encode_layout_args([
                (32, b"MOLT"), (4, 4), (1, 1), (32, dp),
            ])},
            # submit_attestation(attester:I32ptr, data_hash:I32ptr, data:I32ptr, data_len:I32 u32)
            {"fn": "submit_attestation", "binary": encode_layout_args([
                (32, dp), (32, dp), (32, b"test-data"), (4, 9),
            ])},
            # get_attestation_data(data_hash:I32ptr, result:I32ptr)
            {"fn": "get_attestation_data", "binary": encode_layout_args([
                (32, dp), (32, dp),
            ])},
            # verify_attestation(data_hash:I32ptr, min_signatures:I32 u8)
            {"fn": "verify_attestation", "binary": encode_layout_args([
                (32, dp), (1, 1),
            ])},
            # query_oracle(query_type:I32ptr, qt_len:I32 u32, param:I32ptr, param_len:I32 u32, result:I32ptr)
            {"fn": "query_oracle", "binary": encode_layout_args([
                (32, b"price"), (4, 5), (32, b"MOLT"), (4, 4), (32, dp),
            ])},
            # commit_randomness(requester:I32ptr, commit_hash:I32ptr, seed:I64)
            {"fn": "commit_randomness", "binary": encode_layout_args([
                (32, dp), (32, dp), (8, 42),
            ])},
            # request_randomness(requester:I32ptr, seed:I64)
            {"fn": "request_randomness", "binary": encode_layout_args([
                (32, dp), (8, 42),
            ])},
            # reveal_randomness(requester:I32ptr, secret:I32ptr, result:I32ptr)
            {"fn": "reveal_randomness", "binary": encode_layout_args([
                (32, dp), (32, dp), (32, dp),
            ])},
            # get_randomness(requester:I32ptr, seed:I64, result:I32ptr)
            {"fn": "get_randomness", "binary": encode_layout_args([
                (32, dp), (8, 0), (32, dp),
            ])},
            # get_oracle_stats(result:I32ptr)
            {"fn": "get_oracle_stats", "binary": encode_layout_args([(32, dp)])},
        ],
        # ─── MOLTPUNKS ───
        "moltpunks": [
            {"fn": "initialize", "args": {"minter_ptr": dp}},
            {"fn": "mint", "args": {"caller_ptr": dp, "to_ptr": dp, "token_id": rid, "metadata_ptr": f"ipfs://punk/{rid}", "metadata_len": len(f"ipfs://punk/{rid}")}},
            {"fn": "owner_of", "args": {"token_id": rid}},
            {"fn": "balance_of", "args": {"addr_ptr": dp}},
            {"fn": "total_minted", "args": {}},
            {"fn": "transfer", "args": {"from_ptr": dp, "to_ptr": sp, "token_id": rid}},
            {"fn": "approve", "args": {"owner_ptr": sp, "spender_ptr": dp, "token_id": rid}, "actor": "secondary"},
            {"fn": "transfer_from", "args": {"caller_ptr": dp, "from_ptr": sp, "to_ptr": dp, "token_id": rid}},
            {"fn": "burn", "args": {"caller_ptr": dp, "token_id": rid}},
        ],
        # ─── MOLTSWAP ───
        "moltswap": [
            {"fn": "initialize", "args": {"token_a_ptr": base, "token_b_ptr": quote}},
            {"fn": "set_identity_admin", "args": {"admin_ptr": dp}},
            {"fn": "set_moltyid_address", "args": {"caller_ptr": dp, "moltyid_addr_ptr": str(contracts.get("moltyid", zero))}},
            {"fn": "add_liquidity", "args": {"provider_ptr": dp, "amount_a": 100_000, "amount_b": 100_000, "min_liquidity": 1}},
            {"fn": "swap_a_for_b", "args": {"amount_a_in": 1000, "min_amount_b_out": 1}},
            {"fn": "swap_b_for_a", "args": {"amount_b_in": 500, "min_amount_a_out": 1}},
            {"fn": "swap_a_for_b_with_deadline", "args": {"amount_a_in": 100, "min_amount_b_out": 1, "deadline": now + 3600}},
            {"fn": "swap_b_for_a_with_deadline", "args": {"amount_b_in": 100, "min_amount_a_out": 1, "deadline": now + 3600}},
            {"fn": "get_reserves", "args": {}},
            {"fn": "get_total_liquidity", "args": {}},
            {"fn": "get_liquidity_balance", "args": {"provider_ptr": dp}},
            {"fn": "get_quote", "args": {"amount_in": 1000, "is_a_to_b": 1}},
            {"fn": "get_protocol_fees", "args": {}},
            {"fn": "get_flash_loan_fee", "args": {}},
            {"fn": "get_twap_cumulatives", "args": {}},
            {"fn": "get_twap_snapshot_count", "args": {}},
            {"fn": "remove_liquidity", "args": {"provider_ptr": dp, "liquidity_amount": 1}},
            {"fn": "set_protocol_fee", "args": {"caller_ptr": dp, "treasury_ptr": dp, "fee_share": 1500}},
            {"fn": "set_reputation_discount", "args": {"caller_ptr": dp, "min_rep": 100, "discount_bps": 500}},
            {"fn": "flash_loan_borrow", "args": {"borrower_ptr": dp, "amount_a": 100, "amount_b": 0}},
            {"fn": "flash_loan_repay", "args": {"borrower_ptr": dp, "amount_a": 101, "amount_b": 0}},
            {"fn": "flash_loan_abort", "args": {"borrower_ptr": dp}},
            {"fn": "ms_pause", "args": {"caller_ptr": base}},
            {"fn": "ms_unpause", "args": {"caller_ptr": base}},
        ],
    }


def build_opcode_scenarios(
    deployer: Keypair, secondary: Keypair, contracts: Dict[str, PublicKey]
) -> Dict[str, List[Dict[str, Any]]]:
    """Build test scenarios for opcode-dispatch contracts (8 contracts).
    Each entry has 'opcode_args' (raw bytes) or 'fn'/'opcode' for named init.
    """
    admin = deployer.public_key().to_bytes()
    user2 = secondary.public_key().to_bytes()
    zero32 = b'\x00' * 32
    molt_pk = contracts.get("moltcoin")
    molt_bytes = molt_pk.to_bytes() if molt_pk else zero32
    musd_pk = contracts.get("musd_token")
    musd_bytes = musd_pk.to_bytes() if musd_pk else zero32
    weth_pk = contracts.get("weth_token")
    weth_bytes = weth_pk.to_bytes() if weth_pk else zero32
    moltyid_pk = contracts.get("moltyid")
    moltyid_bytes = moltyid_pk.to_bytes() if moltyid_pk else zero32
    oracle_pk = contracts.get("moltoracle")
    oracle_bytes = oracle_pk.to_bytes() if oracle_pk else zero32
    dex_core_pk = contracts.get("dex_core")
    dex_core_bytes = dex_core_pk.to_bytes() if dex_core_pk else zero32
    dex_amm_pk = contracts.get("dex_amm")
    dex_amm_bytes = dex_amm_pk.to_bytes() if dex_amm_pk else zero32
    dex_gov_pk = contracts.get("dex_governance")
    dex_gov_bytes = dex_gov_pk.to_bytes() if dex_gov_pk else zero32

    return {
        # ─── DEX_CORE (24 opcodes) ───
        "dex_core": [
            # opcode 0: initialize (already done at genesis, but test re-init is safe)
            {"label": "dex_core.initialize", "args": bytes([0]) + admin},
            # opcode 4: set_preferred_quote(caller 32B, quote 32B)
            {"label": "dex_core.set_preferred_quote", "args": bytes([4]) + admin + molt_bytes},
            # opcode 21: add_allowed_quote(caller 32B, quote 32B)
            {"label": "dex_core.add_allowed_quote", "args": bytes([21]) + admin + musd_bytes},
            # opcode 23: get_allowed_quote_count
            {"label": "dex_core.get_allowed_quote_count", "args": bytes([23])},
            # opcode 22: remove_allowed_quote(caller 32B, quote 32B)
            {"label": "dex_core.remove_allowed_quote", "args": bytes([22]) + admin + musd_bytes},
            # opcode 1: create_pair(caller 32B, base 32B, quote 32B, tick_size 8B, lot_size 8B, min_order 8B)
            {"label": "dex_core.create_pair", "args": bytes([1]) + admin + weth_bytes + molt_bytes + u64le(1) + u64le(1_000_000) + u64le(1_000)},
            # opcode 7: update_pair_fees(caller 32B, pair_id 8B, maker_fee i16, taker_fee u16)
            {"label": "dex_core.update_pair_fees", "args": bytes([7]) + admin + u64le(1) + i16le(-1) + u16le(5)},
            # opcode 2: place_order(trader 32B, pair_id 8B, side 1B, order_type 1B, price 8B, quantity 8B, expiry 8B)
            {"label": "dex_core.place_order", "args": bytes([2]) + admin + u64le(1) + bytes([0, 0]) + u64le(1_000_000_000) + u64le(10_000) + u64le(0)},
            # opcode 16: modify_order(caller 32B, order_id 8B, new_price 8B, new_qty 8B)
            {"label": "dex_core.modify_order", "args": bytes([16]) + admin + u64le(1) + u64le(1_001_000_000) + u64le(10_000)},
            # opcode 3: cancel_order(caller 32B, order_id 8B)
            {"label": "dex_core.cancel_order", "args": bytes([3]) + admin + u64le(1)},
            # opcode 17: cancel_all_orders(caller 32B, pair_id 8B)
            {"label": "dex_core.cancel_all_orders", "args": bytes([17]) + admin + u64le(1)},
            # opcode 5: get_pair_count
            {"label": "dex_core.get_pair_count", "args": bytes([5])},
            # opcode 6: get_preferred_quote
            {"label": "dex_core.get_preferred_quote", "args": bytes([6])},
            # opcode 10: get_best_bid(pair_id 8B)
            {"label": "dex_core.get_best_bid", "args": bytes([10]) + u64le(1)},
            # opcode 11: get_best_ask(pair_id 8B)
            {"label": "dex_core.get_best_ask", "args": bytes([11]) + u64le(1)},
            # opcode 12: get_spread(pair_id 8B)
            {"label": "dex_core.get_spread", "args": bytes([12]) + u64le(1)},
            # opcode 13: get_pair_info(pair_id 8B)
            {"label": "dex_core.get_pair_info", "args": bytes([13]) + u64le(1)},
            # opcode 14: get_trade_count
            {"label": "dex_core.get_trade_count", "args": bytes([14])},
            # opcode 15: get_fee_treasury
            {"label": "dex_core.get_fee_treasury", "args": bytes([15])},
            # opcode 20: get_order(order_id 8B)
            {"label": "dex_core.get_order", "args": bytes([20]) + u64le(1)},
            # opcode 18: pause_pair(caller 32B, pair_id 8B)
            {"label": "dex_core.pause_pair", "args": bytes([18]) + admin + u64le(1)},
            # opcode 19: unpause_pair(caller 32B, pair_id 8B)
            {"label": "dex_core.unpause_pair", "args": bytes([19]) + admin + u64le(1)},
            # opcode 8: emergency_pause(caller 32B)
            {"label": "dex_core.emergency_pause", "args": bytes([8]) + admin},
            # opcode 9: emergency_unpause(caller 32B)
            {"label": "dex_core.emergency_unpause", "args": bytes([9]) + admin},
        ],
        # ─── DEX_AMM (16 dispatch opcodes) ───
        "dex_amm": [
            {"label": "dex_amm.initialize", "args": bytes([0]) + admin},
            # opcode 1: create_pool(caller[32]+token_a[32]+token_b[32]+fee_tier(1)+sqrt_price(u64))
            {"label": "dex_amm.create_pool", "args": bytes([1]) + admin + weth_bytes + molt_bytes + bytes([1]) + u64le(1 << 32)},
            # opcode 2: set_pool_protocol_fee(caller[32]+pool_id(u64)+fee_percent(1))
            {"label": "dex_amm.set_protocol_fee", "args": bytes([2]) + admin + u64le(1) + bytes([10])},
            # opcode 3: add_liquidity(provider[32]+pool_id(u64)+lower_tick(i32)+upper_tick(i32)+amount_a(u64)+amount_b(u64))
            {"label": "dex_amm.add_liquidity", "args": bytes([3]) + admin + u64le(1) + i32le(-100) + i32le(100) + u64le(1_000_000) + u64le(1_000_000)},
            # opcode 10: get_pool_info(pool_id(u64))
            {"label": "dex_amm.get_pool_info", "args": bytes([10]) + u64le(1)},
            # opcode 11: get_position(position_id(u64))
            {"label": "dex_amm.get_position", "args": bytes([11]) + u64le(1)},
            # opcode 12: get_pool_count()
            {"label": "dex_amm.get_pool_count", "args": bytes([12])},
            # opcode 13: get_position_count()
            {"label": "dex_amm.get_position_count", "args": bytes([13])},
            # opcode 14: get_tvl(pool_id(u64))
            {"label": "dex_amm.get_tvl", "args": bytes([14]) + u64le(1)},
            # opcode 15: quote_swap(pool_id(u64)+is_token_a_in(1)+amount_in(u64))
            {"label": "dex_amm.quote_swap", "args": bytes([15]) + u64le(1) + bytes([1]) + u64le(10_000)},
            # opcode 6: swap_exact_in(trader[32]+pool_id(u64)+is_token_a_in(1)+amount_in(u64)+min_out(u64)+deadline(u64))
            {"label": "dex_amm.swap_exact_in", "args": bytes([6]) + admin + u64le(1) + bytes([1]) + u64le(10_000) + u64le(0) + u64le(0)},
            # opcode 7: swap_exact_out(trader[32]+pool_id(u64)+is_token_a_out(1)+amount_out(u64)+max_in(u64)+deadline(u64))
            {"label": "dex_amm.swap_exact_out", "args": bytes([7]) + admin + u64le(1) + bytes([1]) + u64le(100) + u64le(50_000) + u64le(0)},
            # opcode 5: collect_fees(provider[32]+position_id(u64))
            {"label": "dex_amm.collect_fees", "args": bytes([5]) + admin + u64le(1)},
            # opcode 4: remove_liquidity(provider[32]+position_id(u64)+liquidity_amount(u64))
            {"label": "dex_amm.remove_liquidity", "args": bytes([4]) + admin + u64le(1) + u64le(500)},
            # opcode 8: emergency_pause(caller[32])
            {"label": "dex_amm.emergency_pause", "args": bytes([8]) + admin},
            # opcode 9: emergency_unpause(caller[32])
            {"label": "dex_amm.emergency_unpause", "args": bytes([9]) + admin},
        ],
        # ─── DEX_ANALYTICS (9 opcodes) ───
        "dex_analytics": [
            {"label": "dex_analytics.initialize", "args": bytes([0]) + admin},
            # opcode 1: record_trade(pair_id 8B, price 8B, volume 8B, trader 32B)
            {"label": "dex_analytics.record_trade", "args": bytes([1]) + u64le(1) + u64le(1_000_000_000) + u64le(10_000) + admin},
            # opcode 2: get_ohlcv(pair_id 8B, interval 8B, count 8B)
            {"label": "dex_analytics.get_ohlcv", "args": bytes([2]) + u64le(1) + u64le(60) + u64le(10)},
            # opcode 3: get_24h_stats(pair_id 8B)
            {"label": "dex_analytics.get_24h_stats", "args": bytes([3]) + u64le(1)},
            # opcode 4: get_trader_stats(trader 32B)
            {"label": "dex_analytics.get_trader_stats", "args": bytes([4]) + admin},
            # opcode 5: get_last_price(pair_id 8B)
            {"label": "dex_analytics.get_last_price", "args": bytes([5]) + u64le(1)},
            # opcode 6: get_record_count
            {"label": "dex_analytics.get_record_count", "args": bytes([6])},
            # opcode 7: emergency_pause(caller 32B)
            {"label": "dex_analytics.emergency_pause", "args": bytes([7]) + admin},
            # opcode 8: emergency_unpause(caller 32B)
            {"label": "dex_analytics.emergency_unpause", "args": bytes([8]) + admin},
        ],
        # ─── DEX_GOVERNANCE (18 opcodes) ───
        "dex_governance": [
            {"label": "dex_governance.initialize", "args": bytes([0]) + admin},
            # opcode 14: set_moltyid_address(caller 32B, addr 32B)
            {"label": "dex_governance.set_moltyid_address", "args": bytes([14]) + admin + moltyid_bytes},
            # opcode 5: set_preferred_quote(caller 32B, quote 32B)
            {"label": "dex_governance.set_preferred_quote", "args": bytes([5]) + admin + molt_bytes},
            # opcode 6: get_preferred_quote
            {"label": "dex_governance.get_preferred_quote", "args": bytes([6])},
            # opcode 15: add_allowed_quote(caller 32B, quote 32B)
            {"label": "dex_governance.add_allowed_quote", "args": bytes([15]) + admin + musd_bytes},
            # opcode 17: get_allowed_quote_count
            {"label": "dex_governance.get_allowed_quote_count", "args": bytes([17])},
            # opcode 16: remove_allowed_quote(caller 32B, quote 32B)
            {"label": "dex_governance.remove_allowed_quote", "args": bytes([16]) + admin + musd_bytes},
            # opcode 11: set_listing_requirements(caller 32B, min_liquidity 8B, min_holders 8B)
            {"label": "dex_governance.set_listing_requirements", "args": bytes([11]) + admin + u64le(1000) + u64le(1)},
            # opcode 1: propose_new_pair(proposer 32B, base 32B, quote 32B)
            {"label": "dex_governance.propose_new_pair", "args": bytes([1]) + admin + weth_bytes + molt_bytes},
            # opcode 2: vote(voter 32B, proposal_id 8B, vote 1B)
            {"label": "dex_governance.vote", "args": bytes([2]) + admin + u64le(0) + bytes([1])},
            # opcode 3: finalize_proposal(caller 32B, proposal_id 8B)
            {"label": "dex_governance.finalize_proposal", "args": bytes([3]) + admin + u64le(0)},
            # opcode 4: execute_proposal(caller 32B, proposal_id 8B)
            {"label": "dex_governance.execute_proposal", "args": bytes([4]) + admin + u64le(0)},
            # opcode 9: propose_fee_change(proposer 32B, pair_id 8B, maker i16, taker u16)
            {"label": "dex_governance.propose_fee_change", "args": bytes([9]) + admin + u64le(1) + i16le(-1) + u16le(5)},
            # opcode 10: emergency_delist(caller 32B, pair_id 8B)
            {"label": "dex_governance.emergency_delist", "args": bytes([10]) + admin + u64le(1)},
            # opcode 7: get_proposal_count
            {"label": "dex_governance.get_proposal_count", "args": bytes([7])},
            # opcode 8: get_proposal_info(proposal_id 8B)
            {"label": "dex_governance.get_proposal_info", "args": bytes([8]) + u64le(0)},
            # opcode 12: emergency_pause(caller 32B)
            {"label": "dex_governance.emergency_pause", "args": bytes([12]) + admin},
            # opcode 13: emergency_unpause(caller 32B)
            {"label": "dex_governance.emergency_unpause", "args": bytes([13]) + admin},
        ],
        # ─── DEX_MARGIN (16 opcodes, NO separate initialize export) ───
        "dex_margin": [
            {"label": "dex_margin.initialize", "args": bytes([0]) + admin},
            # opcode 15: set_moltcoin_address(caller 32B, addr 32B)
            {"label": "dex_margin.set_moltcoin_address", "args": bytes([15]) + admin + molt_bytes},
            # opcode 1: set_mark_price(caller 32B, pair_id 8B, price 8B)
            {"label": "dex_margin.set_mark_price", "args": bytes([1]) + admin + u64le(1) + u64le(1_000_000_000)},
            # opcode 7: set_max_leverage(caller 32B, max 8B)
            {"label": "dex_margin.set_max_leverage", "args": bytes([7]) + admin + u64le(10)},
            # opcode 8: set_maintenance_margin(caller 32B, margin_bps 8B)
            {"label": "dex_margin.set_maintenance_margin", "args": bytes([8]) + admin + u64le(500)},
            # opcode 2: open_position(trader 32B, pair_id 8B, side 1B, size 8B, leverage 8B, margin 8B)
            {"label": "dex_margin.open_position", "args": bytes([2]) + admin + u64le(1) + bytes([0]) + u64le(1_000_000_000) + u64le(2) + u64le(300_000_000)},
            # opcode 4: add_margin(caller 32B, position_id 8B, amount 8B)
            {"label": "dex_margin.add_margin", "args": bytes([4]) + admin + u64le(1) + u64le(10_000_000)},
            # opcode 5: remove_margin(caller 32B, position_id 8B, amount 8B)
            {"label": "dex_margin.remove_margin", "args": bytes([5]) + admin + u64le(1) + u64le(1_000_000)},
            # opcode 10: get_position_info(position_id 8B)
            {"label": "dex_margin.get_position_info", "args": bytes([10]) + u64le(1)},
            # opcode 11: get_margin_ratio(position_id 8B)
            {"label": "dex_margin.get_margin_ratio", "args": bytes([11]) + u64le(1)},
            # opcode 12: get_tier_info(tier 8B)
            {"label": "dex_margin.get_tier_info", "args": bytes([12]) + u64le(0)},
            # opcode 3: close_position(caller 32B, position_id 8B)
            {"label": "dex_margin.close_position", "args": bytes([3]) + admin + u64le(1)},
            # opcode 6: liquidate(caller 32B, position_id 8B)
            {"label": "dex_margin.liquidate", "args": bytes([6]) + admin + u64le(99)},  # non-existent position = safe
            # opcode 9: withdraw_insurance(caller 32B, amount 8B)
            {"label": "dex_margin.withdraw_insurance", "args": bytes([9]) + admin + u64le(0)},
            # opcode 13: emergency_pause(caller 32B)
            {"label": "dex_margin.emergency_pause", "args": bytes([13]) + admin},
            # opcode 14: emergency_unpause(caller 32B)
            {"label": "dex_margin.emergency_unpause", "args": bytes([14]) + admin},
        ],
        # ─── DEX_REWARDS (16 opcodes) ───
        "dex_rewards": [
            {"label": "dex_rewards.initialize", "args": bytes([0]) + admin},
            # opcode 12: set_moltcoin_address(caller 32B, addr 32B)
            {"label": "dex_rewards.set_moltcoin_address", "args": bytes([12]) + admin + molt_bytes},
            # opcode 13: set_rewards_pool(caller 32B, addr 32B)
            {"label": "dex_rewards.set_rewards_pool", "args": bytes([13]) + admin + admin},
            # opcode 5: set_reward_rate(caller 32B, pair_id 8B, rate 8B)
            {"label": "dex_rewards.set_reward_rate", "args": bytes([5]) + admin + u64le(1) + u64le(100)},
            # opcode 11: set_referral_rate(caller 32B, rate 8B)
            {"label": "dex_rewards.set_referral_rate", "args": bytes([11]) + admin + u64le(500)},
            # opcode 1: record_trade(trader 32B, fee_paid 8B, volume 8B)
            {"label": "dex_rewards.record_trade", "args": bytes([1]) + admin + u64le(1_000) + u64le(50_000)},
            # opcode 4: register_referral(trader 32B, referrer 32B)
            {"label": "dex_rewards.register_referral", "args": bytes([4]) + user2 + admin},
            # opcode 6: accrue_lp_rewards(pair_id 8B, provider 32B, liquidity 8B)
            {"label": "dex_rewards.accrue_lp_rewards", "args": bytes([6]) + u64le(1) + admin + u64le(1000)},
            # opcode 2: claim_trading_rewards(trader 32B)
            {"label": "dex_rewards.claim_trading_rewards", "args": bytes([2]) + admin},
            # opcode 3: claim_lp_rewards(provider 32B, pair_id 8B)
            {"label": "dex_rewards.claim_lp_rewards", "args": bytes([3]) + admin + u64le(1)},
            # opcode 7: get_pending_rewards(trader 32B)
            {"label": "dex_rewards.get_pending_rewards", "args": bytes([7]) + admin},
            # opcode 8: get_trading_tier(trader 32B)
            {"label": "dex_rewards.get_trading_tier", "args": bytes([8]) + admin},
            # opcode 14: get_referral_rate
            {"label": "dex_rewards.get_referral_rate", "args": bytes([14])},
            # opcode 15: get_total_distributed
            {"label": "dex_rewards.get_total_distributed", "args": bytes([15])},
            # opcode 9: emergency_pause(caller 32B)
            {"label": "dex_rewards.emergency_pause", "args": bytes([9]) + admin},
            # opcode 10: emergency_unpause(caller 32B)
            {"label": "dex_rewards.emergency_unpause", "args": bytes([10]) + admin},
        ],
        # ─── DEX_ROUTER (12 opcodes, NO separate initialize export) ───
        "dex_router": [
            {"label": "dex_router.initialize", "args": bytes([0]) + admin},
            # opcode 1: set_addresses(caller 32B, core 32B, amm 32B, legacy 32B)
            {"label": "dex_router.set_addresses", "args": bytes([1]) + admin + dex_core_bytes + dex_amm_bytes + zero32},
            # opcode 2: register_route(caller 32B, token_in 32B, token_out 32B, route_type 8B, pool_or_pair_id 8B, secondary_id 8B, split_percent 8B)
            {"label": "dex_router.register_route", "args": bytes([2]) + admin + weth_bytes + molt_bytes + u64le(1) + u64le(1) + u64le(0) + u64le(50)},
            # opcode 4: set_route_enabled(caller 32B, route_id 8B, enabled 1B)
            {"label": "dex_router.set_route_enabled", "args": bytes([4]) + admin + u64le(1) + bytes([1])},
            # opcode 3: swap(trader 32B, route_id 8B, amount_in 8B, min_out 8B)
            {"label": "dex_router.swap", "args": bytes([3]) + admin + u64le(1) + u64le(1000) + u64le(0)},
            # opcode 9: multi_hop_swap(trader 32B, route_ids_count 8B, [route_id 8B]*, amount_in 8B, min_out 8B)
            {"label": "dex_router.multi_hop_swap", "args": bytes([9]) + admin + u64le(1) + u64le(1) + u64le(100) + u64le(0)},
            # opcode 5: get_best_route(token_in 32B, token_out 32B, amount 8B)
            {"label": "dex_router.get_best_route", "args": bytes([5]) + weth_bytes + molt_bytes + u64le(1000)},
            # opcode 6: get_route_info(route_id 8B)
            {"label": "dex_router.get_route_info", "args": bytes([6]) + u64le(1)},
            # opcode 10: get_route_count
            {"label": "dex_router.get_route_count", "args": bytes([10])},
            # opcode 11: get_swap_count
            {"label": "dex_router.get_swap_count", "args": bytes([11])},
            # opcode 7: emergency_pause(caller 32B)
            {"label": "dex_router.emergency_pause", "args": bytes([7]) + admin},
            # opcode 8: emergency_unpause(caller 32B)
            {"label": "dex_router.emergency_unpause", "args": bytes([8]) + admin},
        ],
        # ─── PREDICTION_MARKET (34 opcodes) ───
        "prediction_market": [
            {"label": "prediction_market.initialize", "args": bytes([0]) + admin},
            # opcode 18: set_moltyid_address(caller 32B, addr 32B)
            {"label": "prediction_market.set_moltyid_address", "args": bytes([18]) + admin + moltyid_bytes},
            # opcode 19: set_oracle_address(caller 32B, addr 32B)
            {"label": "prediction_market.set_oracle_address", "args": bytes([19]) + admin + oracle_bytes},
            # opcode 20: set_musd_address(caller 32B, addr 32B)
            {"label": "prediction_market.set_musd_address", "args": bytes([20]) + admin + musd_bytes},
            # opcode 21: set_dex_gov_address(caller 32B, addr 32B)
            {"label": "prediction_market.set_dex_gov_address", "args": bytes([21]) + admin + dex_gov_bytes},
            # opcode 1: create_market(creator 32B, category 1B, close_slot 8B, outcome_count 1B, question_hash 32B, question_len 4B, question ...)
            {"label": "prediction_market.create_market",
             "args": bytes([1]) + admin + bytes([0]) + u64le(99_999_999) + bytes([2]) + admin + u32le(4) + b"test"},
            # opcode 27: get_market_count
            {"label": "prediction_market.get_market_count", "args": bytes([27])},
            # opcode 23: get_market(market_id 8B)
            {"label": "prediction_market.get_market", "args": bytes([23]) + u64le(0)},
            # opcode 25: get_price(market_id 8B, outcome 1B)
            {"label": "prediction_market.get_price", "args": bytes([25]) + u64le(0) + bytes([0])},
            # opcode 24: get_outcome_pool(market_id 8B, outcome 1B)
            {"label": "prediction_market.get_outcome_pool", "args": bytes([24]) + u64le(0) + bytes([0])},
            # opcode 31: get_pool_reserves(market_id 8B)
            {"label": "prediction_market.get_pool_reserves", "args": bytes([31]) + u64le(0)},
            # opcode 32: get_platform_stats
            {"label": "prediction_market.get_platform_stats", "args": bytes([32])},
            # opcode 29: quote_buy(buyer 32B, market_id 8B, outcome 1B, amount 8B)
            {"label": "prediction_market.quote_buy", "args": bytes([29]) + admin + u64le(0) + bytes([0]) + u64le(1000)},
            # opcode 30: quote_sell(seller 32B, market_id 8B, outcome 1B, amount 8B)
            {"label": "prediction_market.quote_sell", "args": bytes([30]) + admin + u64le(0) + bytes([0]) + u64le(100)},
            # opcode 2: add_initial_liquidity(provider 32B, market_id 8B, amount 8B, odds[2B*2])
            {"label": "prediction_market.add_initial_liquidity",
             "args": bytes([2]) + admin + u64le(0) + u64le(10_000) + u16le(5000) + u16le(5000)},
            # opcode 3: add_liquidity(provider 32B, market_id 8B, amount 8B)
            {"label": "prediction_market.add_liquidity", "args": bytes([3]) + admin + u64le(0) + u64le(5_000)},
            # opcode 4: buy_shares(buyer 32B, market_id 8B, outcome 1B, amount 8B)
            {"label": "prediction_market.buy_shares", "args": bytes([4]) + admin + u64le(0) + bytes([0]) + u64le(1_000)},
            # opcode 5: sell_shares(seller 32B, market_id 8B, outcome 1B, amount 8B)
            {"label": "prediction_market.sell_shares", "args": bytes([5]) + admin + u64le(0) + bytes([0]) + u64le(100)},
            # opcode 6: mint_complete_set(user 32B, market_id 8B, amount 8B)
            {"label": "prediction_market.mint_complete_set", "args": bytes([6]) + admin + u64le(0) + u64le(500)},
            # opcode 7: redeem_complete_set(user 32B, market_id 8B, amount 8B)
            {"label": "prediction_market.redeem_complete_set", "args": bytes([7]) + admin + u64le(0) + u64le(100)},
            # opcode 26: get_position(user 32B, market_id 8B)
            {"label": "prediction_market.get_position", "args": bytes([26]) + admin + u64le(0)},
            # opcode 28: get_user_markets(user 32B)
            {"label": "prediction_market.get_user_markets", "args": bytes([28]) + admin},
            # opcode 33: get_lp_balance(provider 32B, market_id 8B)
            {"label": "prediction_market.get_lp_balance", "args": bytes([33]) + admin + u64le(0)},
            # opcode 15: withdraw_liquidity(provider 32B, market_id 8B, amount 8B)
            {"label": "prediction_market.withdraw_liquidity", "args": bytes([15]) + admin + u64le(0) + u64le(100)},
            # opcode 8: submit_resolution(resolver 32B, market_id 8B, winning_outcome 1B, attestation_hash 32B, bond 8B)
            {"label": "prediction_market.submit_resolution", "args": bytes([8]) + admin + u64le(0) + bytes([0]) + admin + u64le(1000)},
            # opcode 9: challenge_resolution(challenger 32B, market_id 8B, evidence_hash 32B, bond 8B)
            {"label": "prediction_market.challenge_resolution", "args": bytes([9]) + admin + u64le(0) + admin + u64le(1000)},
            # opcode 10: finalize_resolution(caller 32B, market_id 8B)
            {"label": "prediction_market.finalize_resolution", "args": bytes([10]) + admin + u64le(0)},
            # opcode 11: dao_resolve(caller 32B, market_id 8B, winning_outcome 1B)
            {"label": "prediction_market.dao_resolve", "args": bytes([11]) + admin + u64le(0) + bytes([0])},
            # opcode 12: dao_void(caller 32B, market_id 8B)
            {"label": "prediction_market.dao_void", "args": bytes([12]) + admin + u64le(0)},
            # opcode 13: redeem_shares(user 32B, market_id 8B, outcome 1B)
            {"label": "prediction_market.redeem_shares", "args": bytes([13]) + admin + u64le(0) + bytes([0])},
            # opcode 14: reclaim_collateral(user 32B, market_id 8B)
            {"label": "prediction_market.reclaim_collateral", "args": bytes([14]) + admin + u64le(0)},
            # opcode 22: close_market(caller 32B, market_id 8B)
            {"label": "prediction_market.close_market", "args": bytes([22]) + admin + u64le(0)},
            # opcode 16: emergency_pause(caller 32B)
            {"label": "prediction_market.emergency_pause", "args": bytes([16]) + admin},
            # opcode 17: emergency_unpause(caller 32B)
            {"label": "prediction_market.emergency_unpause", "args": bytes([17]) + admin},
        ],
    }


async def main() -> int:
    t_start = time.time()
    print("\n" + "=" * 70)
    print("  MOLTCHAIN COMPREHENSIVE E2E — ALL 27 CONTRACTS, ALL FUNCTIONS")
    print(f"  RPC: {RPC_URL}  |  Timeout: {TX_CONFIRM_TIMEOUT}s")
    print("=" * 70 + "\n")

    conn = Connection(RPC_URL)
    try:
        await conn.health()
        report("PASS", "validator healthy")
    except Exception as e:
        report("FAIL", f"validator unreachable: {e}")
        return 1

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

    # Ensure secondary has funds via deployer transfer as fallback
    try:
        bal = await conn.get_balance(secondary.public_key())
        bal_val = bal.get("balance", bal) if isinstance(bal, dict) else bal
        if isinstance(bal_val, (int, float)) and bal_val < 1_000_000_000:
            # Transfer 10 MOLT from deployer to secondary
            blockhash = await conn.get_recent_blockhash()
            ix = TransactionBuilder.transfer(deployer.public_key(), secondary.public_key(), 10_000_000_000)
            tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(deployer)
            sig = await conn.send_transaction(tx)
            await wait_tx(conn, sig)
            report("PASS", f"secondary funded via transfer (10 MOLT)")
    except Exception as e:
        report("PASS", f"secondary transfer fallback: {e}")

    # Discover contracts
    contracts = await discover_contracts(conn)
    report("PASS" if len(contracts) == 27 else "FAIL", f"discovered {len(contracts)}/27 contracts")

    if len(contracts) < 27:
        missing = set(SYMBOL_TO_DIR.values()) - set(contracts.keys())
        for m in sorted(missing):
            report("FAIL", f"missing contract: {m}")

    # ─── Phase 1: Named-export contracts ───
    named_scenarios = build_named_scenarios(deployer, secondary, contracts)
    print(f"\n{'─' * 70}")
    print(f"  Phase 1: Named-export contracts ({len(named_scenarios)} contracts)")
    print(f"{'─' * 70}")

    for contract_name, steps in named_scenarios.items():
        program = contracts.get(contract_name)
        if not program:
            report("FAIL", f"{contract_name}: not deployed")
            continue

        print(f"\n  ┌── {contract_name} ({len(steps)} functions)")
        for step in steps:
            fn = step["fn"]
            actor = deployer if step.get("actor") != "secondary" else secondary
            label = f"{contract_name}.{fn}"
            if "binary" in step:
                bin_data, lay = step["binary"]
                await send_and_confirm_named(conn, actor, program, fn, label=label,
                                             binary_args=bin_data, layout=lay)
            else:
                args = step.get("args", {})
                await send_and_confirm_named(conn, actor, program, fn, args, label)
        print(f"  └── {contract_name} done")

    # ─── Phase 2: Opcode-dispatch contracts ───
    opcode_scenarios = build_opcode_scenarios(deployer, secondary, contracts)
    print(f"\n{'─' * 70}")
    print(f"  Phase 2: Opcode-dispatch contracts ({len(opcode_scenarios)} contracts)")
    print(f"{'─' * 70}")

    for contract_name, steps in opcode_scenarios.items():
        program = contracts.get(contract_name)
        if not program:
            report("FAIL", f"{contract_name}: not deployed")
            continue

        print(f"\n  ┌── {contract_name} ({len(steps)} opcodes)")
        for step in steps:
            label = step["label"]
            opcode_args = step["args"]
            await send_and_confirm_opcode(conn, deployer, program, opcode_args, label)
        print(f"  └── {contract_name} done")

    # ─── Summary ───
    elapsed = time.time() - t_start
    print(f"\n{'=' * 70}")
    print(f"  SUMMARY: PASS={PASS}  FAIL={FAIL}  SKIP={SKIP}")
    print(f"  Elapsed: {elapsed:.1f}s ({elapsed/60:.1f}min)")
    print(f"{'=' * 70}")

    # Write report
    report_path = ROOT / "tests" / "artifacts" / "comprehensive-e2e-report.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps({
        "summary": {"pass": PASS, "fail": FAIL, "skip": SKIP},
        "elapsed_seconds": round(elapsed, 1),
        "results": RESULTS,
    }, indent=2))
    print(f"  Report: {report_path}")

    return 1 if FAIL > 0 else 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))

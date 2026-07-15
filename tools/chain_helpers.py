#!/usr/bin/env python3
"""Shared helpers for authenticated local-chain maintenance tools."""

import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Optional

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "sdk", "python"))

from lichen import Connection, Instruction, Keypair, PublicKey, TransactionBuilder


REPO_ROOT = Path(__file__).resolve().parent.parent
CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
CONTRACT_TX_LOOKUP_ATTEMPTS = int(
    os.environ.get("LICHEN_CONTRACT_TX_LOOKUP_ATTEMPTS", "80")
)
CONTRACT_TX_LOOKUP_DELAY_SECS = float(
    os.environ.get("LICHEN_CONTRACT_TX_LOOKUP_DELAY_SECS", "0.25")
)


def find_genesis_keypair_path(role: str, network: Optional[str] = None) -> Path:
    network = (network or os.environ.get("LICHEN_NETWORK", "testnet")).lower()
    filename = f"{role}-lichen-{network}-1.json"
    data_dir = REPO_ROOT / "data"
    preferred_state = "7001" if network == "testnet" else "8001"
    candidates = [
        data_dir / f"state-{preferred_state}" / "genesis-keys" / filename,
        data_dir / f"state-{network}" / "genesis-keys" / filename,
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate

    matches = sorted(data_dir.glob(f"state-*/genesis-keys/{filename}"))
    if matches:
        return matches[0]
    raise FileNotFoundError(
        f"Genesis keypair not found for role '{role}' on network '{network}'"
    )


def load_genesis_keypair(role: str, network: Optional[str] = None) -> Keypair:
    return Keypair.load(find_genesis_keypair_path(role, network))


async def await_contract_success(
    conn: Connection, signature: str, context: str
) -> str:
    last_error = None
    for _ in range(CONTRACT_TX_LOOKUP_ATTEMPTS):
        await asyncio.sleep(CONTRACT_TX_LOOKUP_DELAY_SECS)
        try:
            tx = await conn.get_transaction(signature)
        except Exception as exc:
            last_error = str(exc)
            continue
        if not tx:
            continue
        if tx.get("error"):
            raise RuntimeError(f"{context} failed: {tx['error']}")
        return_code = tx.get("return_code")
        if return_code not in (None, 0):
            return_data = tx.get("return_data")
            details = f", return_data={return_data}" if return_data else ""
            raise RuntimeError(f"{context} returned code {return_code}{details}")
        return signature

    if last_error:
        raise RuntimeError(f"{context} confirmation unavailable: {last_error}")
    raise RuntimeError(f"{context} confirmation unavailable: transaction not found")


async def call_contract_raw(
    conn: Connection,
    caller: Keypair,
    program_pubkey: PublicKey,
    func: str,
    raw_args: list,
    value: int = 0,
    compute_budget: Optional[int] = None,
    compute_unit_price: Optional[int] = None,
) -> str:
    payload = json.dumps(
        {"Call": {"function": func, "args": raw_args, "value": value}}
    )
    instruction = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.address(), program_pubkey],
        data=payload.encode(),
    )
    transaction = (
        TransactionBuilder()
        .add(instruction)
        .set_recent_blockhash(await conn.get_recent_blockhash())
        .set_compute_budget(compute_budget)
        .set_compute_unit_price(compute_unit_price)
        .build_and_sign(caller, await conn.get_chain_id())
    )
    signature = await conn.send_transaction(transaction)
    return await await_contract_success(
        conn, signature, f"{program_pubkey}.{func}"
    )

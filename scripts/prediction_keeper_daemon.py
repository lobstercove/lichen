#!/usr/bin/env python3
"""
Prediction market keeper daemon (safe scope)

Sweeps:
- close expired ACTIVE markets (opcode 22: close_market)
- finalize RESOLVING markets after dispute window (opcode 10: finalize_resolution)

Default mode is dry-run. Enable live submissions with LICHEN_KEEPER_DRY_RUN=false.
"""

import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import httpx

ROOT = Path(__file__).resolve().parents[1]
SDK_PY = ROOT / "sdk" / "python"
if str(SDK_PY) not in sys.path:
    sys.path.insert(0, str(SDK_PY))

from lichen.connection import Connection  # noqa: E402  # type: ignore[import-not-found]
from lichen.keypair import Keypair  # noqa: E402  # type: ignore[import-not-found]
from lichen.publickey import PublicKey  # noqa: E402  # type: ignore[import-not-found]
from lichen.transaction import Instruction, TransactionBuilder  # noqa: E402  # type: ignore[import-not-found]

CONTRACT_PROGRAM_ID = PublicKey(bytes([0xFF] * 32))

RPC_URL = os.getenv("LICHEN_RPC_URL", "http://127.0.0.1:8899").rstrip("/")
API_BASE = os.getenv("LICHEN_API_BASE", f"{RPC_URL}/api/v1").rstrip("/")
INTERVAL_SECS = max(5, int(os.getenv("LICHEN_KEEPER_INTERVAL_SECS", "15")))
MAX_ACTIONS_PER_TICK = max(1, int(os.getenv("LICHEN_KEEPER_MAX_ACTIONS_PER_TICK", "20")))
DRY_RUN = os.getenv("LICHEN_KEEPER_DRY_RUN", "true").strip().lower() not in {"0", "false", "no", "off"}
KEYPAIR_PATH = Path(os.path.expanduser(os.getenv("LICHEN_KEEPER_KEYPAIR", "~/.lichen/keypairs/id.json")))
KEYPAIR_PASSWORD = os.getenv("LICHEN_KEEPER_KEYPAIR_PASSWORD")
HTTP_TIMEOUT_SECS = float(os.getenv("LICHEN_KEEPER_HTTP_TIMEOUT", "12"))


def log(msg: str) -> None:
    print(f"[prediction-keeper] {msg}", flush=True)


async def api_get(client: httpx.AsyncClient, path: str) -> Any:
    res = await client.get(f"{API_BASE}{path}")
    res.raise_for_status()
    body = res.json()
    if isinstance(body, dict) and "success" in body:
        if not body.get("success", False):
            raise RuntimeError(body.get("error") or f"API call failed: {path}")
        return body.get("data")
    return body


async def resolve_predict_program(conn: Connection) -> PublicKey:
    try:
        entry = await conn._rpc("getSymbolRegistry", ["PREDICT"])
        if isinstance(entry, dict):
            program = entry.get("program") or entry.get("address")
            if isinstance(program, str) and program:
                return PublicKey(program)
    except Exception:
        pass

    entries = await conn._rpc("getAllSymbolRegistry", [512])
    rows = entries.get("entries", []) if isinstance(entries, dict) else []
    for row in rows:
        if not isinstance(row, dict):
            continue
        if row.get("symbol") == "PREDICT" and isinstance(row.get("program"), str):
            return PublicKey(row["program"])

    raise RuntimeError("PREDICT program address not found in symbol registry")


def contract_call_instruction(
    caller: PublicKey,
    target_program: PublicKey,
    args_bytes: bytes,
    value: int = 0,
) -> Instruction:
    payload = {
        "Call": {
            "function": "call",
            "args": list(args_bytes),
            "value": int(value),
        }
    }
    data = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    return Instruction(
        program_id=CONTRACT_PROGRAM_ID,
        accounts=[caller, target_program],
        data=data,
    )


def encode_close_market(caller: PublicKey, market_id: int) -> bytes:
    return bytes([22]) + caller.to_bytes() + int(market_id).to_bytes(8, "little", signed=False)


def encode_finalize_resolution(caller: PublicKey, market_id: int) -> bytes:
    return bytes([10]) + caller.to_bytes() + int(market_id).to_bytes(8, "little", signed=False)


async def send_contract_call(
    conn: Connection,
    signer: Keypair,
    target_program: PublicKey,
    args_bytes: bytes,
    value: int = 0,
) -> str:
    recent_blockhash = await conn.get_recent_blockhash()
    ix = contract_call_instruction(signer.public_key(), target_program, args_bytes, value)
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(recent_blockhash)
        .build_and_sign(signer)
    )
    return await conn.send_transaction(tx)


def pick_actions(
    markets: List[Dict[str, Any]],
    current_slot: int,
    dispute_period_slots: int,
) -> Tuple[List[int], List[int]]:
    close_ids: List[int] = []
    finalize_ids: List[int] = []

    for m in markets:
        if not isinstance(m, dict):
            continue
        market_id = int(m.get("id") or 0)
        if market_id <= 0:
            continue

        status = str(m.get("status") or "").lower()
        close_slot = int(m.get("close_slot") or 0)
        resolve_slot = int(m.get("resolve_slot") or 0)

        if status == "active" and close_slot > 0 and current_slot > close_slot:
            close_ids.append(market_id)
            continue

        if status == "resolving" and resolve_slot > 0:
            dispute_end_slot = resolve_slot + max(0, dispute_period_slots)
            if current_slot > dispute_end_slot:
                finalize_ids.append(market_id)

    return close_ids, finalize_ids


async def sweep_once(
    conn: Connection,
    http: httpx.AsyncClient,
    signer: Keypair,
    predict_program: PublicKey,
) -> None:
    stats = await api_get(http, "/prediction-market/stats")
    cfg = await api_get(http, "/prediction-market/config")
    markets_page = await api_get(http, "/prediction-market/markets?limit=200")

    current_slot = int((stats or {}).get("current_slot") or 0)
    dispute_period_slots = int((cfg or {}).get("dispute_period_slots") or 0)

    if isinstance(markets_page, dict):
        markets = markets_page.get("markets", [])
    elif isinstance(markets_page, list):
        markets = markets_page
    else:
        markets = []

    close_ids, finalize_ids = pick_actions(markets, current_slot, dispute_period_slots)
    planned = [("close", mid) for mid in close_ids] + [("finalize", mid) for mid in finalize_ids]

    if not planned:
        log(f"slot={current_slot} no actions")
        return

    limited = planned[:MAX_ACTIONS_PER_TICK]
    log(
        f"slot={current_slot} actions={len(planned)} executing={len(limited)} "
        f"mode={'dry-run' if DRY_RUN else 'live'}"
    )

    for action, market_id in limited:
        if action == "close":
            args = encode_close_market(signer.public_key(), market_id)
        else:
            args = encode_finalize_resolution(signer.public_key(), market_id)

        if DRY_RUN:
            log(f"DRY-RUN {action} market_id={market_id}")
            continue

        try:
            sig = await send_contract_call(conn, signer, predict_program, args)
            log(f"LIVE {action} market_id={market_id} sig={sig}")
        except Exception as exc:
            log(f"ERROR action={action} market_id={market_id} err={exc}")


async def run() -> None:
    if not KEYPAIR_PATH.exists():
        raise FileNotFoundError(f"keeper keypair not found: {KEYPAIR_PATH}")

    signer = Keypair.load(KEYPAIR_PATH, KEYPAIR_PASSWORD)
    conn = Connection(RPC_URL)

    async with httpx.AsyncClient(timeout=HTTP_TIMEOUT_SECS) as http:
        predict_program = await resolve_predict_program(conn)
        log(
            f"started rpc={RPC_URL} api={API_BASE} signer={signer.public_key()} "
            f"predict_program={predict_program} interval={INTERVAL_SECS}s dry_run={DRY_RUN}"
        )

        while True:
            try:
                await sweep_once(conn, http, signer, predict_program)
            except Exception as exc:
                log(f"tick error: {exc}")
            await asyncio.sleep(INTERVAL_SECS)


if __name__ == "__main__":
    try:
        asyncio.run(run())
    except KeyboardInterrupt:
        log("stopped")

#!/usr/bin/env python3
"""
Lichen custody withdrawal E2E.

Exercises the live withdrawal slice supported by the local validator + custody
stack without requiring external Solana/EVM RPCs:

1. Fund the wrapped-token admin if it has no spendable LICN for fees.
2. Mint wSOL to a fresh user with the on-chain genesis token admin.
3. Create a custody withdrawal job.
4. Burn the user's wrapped tokens on Lichen.
5. Submit the burn transaction signature to custody.
6. Poll custody audit events until the burn is verified.

This test intentionally stops at custody burn verification. Outbound chain
confirmation is only possible when custody is also configured with external
chain RPC endpoints.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import struct
import sys
import time
from pathlib import Path
from typing import Any, Optional, Tuple
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from lichen import Connection, Keypair, PublicKey  # type: ignore

RPC_URL = os.getenv("RPC_URL", "http://127.0.0.1:8899")
CUSTODY_URL = os.getenv("CUSTODY_URL", "http://127.0.0.1:9105")
EXPLICIT_GENESIS_KEYS_DIR = os.getenv("GENESIS_KEYS_DIR")
DEFAULT_GENESIS_KEYS_DIR = Path(
    EXPLICIT_GENESIS_KEYS_DIR or str(ROOT / "data" / "state-7001" / "genesis-keys")
)
WITHDRAWAL_TOKEN_SYMBOL = os.getenv("WITHDRAWAL_TOKEN_SYMBOL", "WSOL")
WITHDRAWAL_ASSET = os.getenv("WITHDRAWAL_ASSET", "wSOL")
WITHDRAWAL_DEST_CHAIN = os.getenv("WITHDRAWAL_DEST_CHAIN", "solana")
WITHDRAWAL_AMOUNT_SPORES = int(os.getenv("WITHDRAWAL_AMOUNT_SPORES", "1000000000"))
ADMIN_FEE_FUNDING_SPORES = int(os.getenv("ADMIN_FEE_FUNDING_SPORES", "2000000000"))
USER_FEE_FUNDING_SPORES = int(os.getenv("USER_FEE_FUNDING_SPORES", "1000000000"))

PASS = 0
FAIL = 0


def _resolve_fixture_defaults() -> tuple[str, Path, str]:
    token = (os.getenv("CUSTODY_API_AUTH_TOKEN") or "").strip()
    token_source = "env" if token else ""
    genesis_keys_dir = DEFAULT_GENESIS_KEYS_DIR
    helper = ROOT / "tests" / "resolve-custody-withdrawal-fixtures.py"

    if not helper.exists():
        return token, genesis_keys_dir, token_source

    if token and EXPLICIT_GENESIS_KEYS_DIR:
        return token, genesis_keys_dir, token_source

    try:
        result = subprocess.run(
            [sys.executable, str(helper)],
            check=True,
            capture_output=True,
            text=True,
            cwd=str(ROOT),
        )
        payload = json.loads(result.stdout)
    except Exception:
        return token, genesis_keys_dir, token_source

    if not token:
        discovered_token = str(payload.get("custody_api_auth_token") or "").strip()
        if discovered_token:
            token = discovered_token
            token_source = str(payload.get("custody_api_auth_token_source") or "discovered")

    if not EXPLICIT_GENESIS_KEYS_DIR:
        discovered_genesis_dir = str(payload.get("genesis_keys_dir") or "").strip()
        if discovered_genesis_dir:
            genesis_keys_dir = Path(discovered_genesis_dir)

    return token, genesis_keys_dir, token_source


CUSTODY_API_AUTH_TOKEN, GENESIS_KEYS_DIR, CUSTODY_API_AUTH_TOKEN_SOURCE = _resolve_fixture_defaults()


def report(status: str, message: str, detail: str = "") -> None:
    global PASS, FAIL
    if status == "PASS":
        PASS += 1
        tag = "PASS"
    else:
        FAIL += 1
        tag = "FAIL"
    print(f"  {tag}  {message}")
    if detail:
        print(f"         {detail}")


def _http_json(
    url: str,
    *,
    method: str = "GET",
    body: Optional[dict[str, Any]] = None,
    headers: Optional[dict[str, str]] = None,
) -> Tuple[int, Any]:
    request_headers = {"Content-Type": "application/json"}
    if headers:
        request_headers.update(headers)

    payload = json.dumps(body).encode("utf-8") if body is not None else None
    request = Request(url, data=payload, headers=request_headers, method=method)

    try:
        with urlopen(request, timeout=10) as response:
            raw = response.read().decode("utf-8")
            return response.status, json.loads(raw) if raw else None
    except HTTPError as exc:
        raw = exc.read().decode("utf-8")
        try:
            parsed = json.loads(raw) if raw else None
        except json.JSONDecodeError:
            parsed = raw
        return exc.code, parsed


def _extract_spores(balance: Any) -> int:
    if isinstance(balance, dict):
        for key in ("spendable", "spores", "balance", "amount"):
            value = balance.get(key)
            if isinstance(value, int):
                return value
    if isinstance(balance, int):
        return balance
    return 0


def _extract_token_balance(balance: Any) -> int:
    if isinstance(balance, dict):
        value = balance.get("balance")
        if isinstance(value, int):
            return value
        if isinstance(value, str):
            try:
                return int(value)
            except ValueError:
                return 0
    if isinstance(balance, int):
        return balance
    return 0


def _authorization_headers() -> dict[str, str]:
    if not CUSTODY_API_AUTH_TOKEN:
        raise RuntimeError("CUSTODY_API_AUTH_TOKEN is required for custody withdrawal E2E")
    return {"Authorization": f"Bearer {CUSTODY_API_AUTH_TOKEN}"}


def _find_role_keypair_path(role: str) -> Path:
    if not GENESIS_KEYS_DIR.exists():
        raise FileNotFoundError(f"Genesis key directory missing: {GENESIS_KEYS_DIR}")

    for key_file in sorted(GENESIS_KEYS_DIR.iterdir()):
        if key_file.is_file() and key_file.name.startswith(f"{role}-") and key_file.suffix == ".json":
            return key_file

    raise FileNotFoundError(f"No keypair found for role '{role}' in {GENESIS_KEYS_DIR}")


async def wait_for_transaction(
    conn: Connection,
    signature: str,
    timeout_secs: float = 20.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        try:
            info = await conn.get_transaction(signature)
        except Exception as exc:
            if "Transaction not found" in str(exc):
                await asyncio.sleep(0.5)
                continue
            raise
        if info:
            return info
        await asyncio.sleep(0.5)
    raise TimeoutError(f"Transaction {signature} was not indexed before timeout")


async def wait_for_spendable_balance(
    conn: Connection,
    owner: PublicKey,
    minimum_spores: int,
    timeout_secs: float = 20.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        balance = await conn.get_balance(owner)
        if _extract_spores(balance) >= minimum_spores:
            return balance
        await asyncio.sleep(0.5)
    raise TimeoutError(f"Balance for {owner.to_base58()} did not reach {minimum_spores}")


async def get_token_balance(conn: Connection, token_program: PublicKey, owner: PublicKey) -> int:
    result = await conn._rpc("getTokenBalance", [str(token_program), str(owner)])
    return _extract_token_balance(result)


async def wait_for_token_balance(
    conn: Connection,
    token_program: PublicKey,
    owner: PublicKey,
    minimum_balance: int,
    timeout_secs: float = 20.0,
) -> int:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        balance = await get_token_balance(conn, token_program, owner)
        if balance >= minimum_balance:
            return balance
        await asyncio.sleep(0.5)
    raise TimeoutError(
        f"Token balance for {owner.to_base58()} did not reach {minimum_balance}"
    )


async def resolve_symbol_program(conn: Connection, symbol: str) -> PublicKey:
    registry = await conn._rpc("getAllSymbolRegistry", [])
    entries = registry.get("entries", []) if isinstance(registry, dict) else registry
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        if str(entry.get("symbol") or "").upper() != symbol.upper():
            continue
        program = str(entry.get("program") or entry.get("address") or "")
        if program:
            return PublicKey.from_base58(program)
    raise RuntimeError(f"Could not resolve symbol {symbol} from registry")


async def call_contract_checked(
    conn: Connection,
    signer: Keypair,
    contract: PublicKey,
    function_name: str,
    args: bytes,
    timeout_secs: float = 20.0,
) -> dict[str, Any]:
    signature = await conn.call_contract(signer, contract, function_name, args)
    info = await wait_for_transaction(conn, signature, timeout_secs=timeout_secs)
    if info.get("error"):
        raise RuntimeError(f"{function_name} failed: {info['error']}")
    return_code = info.get("return_code")
    if return_code not in (None, 0):
        raise RuntimeError(
            f"{contract}.{function_name} returned code {return_code}, "
            f"return_data={info.get('return_data')}"
        )
    return info


async def wait_for_withdrawal_events(
    job_id: str,
    required_event_types: set[str],
    timeout_secs: float = 30.0,
) -> list[str]:
    deadline = time.monotonic() + timeout_secs
    observed: list[str] = []
    while time.monotonic() < deadline:
        status_code, payload = _http_json(
            f"{CUSTODY_URL.rstrip('/')}/events?entity_id={job_id}&limit=20",
            headers=_authorization_headers(),
        )
        if status_code != 200 or not isinstance(payload, dict):
            await asyncio.sleep(1.0)
            continue

        events = payload.get("events", [])
        observed = [
            str(event.get("event_type"))
            for event in events
            if isinstance(event, dict) and event.get("event_type")
        ]
        if required_event_types.issubset(set(observed)):
            return observed
        await asyncio.sleep(1.0)

    raise TimeoutError(
        f"Did not observe required withdrawal events for {job_id}: "
        f"required={sorted(required_event_types)}, observed={observed}"
    )


async def main() -> int:
    print("\n" + "=" * 62)
    print("  Lichen Custody Withdrawal E2E")
    print(f"  RPC: {RPC_URL}")
    print(f"  Custody: {CUSTODY_URL}")
    print(f"  Genesis keys: {GENESIS_KEYS_DIR}")
    print("=" * 62 + "\n")

    conn = Connection(RPC_URL)

    try:
        await conn.health()
        report("PASS", "RPC health check")
    except Exception as exc:
        report("FAIL", "RPC health check", str(exc))
        print_summary()
        return 1

    try:
        status_code, payload = _http_json(f"{CUSTODY_URL.rstrip('/')}/health")
        if status_code == 200 and isinstance(payload, dict) and payload.get("status") == "ok":
            report("PASS", "Custody health check")
        else:
            raise RuntimeError(f"status={status_code}, payload={payload}")
    except (RuntimeError, URLError, ValueError) as exc:
        report("FAIL", "Custody health check", str(exc))
        print_summary()
        return 1

    try:
        auth_headers = _authorization_headers()
        token_admin = Keypair.load(_find_role_keypair_path("genesis-primary"))
        treasury = Keypair.load(_find_role_keypair_path("treasury"))
        token_program = await resolve_symbol_program(conn, WITHDRAWAL_TOKEN_SYMBOL)
        report(
            "PASS",
            "Withdrawal fixture keypairs loaded",
            f"admin={token_admin.address().to_base58()}, treasury={treasury.address().to_base58()}",
        )
        report(
            "PASS",
            "Wrapped token resolved",
            f"{WITHDRAWAL_TOKEN_SYMBOL} -> {token_program}",
        )
    except Exception as exc:
        report("FAIL", "Withdrawal fixture discovery", str(exc))
        print_summary()
        return 1

    try:
        admin_balance = await conn.get_balance(token_admin.address())
        if _extract_spores(admin_balance) < ADMIN_FEE_FUNDING_SPORES:
            fund_admin_signature = await conn.transfer(
                treasury, token_admin.address(), ADMIN_FEE_FUNDING_SPORES
            )
            await wait_for_transaction(conn, fund_admin_signature)
            report("PASS", "Wrapped token admin funded", fund_admin_signature)
        else:
            report(
                "PASS",
                "Wrapped token admin already funded",
                f"spendable={_extract_spores(admin_balance)}",
            )

        withdrawal_user = Keypair.generate()
        fund_user_signature = await conn.transfer(
            treasury, withdrawal_user.address(), USER_FEE_FUNDING_SPORES
        )
        await wait_for_transaction(conn, fund_user_signature)
        user_balance = await wait_for_spendable_balance(
            conn, withdrawal_user.address(), USER_FEE_FUNDING_SPORES
        )
        report("PASS", "Withdrawal user funded", fund_user_signature)
        report(
            "PASS",
            "Withdrawal user spendable balance",
            f"spores={_extract_spores(user_balance)}",
        )
    except Exception as exc:
        report("FAIL", "Withdrawal funding flow", str(exc))
        print_summary()
        return 1

    try:
        mint_args = (
            bytes(token_admin.address().to_bytes())
            + bytes(withdrawal_user.address().to_bytes())
            + struct.pack("<Q", WITHDRAWAL_AMOUNT_SPORES)
        )
        mint_info = await call_contract_checked(
            conn,
            token_admin,
            token_program,
            "mint",
            mint_args,
        )
        minted_balance = await wait_for_token_balance(
            conn,
            token_program,
            withdrawal_user.address(),
            WITHDRAWAL_AMOUNT_SPORES,
        )
        report("PASS", "Wrapped token minted for withdrawal fixture", mint_info["signature"])
        report("PASS", "Wrapped token balance available", f"balance={minted_balance}")
    except Exception as exc:
        report("FAIL", "Wrapped token mint flow", str(exc))
        print_summary()
        return 1

    try:
        create_status, create_payload = _http_json(
            f"{CUSTODY_URL.rstrip('/')}/withdrawals",
            method="POST",
            headers=auth_headers,
            body={
                "user_id": withdrawal_user.address().to_base58(),
                "asset": WITHDRAWAL_ASSET,
                "amount": WITHDRAWAL_AMOUNT_SPORES,
                "dest_chain": WITHDRAWAL_DEST_CHAIN,
                "dest_address": withdrawal_user.address().to_base58(),
            },
        )
        if create_status != 200 or not isinstance(create_payload, dict):
            raise RuntimeError(f"status={create_status}, payload={create_payload}")
        if create_payload.get("error"):
            raise RuntimeError(str(create_payload["error"]))
        job_id = str(create_payload.get("job_id") or "")
        if len(job_id) != 36:
            raise RuntimeError(f"Unexpected withdrawal payload: {create_payload}")
        report("PASS", "Custody withdrawal job created", job_id)
    except Exception as exc:
        report("FAIL", "Custody withdrawal creation", str(exc))
        print_summary()
        return 1

    try:
        burn_args = bytes(withdrawal_user.address().to_bytes()) + struct.pack(
            "<Q", WITHDRAWAL_AMOUNT_SPORES
        )
        burn_info = await call_contract_checked(
            conn,
            withdrawal_user,
            token_program,
            "burn",
            burn_args,
        )
        remaining_balance = await get_token_balance(
            conn, token_program, withdrawal_user.address()
        )
        if remaining_balance != 0:
            raise RuntimeError(f"Expected burned balance to reach zero, got {remaining_balance}")
        report("PASS", "Wrapped token burn transaction", burn_info["signature"])
        report("PASS", "Wrapped token balance burned", f"balance={remaining_balance}")
    except Exception as exc:
        report("FAIL", "Wrapped token burn flow", str(exc))
        print_summary()
        return 1

    try:
        submit_status, submit_payload = _http_json(
            f"{CUSTODY_URL.rstrip('/')}/withdrawals/{job_id}/burn",
            method="PUT",
            headers=auth_headers,
            body={"burn_tx_signature": burn_info["signature"]},
        )
        if submit_status != 200 or not isinstance(submit_payload, dict):
            raise RuntimeError(f"status={submit_status}, payload={submit_payload}")
        if submit_payload.get("error"):
            raise RuntimeError(str(submit_payload["error"]))
        if submit_payload.get("burn_tx_signature") != burn_info["signature"]:
            raise RuntimeError(f"Unexpected burn submission payload: {submit_payload}")
        report("PASS", "Custody burn signature submitted")

        event_types = await wait_for_withdrawal_events(
            job_id,
            {
                "withdrawal.requested",
                "withdrawal.burn_submitted",
                "withdrawal.burn_confirmed",
            },
        )
        report("PASS", "Custody burn verification events", ", ".join(event_types))
    except Exception as exc:
        report("FAIL", "Custody burn verification flow", str(exc))
        print_summary()
        return 1

    print_summary()
    return 0 if FAIL == 0 else 1


def print_summary() -> None:
    print("\n" + "=" * 62)
    print(f"  Custody Withdrawal E2E Results: PASS {PASS} / FAIL {FAIL} / TOTAL {PASS + FAIL}")
    print("=" * 62)


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
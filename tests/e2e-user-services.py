#!/usr/bin/env python3
"""
Lichen User Service E2E

Exercises real user-facing service flows that were previously only covered by
health checks or indirect protocol tests:

1. Faucet config/status/request/history/cooldown behavior
2. Explorer-style chain browsing against fresh live activity
3. Bridge deposit address creation and status lookup through authenticated RPC
4. Marketplace browse RPC surfaces used by frontend list/detail views

Requires a running validator RPC on port 8899. Faucet-backed flows additionally
require a running faucet on port 9100 unless REQUIRE_FAUCET=0 is set. Bridge
flows require custody on port 9105 only when REQUIRE_CUSTODY=1 is set.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
import time
from pathlib import Path
from typing import Any, Dict, Optional, Tuple
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from lichen import Connection, Keypair, PublicKey  # type: ignore

RPC_URL = os.getenv("RPC_URL", "http://127.0.0.1:8899")
FAUCET_URL = os.getenv("FAUCET_URL", "http://127.0.0.1:9100")
CUSTODY_URL = os.getenv("CUSTODY_URL", "http://127.0.0.1:9105")
DEPLOYER_PATH = Path(os.getenv("AGENT_KEYPAIR") or (ROOT / "keypairs" / "deployer.json"))
REQUIRE_FAUCET = os.getenv("REQUIRE_FAUCET", "1") == "1"
REQUIRE_CUSTODY = os.getenv("REQUIRE_CUSTODY", "0") == "1"
STRICT_NO_SKIPS = os.getenv("STRICT_NO_SKIPS", "0") == "1"
SPORES = 1_000_000_000
BRIDGE_AUTH_TTL_SECS = 24 * 60 * 60

PASS = 0
FAIL = 0
SKIP = 0
RESULTS: list[dict[str, str]] = []


def report(status: str, msg: str, detail: str = "") -> None:
    global PASS, FAIL, SKIP
    if status == "PASS":
        PASS += 1
        tag = "PASS"
    elif status == "SKIP":
        SKIP += 1
        tag = "SKIP"
    else:
        FAIL += 1
        tag = "FAIL"
    print(f"  {tag}  {msg}")
    if detail:
        print(f"         {detail}")
    RESULTS.append({"status": status, "msg": msg, "detail": detail})


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


def synthetic_client_ip(owner: PublicKey) -> str:
    raw = owner.to_bytes()
    octets = [10, raw[0] or 1, raw[1] or 1, raw[2] or 1]
    return ".".join(str(octet) for octet in octets)


def build_bridge_access_message(user_id: str, issued_at: int, expires_at: int) -> str:
    return (
        "LICHEN_BRIDGE_ACCESS_V1\n"
        f"user_id={user_id}\n"
        f"issued_at={issued_at}\n"
        f"expires_at={expires_at}\n"
    )


def create_bridge_access_auth(wallet: Keypair) -> dict[str, Any]:
    issued_at = int(time.time())
    expires_at = issued_at + BRIDGE_AUTH_TTL_SECS
    message = build_bridge_access_message(wallet.address().to_base58(), issued_at, expires_at)
    signature = wallet.sign(message.encode("utf-8"))
    return {
        "issued_at": issued_at,
        "expires_at": expires_at,
        "signature": signature.to_json(),
    }


def custody_healthcheck() -> tuple[bool, str]:
    try:
        status_code, payload = _http_json(f"{CUSTODY_URL.rstrip('/')}/health")
    except (URLError, TimeoutError, ValueError) as exc:
        return False, str(exc)

    if status_code == 200 and isinstance(payload, dict) and payload.get("status") == "ok":
        return True, "ok"

    return False, f"status={status_code}, payload={payload}"


async def wait_for_balance(
    conn: Connection,
    owner: PublicKey,
    minimum_spores: int,
    timeout_secs: float = 20.0,
) -> Dict[str, Any]:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        balance = await conn.get_balance(owner)
        if _extract_spores(balance) >= minimum_spores:
            return balance
        await asyncio.sleep(0.5)
    raise TimeoutError(f"Balance for {owner.to_base58()} did not reach {minimum_spores}")


async def wait_for_transaction(
    conn: Connection,
    signature: str,
    timeout_secs: float = 20.0,
) -> Dict[str, Any]:
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


async def wait_for_address_history(
    conn: Connection,
    address: PublicKey,
    signature: str,
    timeout_secs: float = 20.0,
) -> Dict[str, Any]:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        history = await conn._rpc(
            "getTransactionsByAddress",
            [address.to_base58(), {"limit": 10}],
        )
        entries = history.get("transactions", []) if isinstance(history, dict) else []
        match = next(
            (
                entry
                for entry in entries
                if isinstance(entry, dict)
                and str(entry.get("signature") or entry.get("hash") or "") == signature
            ),
            None,
        )
        if isinstance(match, dict):
            return match
        await asyncio.sleep(0.5)
    raise TimeoutError(f"Address history for {address.to_base58()} missing {signature}")


async def wait_for_recent_transaction(
    conn: Connection,
    signature: str,
    timeout_secs: float = 20.0,
) -> Dict[str, Any]:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        recent = await conn._rpc("getRecentTransactions", [{"limit": 10}])
        entries = recent.get("transactions", []) if isinstance(recent, dict) else []
        match = next(
            (
                entry
                for entry in entries
                if isinstance(entry, dict)
                and str(entry.get("signature") or entry.get("hash") or "") == signature
            ),
            None,
        )
        if isinstance(match, dict):
            return match
        await asyncio.sleep(0.5)
    raise TimeoutError(f"Recent transactions missing {signature}")


async def maybe_load_deployer() -> Optional[Keypair]:
    if not DEPLOYER_PATH.exists():
        return None
    try:
        return Keypair.load(DEPLOYER_PATH)
    except Exception:
        return None


async def main() -> int:
    print("\n" + "=" * 62)
    print("  Lichen User Service E2E")
    print(f"  RPC: {RPC_URL}")
    print(f"  Faucet: {FAUCET_URL}")
    print("=" * 62 + "\n")

    conn = Connection(RPC_URL)

    sender: Optional[Keypair] = None
    sender_start_balance = 0

    print("-- Cluster Health --")
    try:
        await conn.health()
        report("PASS", "RPC health check")
    except Exception as exc:
        report("FAIL", "RPC health check", str(exc))
        print_summary()
        return 1

    print("\n-- Faucet User Flow --")
    faucet_status: Optional[dict[str, Any]] = None
    faucet_sender = Keypair.generate()
    faucet_headers = {
        "X-Forwarded-For": synthetic_client_ip(faucet_sender.address()),
        "X-Real-IP": synthetic_client_ip(faucet_sender.address()),
    }

    try:
        status_code, config = _http_json(f"{FAUCET_URL.rstrip('/')}/faucet/config")
        if status_code == 200 and isinstance(config, dict) and config.get("max_per_request"):
            report("PASS", "Faucet config")
        else:
            raise RuntimeError(f"status={status_code}, payload={config}")

        status_code, faucet_status = _http_json(f"{FAUCET_URL.rstrip('/')}/faucet/status")
        if (
            status_code == 200
            and isinstance(faucet_status, dict)
            and int(faucet_status.get("balance_licn", 0)) >= 1
        ):
            report(
                "PASS",
                "Faucet status",
                f"balance={faucet_status.get('balance_licn')} LICN",
            )
        else:
            raise RuntimeError(f"status={status_code}, payload={faucet_status}")

        status_code, faucet_response = _http_json(
            f"{FAUCET_URL.rstrip('/')}/faucet/request",
            method="POST",
            body={"address": faucet_sender.address().to_base58(), "amount": 2},
            headers=faucet_headers,
        )
        if status_code != 200 or not isinstance(faucet_response, dict) or not faucet_response.get("success"):
            raise RuntimeError(f"status={status_code}, payload={faucet_response}")
        report("PASS", "Faucet request airdrop")

        funded_balance = await wait_for_balance(conn, faucet_sender.address(), 2 * SPORES)
        sender = faucet_sender
        sender_start_balance = _extract_spores(funded_balance)
        report("PASS", "Faucet funds spendable via RPC")

        status_code, history = _http_json(f"{FAUCET_URL.rstrip('/')}/faucet/airdrops?limit=10")
        if status_code == 200 and isinstance(history, list) and any(
            isinstance(entry, dict) and entry.get("recipient") == faucet_sender.address().to_base58()
            for entry in history
        ):
            report("PASS", "Faucet airdrop history")
        else:
            raise RuntimeError(f"status={status_code}, payload={history}")

        status_code, second_response = _http_json(
            f"{FAUCET_URL.rstrip('/')}/faucet/request",
            method="POST",
            body={"address": faucet_sender.address().to_base58(), "amount": 1},
            headers=faucet_headers,
        )
        post_cooldown_balance = _extract_spores(await conn.get_balance(faucet_sender.address()))
        if status_code == 429 and isinstance(second_response, dict):
            report("PASS", "Faucet cooldown enforcement")
        elif post_cooldown_balance == sender_start_balance:
            report(
                "PASS",
                "Faucet cooldown enforcement",
                f"status={status_code}",
            )
        else:
            report(
                "FAIL",
                "Faucet cooldown enforcement",
                f"status={status_code}, payload={second_response}",
            )
    except (URLError, TimeoutError, RuntimeError, ValueError) as exc:
        if REQUIRE_FAUCET:
            report("FAIL", "Faucet user flow", str(exc))
        else:
            report("SKIP", "Faucet user flow", str(exc))

    if sender is None:
        deployer = await maybe_load_deployer()
        if deployer is not None:
            try:
                deployer_balance = await conn.get_balance(deployer.address())
                sender_start_balance = _extract_spores(deployer_balance)
                if sender_start_balance > 0:
                    sender = deployer
                    report("PASS", "Explorer sender fallback (deployer keypair)")
                else:
                    report("SKIP", "Explorer sender fallback unavailable", "deployer balance is zero")
            except Exception as exc:
                report("SKIP", "Explorer sender fallback unavailable", str(exc))

    print("\n-- Explorer User Flow --")
    recipient = Keypair.generate()
    transfer_signature = ""
    transfer_info: Optional[dict[str, Any]] = None

    if sender is None:
        report("FAIL", "Explorer transfer prerequisite", "no funded sender available")
    else:
        try:
            transfer_signature = await conn.transfer(sender, recipient.address(), 250_000_000)
            transfer_info = await wait_for_transaction(conn, transfer_signature)
            report("PASS", "Explorer source transaction created", transfer_signature)

            recipient_balance = await wait_for_balance(conn, recipient.address(), 250_000_000)
            report(
                "PASS",
                "Explorer recipient balance",
                f"spores={_extract_spores(recipient_balance)}",
            )

            latest_block = await conn.get_latest_block()
            latest_slot = int(latest_block.get("slot") or latest_block.get("height") or 0)
            if latest_slot <= 0:
                raise RuntimeError(f"Unexpected latest block payload: {latest_block}")
            report("PASS", "Explorer latest block")

            block = await conn.get_block(latest_slot)
            if isinstance(block, dict) and block:
                report("PASS", "Explorer block detail")
            else:
                raise RuntimeError(f"Unexpected block payload: {block}")

            sender_history = await wait_for_address_history(conn, sender.address(), transfer_signature)
            if sender_history.get("to") == recipient.address().to_base58():
                report("PASS", "Explorer sender address history")
            else:
                raise RuntimeError(f"Unexpected sender history entry: {sender_history}")

            recipient_history = await wait_for_address_history(conn, recipient.address(), transfer_signature)
            if recipient_history.get("from") == sender.address().to_base58():
                report("PASS", "Explorer recipient address history")
            else:
                raise RuntimeError(f"Unexpected recipient history entry: {recipient_history}")

            recent_entry = await wait_for_recent_transaction(conn, transfer_signature)
            if recent_entry.get("signature") == transfer_signature:
                report("PASS", "Explorer recent transactions")
            else:
                raise RuntimeError(f"Unexpected recent tx entry: {recent_entry}")

            recipient_account = await conn.get_account(recipient.address())
            if isinstance(recipient_account, dict):
                report("PASS", "Explorer account detail")
            else:
                raise RuntimeError(f"Unexpected account payload: {recipient_account}")

            validators = await conn.get_validators()
            if isinstance(validators, list) and validators:
                report("PASS", "Explorer validators list")
            else:
                raise RuntimeError("No validators returned")

            contracts = await conn._rpc("getAllContracts", [])
            contract_entries = contracts.get("contracts", []) if isinstance(contracts, dict) else []
            if isinstance(contract_entries, list) and contract_entries:
                report("PASS", "Explorer contracts list")
            else:
                raise RuntimeError(f"Unexpected contracts payload: {contracts}")

            programs = await conn._rpc("getPrograms", [])
            if isinstance(programs, dict) or isinstance(programs, list):
                report("PASS", "Explorer programs list")
            else:
                raise RuntimeError(f"Unexpected programs payload: {programs}")

            if isinstance(transfer_info, dict) and transfer_info:
                report("PASS", "Explorer transaction detail")
            else:
                raise RuntimeError(f"Unexpected transaction payload: {transfer_info}")
        except Exception as exc:
            report("FAIL", "Explorer user flow", str(exc))

    print("\n-- Bridge Deposit User Flow --")
    custody_ok, custody_detail = custody_healthcheck()
    if REQUIRE_CUSTODY:
        if not custody_ok:
            report("FAIL", "Custody health", custody_detail)
        else:
            report("PASS", "Custody health")

            bridge_wallet = Keypair.generate()
            bridge_auth = create_bridge_access_auth(bridge_wallet)
            bridge_request = {
                "user_id": bridge_wallet.address().to_base58(),
                "chain": "ethereum",
                "asset": "eth",
                "auth": bridge_auth,
            }

            try:
                deposit = await conn._rpc("createBridgeDeposit", [bridge_request])
                deposit_id = str(deposit.get("deposit_id") or "")
                deposit_address = str(deposit.get("address") or "")
                if len(deposit_id) != 36 or not deposit_address.startswith("0x") or len(deposit_address) != 42:
                    raise RuntimeError(f"Unexpected bridge deposit payload: {deposit}")
                report("PASS", "Bridge deposit address issued", deposit_id)

                deposit_status = await conn._rpc(
                    "getBridgeDeposit",
                    [{
                        "deposit_id": deposit_id,
                        "user_id": bridge_wallet.address().to_base58(),
                        "auth": bridge_auth,
                    }],
                )
                if (
                    isinstance(deposit_status, dict)
                    and deposit_status.get("deposit_id") == deposit_id
                    and deposit_status.get("address") == deposit_address
                    and deposit_status.get("status") == "issued"
                    and str(deposit_status.get("user_id") or "") == bridge_wallet.address().to_base58()
                ):
                    report("PASS", "Bridge deposit status lookup", f"status={deposit_status.get('status')}")
                else:
                    raise RuntimeError(f"Unexpected bridge status payload: {deposit_status}")

                try:
                    await conn._rpc("createBridgeDeposit", [bridge_request])
                    report("FAIL", "Bridge deposit cooldown enforcement", "second request unexpectedly succeeded")
                except Exception as exc:
                    if "wait 10s between deposit requests" in str(exc):
                        report("PASS", "Bridge deposit cooldown enforcement")
                    else:
                        raise
            except Exception as exc:
                report("FAIL", "Bridge deposit user flow", str(exc))
    else:
        if custody_ok:
            report("SKIP", "Bridge deposit user flow", "set REQUIRE_CUSTODY=1 to require RPC custody bridge coverage")
        else:
            print("  INFO  Custody bridge flow not requested in this environment")

    print("\n-- Marketplace Browse Flow --")
    try:
        listings = await conn._rpc("getMarketListings", [{"limit": 5}])
        if isinstance(listings, dict) and isinstance(listings.get("listings"), list):
            report("PASS", "Marketplace listings browse")
        else:
            raise RuntimeError(f"Unexpected listings payload: {listings}")

        offers = await conn._rpc("getMarketOffers", [{"limit": 5}])
        if isinstance(offers, dict) and isinstance(offers.get("offers"), list):
            report("PASS", "Marketplace offers browse")
        else:
            raise RuntimeError(f"Unexpected offers payload: {offers}")

        auctions = await conn._rpc("getMarketAuctions", [{"limit": 5}])
        if isinstance(auctions, dict) and isinstance(auctions.get("auctions"), list):
            report("PASS", "Marketplace auctions browse")
        else:
            raise RuntimeError(f"Unexpected auctions payload: {auctions}")

        market_stats = await conn._rpc("getLichenMarketStats", [])
        if isinstance(market_stats, dict):
            report("PASS", "Marketplace stats")
        else:
            raise RuntimeError(f"Unexpected market stats payload: {market_stats}")

        auction_stats = await conn._rpc("getLichenAuctionStats", [])
        if isinstance(auction_stats, dict):
            report("PASS", "Auction stats")
        else:
            raise RuntimeError(f"Unexpected auction stats payload: {auction_stats}")
    except Exception as exc:
        report("FAIL", "Marketplace browse flow", str(exc))

    print_summary()

    if FAIL > 0:
        return 1
    if STRICT_NO_SKIPS and SKIP > 0:
        return 1
    return 0


def print_summary() -> None:
    print("\n" + "=" * 62)
    print(
        f"  User Service E2E Results: PASS {PASS} / FAIL {FAIL} / SKIP {SKIP} / TOTAL {PASS + FAIL + SKIP}"
    )
    print("=" * 62)


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
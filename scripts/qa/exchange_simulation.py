#!/usr/bin/env python3
"""Local exchange integration simulation for native LICN.

Flow:
1. Load the local genesis-funded deployer/funder keypair.
2. Generate customer, deposit, hot, cold, and withdrawal-destination wallets.
3. Fund the customer, send a deposit to the exchange deposit wallet, and detect it.
4. Credit an internal ledger exactly once.
5. Sweep from deposit wallet to hot wallet.
6. Withdraw from hot wallet to the withdrawal destination.
7. Reconcile balances, transaction lookups, account history, and tx counts.

This script is intended for a clean local three-validator stack:
    scripts/start-local-stack.sh testnet
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, Optional


ROOT = Path(__file__).resolve().parents[2]
SDK_PYTHON = ROOT / "sdk" / "python"
if str(SDK_PYTHON) not in sys.path:
    sys.path.insert(0, str(SDK_PYTHON))

from lichen import Connection, Keypair, PublicKey  # noqa: E402


SPORES_PER_LICN = 1_000_000_000
DEFAULT_RPC_URL = "http://127.0.0.1:8899"
DEFAULT_FUNDER = ROOT / "keypairs" / "deployer.json"
DEFAULT_REPORT = ROOT / "tests" / "artifacts" / "exchange-simulation-report.json"
DEFAULT_KEYPAIR_PASSWORD_FILE = ROOT / "data" / "local-cluster" / "keypair-password"
DEFAULT_CLI = ROOT / "target" / "release" / "lichen"

CUSTOMER_FUND_AMOUNT = 1 * SPORES_PER_LICN
DEPOSIT_AMOUNT = 200_000_000
SWEEP_AMOUNT = 190_000_000
WITHDRAWAL_AMOUNT = 50_000_000
CLI_TRANSFER_AMOUNT_LICN = "0.01"
CLI_TRANSFER_AMOUNT_SPORES = 10_000_000
MIN_FINALITY_BUFFER_SLOTS = 8
HIGH_VALUE_FINALITY_BUFFER_SLOTS = 32
TX_TIMEOUT_SECS = 45.0
BALANCE_TIMEOUT_SECS = 30.0


def now_ms() -> int:
    return int(time.time() * 1000)


def address(keypair: Keypair) -> PublicKey:
    return keypair.pubkey()


def address_str(keypair: Keypair) -> str:
    return keypair.pubkey().to_base58()


def resolve_funder_keypair() -> Path:
    configured = os.getenv("EXCHANGE_SIM_FUNDER_KEYPAIR")
    if configured:
        return Path(configured)

    treasury_candidates = sorted(
        (ROOT / "data").glob("state-*/genesis-keys/treasury-*.json")
    )
    if treasury_candidates:
        return treasury_candidates[0]

    return DEFAULT_FUNDER


def spores(balance: Dict[str, Any]) -> int:
    value = balance.get("spores", balance.get("balance", 0))
    return int(value)


def run_cli(lichen_bin: Path, rpc_url: str, args: list[str]) -> Dict[str, Any]:
    command = [str(lichen_bin), "--rpc-url", rpc_url, *args]
    completed = subprocess.run(
        command,
        cwd=ROOT,
        text=True,
        capture_output=True,
        timeout=45,
        check=False,
    )
    result = {
        "command": command,
        "returncode": completed.returncode,
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }
    if completed.returncode != 0:
        raise RuntimeError(f"CLI command failed: {command}\n{completed.stderr}\n{completed.stdout}")
    return result


def extract_hex_signature(output: str) -> str:
    match = re.search(r"\b[0-9a-fA-F]{64}\b", output)
    if not match:
        raise ValueError(f"no 64-char hex signature found in CLI output: {output}")
    return match.group(0).lower()


async def wait_for_slot(conn: Connection, min_slot: int = 1, timeout: float = 45.0) -> int:
    deadline = time.time() + timeout
    last_error: Optional[Exception] = None
    while time.time() < deadline:
        try:
            slot = await conn.get_slot()
            if slot >= min_slot:
                return int(slot)
        except Exception as exc:
            last_error = exc
        await asyncio.sleep(0.5)
    raise TimeoutError(f"chain did not reach slot {min_slot}; last_error={last_error}")


async def wait_for_balance_at_least(
    conn: Connection,
    pubkey: PublicKey,
    minimum_spores: int,
    timeout: float = BALANCE_TIMEOUT_SECS,
) -> int:
    deadline = time.time() + timeout
    last_balance = 0
    while time.time() < deadline:
        last_balance = spores(await conn.get_balance(pubkey))
        if last_balance >= minimum_spores:
            return last_balance
        await asyncio.sleep(0.5)
    raise TimeoutError(
        f"{pubkey.to_base58()} balance {last_balance} below required {minimum_spores}"
    )


async def wait_for_transaction(
    conn: Connection,
    signature: str,
    timeout: float = TX_TIMEOUT_SECS,
) -> Dict[str, Any]:
    deadline = time.time() + timeout
    last_error: Optional[Exception] = None
    while time.time() < deadline:
        try:
            tx = await conn.get_transaction(signature)
            if tx and tx.get("signature") == signature:
                return tx
        except Exception as exc:
            last_error = exc
        await asyncio.sleep(0.5)
    raise TimeoutError(f"transaction {signature} was not available; last_error={last_error}")


async def wait_for_operational_buffer(
    conn: Connection,
    tx_slot: int,
    buffer_slots: int,
    timeout: float = TX_TIMEOUT_SECS,
) -> int:
    deadline = time.time() + timeout
    required = tx_slot + buffer_slots
    last_slot = 0
    while time.time() < deadline:
        latest = await conn.get_slot()
        last_slot = int(latest)
        if last_slot >= required:
            return last_slot
        await asyncio.sleep(0.5)
    raise TimeoutError(f"slot {last_slot} did not reach required buffer slot {required}")


async def wait_for_history_entry(
    conn: Connection,
    pubkey: PublicKey,
    signature: str,
    expected_amount: int,
    expected_from: str,
    expected_to: str,
    timeout: float = TX_TIMEOUT_SECS,
) -> Dict[str, Any]:
    deadline = time.time() + timeout
    while time.time() < deadline:
        history = await conn.get_transactions_by_address(pubkey, limit=50)
        for entry in history.get("transactions", []):
            if (
                entry.get("hash") == signature
                and int(entry.get("amount_spores", 0)) == expected_amount
                and entry.get("from") == expected_from
                and entry.get("to") == expected_to
            ):
                return entry
        await asyncio.sleep(0.5)
    raise TimeoutError(f"history entry {signature} not found for {pubkey.to_base58()}")


class InternalLedger:
    def __init__(self) -> None:
        self.credits: Dict[str, int] = {}
        self.seen_deposits = set()

    def credit_once(self, customer_id: str, signature: str, amount_spores: int) -> None:
        if signature in self.seen_deposits:
            return
        self.seen_deposits.add(signature)
        self.credits[customer_id] = self.credits.get(customer_id, 0) + amount_spores

    def debit(self, customer_id: str, amount_spores: int) -> None:
        available = self.credits.get(customer_id, 0)
        if available < amount_spores:
            raise ValueError(f"insufficient internal balance: {available} < {amount_spores}")
        self.credits[customer_id] = available - amount_spores


async def main() -> int:
    rpc_url = os.getenv("RPC_URL", DEFAULT_RPC_URL)
    fun_keypair = resolve_funder_keypair()
    report_path = Path(os.getenv("EXCHANGE_SIM_REPORT", str(DEFAULT_REPORT)))
    lichen_bin = Path(os.getenv("LICHEN_BIN", str(DEFAULT_CLI)))
    skip_cli = os.getenv("EXCHANGE_SIM_SKIP_CLI", "0") == "1"
    keypair_password = os.getenv("LICHEN_KEYPAIR_PASSWORD")
    if keypair_password is None and DEFAULT_KEYPAIR_PASSWORD_FILE.exists():
        keypair_password = DEFAULT_KEYPAIR_PASSWORD_FILE.read_text().strip()

    if not fun_keypair.exists():
        print(f"FATAL: funder keypair not found: {fun_keypair}", file=sys.stderr)
        print("Start the local stack first or set EXCHANGE_SIM_FUNDER_KEYPAIR.", file=sys.stderr)
        return 2
    if not skip_cli and not lichen_bin.exists():
        print(f"FATAL: CLI binary not found: {lichen_bin}", file=sys.stderr)
        print("Build release binaries first or set LICHEN_BIN.", file=sys.stderr)
        return 2

    conn = Connection(rpc_url)
    started_at = now_ms()
    initial_slot = await wait_for_slot(conn)

    funder = Keypair.load(fun_keypair, password=keypair_password)
    customer = Keypair.generate()
    deposit = Keypair.generate()
    hot = Keypair.generate()
    cold = Keypair.generate()
    withdrawal_destination = Keypair.generate()

    ledger = InternalLedger()
    customer_id = "local-customer-1"

    participants = {
        "funder": address_str(funder),
        "customer": address_str(customer),
        "deposit": address_str(deposit),
        "hot": address_str(hot),
        "cold": address_str(cold),
        "withdrawal_destination": address_str(withdrawal_destination),
    }

    print(json.dumps({"event": "start", "rpc_url": rpc_url, "slot": initial_slot, **participants}))

    fund_sig = await conn.transfer(funder, address(customer), CUSTOMER_FUND_AMOUNT)
    fund_tx = await wait_for_transaction(conn, fund_sig)
    await wait_for_balance_at_least(conn, address(customer), CUSTOMER_FUND_AMOUNT - 2_000_000)

    deposit_sig = await conn.transfer(customer, address(deposit), DEPOSIT_AMOUNT)
    deposit_tx = await wait_for_transaction(conn, deposit_sig)
    await wait_for_operational_buffer(conn, int(deposit_tx["slot"]), MIN_FINALITY_BUFFER_SLOTS)
    deposit_entry = await wait_for_history_entry(
        conn,
        address(deposit),
        deposit_sig,
        DEPOSIT_AMOUNT,
        address_str(customer),
        address_str(deposit),
    )

    ledger.credit_once(customer_id, deposit_sig, DEPOSIT_AMOUNT)
    ledger.credit_once(customer_id, deposit_sig, DEPOSIT_AMOUNT)
    if ledger.credits[customer_id] != DEPOSIT_AMOUNT:
        raise AssertionError("deposit credit was not idempotent")

    sweep_sig = await conn.transfer(deposit, address(hot), SWEEP_AMOUNT)
    sweep_tx = await wait_for_transaction(conn, sweep_sig)
    sweep_entry = await wait_for_history_entry(
        conn,
        address(hot),
        sweep_sig,
        SWEEP_AMOUNT,
        address_str(deposit),
        address_str(hot),
    )

    withdrawal_sig = await conn.transfer(hot, address(withdrawal_destination), WITHDRAWAL_AMOUNT)
    withdrawal_tx = await wait_for_transaction(conn, withdrawal_sig)
    await wait_for_operational_buffer(
        conn,
        int(withdrawal_tx["slot"]),
        HIGH_VALUE_FINALITY_BUFFER_SLOTS,
    )
    withdrawal_entry = await wait_for_history_entry(
        conn,
        address(withdrawal_destination),
        withdrawal_sig,
        WITHDRAWAL_AMOUNT,
        address_str(hot),
        address_str(withdrawal_destination),
    )

    ledger.debit(customer_id, WITHDRAWAL_AMOUNT)

    post_python_balances = {
        name: spores(await conn.get_balance(PublicKey(value)))
        for name, value in participants.items()
        if name != "funder"
    }

    if post_python_balances["withdrawal_destination"] != WITHDRAWAL_AMOUNT:
        raise AssertionError(
            "withdrawal destination balance mismatch: "
            f"{post_python_balances['withdrawal_destination']} != {WITHDRAWAL_AMOUNT}"
        )
    if post_python_balances["hot"] < SWEEP_AMOUNT - WITHDRAWAL_AMOUNT - 5_000_000:
        raise AssertionError(f"hot wallet reconciliation failed: {post_python_balances['hot']}")

    deposit_count = await conn.get_account_tx_count(address(deposit))
    hot_count = await conn.get_account_tx_count(address(hot))
    dest_count = await conn.get_account_tx_count(address(withdrawal_destination))

    cli_report: Optional[Dict[str, Any]] = None
    if not skip_cli:
        cli_dir = report_path.parent / "exchange-cli-smoke"
        cli_dir.mkdir(parents=True, exist_ok=True)
        hot_keypair_path = cli_dir / "hot.json"
        cold_keypair_path = cli_dir / "cold.json"
        hot.save(hot_keypair_path)
        cold.save(cold_keypair_path)

        cli_balance_before = run_cli(lichen_bin, rpc_url, ["balance", address_str(hot)])
        cli_transfer = run_cli(
            lichen_bin,
            rpc_url,
            [
                "transfer",
                address_str(cold),
                CLI_TRANSFER_AMOUNT_LICN,
                "--keypair",
                str(hot_keypair_path),
            ],
        )
        cli_signature = extract_hex_signature(cli_transfer["stdout"])
        cli_tx = await wait_for_transaction(conn, cli_signature)
        await wait_for_history_entry(
            conn,
            address(cold),
            cli_signature,
            CLI_TRANSFER_AMOUNT_SPORES,
            address_str(hot),
            address_str(cold),
        )
        cli_history = run_cli(
            lichen_bin,
            rpc_url,
            ["account", "history", address_str(cold), "--limit", "5"],
        )
        if cli_signature not in cli_history["stdout"]:
            raise AssertionError("CLI account history did not include CLI transfer signature")
        cli_lookup = run_cli(
            lichen_bin,
            rpc_url,
            ["--output", "json", "tx", cli_signature],
        )
        if cli_signature not in cli_lookup["stdout"]:
            raise AssertionError("CLI tx lookup did not include CLI transfer signature")
        cli_balance_after = run_cli(lichen_bin, rpc_url, ["balance", address_str(cold)])
        cold_balance = spores(await conn.get_balance(address(cold)))
        if cold_balance != CLI_TRANSFER_AMOUNT_SPORES:
            raise AssertionError(
                f"CLI cold balance mismatch: {cold_balance} != {CLI_TRANSFER_AMOUNT_SPORES}"
            )

        cli_report = {
            "transfer_signature": cli_signature,
            "transfer_slot": cli_tx.get("slot"),
            "hot_keypair_path": str(hot_keypair_path),
            "cold_keypair_path": str(cold_keypair_path),
            "balance_before": cli_balance_before,
            "transfer": cli_transfer,
            "history": cli_history,
            "tx_lookup": cli_lookup,
            "balance_after": cli_balance_after,
            "cold_balance_spores": cold_balance,
        }

        hot_keypair_path.unlink(missing_ok=True)
        cold_keypair_path.unlink(missing_ok=True)

    report = {
        "status": "passed",
        "started_at_ms": started_at,
        "finished_at_ms": now_ms(),
        "rpc_url": rpc_url,
        "initial_slot": initial_slot,
        "final_slot": await conn.get_slot(),
        "participants": participants,
        "amounts_spores": {
            "customer_funding": CUSTOMER_FUND_AMOUNT,
            "deposit": DEPOSIT_AMOUNT,
            "sweep": SWEEP_AMOUNT,
            "withdrawal": WITHDRAWAL_AMOUNT,
            "internal_customer_balance": ledger.credits[customer_id],
        },
        "transactions": {
            "fund": {"signature": fund_sig, "slot": fund_tx.get("slot")},
            "deposit": {
                "signature": deposit_sig,
                "slot": deposit_tx.get("slot"),
                "history": deposit_entry,
            },
            "sweep": {
                "signature": sweep_sig,
                "slot": sweep_tx.get("slot"),
                "history": sweep_entry,
            },
            "withdrawal": {
                "signature": withdrawal_sig,
                "slot": withdrawal_tx.get("slot"),
                "history": withdrawal_entry,
            },
        },
        "account_tx_counts": {
            "deposit": deposit_count,
            "hot": hot_count,
            "withdrawal_destination": dest_count,
        },
        "cli_smoke": cli_report,
        "post_python_flow_balances_spores": post_python_balances,
        "final_balances_spores": {
            name: spores(await conn.get_balance(PublicKey(value)))
            for name, value in participants.items()
            if name != "funder"
        },
        "finality_policy": {
            "standard_buffer_slots": MIN_FINALITY_BUFFER_SLOTS,
            "high_value_buffer_slots": HIGH_VALUE_FINALITY_BUFFER_SLOTS,
        },
    }

    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True))
    print(json.dumps({"event": "passed", "report": str(report_path), "final_slot": report["final_slot"]}))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(asyncio.run(main()))
    except KeyboardInterrupt:
        raise SystemExit(130)

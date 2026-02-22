#!/usr/bin/env python3
"""
DEPRECATED: Genesis auto-funding is now built into the validator.
During genesis boot, the validator automatically transfers 10K MOLT from the
validator_rewards treasury to the deployer account. This script is kept for
reference and manual use only.

Original purpose: Fund the genesis/deployer account from the validator_rewards treasury.
"""
import asyncio
import json
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from moltchain import Connection, Keypair, TransactionBuilder


def load_keypair_flexible(path: Path) -> Keypair:
    """Load keypair handling all genesis key formats."""
    try:
        return Keypair.load(path)
    except Exception:
        pass
    raw = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(raw, dict):
        # Try secret_key (hex string, 32 bytes seed)
        sk = raw.get("secret_key") or raw.get("privateKey") or raw.get("seed")
        if isinstance(sk, str):
            h = sk.strip().lower().removeprefix("0x")
            if len(h) == 64:
                return Keypair.from_seed(bytes.fromhex(h))
        if isinstance(sk, list) and len(sk) == 32:
            return Keypair.from_seed(bytes(sk))
    raise ValueError(f"unsupported keypair format: {path}")


async def main():
    conn = Connection("http://127.0.0.1:8899")

    # Load treasury keypair (validator_rewards has 150M MOLT)
    treasury_key_path = list(
        (ROOT / "data" / "state-8000" / "genesis-keys").glob("validator_rewards-*.json")
    )
    if not treasury_key_path:
        print("ERROR: No treasury keypair found in genesis-keys/")
        sys.exit(1)

    treasury_kp = load_keypair_flexible(treasury_key_path[0])

    # Load deployer (genesis primary)
    deployer_path = ROOT / "keypairs" / "deployer.json"
    if not deployer_path.exists():
        # Try to find genesis primary keypair
        genesis_keys = list(
            (ROOT / "data" / "state-8000" / "genesis-keys").glob("genesis-primary-*.json")
        )
        if genesis_keys:
            import shutil
            shutil.copy(genesis_keys[0], deployer_path)
            print(f"Copied {genesis_keys[0].name} -> deployer.json")
        else:
            print("ERROR: No deployer keypair found")
            sys.exit(1)

    deployer_kp = load_keypair_flexible(deployer_path)

    print(f"Treasury: {treasury_kp.public_key()}")
    print(f"Deployer: {deployer_kp.public_key()}")

    # Check current balances
    tbal = await conn.get_balance(str(treasury_kp.public_key()))
    dbal = await conn.get_balance(str(deployer_kp.public_key()))
    print(f"Treasury balance: {tbal}")
    print(f"Deployer balance: {dbal}")

    if isinstance(dbal, dict):
        dbal_shells = dbal.get("shells", dbal.get("spendable", 0))
    else:
        dbal_shells = int(dbal)

    if dbal_shells >= 10_000_000_000_000:  # 10K MOLT
        print("Deployer already funded (>=10K MOLT)")
        return

    # Fund 10K MOLT using TransactionBuilder.transfer
    amount = 10_000 * 1_000_000_000  # 10K MOLT in shells
    blockhash = await conn.get_recent_blockhash()
    ix = TransactionBuilder.transfer(
        treasury_kp.public_key(), deployer_kp.public_key(), amount
    )
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(treasury_kp)
    )
    sig = await conn.send_transaction(tx)
    print(f"Transfer TX: {sig}")

    await asyncio.sleep(2)
    dbal2 = await conn.get_balance(str(deployer_kp.public_key()))
    print(f"Deployer balance after: {dbal2}")
    print("Done!")

if __name__ == "__main__":
    asyncio.run(main())

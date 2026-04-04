#!/usr/bin/env python3
"""
Post-Genesis Deployment — Wrapped Token + DEX Contracts
========================================================

This script deploys and initializes all wrapped-asset tokens and DEX contracts
on a running Lichen validator. Run once after genesis to bring the full
DEX trading infrastructure online.

Deployment order matters:
  Phase 1 — Wrapped tokens (lusd_token, wsol_token, weth_token)
            These are the quote/base assets the DEX trades.
  Phase 2 — DEX core    (dex_core, dex_amm, dex_router)
            Core trading engine. dex_core gets the token addresses.
  Phase 3 — DEX modules (dex_margin, dex_rewards, dex_governance, dex_analytics)
            Extended functionality, wired to dex_core.

Each contract is:
  1. Deployed   — WASM uploaded via Deploy instruction
  2. Initialized — "initialize" called with the treasury multisig as admin
  3. Configured  — Token addresses / cross-references registered

Usage:
  python tools/deploy_dex.py                          # default: localhost:8899
  python tools/deploy_dex.py --rpc http://node:8899   # custom RPC
  python tools/deploy_dex.py --admin <base58>         # explicit admin pubkey
"""

import sys
import os
import json
import struct
import asyncio
import hashlib
from pathlib import Path
from typing import Optional, Dict

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey, TransactionBuilder, Instruction

# ===========================================================================
# Configuration
# ===========================================================================

REPO_ROOT = Path(__file__).resolve().parent.parent
RPC_URL = "http://127.0.0.1:8899"
KEYPAIR_DIR = REPO_ROOT / "keypairs"
DEPLOYER_PATH = KEYPAIR_DIR / "deployer.json"
CONTRACT_PROGRAM = PublicKey(b'\xff' * 32)   # contract runtime program address (for Call instructions)
SYSTEM_PROGRAM = PublicKey(b'\x00' * 32)       # system program (for Deploy instructions, type 17)
OUTPUT_PATH = REPO_ROOT / "deploy-manifest.json"
CONTRACT_TX_LOOKUP_ATTEMPTS = int(os.environ.get("LICHEN_CONTRACT_TX_LOOKUP_ATTEMPTS", "80"))
CONTRACT_TX_LOOKUP_DELAY_SECS = float(os.environ.get("LICHEN_CONTRACT_TX_LOOKUP_DELAY_SECS", "0.25"))

# Contracts in deployment order
PHASE_1_TOKENS = [
    {"name": "lusd_token",  "wasm": "lusd_token.wasm"},
    {"name": "wsol_token",  "wasm": "wsol_token.wasm"},
    {"name": "weth_token",  "wasm": "weth_token.wasm"},
    {"name": "wbnb_token",  "wasm": "wbnb_token.wasm"},
]

PHASE_2_DEX_CORE = [
    {"name": "dex_core",    "wasm": "dex_core.wasm"},
    {"name": "dex_amm",     "wasm": "dex_amm.wasm"},
    {"name": "dex_router",  "wasm": "dex_router.wasm"},
]

PHASE_3_DEX_MODULES = [
    {"name": "dex_margin",     "wasm": "dex_margin.wasm"},
    {"name": "dex_rewards",    "wasm": "dex_rewards.wasm"},
    {"name": "dex_governance", "wasm": "dex_governance.wasm"},
    {"name": "dex_analytics",  "wasm": "dex_analytics.wasm"},
]

PHASE_4_PREDICTION = [
    {"name": "prediction_market", "wasm": "prediction_market.wasm"},
]

ALL_CONTRACTS = PHASE_1_TOKENS + PHASE_2_DEX_CORE + PHASE_3_DEX_MODULES + PHASE_4_PREDICTION

WASM_SEARCH_DIRS = [
    REPO_ROOT / "contracts" / "target" / "wasm32-unknown-unknown" / "release",
    REPO_ROOT / "contracts" / "build",
    REPO_ROOT / "contracts",
]


# ===========================================================================
# Helpers
# ===========================================================================

def find_wasm(filename: str) -> Optional[Path]:
    # Also search in contracts/<name>/<name>.wasm (per-contract directories)
    stem = filename.replace(".wasm", "")
    per_contract = REPO_ROOT / "contracts" / stem / filename
    if per_contract.exists():
        return per_contract
    for d in WASM_SEARCH_DIRS:
        p = d / filename
        if p.exists():
            return p
    return None


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

    raise FileNotFoundError(f"Genesis keypair not found for role '{role}' on network '{network}'")


def load_genesis_keypair(role: str, network: Optional[str] = None) -> Keypair:
    return Keypair.load(find_genesis_keypair_path(role, network))


def load_or_create_deployer() -> Keypair:
    KEYPAIR_DIR.mkdir(parents=True, exist_ok=True)
    if DEPLOYER_PATH.exists():
        kp = Keypair.load(DEPLOYER_PATH)
        print(f"🔑 Deployer: {kp.address()}")
        return kp
    kp = Keypair.generate()
    kp.save(DEPLOYER_PATH)
    print(f"🔑 New deployer generated: {kp.address()}")
    return kp


def derive_program_address(deployer: PublicKey, wasm_bytes: bytes) -> PublicKey:
    h = hashlib.sha256(deployer.to_bytes() + wasm_bytes).digest()
    return PublicKey(h[:32])


def keypair_address_bytes(keypair: Keypair) -> bytes:
    return bytes(keypair.address().to_bytes())


# Maps symbol registry names → deploy_dex contract names
SYMBOL_TO_CONTRACT = {
    "LUSD": "lusd_token",
    "WSOL": "wsol_token",
    "WETH": "weth_token",
    "WBNB": "wbnb_token",
    "DEX": "dex_core",
    "DEXAMM": "dex_amm",
    "DEXROUTER": "dex_router",
    "DEXMARGIN": "dex_margin",
    "DEXREWARDS": "dex_rewards",
    "DEXGOV": "dex_governance",
    "ANALYTICS": "dex_analytics",
    "PREDICT": "prediction_market",
    "LICN": "lichencoin",
}

# DEX contracts that use opcode dispatch (match args[0]) instead of named exports.
# dex_margin has NO named 'initialize' export — must use opcode 0 via the 'call' export.
# All other DEX contracts have named 'initialize' BUT use opcode dispatch for operational calls.
OPCODE_ONLY_INIT = {"dex_margin"}


async def discover_existing_contracts(conn: Connection) -> Dict[str, PublicKey]:
    """Query the symbol registry AND getAllContracts for contracts already
    deployed (e.g. at genesis).  Returns {contract_name: PublicKey}."""
    found = {}

    # 1. Symbol registry (fast, precise)
    try:
        result = await conn._rpc("getAllSymbolRegistry")
        entries = result.get("entries", [])
        for entry in entries:
            sym = entry.get("symbol", "")
            prog = entry.get("program", "")
            if sym in SYMBOL_TO_CONTRACT and prog:
                found[SYMBOL_TO_CONTRACT[sym]] = PublicKey.from_base58(prog)
    except Exception:
        pass

    # 2. getAllContracts fallback — picks up genesis contracts not in symbol registry
    try:
        result = await conn._rpc("getAllContracts")
        contracts_list = result if isinstance(result, list) else result.get("contracts", [])
        for c in contracts_list:
            name = c.get("name", "")
            addr = c.get("address", c.get("program_id", ""))
            if name and addr and name not in found:
                found[name] = PublicKey.from_base58(addr)
    except Exception:
        pass

    if found:
        print(f"\n🔍 Discovered {len(found)} existing contract(s) on-chain (genesis-deployed):")
        for name, pk in sorted(found.items()):
            print(f"   {name:20s} → {pk}")
    return found


async def deploy_contract(
    conn: Connection, deployer: Keypair, name: str, wasm_bytes: bytes,
    treasury_pubkey: PublicKey = None
) -> tuple:
    """Deploy a single contract via system program instruction type 17.
    Returns (signature, program_pubkey)."""
    program_pubkey = derive_program_address(deployer.address(), wasm_bytes)
    if treasury_pubkey is None:
        treasury_pubkey = deployer.address()
    # Instruction type 17: [17 | code_length(4 LE) | raw_wasm_bytes]
    data = bytearray()
    data.append(17)
    data.extend(struct.pack('<I', len(wasm_bytes)))
    data.extend(wasm_bytes)
    ix = Instruction(
        program_id=SYSTEM_PROGRAM,
        accounts=[deployer.address(), treasury_pubkey],
        data=bytes(data),
    )
    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(deployer)
    )
    sig = await conn.send_transaction(tx)
    return sig, program_pubkey


async def call_contract(
    conn: Connection, caller: Keypair, program_pubkey: PublicKey,
    func: str, args: Optional[dict] = None
) -> str:
    """Send a Call instruction to a deployed contract. Returns signature."""
    args_bytes = json.dumps(args or {}).encode()
    payload = json.dumps({"Call": {"function": func, "args": list(args_bytes), "value": 0}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.address(), program_pubkey],
        data=payload.encode(),
    )
    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(caller)
    )
    sig = await conn.send_transaction(tx)
    return await await_contract_success(conn, sig, f"{program_pubkey}.{func}")


async def call_contract_raw(
    conn: Connection, caller: Keypair, program_pubkey: PublicKey,
    func: str, raw_args: list, value: int = 0
) -> str:
    """Send a Call instruction with raw byte list args. Returns signature."""
    payload = json.dumps({"Call": {"function": func, "args": raw_args, "value": value}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.address(), program_pubkey],
        data=payload.encode(),
    )
    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(caller)
    )
    sig = await conn.send_transaction(tx)
    return await await_contract_success(conn, sig, f"{program_pubkey}.{func}")


async def await_contract_success(conn: Connection, signature: str, context: str) -> str:
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


# ===========================================================================
# Deployment Phases
# ===========================================================================

async def phase_deploy(
    conn: Connection, deployer: Keypair, contracts: list, label: str,
    treasury_pubkey: PublicKey = None,
    existing: Dict[str, PublicKey] = None
) -> Dict[str, PublicKey]:
    """Deploy a batch of contracts. Skips already-deployed (genesis) contracts.
    Returns {name: pubkey}."""
    print(f"\n{'═' * 60}")
    print(f"  {label}")
    print(f"{'═' * 60}")
    deployed = {}
    for c in contracts:
        name = c["name"]
        # Skip if already on-chain (genesis-deployed)
        if existing and name in existing:
            deployed[name] = existing[name]
            print(f"\n  📦 {name} — already on-chain (genesis)")
            print(f"     📍 Address   {existing[name]}")
            continue
        wasm_path = find_wasm(c["wasm"])
        if not wasm_path:
            print(f"  ⚠️  {name}: WASM not found ({c['wasm']}), skipping")
            continue
        wasm_bytes = wasm_path.read_bytes()
        print(f"\n  📦 {name} — {len(wasm_bytes):,} bytes")
        try:
            sig, pubkey = await deploy_contract(conn, deployer, name, wasm_bytes, treasury_pubkey)
            deployed[name] = pubkey
            print(f"     ✅ Deployed  sig={sig}")
            print(f"     📍 Address   {pubkey}")
        except Exception as e:
            print(f"     ❌ Deploy failed: {e}")
    return deployed


async def phase_initialize_tokens(
    conn: Connection, deployer: Keypair, addrs: Dict[str, PublicKey],
    admin_pubkey: PublicKey
) -> None:
    """Initialize all wrapped token contracts with admin = treasury multisig."""
    print(f"\n{'═' * 60}")
    print(f"  INITIALIZING WRAPPED TOKENS")
    print(f"{'═' * 60}")
    print(f"  Admin: {admin_pubkey}")
    admin_bytes = list(admin_pubkey.to_bytes())  # 32-byte admin address

    for name in ["lusd_token", "wsol_token", "weth_token", "wbnb_token"]:
        if name not in addrs:
            print(f"  ⚠️  {name} not deployed, skipping init")
            continue
        try:
            sig = await call_contract_raw(
                conn, deployer, addrs[name], "initialize", admin_bytes
            )
            print(f"  ✅ {name}.initialize() — sig={sig}")
        except Exception as e:
            print(f"  ⚠️  {name}.initialize() failed: {e}")


async def phase_initialize_dex(
    conn: Connection, deployer: Keypair, addrs: Dict[str, PublicKey],
    admin_pubkey: PublicKey
) -> None:
    """Initialize DEX contracts and wire cross-references.

    DEX contracts use **opcode dispatch** via a single ``call()`` WASM export:
      - args[0] = opcode byte
      - args[1..] = serialised arguments (pubkeys as raw 32-byte, integers as LE)

    Some contracts (dex_router, dex_rewards, dex_governance, dex_analytics) also
    expose a named ``initialize`` export for convenience.  dex_margin does NOT
    have a named ``initialize``; it must be initialised via opcode 0.
    """
    print(f"\n{'═' * 60}")
    print(f"  INITIALIZING DEX CONTRACTS")
    print(f"{'═' * 60}")

    deployer_bytes = list(deployer.address().to_bytes())
    admin_bytes = list(admin_pubkey.to_bytes())

    # ── Initialize each DEX contract ──────────────────────────
    for name in ["dex_core", "dex_amm", "dex_router",
                  "dex_margin", "dex_rewards", "dex_governance", "dex_analytics"]:
        if name not in addrs:
            print(f"  ⚠️  {name} not deployed, skipping init")
            continue
        try:
            if name in OPCODE_ONLY_INIT:
                # Opcode 0 = initialize(admin[32]) via the "call" export
                data = [0] + admin_bytes
                sig = await call_contract_raw(conn, deployer, addrs[name], "call", data)
            else:
                # Named "initialize" export (takes no args or reads caller as admin)
                sig = await call_contract(conn, deployer, addrs[name], "initialize")
            print(f"  ✅ {name}.initialize() — sig={sig}")
        except Exception as e:
            print(f"  ⚠️  {name}.initialize() failed: {e}")

    # ── Create trading pairs on dex_core (opcode 1) ──────────
    # Opcode 1: create_pair(caller[32], base[32], quote[32],
    #                        tick_size(u64), lot_size(u64), min_order(u64))
    if "dex_core" in addrs:
        print(f"\n  --- Creating trading pairs on dex_core ---")

        # Resolve token symbols to their on-chain addresses
        symbol_addrs = {
            "lUSD":  addrs.get("lusd_token"),
            "wSOL":  addrs.get("wsol_token"),
            "wETH":  addrs.get("weth_token"),
            "wBNB":  addrs.get("wbnb_token"),
            "LICN":  addrs.get("lichencoin"),
        }

        # Default CLOB parameters (spores-denominated)
        DEFAULT_TICK   = 1_000_000       # 0.001 LICN price increment
        DEFAULT_LOT    = 1_000_000_000   # 1 LICN lot size
        DEFAULT_MIN    = 1_000_000_000   # 1 LICN minimum order

        pairs = [
            ("LICN", "lUSD"),
            ("wSOL", "lUSD"),
            ("wETH", "lUSD"),
            ("wBNB", "lUSD"),
            ("wSOL", "LICN"),
            ("wETH", "LICN"),
            ("wBNB", "LICN"),
        ]

        for base_sym, quote_sym in pairs:
            base_pk = symbol_addrs.get(base_sym)
            quote_pk = symbol_addrs.get(quote_sym)
            if not base_pk or not quote_pk:
                print(f"  ⚠️  create_pair({base_sym}/{quote_sym}): token address unknown, skipping")
                continue
            data = (bytes([1])
                    + bytes(deployer.address().to_bytes())
                    + bytes(base_pk.to_bytes())
                    + bytes(quote_pk.to_bytes())
                    + struct.pack('<Q', DEFAULT_TICK)
                    + struct.pack('<Q', DEFAULT_LOT)
                    + struct.pack('<Q', DEFAULT_MIN))
            try:
                sig = await call_contract_raw(
                    conn, deployer, addrs["dex_core"], "call", list(data))
                print(f"  ✅ create_pair({base_sym}/{quote_sym}) — sig={sig}")
            except Exception as e:
                print(f"  ⚠️  create_pair({base_sym}/{quote_sym}) failed: {e}")

        # ── Set preferred quote token (opcode 4) ──────────────
        # Opcode 4: set_preferred_quote(caller[32], quote_addr[32])
        print(f"\n  --- Setting preferred quote token (lUSD) ---")
        lusd_pk = symbol_addrs.get("lUSD")
        if lusd_pk:
            data = (bytes([4])
                    + bytes(deployer.address().to_bytes())
                    + bytes(lusd_pk.to_bytes()))
            try:
                sig = await call_contract_raw(
                    conn, deployer, addrs["dex_core"], "call", list(data))
                print(f"  ✅ set_preferred_quote(lUSD) — sig={sig}")
            except Exception as e:
                print(f"  ⚠️  set_preferred_quote(lUSD) failed: {e}")

    # ── Wire dex_router to dex_core + dex_amm (opcode 1) ─────
    # Opcode 1: set_addresses(caller[32], core_addr[32], amm_addr[32])
    if "dex_router" in addrs and "dex_core" in addrs and "dex_amm" in addrs:
        print(f"\n  --- Wiring dex_router → dex_core + dex_amm ---")
        data = (bytes([1])
                + bytes(deployer.address().to_bytes())
                + bytes(addrs["dex_core"].to_bytes())
                + bytes(addrs["dex_amm"].to_bytes()))
        try:
            sig = await call_contract_raw(
                conn, deployer, addrs["dex_router"], "call", list(data))
            print(f"  ✅ dex_router.set_addresses(core, amm) — sig={sig}")
        except Exception as e:
            print(f"  ⚠️  dex_router.set_addresses() failed: {e}")


async def phase_initialize_prediction_market(
    conn: Connection, deployer: Keypair, addrs: Dict[str, PublicKey],
    admin_pubkey: PublicKey
) -> None:
    """Initialize prediction_market and wire its cross-contract references."""
    print(f"\n{'═' * 60}")
    print(f"  INITIALIZING PREDICTION MARKET")
    print(f"{'═' * 60}")

    name = "prediction_market"
    if name not in addrs:
        print(f"  ⚠️  {name} not deployed, skipping init")
        return

    # Initialize with admin key
    admin_bytes = list(admin_pubkey.to_bytes())
    try:
        sig = await call_contract_raw(
            conn, deployer, addrs[name], "initialize", admin_bytes
        )
        print(f"  ✅ {name}.initialize() — sig={sig}")
    except Exception as e:
        print(f"  ⚠️  {name}.initialize() failed: {e}")

    # Wire LichenID address (for reputation checks)
    if "lichenid" in addrs:
        try:
            sig = await call_contract(
                conn, deployer, addrs[name], "set_lichenid_address",
                {"address": str(addrs["lichenid"])}
            )
            print(f"  ✅ {name}.set_lichenid_address() — sig={sig}")
        except Exception as e:
            print(f"  ⚠️  {name}.set_lichenid_address() failed: {e}")

    # Wire LichenOracle address (for resolution attestation)
    if "lichenoracle" in addrs:
        try:
            sig = await call_contract(
                conn, deployer, addrs[name], "set_oracle_address",
                {"address": str(addrs["lichenoracle"])}
            )
            print(f"  ✅ {name}.set_oracle_address() — sig={sig}")
        except Exception as e:
            print(f"  ⚠️  {name}.set_oracle_address() failed: {e}")

    # Wire lUSD token address (collateral token)
    if "lusd_token" in addrs:
        try:
            sig = await call_contract(
                conn, deployer, addrs[name], "set_musd_address",
                {"address": str(addrs["lusd_token"])}
            )
            print(f"  ✅ {name}.set_musd_address() — sig={sig}")
        except Exception as e:
            print(f"  ⚠️  {name}.set_musd_address() failed: {e}")

    # Wire DEX governance address (for DAO dispute resolution)
    if "dex_governance" in addrs:
        try:
            sig = await call_contract(
                conn, deployer, addrs[name], "set_dex_gov_address",
                {"address": str(addrs["dex_governance"])}
            )
            print(f"  ✅ {name}.set_dex_gov_address() — sig={sig}")
        except Exception as e:
            print(f"  ⚠️  {name}.set_dex_gov_address() failed: {e}")


async def phase_verify(
    conn: Connection, addrs: Dict[str, PublicKey]
) -> None:
    """Verify all contracts are on-chain."""
    print(f"\n{'═' * 60}")
    print(f"  VERIFYING CONTRACTS ON-CHAIN")
    print(f"{'═' * 60}")
    for name, pubkey in addrs.items():
        try:
            info = await conn.get_contract_info(pubkey)
            print(f"  ✅ {name:20s} — on-chain ✓")
        except Exception as e:
            print(f"  ⚠️  {name:20s} — verification failed: {e}")


def save_manifest(deployer_pubkey: PublicKey, addrs: Dict[str, PublicKey]) -> None:
    """Write deploy-manifest.json so custody + other services can look up addresses."""
    manifest = {
        "deployer": str(deployer_pubkey),
        "deployed_at": __import__("datetime").datetime.utcnow().isoformat() + "Z",
        "contracts": {name: str(pubkey) for name, pubkey in addrs.items()},
        "token_contracts": {
            "lUSD": str(addrs["lusd_token"]) if "lusd_token" in addrs else None,
            "wSOL": str(addrs["wsol_token"]) if "wsol_token" in addrs else None,
            "wETH": str(addrs["weth_token"]) if "weth_token" in addrs else None,
            "wBNB": str(addrs["wbnb_token"]) if "wbnb_token" in addrs else None,
        },
        "dex_contracts": {
            name: str(addrs[name])
            for name in ["dex_core", "dex_amm", "dex_router",
                         "dex_margin", "dex_rewards", "dex_governance", "dex_analytics",
                         "prediction_market"]
            if name in addrs
        },
        "trading_pairs": [
            "LICN/lUSD", "wSOL/lUSD", "wETH/lUSD", "wBNB/lUSD",
            "wSOL/LICN", "wETH/LICN", "wBNB/LICN",
        ],
    }
    OUTPUT_PATH.write_text(json.dumps(manifest, indent=2))
    print(f"\n  📄 Manifest saved to {OUTPUT_PATH}")


# ===========================================================================
# Main
# ===========================================================================

async def main():
    import argparse
    parser = argparse.ArgumentParser(description="Deploy Lichen DEX + wrapped tokens")
    parser.add_argument("--rpc", default=RPC_URL, help="Lichen RPC URL")
    parser.add_argument("--admin", default=None, help="Admin/treasury pubkey (base58). Required for mainnet.")
    parser.add_argument("--network", default="testnet", choices=["testnet", "mainnet"],
                        help="Network type. Mainnet requires --admin (multisig address).")
    args = parser.parse_args()

    deployer = load_or_create_deployer()
    conn = Connection(args.rpc)

    # Resolve admin pubkey — enforce multisig for mainnet
    if args.admin:
        admin_pubkey = PublicKey.from_base58(args.admin)
        if admin_pubkey == deployer.address():
            if args.network == "mainnet":
                print("❌ MAINNET ERROR: --admin must be a multisig address, not the deployer keypair")
                print("   Deploy a multisig contract first, then use its address as --admin")
                sys.exit(1)
            else:
                print(f"⚠️  WARNING: Admin is the deployer keypair (single-key control)")
                print(f"   For production, use a multisig address instead")
        print(f"🏛️  Admin: {admin_pubkey}")
    else:
        if args.network == "mainnet":
            print("❌ MAINNET ERROR: --admin is required for mainnet deployments")
            print("   A multisig-controlled admin address must be specified:")
            print("   python3 deploy_dex.py --network mainnet --admin <MULTISIG_PUBKEY>")
            sys.exit(1)
        admin_pubkey = deployer.address()
        print(f"⚠️  Admin (deployer — single-key, testnet only): {admin_pubkey}")
        print(f"   For production: use --admin <MULTISIG_PUBKEY> --network mainnet")

    # Health check
    try:
        health = await conn.health()
        print(f"✅ Validator healthy: {health}")
    except Exception as e:
        print(f"❌ Cannot reach validator at {args.rpc}: {e}")
        sys.exit(1)

    # ── Ensure deployer is funded (self-fund via requestAirdrop if needed) ──
    deployer_pk_str = str(deployer.address())
    try:
        bal = await conn.get_balance(deployer.address())
        # bal may be dict with 'spores' key or an int
        if isinstance(bal, dict):
            spores = int(bal.get("spores", bal.get("balance", 0)))
        else:
            spores = int(bal or 0)
    except Exception:
        spores = 0

    MIN_SPORES = 10_000_000_000  # 10 LICN minimum for deployment fees
    if spores < MIN_SPORES:
        print(f"💰 Deployer balance: {spores / 1e9:.4f} LICN — requesting airdrop...")
        for attempt in range(3):
            try:
                result = await conn._rpc("requestAirdrop", [deployer_pk_str, 10])
                sig = result.get("signature", "")
                print(f"  ✅ Airdrop received — sig={sig}")
                break
            except Exception as e:
                err_str = str(e)
                if "rate limit" in err_str.lower():
                    import time
                    print(f"  ⏳ Rate limited, waiting 60s...")
                    time.sleep(60)
                else:
                    print(f"  ⚠️  Airdrop attempt {attempt+1} failed: {e}")
                    break
    else:
        print(f"💰 Deployer balance: {spores / 1e9:.4f} LICN — sufficient")

    all_addrs: Dict[str, PublicKey] = {}

    # ── Pre-check: discover contracts deployed at genesis ──
    existing = await discover_existing_contracts(conn)

    # Merge ALL discovered genesis contracts into all_addrs so the manifest
    # includes genesis-deployed contracts like lichencoin, lichenid, etc.
    all_addrs.update(existing)

    # ── Phase 1: Wrapped Tokens ──
    # Resolve treasury pubkey for deploy instruction accounts
    treasury_pubkey = admin_pubkey

    addrs = await phase_deploy(conn, deployer, PHASE_1_TOKENS, "PHASE 1 — WRAPPED TOKEN CONTRACTS", treasury_pubkey, existing)
    all_addrs.update(addrs)
    await phase_initialize_tokens(conn, deployer, all_addrs, admin_pubkey)

    # ── Phase 2: DEX Core ──
    addrs = await phase_deploy(conn, deployer, PHASE_2_DEX_CORE, "PHASE 2 — DEX CORE CONTRACTS", treasury_pubkey, existing)
    all_addrs.update(addrs)

    # ── Phase 3: DEX Modules ──
    addrs = await phase_deploy(conn, deployer, PHASE_3_DEX_MODULES, "PHASE 3 — DEX MODULE CONTRACTS", treasury_pubkey, existing)
    all_addrs.update(addrs)

    # Initialize DEX + wire everything together
    await phase_initialize_dex(conn, deployer, all_addrs, admin_pubkey)

    # ── Phase 4: Prediction Market ──
    addrs = await phase_deploy(conn, deployer, PHASE_4_PREDICTION, "PHASE 4 — PREDICTION MARKET", treasury_pubkey, existing)
    all_addrs.update(addrs)

    # Initialize prediction market + wire cross-references
    await phase_initialize_prediction_market(conn, deployer, all_addrs, admin_pubkey)

    # Verify
    await phase_verify(conn, all_addrs)

    # Save manifest
    save_manifest(deployer.address(), all_addrs)

    # ── Summary ──
    print(f"\n{'═' * 60}")
    print(f"  DEPLOYMENT COMPLETE")
    print(f"{'═' * 60}")
    print(f"  Deployer:  {deployer.address()}")
    print(f"  Admin:     {admin_pubkey}")
    print(f"  Contracts: {len(all_addrs)}/{len(ALL_CONTRACTS)}")
    print()
    for name, pubkey in all_addrs.items():
        if "token" in name:
            tag = "TOKEN"
        elif name == "prediction_market":
            tag = "PRED "
        else:
            tag = "DEX  "
        print(f"  [{tag}] {name:20s} → {pubkey}")
    print()

    if len(all_addrs) < len(ALL_CONTRACTS):
        missing = [c["name"] for c in ALL_CONTRACTS if c["name"] not in all_addrs]
        print(f"  ⚠️  Missing: {', '.join(missing)}")
        print(f"  Build WASM first: cargo build --release --target wasm32-unknown-unknown")

    custody_keypair_path = f"/etc/lichen/custody-treasury-{args.network}.json"

    print(f"\n  Next steps:")
    print(f"  1. Copy deployer keypair to custody treasury (CRITICAL — admin must match):")
    print(f"     sudo cp {DEPLOYER_PATH} {custody_keypair_path}")
    print(f"  2. Copy token addresses to custody config:")
    for name in ["lusd_token", "wsol_token", "weth_token", "wbnb_token"]:
        if name in all_addrs:
            env_key = f"CUSTODY_{name.upper()}_ADDR"
            print(f"     export {env_key}={all_addrs[name]}")
    print(f"  3. Set CUSTODY_TREASURY_KEYPAIR={custody_keypair_path} in custody env")
    print(f"  4. Restart custody service with new env vars")
    print(f"  5. First deposit will trigger wrapped token minting ✅")


if __name__ == "__main__":
    asyncio.run(main())

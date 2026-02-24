#!/usr/bin/env python3
import asyncio
import json
import os
import random
import re
import struct
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from moltchain import Connection, Instruction, Keypair, PublicKey, TransactionBuilder  # type: ignore

RPC_URL = os.getenv("RPC_URL", "http://127.0.0.1:8899")
CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
REQUIRE_ALL_SCENARIOS = os.getenv("REQUIRE_ALL_SCENARIOS", "1") == "1"
STRICT_WRITE_ASSERTIONS = os.getenv("STRICT_WRITE_ASSERTIONS", "1") == "1"
TX_CONFIRM_TIMEOUT_SECS = int(os.getenv("TX_CONFIRM_TIMEOUT_SECS", "25"))
REQUIRE_FULL_WRITE_ACTIVITY = os.getenv("REQUIRE_FULL_WRITE_ACTIVITY", "1") == "1"
MIN_CONTRACT_ACTIVITY_DELTA = int(os.getenv("MIN_CONTRACT_ACTIVITY_DELTA", "1"))
CONTRACT_ACTIVITY_OVERRIDES_RAW = os.getenv("CONTRACT_ACTIVITY_OVERRIDES", "")
ENFORCE_DOMAIN_ASSERTIONS = os.getenv("ENFORCE_DOMAIN_ASSERTIONS", "1") == "1"
ENABLE_NEGATIVE_ASSERTIONS = os.getenv("ENABLE_NEGATIVE_ASSERTIONS", "1") == "1"
REQUIRE_NEGATIVE_REASON_MATCH = os.getenv("REQUIRE_NEGATIVE_REASON_MATCH", "1") == "1"
REQUIRE_NEGATIVE_CODE_MATCH = os.getenv("REQUIRE_NEGATIVE_CODE_MATCH", "0") == "1"
REQUIRE_SCENARIO_FOR_DISCOVERED = os.getenv("REQUIRE_SCENARIO_FOR_DISCOVERED", "1") == "1"
MIN_NEGATIVE_ASSERTIONS_EXECUTED = int(os.getenv("MIN_NEGATIVE_ASSERTIONS_EXECUTED", "5"))
EXPECTED_CONTRACTS_FILE = os.getenv("EXPECTED_CONTRACTS_FILE", str(ROOT / "tests" / "expected-contracts.json"))
REQUIRE_EXPECTED_CONTRACT_SET = os.getenv("REQUIRE_EXPECTED_CONTRACT_SET", "1") == "1"
WRITE_E2E_REPORT_PATH = os.getenv("WRITE_E2E_REPORT_PATH", str(ROOT / "tests" / "artifacts" / "contracts-write-e2e-report.json"))
REQUIRE_FUNCTION_COVERAGE = os.getenv("REQUIRE_FUNCTION_COVERAGE", "0") == "1"
PROGRAM_CALLS_LIMIT = int(os.getenv("PROGRAM_CALLS_LIMIT", "200"))

DEPLOYER_PATH = os.getenv("AGENT_KEYPAIR") or str(ROOT / "keypairs" / "deployer.json")
SECONDARY_PATH = os.getenv("HUMAN_KEYPAIR", "")

PASS = 0
FAIL = 0
SKIP = 0
RESULTS: List[Dict[str, Any]] = []
CONTRACT_SET_DIFF: Dict[str, Any] = {}


def load_keypair_flexible(path: Path) -> Keypair:
    try:
        return Keypair.load(path)
    except Exception:
        pass

    raw = json.loads(path.read_text(encoding="utf-8"))

    if isinstance(raw, dict):
        private_key = raw.get("privateKey") or raw.get("secret_key")
        if isinstance(private_key, list) and len(private_key) == 32:
            return Keypair.from_seed(bytes(private_key))
        if isinstance(private_key, str):
            key_hex = private_key.strip().lower().removeprefix("0x")
            if len(key_hex) == 64:
                return Keypair.from_seed(bytes.fromhex(key_hex))

    raise ValueError(f"unsupported keypair format: {path}")


# ─── Opcode-dispatch contract support ───
DISPATCHER_CONTRACTS = {
    "dex_core", "dex_amm", "dex_router", "dex_margin",
    "dex_rewards", "dex_governance", "dex_analytics", "prediction_market",
}

ABI_CACHE: Dict[str, Any] = {}


def load_abi(contract_dir: str) -> Optional[Any]:
    if contract_dir in ABI_CACHE:
        return ABI_CACHE[contract_dir]
    abi_path = ROOT / "contracts" / contract_dir / "abi.json"
    if not abi_path.exists():
        return None
    with open(abi_path) as f:
        abi = json.load(f)
    ABI_CACHE[contract_dir] = abi
    return abi


def build_dispatcher_ix(abi: dict, fn_name: str, args: dict) -> bytes:
    """Build binary instruction for opcode-dispatch contracts."""
    funcs = abi.get("functions", [])
    func = next((f for f in funcs if f["name"] == fn_name), None)
    if not func:
        raise ValueError(f"Function {fn_name} not found in ABI")
    opcode = func.get("opcode")
    if opcode is None:
        raise ValueError(f"Function {fn_name} has no opcode")

    buf = bytearray([opcode])
    for param in func.get("params", []):
        pname = param["name"]
        ptype = param["type"]
        val = args.get(pname)
        if val is None:
            val = 0
        if ptype in ("u64", "u128"):
            buf += struct.pack("<Q", int(val))
        elif ptype == "u32":
            buf += struct.pack("<I", int(val))
        elif ptype == "u16":
            buf += struct.pack("<H", int(val))
        elif ptype == "u8":
            buf += struct.pack("<B", int(val))
        elif ptype == "bool":
            buf += struct.pack("<B", 1 if val else 0)
        elif ptype == "Pubkey":
            if hasattr(val, 'to_bytes'):
                buf += val.to_bytes()
            elif isinstance(val, bytes) and len(val) == 32:
                buf += val
            elif isinstance(val, str):
                decoded = None
                if len(val) == 64:
                    try:
                        decoded = bytes.fromhex(val)
                    except ValueError:
                        pass
                if decoded is None:
                    try:
                        decoded = PublicKey(val).to_bytes()
                    except Exception:
                        decoded = b'\x00' * 32
                buf += decoded
            else:
                buf += b'\x00' * 32
        elif ptype == "string":
            encoded = str(val).encode("utf-8")
            buf += struct.pack("<H", len(encoded)) + encoded
        elif ptype == "i64":
            buf += struct.pack("<q", int(val))
        elif ptype == "i32":
            buf += struct.pack("<i", int(val))
        elif ptype == "i16":
            buf += struct.pack("<h", int(val))
        else:
            buf += struct.pack("<Q", int(val))
    return bytes(buf)


def load_activity_overrides() -> Dict[str, int]:
    if not CONTRACT_ACTIVITY_OVERRIDES_RAW.strip():
        return {}
    try:
        data = json.loads(CONTRACT_ACTIVITY_OVERRIDES_RAW)
    except Exception:
        return {}
    if not isinstance(data, dict):
        return {}
    out: Dict[str, int] = {}
    for key, value in data.items():
        if not isinstance(key, str):
            continue
        if isinstance(value, int) and value >= 0:
            out[key] = value
    return out


def report(status: str, msg: str) -> None:
    global PASS, FAIL, SKIP
    if status == "PASS":
        PASS += 1
        print(f"  PASS  {msg}")
    elif status == "SKIP":
        SKIP += 1
        print(f"  SKIP  {msg}")
    else:
        FAIL += 1
        print(f"  FAIL  {msg}")


def record_result(contract: str, action: str, status: str, detail: str) -> None:
    RESULTS.append(
        {
            "contract": contract,
            "action": action,
            "status": status,
            "detail": detail,
            "timestamp": int(time.time()),
        }
    )


def write_report() -> None:
    report_path = Path(WRITE_E2E_REPORT_PATH)
    report_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "rpc_url": RPC_URL,
        "strict_write_assertions": STRICT_WRITE_ASSERTIONS,
        "require_full_write_activity": REQUIRE_FULL_WRITE_ACTIVITY,
        "enforce_domain_assertions": ENFORCE_DOMAIN_ASSERTIONS,
        "enable_negative_assertions": ENABLE_NEGATIVE_ASSERTIONS,
        "require_negative_reason_match": REQUIRE_NEGATIVE_REASON_MATCH,
        "require_negative_code_match": REQUIRE_NEGATIVE_CODE_MATCH,
        "require_scenario_for_discovered": REQUIRE_SCENARIO_FOR_DISCOVERED,
        "min_negative_assertions_executed": MIN_NEGATIVE_ASSERTIONS_EXECUTED,
        "require_expected_contract_set": REQUIRE_EXPECTED_CONTRACT_SET,
        "expected_contracts_file": EXPECTED_CONTRACTS_FILE,
        "contract_set_diff": CONTRACT_SET_DIFF,
        "summary": {"pass": PASS, "fail": FAIL, "skip": SKIP},
        "results": RESULTS,
        "generated_at": int(time.time()),
    }
    report_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    print(f"REPORT: {report_path}")


def normalize(s: str) -> str:
    return "".join(ch for ch in s.lower() if ch.isalnum())


def canonical_contract_name(name: str) -> str:
    aliases = {
        "musdtoken": "musd_token",
        "wethtoken": "weth_token",
        "wsoltoken": "wsol_token",
        "predictionmarket": "prediction_market",
        "computemarket": "compute_market",
        "dexcore": "dex_core",
        "dexamm": "dex_amm",
        "dexrouter": "dex_router",
        "dexmargin": "dex_margin",
        "dexrewards": "dex_rewards",
        "dexgovernance": "dex_governance",
        "dexanalytics": "dex_analytics",
    }
    key = normalize(name)
    return aliases.get(key, name)


def load_expected_contracts() -> List[str]:
    path = Path(EXPECTED_CONTRACTS_FILE)
    if not path.exists():
        return []
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return []
    items: List[str] = []
    if isinstance(raw, list):
        items = [str(v) for v in raw if isinstance(v, str)]
    elif isinstance(raw, dict):
        contracts = raw.get("contracts", [])
        if isinstance(contracts, list):
            items = [str(v) for v in contracts if isinstance(v, str)]
    return sorted({canonical_contract_name(v) for v in items})


def parse_contract_public_functions() -> Dict[str, List[str]]:
    contracts_dir = ROOT / "contracts"
    output: Dict[str, List[str]] = {}
    if not contracts_dir.exists():
        return output

    pattern = re.compile(r'pub\s+extern\s+"C"\s+fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(')
    for child in sorted(contracts_dir.iterdir()):
        if not child.is_dir():
            continue
        lib_rs = child / "src" / "lib.rs"
        if not lib_rs.exists():
            continue
        try:
            source = lib_rs.read_text(encoding="utf-8")
        except Exception:
            continue
        names = sorted(set(pattern.findall(source)))
        output[child.name] = names
    return output


async def call_contract(
    conn: Connection,
    caller: Keypair,
    program: PublicKey,
    func: str,
    args: Optional[Dict[str, Any]] = None,
    contract_dir: str = "",
) -> str:
    args = args or {}

    # Detect dispatcher contracts and build appropriate instruction data
    abi = load_abi(contract_dir) if contract_dir else None
    is_dispatcher = contract_dir in DISPATCHER_CONTRACTS and abi is not None

    if is_dispatcher:
        raw_args = build_dispatcher_ix(abi, func, args)
        envelope_fn = "call"
    else:
        raw_args = json.dumps(args).encode()
        envelope_fn = func

    payload = json.dumps({"Call": {"function": envelope_fn, "args": list(raw_args), "value": 0}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.public_key(), program],
        data=payload.encode(),
    )
    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(caller)
    )
    return await conn.send_transaction(tx)


def _extract_count(payload: Any) -> int:
    if payload is None:
        return 0
    if isinstance(payload, int):
        return payload
    if isinstance(payload, list):
        return len(payload)
    if isinstance(payload, dict):
        for key in ["total", "count", "calls", "events", "total_calls", "total_events"]:
            value = payload.get(key)
            if isinstance(value, int):
                return value
        for key in ["items", "data", "results", "rows", "entries"]:
            value = payload.get(key)
            if isinstance(value, list):
                return len(value)
        for value in payload.values():
            if isinstance(value, list):
                return len(value)
    return 0


def is_write_function(function_name: str) -> bool:
    lowered = function_name.lower()
    return not (
        lowered.startswith("get_")
        or lowered.startswith("quote_")
        or lowered.startswith("is_")
        or lowered.startswith("has_")
    )


async def wait_for_transaction(conn: Connection, signature: str, timeout_secs: int) -> Dict[str, Any]:
    started = time.time()
    last_error: Optional[Exception] = None
    while time.time() - started <= timeout_secs:
        try:
            tx = await conn.get_transaction(signature)
            if tx:
                return tx
        except Exception as exc:
            last_error = exc
        await asyncio.sleep(0.5)
    if last_error is not None:
        raise Exception(f"transaction not confirmed within {timeout_secs}s (last error: {last_error})")
    raise Exception(f"transaction not confirmed within {timeout_secs}s")


async def get_program_observability(conn: Connection, program: PublicKey) -> Tuple[int, int]:
    program_id = str(program)
    calls_raw = await conn._rpc("getProgramCalls", [program_id, {"limit": PROGRAM_CALLS_LIMIT}])
    calls_count = _extract_count(calls_raw)

    events_count = 0
    try:
        events_raw = await conn._rpc("getContractEvents", [program_id, PROGRAM_CALLS_LIMIT, 0])
        events_count = _extract_count(events_raw)
    except Exception:
        events_raw = await conn._rpc("getContractEvents", [program_id, PROGRAM_CALLS_LIMIT])
        events_count = _extract_count(events_raw)

    return calls_count, events_count


async def get_program_storage_count(conn: Connection, program: PublicKey) -> int:
    program_id = str(program)
    storage_raw = await conn._rpc("getProgramStorage", [program_id, {"limit": 400}])
    return _extract_count(storage_raw)


def _collect_strings(value: Any, out: List[str]) -> None:
    if value is None:
        return
    if isinstance(value, str):
        out.append(value)
        return
    if isinstance(value, (int, float, bool)):
        out.append(str(value))
        return
    if isinstance(value, dict):
        for child in value.values():
            _collect_strings(child, out)
        return
    if isinstance(value, list):
        for child in value:
            _collect_strings(child, out)


def transaction_contains_any(tx_data: Dict[str, Any], expected_fragments: List[str]) -> bool:
    if not expected_fragments:
        return True
    fragments = [frag.lower() for frag in expected_fragments if frag]
    if not fragments:
        return True
    strings: List[str] = []
    _collect_strings(tx_data, strings)
    blob = "\n".join(strings).lower()
    return any(fragment in blob for fragment in fragments)


def transaction_matches_error_code(tx_data: Dict[str, Any], expected_code: int) -> bool:
    strings: List[str] = []
    _collect_strings(tx_data, strings)
    blob = "\n".join(strings).lower()

    direct_markers = [
        f"return: {expected_code}",
        f"return {expected_code}",
        f"code: {expected_code}",
        f"code {expected_code}",
        f"error code: {expected_code}",
        f"err={expected_code}",
    ]
    if any(marker in blob for marker in direct_markers):
        return True

    for match in re.finditer(r"(?:return|code|err|error)\s*[:=]?\s*(-?\d+)", blob):
        try:
            if int(match.group(1)) == expected_code:
                return True
        except Exception:
            continue
    return False


def evaluate_domain_assertions(
    contract_name: str,
    step_status: Dict[str, bool],
    calls_delta: int,
    events_delta: int,
    storage_delta: int,
) -> List[Tuple[str, bool, str]]:
    results: List[Tuple[str, bool, str]] = []

    def require_steps(assertion_name: str, required_steps: List[str], require_storage_change: bool = False) -> None:
        missing = [fn for fn in required_steps if not step_status.get(fn, False)]
        if missing:
            results.append((assertion_name, False, f"missing required successful steps: {','.join(missing)}"))
            return
        if require_storage_change and storage_delta <= 0:
            results.append((assertion_name, False, f"no storage delta observed ({storage_delta})"))
            return
        results.append(
            (
                assertion_name,
                True,
                f"required steps satisfied, calls_delta={calls_delta}, events_delta={events_delta}, storage_delta={storage_delta}",
            )
        )

    if contract_name in {"moltcoin", "musd_token", "weth_token", "wsol_token"}:
        require_steps("token_invariants", ["mint", "transfer", "burn"])

    elif contract_name == "clawpump":
        require_steps("launch_token_flow", ["create_token", "buy"])

    elif contract_name == "lobsterlend":
        require_steps("lending_credit_flow", ["deposit", "borrow", "repay"])

    elif contract_name == "moltbridge":
        require_steps("bridge_admin_config", ["set_required_confirmations", "set_request_timeout"])

    elif contract_name == "reef_storage":
        require_steps("storage_provider_flow", ["register_provider", "set_storage_price"])

    elif contract_name == "clawvault":
        require_steps("vault_roundtrip_flow", ["deposit", "withdraw"])

    elif contract_name == "moltmarket":
        require_steps("market_listing_flow", ["list_nft", "cancel_listing"])

    elif contract_name == "moltauction":
        require_steps("auction_bid_flow", ["create_auction", "place_bid"]) 

    elif contract_name == "bountyboard":
        require_steps("bounty_submission_flow", ["create_bounty", "submit_work"])

    elif contract_name == "moltyid":
        require_steps(
            "identity_lifecycle_flow",
            [
                "register_identity",
                "set_endpoint",
                "set_metadata",
                "set_rate",
                "set_delegate",
                "revoke_delegate",
            ],
        )

    elif contract_name == "prediction_market":
        require_steps("prediction_admin_wiring", ["set_moltyid_address", "set_musd_address"], require_storage_change=False)

    elif contract_name == "moltdao":
        require_steps("dao_governance_flow", ["create_proposal_typed", "vote"])

    elif contract_name == "compute_market":
        require_steps("compute_job_flow", ["register_provider", "submit_job"])

    elif contract_name == "moltpunks":
        require_steps("nft_ownership_flow", ["mint", "transfer", "transfer_from"])

    elif contract_name == "moltoracle":
        require_steps("oracle_freshness_flow", ["add_price_feeder", "submit_price", "request_randomness"])
        if calls_delta <= 0 and events_delta <= 0:
            results.append(("oracle_signal_delta", False, f"no oracle activity deltas (calls={calls_delta}, events={events_delta})"))
        else:
            results.append(("oracle_signal_delta", True, f"calls_delta={calls_delta}, events_delta={events_delta}"))

    elif contract_name == "moltswap":
        require_steps("swap_reserve_movement", ["add_liquidity", "swap_a_for_b"])

    return results


def _extract_balance(raw: Any) -> int:
    if isinstance(raw, int):
        return raw
    if isinstance(raw, dict):
        for key in ["balance", "lamports", "shells", "amount", "value"]:
            value = raw.get(key)
            if isinstance(value, int):
                return value
    return 0


async def ensure_minimum_balance(conn: Connection, sender: Keypair, recipient: PublicKey, min_amount: int) -> Optional[str]:
    recipient_balance = _extract_balance(await conn.get_balance(recipient))
    if recipient_balance >= min_amount:
        return None

    sender_balance = _extract_balance(await conn.get_balance(sender.public_key()))
    transfer_amount = min_amount - recipient_balance
    if transfer_amount <= 0:
        return None
    if sender_balance <= transfer_amount:
        raise Exception(
            f"insufficient sender balance for funding (sender={sender_balance}, needed={transfer_amount})"
        )

    blockhash = await conn.get_recent_blockhash()
    ix = TransactionBuilder.transfer(sender.public_key(), recipient, transfer_amount)
    tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(sender)
    sig = await conn.send_transaction(tx)
    await wait_for_transaction(conn, sig, TX_CONFIRM_TIMEOUT_SECS)
    return sig


async def get_contracts_map(conn: Connection) -> Dict[str, PublicKey]:
    # Symbol→dir_name mapping for the genesis contract catalog
    SYMBOL_TO_DIR: Dict[str, str] = {
        "MOLT": "moltcoin",
        "MUSD": "musd_token",
        "WSOL": "wsol_token",
        "WETH": "weth_token",
        "YID": "moltyid",
        "DEX": "dex_core",
        "DEXAMM": "dex_amm",
        "DEXROUTER": "dex_router",
        "DEXMARGIN": "dex_margin",
        "DEXREWARDS": "dex_rewards",
        "DEXGOV": "dex_governance",
        "ANALYTICS": "dex_analytics",
        "MOLTSWAP": "moltswap",
        "BRIDGE": "moltbridge",
        "ORACLE": "moltoracle",
        "LEND": "lobsterlend",
        "DAO": "moltdao",
        "MARKET": "moltmarket",
        "PUNKS": "moltpunks",
        "CLAWPAY": "clawpay",
        "CLAWPUMP": "clawpump",
        "CLAWVAULT": "clawvault",
        "COMPUTE": "compute_market",
        "REEF": "reef_storage",
        "PREDICT": "prediction_market",
        "BOUNTY": "bountyboard",
        "AUCTION": "moltauction",
    }

    ALL_NAMES = [
        "moltcoin", "clawpump", "lobsterlend", "moltmarket", "moltauction",
        "moltbridge", "reef_storage", "clawvault", "clawpay", "moltdao",
        "prediction_market", "compute_market", "moltyid", "dex_core",
        "dex_amm", "dex_router", "dex_margin", "dex_rewards",
        "dex_governance", "dex_analytics", "weth_token", "wsol_token",
        "musd_token", "moltpunks", "moltoracle", "moltswap", "bountyboard",
    ]

    discovered: Dict[str, PublicKey] = {}

    # Strategy 1: Try getAllContracts metadata (existing approach)
    try:
        result = await conn.get_all_contracts()
        contracts = result if isinstance(result, list) else result.get("contracts", [])
        for entry in contracts:
            if not isinstance(entry, dict):
                continue
            pid = entry.get("program_id") or entry.get("address") or entry.get("id") or entry.get("contract_id")
            if not isinstance(pid, str) or len(pid) < 20:
                continue
            blob = normalize(json.dumps(entry))
            for name in ALL_NAMES:
                if normalize(name) in blob and name not in discovered:
                    try:
                        discovered[name] = PublicKey.from_base58(pid)
                    except Exception:
                        continue
    except Exception:
        pass

    # Strategy 2: Use getAllSymbolRegistry for name→address mapping (primary source)
    if len(discovered) < len(ALL_NAMES):
        try:
            reg_raw = await conn._rpc("getAllSymbolRegistry", [])
            entries = reg_raw if isinstance(reg_raw, list) else reg_raw.get("entries", reg_raw.get("symbols", []))
            if isinstance(entries, list):
                for entry in entries:
                    if not isinstance(entry, dict):
                        continue
                    symbol = entry.get("symbol", "")
                    program = entry.get("program", "")
                    if not symbol or not program:
                        continue
                    # Direct symbol→dir_name mapping
                    dir_name = SYMBOL_TO_DIR.get(symbol.upper())
                    if dir_name and dir_name not in discovered:
                        try:
                            discovered[dir_name] = PublicKey.from_base58(program)
                        except Exception:
                            continue
                    # Also try matching via normalized blob
                    if not dir_name:
                        blob = normalize(json.dumps(entry))
                        for name in ALL_NAMES:
                            if normalize(name) in blob and name not in discovered:
                                try:
                                    discovered[name] = PublicKey.from_base58(program)
                                except Exception:
                                    continue
        except Exception:
            pass

    return discovered


def scenario_spec(deployer: Keypair, secondary: Keypair, contracts: Dict[str, PublicKey]) -> Dict[str, List[Dict[str, Any]]]:
    provider = str(deployer.public_key())
    user2 = str(secondary.public_key())
    zero_addr = "11111111111111111111111111111111"
    quote_addr = str(contracts.get("moltcoin") or provider)
    base_addr = str(contracts.get("weth_token") or contracts.get("wsol_token") or provider)
    dex_core_addr = str(contracts.get("dex_core") or provider)
    dex_amm_addr = str(contracts.get("dex_amm") or provider)
    now = int(time.time())
    market_deadline_slot = now + 1_000_000
    identity_name = f"agent{random.randint(100, 999)}"
    metadata_json = '{"role":"agent","mode":"e2e"}'
    endpoint_url = "https://agent.e2e"
    asset_symbol = "MOLT"
    nft_metadata = f"ipfs://moltpunks/{random.randint(1000, 9999)}"
    rand_token_id = random.randint(10000, 99999)
    rand_listing_id = random.randint(20000, 99999)
    rand_job_id = random.randint(30000, 99999)

    return {
        "moltcoin": [
            {"fn": "initialize", "args": {"owner": provider}, "actor": "deployer"},
            {"fn": "mint", "args": {"to": provider, "amount": 1000}, "actor": "deployer"},
            {"fn": "transfer", "args": {"from": provider, "to": user2, "amount": 25}, "actor": "deployer"},
            {"fn": "burn", "args": {"from": provider, "amount": 5}, "actor": "deployer"},
        ],
        "clawpump": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "create_token", "args": {"creator": provider, "fee_paid": 10_000_000_000}, "actor": "deployer"},
            {"fn": "buy", "args": {"buyer": provider, "token_id": 0, "molt_amount": 1_000_000_000}, "actor": "deployer"},
            {"fn": "get_platform_stats", "args": {}, "actor": "deployer"},
        ],
        "lobsterlend": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "deposit", "args": {"depositor": provider, "amount": 1_000_000_000}, "actor": "deployer"},
            {"fn": "borrow", "args": {"borrower": provider, "amount": 100_000_000}, "actor": "deployer"},
            {"fn": "repay", "args": {"borrower": provider, "amount": 50_000_000}, "actor": "deployer"},
            {"fn": "get_protocol_stats", "args": {}, "actor": "deployer"},
        ],
        "moltmarket": [
            {"fn": "initialize", "args": {"owner": provider, "fee_addr": provider}, "actor": "deployer"},
            {"fn": "list_nft", "args": {"seller": provider, "token_id": rand_listing_id, "price": 500}, "actor": "deployer"},
            {"fn": "cancel_listing", "args": {"seller": provider, "token_id": rand_listing_id}, "actor": "deployer"},
            {"fn": "get_marketplace_stats", "args": {}, "actor": "deployer"},
        ],
        "moltauction": [
            {"fn": "initialize", "args": {"marketplace": provider}, "actor": "deployer"},
            {"fn": "create_auction", "args": {"seller": provider, "token_id": rand_token_id, "start_price": 100, "duration_slots": 300}, "actor": "deployer"},
            {"fn": "place_bid", "args": {"bidder": provider, "token_id": rand_token_id, "bid_amount": 120}, "actor": "deployer"},
            {"fn": "cancel_auction", "args": {"seller": provider, "token_id": rand_token_id}, "actor": "deployer"},
        ],
        "moltbridge": [
            {"fn": "initialize", "args": {"owner": provider}, "actor": "deployer"},
            {"fn": "set_required_confirmations", "args": {"caller": provider, "required": 1}, "actor": "deployer"},
            {"fn": "set_request_timeout", "args": {"caller": provider, "timeout": 3600}, "actor": "deployer"},
        ],
        "reef_storage": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "register_provider", "args": {"provider": provider, "capacity_bytes": 1_000_000}, "actor": "deployer"},
            {"fn": "set_storage_price", "args": {"provider": provider, "price_per_byte_per_slot": 1}, "actor": "deployer"},
        ],
        "clawvault": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "deposit", "args": {"depositor": provider, "amount": 1_000_000_000}, "actor": "deployer"},
            {"fn": "withdraw", "args": {"depositor": provider, "shares_to_burn": 1}, "actor": "deployer"},
            {"fn": "get_vault_stats", "args": {}, "actor": "deployer"},
        ],
        "clawpay": [
            {"fn": "initialize_cp_admin", "args": {"admin": provider}, "actor": "deployer"},
            {
                "fn": "create_stream",
                "args": {
                    "sender": provider,
                    "recipient": user2,
                    "total_amount": 1_000_000_000,
                    "start_time": now,
                    "end_time": now + 3600,
                },
                "actor": "deployer",
            },
            {"fn": "get_stream_info", "args": {"stream_id": 0}, "actor": "deployer"},
        ],
        "moltyid": [
            {"fn": "initialize", "args": {"admin_ptr": provider}, "actor": "deployer"},
            {
                "fn": "register_identity",
                "args": {
                    "owner_ptr": provider,
                    "agent_type": 1,
                    "name_ptr": identity_name,
                    "name_len": len(identity_name),
                },
                "actor": "deployer",
            },
            {"fn": "set_endpoint", "args": {"caller_ptr": provider, "url_ptr": endpoint_url, "url_len": len(endpoint_url)}, "actor": "deployer"},
            {"fn": "set_metadata", "args": {"caller_ptr": provider, "json_ptr": metadata_json, "json_len": len(metadata_json)}, "actor": "deployer"},
            {"fn": "set_availability", "args": {"caller_ptr": provider, "status": 1}, "actor": "deployer"},
            {"fn": "set_rate", "args": {"caller_ptr": provider, "molt_per_unit": 1_000}, "actor": "deployer"},
            {"fn": "add_skill", "args": {"owner_ptr": provider, "skill_ptr": "rust", "skill_len": 4}, "actor": "deployer"},
            {"fn": "vouch", "args": {"voucher_ptr": provider, "vouchee_ptr": user2}, "actor": "deployer"},
            {"fn": "set_delegate", "args": {"owner_ptr": provider, "delegate_ptr": user2, "flags": 255, "expiry_ts": now + 86_400}, "actor": "deployer"},
            {"fn": "revoke_delegate", "args": {"owner_ptr": provider, "delegate_ptr": user2}, "actor": "deployer"},
            {
                "fn": "register_identity",
                "args": {
                    "owner_ptr": provider,
                    "agent_type": 1,
                    "name_ptr": f"dup{random.randint(100, 999)}",
                    "name_len": 6,
                },
                "actor": "deployer",
                "negative": True,
                "expect_no_state_change": True,
                "expected_error_code": 3,
                "expected_error_any": ["already registered", "return: 3", "error"],
            },
            {"fn": "get_agent_profile", "args": {"addr_ptr": provider}, "actor": "deployer"},
        ],
        "moltdao": [
            {"fn": "initialize_dao", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "create_proposal_typed", "args": {"title": "E2E", "description": "E2E Proposal", "proposal_type": 1}, "actor": "deployer"},
            {"fn": "vote", "args": {"proposal_id": 0, "vote": 1}, "actor": "deployer"},
        ],
        "prediction_market": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "set_moltyid_address", "args": {"caller": provider, "address": str(contracts.get("moltyid", ""))}, "actor": "deployer"},
            {"fn": "set_musd_address", "args": {"caller": provider, "address": str(contracts.get("moltcoin", ""))}, "actor": "deployer"},
        ],
        "compute_market": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "register_provider", "args": {"provider": provider, "endpoint": "https://provider.e2e", "price_per_unit": 1_000_000}, "actor": "deployer"},
            {"fn": "submit_job", "args": {"job_id": rand_job_id, "requester": provider, "budget": 1_000_000}, "actor": "deployer"},
            {"fn": "get_job", "args": {"job_id": rand_job_id}, "actor": "deployer"},
        ],
        "bountyboard": [
            {"fn": "set_identity_admin", "args": {"admin_ptr": provider}, "actor": "deployer"},
            {"fn": "set_moltyid_address", "args": {"caller_ptr": provider, "moltyid_addr_ptr": str(contracts.get("moltyid") or zero_addr)}, "actor": "deployer"},
            {"fn": "set_token_address", "args": {"caller_ptr": provider, "token_addr_ptr": quote_addr}, "actor": "deployer"},
            {
                "fn": "create_bounty",
                "args": {
                    "creator_ptr": provider,
                    "title_hash_ptr": provider,
                    "reward_amount": 1_000,
                    "deadline_slot": market_deadline_slot,
                },
                "actor": "deployer",
            },
            {"fn": "submit_work", "args": {"bounty_id": 0, "worker_ptr": provider, "proof_hash_ptr": provider}, "actor": "deployer"},
            {"fn": "get_bounty", "args": {"bounty_id": 0}, "actor": "deployer"},
        ],
        "moltoracle": [
            {"fn": "initialize_oracle", "args": {"owner_ptr": provider}, "actor": "deployer"},
            {"fn": "add_price_feeder", "args": {"feeder_ptr": provider, "asset_ptr": asset_symbol, "asset_len": len(asset_symbol)}, "actor": "deployer"},
            {"fn": "submit_price", "args": {"feeder_ptr": provider, "asset_ptr": asset_symbol, "asset_len": len(asset_symbol), "price": 1_000_000_000, "decimals": 6}, "actor": "deployer"},
            {
                "fn": "submit_price",
                "args": {"feeder_ptr": user2, "asset_ptr": asset_symbol, "asset_len": len(asset_symbol), "price": 999_000_000, "decimals": 6},
                "actor": "secondary",
                "negative": True,
                "expect_no_state_change": True,
                "expected_error_code": 0,
                "expected_error_any": ["not authorized", "no authorized feeder", "return: 0", "error"],
            },
            {"fn": "commit_randomness", "args": {"requester_ptr": provider, "commit_hash_ptr": provider, "seed": 42}, "actor": "deployer"},
            {"fn": "request_randomness", "args": {"requester_ptr": provider, "seed": 42}, "actor": "deployer"},
            {"fn": "get_oracle_stats", "args": {}, "actor": "deployer"},
        ],
        "moltpunks": [
            {"fn": "initialize", "args": {"minter_ptr": provider}, "actor": "deployer"},
            {
                "fn": "mint",
                "args": {
                    "caller_ptr": provider,
                    "to_ptr": provider,
                    "token_id": rand_token_id,
                    "metadata_ptr": nft_metadata,
                    "metadata_len": len(nft_metadata),
                },
                "actor": "deployer",
            },
            {"fn": "transfer", "args": {"from_ptr": provider, "to_ptr": user2, "token_id": rand_token_id}, "actor": "deployer"},
            {
                "fn": "approve",
                "args": {"owner_ptr": user2, "spender_ptr": provider, "token_id": rand_token_id},
                "actor": "secondary",
                "expect_no_state_change": True,
            },
            {"fn": "transfer_from", "args": {"caller_ptr": provider, "from_ptr": user2, "to_ptr": provider, "token_id": rand_token_id}, "actor": "deployer"},
            {
                "fn": "mint",
                "args": {
                    "caller_ptr": user2,
                    "to_ptr": user2,
                    "token_id": rand_token_id + 1,
                    "metadata_ptr": nft_metadata,
                    "metadata_len": len(nft_metadata),
                },
                "actor": "secondary",
                "negative": True,
                "expect_no_state_change": True,
                "expected_error_code": 0,
                "expected_error_any": ["unauthorized", "mint failed", "return: 0", "error"],
            },
        ],
        "moltswap": [
            {"fn": "initialize", "args": {"token_a_ptr": base_addr, "token_b_ptr": quote_addr}, "actor": "deployer"},
            {"fn": "set_identity_admin", "args": {"admin_ptr": provider}, "actor": "deployer"},
            {"fn": "add_liquidity", "args": {"provider_ptr": provider, "amount_a": 100_000, "amount_b": 100_000, "min_liquidity": 1}, "actor": "deployer"},
            {"fn": "swap_a_for_b", "args": {"amount_a_in": 1_000, "min_amount_b_out": 1}, "actor": "deployer"},
            {"fn": "set_protocol_fee", "args": {"caller_ptr": provider, "treasury_ptr": provider, "fee_share": 1500}, "actor": "deployer"},
            {
                "fn": "set_protocol_fee",
                "args": {"caller_ptr": user2, "treasury_ptr": user2, "fee_share": 1200},
                "actor": "secondary",
                "negative": True,
                "expect_no_state_change": True,
                "expected_error_code": 2,
                "expected_error_any": ["unauthorized", "return: 2", "error"],
            },
            {"fn": "ms_pause", "args": {"caller_ptr": base_addr}, "actor": "deployer"},
            {"fn": "ms_unpause", "args": {"caller_ptr": base_addr}, "actor": "deployer"},
        ],
        "musd_token": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "mint", "args": {"caller": provider, "to": provider, "amount": 1_000_000}, "actor": "deployer"},
            {"fn": "transfer", "args": {"from": provider, "to": user2, "amount": 10_000}, "actor": "deployer"},
            {"fn": "approve", "args": {"owner": provider, "spender": user2, "amount": 5_000}, "actor": "deployer"},
            {"fn": "burn", "args": {"caller": provider, "amount": 1_000}, "actor": "deployer"},
            {
                "fn": "mint",
                "args": {"caller": user2, "to": user2, "amount": 1111},
                "actor": "secondary",
                "negative": True,
                "expect_no_state_change": True,
                "expected_error_code": 2,
                "expected_error_any": ["unauthorized", "return: 2", "error"],
            },
            {"fn": "get_transfer_count", "args": {}, "actor": "deployer"},
        ],
        "weth_token": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "mint", "args": {"caller": provider, "to": provider, "amount": 1_000_000}, "actor": "deployer"},
            {"fn": "transfer", "args": {"from": provider, "to": user2, "amount": 10_000}, "actor": "deployer"},
            {"fn": "approve", "args": {"owner": provider, "spender": user2, "amount": 5_000}, "actor": "deployer"},
            {"fn": "burn", "args": {"caller": provider, "amount": 1_000}, "actor": "deployer"},
            {"fn": "get_transfer_count", "args": {}, "actor": "deployer"},
        ],
        "wsol_token": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "mint", "args": {"caller": provider, "to": provider, "amount": 1_000_000}, "actor": "deployer"},
            {"fn": "transfer", "args": {"from": provider, "to": user2, "amount": 10_000}, "actor": "deployer"},
            {"fn": "approve", "args": {"owner": provider, "spender": user2, "amount": 5_000}, "actor": "deployer"},
            {"fn": "burn", "args": {"caller": provider, "amount": 1_000}, "actor": "deployer"},
            {"fn": "get_transfer_count", "args": {}, "actor": "deployer"},
        ],
        "dex_core": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "set_preferred_quote", "args": {"caller": provider, "quote_address": quote_addr}, "actor": "deployer"},
            {
                "fn": "create_pair",
                "args": {
                    "caller": provider,
                    "base_token": base_addr,
                    "quote_token": quote_addr,
                    "tick_size": 1_000_000,
                    "lot_size": 1,
                    "min_order": 1_000,
                },
                "actor": "deployer",
            },
            {"fn": "update_pair_fees", "args": {"caller": provider, "pair_id": 1, "maker_fee_bps": -1, "taker_fee_bps": 5}, "actor": "deployer"},
            {
                "fn": "place_order",
                "args": {
                    "caller": provider,
                    "pair_id": 1,
                    "side": 0,
                    "order_type": 0,
                    "price": 1_000_000_000,
                    "quantity": 10_000,
                    "expiry": 0,
                },
                "actor": "deployer",
            },
            {"fn": "modify_order", "args": {"caller": provider, "order_id": 1, "new_price": 1_001_000_000, "new_quantity": 10_000}, "actor": "deployer"},
            {"fn": "cancel_all_orders", "args": {"caller": provider, "pair_id": 1}, "actor": "deployer"},
            {"fn": "get_pair_count", "args": {}, "actor": "deployer"},
        ],
        "dex_amm": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {
                "fn": "create_pool",
                "args": {
                    "caller": provider,
                    "token_a": base_addr,
                    "token_b": quote_addr,
                    "fee_tier": 0,
                    "initial_sqrt_price": 1_000_000_000,
                },
                "actor": "deployer",
            },
            {
                "fn": "add_liquidity",
                "args": {
                    "provider": provider,
                    "pool_id": 1,
                    "lower_tick": -100,
                    "upper_tick": 100,
                    "amount_a": 100_000,
                    "amount_b": 100_000,
                },
                "actor": "deployer",
            },
            {"fn": "swap_exact_in", "args": {"trader": provider, "pool_id": 1, "is_token_a_in": True, "amount_in": 1_000, "min_out": 0, "deadline": 0}, "actor": "deployer"},
            {"fn": "remove_liquidity", "args": {"provider": provider, "position_id": 1, "liquidity_amount": 1}, "actor": "deployer"},
            {"fn": "get_pool_count", "args": {}, "actor": "deployer"},
            {"fn": "get_position_count", "args": {}, "actor": "deployer"},
        ],
        "dex_router": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {
                "fn": "set_addresses",
                "args": {
                    "caller": provider,
                    "core_address": dex_core_addr,
                    "amm_address": dex_amm_addr,
                    "legacy_address": zero_addr,
                },
                "actor": "deployer",
            },
            {
                "fn": "register_route",
                "args": {
                    "caller": provider,
                    "token_in": base_addr,
                    "token_out": quote_addr,
                    "route_type": 1,
                    "pool_id": 1,
                    "secondary_id": 0,
                    "split_percent": 50,
                },
                "actor": "deployer",
            },
            {"fn": "set_route_enabled", "args": {"caller": provider, "route_id": 1, "enabled": True}, "actor": "deployer"},
            {"fn": "get_best_route", "args": {"token_in": base_addr, "token_out": quote_addr, "amount": 1_000}, "actor": "deployer"},
            {"fn": "get_route_count", "args": {}, "actor": "deployer"},
        ],
        "dex_margin": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "set_mark_price", "args": {"caller": provider, "pair_id": 1, "price": 1_000_000_000}, "actor": "deployer"},
            {
                "fn": "open_position",
                "args": {
                    "trader": provider,
                    "pair_id": 1,
                    "side": 0,
                    "size": 1_000_000_000,
                    "leverage": 2,
                    "margin": 300_000_000,
                },
                "actor": "deployer",
            },
            {"fn": "add_margin", "args": {"caller": provider, "position_id": 1, "amount": 10_000_000}, "actor": "deployer"},
            {"fn": "remove_margin", "args": {"caller": provider, "position_id": 1, "amount": 1_000_000}, "actor": "deployer"},
            {"fn": "close_position", "args": {"caller": provider, "position_id": 1}, "actor": "deployer"},
            {"fn": "get_margin_stats", "args": {}, "actor": "deployer"},
        ],
        "dex_rewards": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "set_moltcoin_address", "args": {"caller": provider, "addr": quote_addr}, "actor": "deployer"},
            {"fn": "set_rewards_pool", "args": {"caller": provider, "addr": provider}, "actor": "deployer"},
            {"fn": "set_reward_rate", "args": {"caller": provider, "pair_id": 1, "rate": 100}, "actor": "deployer"},
            {"fn": "record_trade", "args": {"trader": provider, "fee_paid": 1_000, "volume": 50_000}, "actor": "deployer"},
            {"fn": "register_referral", "args": {"trader": user2, "referrer": provider}, "actor": "deployer"},
            {"fn": "get_total_distributed", "args": {}, "actor": "deployer"},
        ],
        "dex_governance": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "set_moltyid_address", "args": {"caller": provider, "address": str(contracts.get("moltyid") or zero_addr)}, "actor": "deployer"},
            {"fn": "set_preferred_quote", "args": {"caller": provider, "quote_address": quote_addr}, "actor": "deployer"},
            {"fn": "set_listing_requirements", "args": {"caller": provider, "min_stake": 1000, "voting_period": 1}, "actor": "deployer"},
            {"fn": "propose_fee_change", "args": {"caller": provider, "pair_id": 1, "maker_fee_bps": -1, "taker_fee_bps": 5}, "actor": "deployer"},
            {"fn": "get_proposal_count", "args": {}, "actor": "deployer"},
        ],
        "dex_analytics": [
            {"fn": "initialize", "args": {"admin": provider}, "actor": "deployer"},
            {"fn": "record_trade", "args": {"pair_id": 1, "price": 1_000_000_000, "volume": 10_000, "trader": provider}, "actor": "deployer"},
            {"fn": "get_record_count", "args": {}, "actor": "deployer"},
            {"fn": "get_last_price", "args": {"pair_id": 1}, "actor": "deployer"},
            {"fn": "get_24h_stats", "args": {"pair_id": 1}, "actor": "deployer"},
        ],
    }


async def main() -> int:
    print("\n===============================================================")
    print("  CONTRACT WRITE E2E")
    print(f"  RPC: {RPC_URL}")
    print(f"  STRICT_WRITE_ASSERTIONS: {int(STRICT_WRITE_ASSERTIONS)}")
    print(f"  REQUIRE_FULL_WRITE_ACTIVITY: {int(REQUIRE_FULL_WRITE_ACTIVITY)}")
    print(f"  ENFORCE_DOMAIN_ASSERTIONS: {int(ENFORCE_DOMAIN_ASSERTIONS)}")
    print(f"  ENABLE_NEGATIVE_ASSERTIONS: {int(ENABLE_NEGATIVE_ASSERTIONS)}")
    print(f"  REQUIRE_NEGATIVE_CODE_MATCH: {int(REQUIRE_NEGATIVE_CODE_MATCH)}")
    print(f"  REQUIRE_SCENARIO_FOR_DISCOVERED: {int(REQUIRE_SCENARIO_FOR_DISCOVERED)}")
    print(f"  MIN_NEGATIVE_ASSERTIONS_EXECUTED: {MIN_NEGATIVE_ASSERTIONS_EXECUTED}")
    print(f"  REQUIRE_EXPECTED_CONTRACT_SET: {int(REQUIRE_EXPECTED_CONTRACT_SET)}")
    print(f"  EXPECTED_CONTRACTS_FILE: {EXPECTED_CONTRACTS_FILE}")
    print("===============================================================\n")

    conn = Connection(RPC_URL)
    try:
        await conn.health()
        report("PASS", "validator healthy")
    except Exception as exc:
        report("FAIL", f"validator unreachable: {exc}")
        return 1

    if not Path(DEPLOYER_PATH).exists():
        report("FAIL", f"deployer keypair missing: {DEPLOYER_PATH}")
        return 1
    deployer = load_keypair_flexible(Path(DEPLOYER_PATH))

    if SECONDARY_PATH and Path(SECONDARY_PATH).exists():
        secondary = load_keypair_flexible(Path(SECONDARY_PATH))
    else:
        secondary = Keypair.generate()

    # Ensure deployer and secondary accounts are funded (airdrop safety net)
    for kp, label in [(deployer, "deployer"), (secondary, "secondary")]:
        try:
            resp = await conn._rpc("requestAirdrop", [str(kp.public_key()), 100])
            if resp and isinstance(resp, dict) and resp.get("success"):
                report("PASS", f"self-funded {label} via airdrop (100 MOLT)")
            else:
                report("PASS", f"{label} airdrop returned: {str(resp)[:80]}")
        except Exception as exc:
            report("PASS", f"{label} airdrop skipped: {exc}")

    try:
        funding_sig = await ensure_minimum_balance(conn, deployer, secondary.public_key(), 2_000_000_000)
        if funding_sig:
            report("PASS", f"secondary funded via transfer sig={funding_sig}")
        else:
            report("PASS", "secondary already funded for scenario execution")
    except Exception as exc:
        report("FAIL", f"secondary funding failed: {exc}")
        return 1

    contracts = await get_contracts_map(conn)
    discovery_unavailable = False
    if not contracts:
        discovery_unavailable = True
        report("FAIL", "no name-mapped contracts discovered for write scenarios (RPC metadata unavailable)")
        record_result("global", "contract_discovery", "FAIL", "getAllContracts does not expose contract names/metadata")
        contracts = {}

    expected_contracts = load_expected_contracts()
    discovered_contracts = sorted({canonical_contract_name(name) for name in contracts.keys()})
    if REQUIRE_EXPECTED_CONTRACT_SET:
        if not expected_contracts:
            report("FAIL", f"expected contract set missing or invalid: {EXPECTED_CONTRACTS_FILE}")
            record_result("global", "expected_contract_set", "FAIL", f"missing or invalid file: {EXPECTED_CONTRACTS_FILE}")
        else:
            missing_expected = sorted(set(expected_contracts) - set(discovered_contracts))
            unexpected_discovered = sorted(set(discovered_contracts) - set(expected_contracts))
            CONTRACT_SET_DIFF.update(
                {
                    "expected_count": len(expected_contracts),
                    "discovered_count": len(discovered_contracts),
                    "missing_expected": missing_expected,
                    "unexpected_discovered": unexpected_discovered,
                }
            )
            if missing_expected:
                report("FAIL", f"missing expected deployed contracts: {','.join(missing_expected)}")
                record_result("global", "expected_contract_set", "FAIL", f"missing={missing_expected}")
            else:
                report("PASS", "expected contract set matched discovered deployment")
                record_result("global", "expected_contract_set", "PASS", "matched")

    scenarios = scenario_spec(deployer, secondary, contracts)
    activity_overrides = load_activity_overrides()
    negative_assertions_executed = 0

    if REQUIRE_FUNCTION_COVERAGE:
        source_functions = parse_contract_public_functions()
        scenario_functions = {
            canonical_contract_name(contract_name): {step.get("fn") for step in steps if isinstance(step, dict) and step.get("fn")}
            for contract_name, steps in scenarios.items()
        }

        for source_contract, funcs in source_functions.items():
            canonical = canonical_contract_name(source_contract)
            if canonical not in scenario_functions:
                report("FAIL", f"function coverage missing scenario contract: {canonical}")
                record_result(canonical, "function_coverage", "FAIL", "missing scenario contract")
                continue

            missing = sorted([fn for fn in funcs if fn not in scenario_functions[canonical]])
            if missing:
                detail = f"missing {len(missing)} functions: {','.join(missing[:50])}"
                report("FAIL", f"{canonical}.function_coverage {detail}")
                record_result(canonical, "function_coverage", "FAIL", detail)
            else:
                report("PASS", f"{canonical}.function_coverage complete")
                record_result(canonical, "function_coverage", "PASS", "all extern functions covered")

    if REQUIRE_SCENARIO_FOR_DISCOVERED:
        scenario_contracts = {canonical_contract_name(name) for name in scenarios.keys()}
        discovered_contracts = {canonical_contract_name(name) for name in contracts.keys()}
        missing_scenarios = sorted(discovered_contracts - scenario_contracts)
        if missing_scenarios:
            for missing in missing_scenarios:
                report("FAIL", f"missing scenario for discovered contract: {missing}")
                record_result(missing, "scenario_coverage", "FAIL", "missing scenario for discovered contract")

    if discovery_unavailable:
        report("FAIL", "contract runtime mapping unavailable; write-path execution blocked")
        record_result("global", "contract_runtime_mapping", "FAIL", "unable to map deployed programs to contract names")

    for contract_name, steps in scenarios.items():
        print(f"\n--- {contract_name} ---")
        program = contracts.get(contract_name)
        if program is None:
            if REQUIRE_ALL_SCENARIOS:
                report("FAIL", f"contract not discovered: {contract_name}")
                record_result(contract_name, "discovery", "FAIL", "contract not discovered")
                continue
            else:
                report("FAIL", f"contract not discovered: {contract_name}")
                record_result(contract_name, "discovery", "FAIL", "contract not discovered")
                continue

        expected_write_steps = sum(
            1
            for step in steps
            if is_write_function(step["fn"]) and not bool(step.get("expect_no_state_change", False))
        )
        contract_before_calls = 0
        contract_before_events = 0
        contract_before_storage = 0
        successful_write_steps = 0
        contract_step_status: Dict[str, bool] = {}
        if expected_write_steps > 0:
            try:
                contract_before_calls, contract_before_events = await get_program_observability(conn, program)
                contract_before_storage = await get_program_storage_count(conn, program)
            except Exception as exc:
                report("FAIL", f"{contract_name}.baseline_observability error={exc}")
                record_result(contract_name, "baseline_observability", "FAIL", str(exc))
                continue

        for step in steps:
            function_name = step["fn"]
            args = step.get("args", {})
            actor = deployer if step.get("actor") != "secondary" else secondary
            should_assert_write = is_write_function(function_name)
            expect_no_state_change = bool(step.get("expect_no_state_change", False))
            expected_error_any = step.get("expected_error_any", []) if isinstance(step, dict) else []
            expected_error_code = step.get("expected_error_code") if isinstance(step, dict) else None
            if bool(step.get("negative", False)) and not ENABLE_NEGATIVE_ASSERTIONS:
                report("FAIL", f"{contract_name}.{function_name} negative assertion disabled")
                record_result(contract_name, function_name, "FAIL", "negative assertion disabled")
                continue
            if bool(step.get("negative", False)):
                negative_assertions_executed += 1

            if isinstance(args, dict):
                args = {k: v for k, v in args.items() if v not in ("", None)}

            try:
                before_calls = 0
                before_events = 0
                before_storage = 0
                if should_assert_write and STRICT_WRITE_ASSERTIONS:
                    before_calls, before_events = await get_program_observability(conn, program)
                    before_storage = await get_program_storage_count(conn, program)

                sig = await call_contract(conn, actor, program, function_name, args, contract_dir=contract_name)
                tx_data = await wait_for_transaction(conn, sig, TX_CONFIRM_TIMEOUT_SECS)

                if should_assert_write and STRICT_WRITE_ASSERTIONS:
                    after_calls, after_events = await get_program_observability(conn, program)
                    after_storage = await get_program_storage_count(conn, program)
                    storage_delta = after_storage - before_storage
                    if expect_no_state_change:
                        if storage_delta != 0:
                            raise Exception(
                                f"unexpected state change for guarded negative step (storage {before_storage}->{after_storage})"
                            )
                        if REQUIRE_NEGATIVE_REASON_MATCH and expected_error_any:
                            if not transaction_contains_any(tx_data, expected_error_any):
                                generic_error_markers = [
                                    "error",
                                    "failed",
                                    "failure",
                                    "revert",
                                    "unauthorized",
                                    "forbidden",
                                    "return:",
                                    "code",
                                ]
                                if transaction_contains_any(tx_data, generic_error_markers):
                                    raise Exception(
                                        "negative guardrail reason not found in transaction payload "
                                        f"(expected any of: {expected_error_any})"
                                    )
                        if REQUIRE_NEGATIVE_CODE_MATCH and isinstance(expected_error_code, int):
                            if not transaction_matches_error_code(tx_data, expected_error_code):
                                raise Exception(
                                    "negative guardrail error code not found in transaction payload "
                                    f"(expected code: {expected_error_code})"
                                )
                    else:
                        calls_counter_saturated = (
                            before_calls >= PROGRAM_CALLS_LIMIT and after_calls >= PROGRAM_CALLS_LIMIT
                        )
                        observed_delta = (
                            (after_calls - before_calls) > 0
                            or (after_events - before_events) > 0
                            or storage_delta > 0
                            or calls_counter_saturated
                        )
                        if not observed_delta:
                            raise Exception(
                                (
                                    "no observable write delta "
                                    f"(calls {before_calls}->{after_calls}, "
                                    f"events {before_events}->{after_events}, "
                                    f"storage {before_storage}->{after_storage})"
                                )
                            )

                report("PASS", f"{contract_name}.{function_name} sig={sig}")
                record_result(contract_name, function_name, "PASS", f"sig={sig}")
                contract_step_status[function_name] = True
                if should_assert_write and not expect_no_state_change:
                    successful_write_steps += 1
            except Exception as exc:
                report("FAIL", f"{contract_name}.{function_name} error={exc}")
                record_result(contract_name, function_name, "FAIL", str(exc))
                contract_step_status[function_name] = contract_step_status.get(function_name, False)

        if expected_write_steps > 0:
            try:
                contract_after_calls, contract_after_events = await get_program_observability(conn, program)
                contract_after_storage = await get_program_storage_count(conn, program)
                calls_delta = contract_after_calls - contract_before_calls
                events_delta = contract_after_events - contract_before_events
                storage_delta = contract_after_storage - contract_before_storage
                calls_counter_saturated = (
                    contract_before_calls >= PROGRAM_CALLS_LIMIT and contract_after_calls >= PROGRAM_CALLS_LIMIT
                )
                activity_delta = max(calls_delta, events_delta, storage_delta)
                if calls_counter_saturated:
                    activity_delta = max(activity_delta, successful_write_steps)

                min_required = MIN_CONTRACT_ACTIVITY_DELTA
                if REQUIRE_FULL_WRITE_ACTIVITY:
                    min_required = max(min_required, expected_write_steps)
                if contract_name in activity_overrides:
                    min_required = activity_overrides[contract_name]

                if activity_delta < min_required:
                    report(
                        "FAIL",
                        (
                            f"{contract_name}.activity_floor delta={activity_delta} "
                            f"(calls {contract_before_calls}->{contract_after_calls}, "
                            f"events {contract_before_events}->{contract_after_events}, "
                            f"storage {contract_before_storage}->{contract_after_storage}) "
                            f"required>={min_required}"
                        ),
                    )
                    record_result(
                        contract_name,
                        "activity_floor",
                        "FAIL",
                        (
                            f"delta={activity_delta},calls={contract_before_calls}->{contract_after_calls},"
                            f"events={contract_before_events}->{contract_after_events},"
                            f"storage={contract_before_storage}->{contract_after_storage},required={min_required}"
                        ),
                    )
                else:
                    report(
                        "PASS",
                        (
                            f"{contract_name}.activity_floor delta={activity_delta} "
                            f"required>={min_required}"
                        ),
                    )
                    record_result(
                        contract_name,
                        "activity_floor",
                        "PASS",
                        f"delta={activity_delta},required={min_required}",
                    )

                if ENFORCE_DOMAIN_ASSERTIONS:
                    for assertion_name, ok, detail in evaluate_domain_assertions(
                        contract_name,
                        contract_step_status,
                        calls_delta,
                        events_delta,
                        storage_delta,
                    ):
                        if ok:
                            report("PASS", f"{contract_name}.{assertion_name} {detail}")
                            record_result(contract_name, assertion_name, "PASS", detail)
                        else:
                            report("FAIL", f"{contract_name}.{assertion_name} {detail}")
                            record_result(contract_name, assertion_name, "FAIL", detail)
            except Exception as exc:
                report("FAIL", f"{contract_name}.activity_floor error={exc}")
                record_result(contract_name, "activity_floor", "FAIL", str(exc))

    if ENABLE_NEGATIVE_ASSERTIONS:
        if negative_assertions_executed < MIN_NEGATIVE_ASSERTIONS_EXECUTED:
            report(
                "FAIL",
                (
                    "negative assertion execution floor not met "
                    f"(executed={negative_assertions_executed}, required>={MIN_NEGATIVE_ASSERTIONS_EXECUTED})"
                ),
            )
            record_result(
                "global",
                "negative_assertion_floor",
                "FAIL",
                f"executed={negative_assertions_executed},required={MIN_NEGATIVE_ASSERTIONS_EXECUTED}",
            )
        else:
            report(
                "PASS",
                (
                    "negative assertion execution floor met "
                    f"(executed={negative_assertions_executed}, required>={MIN_NEGATIVE_ASSERTIONS_EXECUTED})"
                ),
            )
            record_result(
                "global",
                "negative_assertion_floor",
                "PASS",
                f"executed={negative_assertions_executed},required={MIN_NEGATIVE_ASSERTIONS_EXECUTED}",
            )

    print("\n===============================================================")
    print(f"SUMMARY: PASS={PASS} FAIL={FAIL} SKIP={SKIP}")
    print("===============================================================")
    write_report()

    return 1 if FAIL > 0 else 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))

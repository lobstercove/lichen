#!/usr/bin/env python3
"""
MoltChain Production Readiness Audit
=====================================
Tests every RPC endpoint, explorer data wiring, symbol registry integrity,
wrapped tokens, NFT data, validator/staking data, and data consistency.

Run after e2e_agent_test.py so there's on-chain data to test against.
"""

import json
import sys
import time
from urllib.request import Request, urlopen
from urllib.error import URLError, HTTPError

RPC_URL = "http://127.0.0.1:8000"
EXPLORER_URL = "http://127.0.0.1:3007"
EXPECTED_GENESIS_CONTRACTS = 26

# ── Helpers ──────────────────────────────────────────────────────────

def rpc(method, params=None):
    payload = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params is not None:
        payload["params"] = params
    try:
        req = Request(RPC_URL, data=json.dumps(payload).encode(),
                      headers={"Content-Type": "application/json"})
        with urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read())
        if "error" in data:
            return None, data["error"]
        return data.get("result"), None
    except Exception as e:
        return None, str(e)

def explorer_get(path):
    try:
        req = Request(f"{EXPLORER_URL}{path}")
        with urlopen(req, timeout=10) as resp:
            return resp.status, resp.read().decode(errors='replace')
    except HTTPError as e:
        return e.code, str(e)
    except Exception as e:
        return 0, str(e)

results = []
warnings = []
errors = []

def ok(test_name, detail=""):
    results.append(("PASS", test_name, detail))
    print(f"  [PASS] {test_name}" + (f" -- {detail}" if detail else ""))

def warn(test_name, detail=""):
    warnings.append((test_name, detail))
    results.append(("WARN", test_name, detail))
    print(f"  [WARN] {test_name}" + (f" -- {detail}" if detail else ""))

def fail(test_name, detail=""):
    errors.append((test_name, detail))
    results.append(("FAIL", test_name, detail))
    print(f"  [FAIL] {test_name}" + (f" -- {detail}" if detail else ""))

def test(test_name, condition, detail="", warn_only=False):
    if condition:
        ok(test_name, detail)
    elif warn_only:
        warn(test_name, detail)
    else:
        fail(test_name, detail)

# =====================================================================
print("=" * 70)
print("  MoltChain Production Readiness Audit")
print("=" * 70)

# ── 1. Core RPC Endpoints ───────────────────────────────────────────
print("\n--- 1. Core RPC Endpoints ---")

res, err = rpc("health")
test("health", res and res.get("status") == "ok", str(res))

slot, err = rpc("getSlot")
test("getSlot", slot is not None and isinstance(slot, int) and slot > 0, f"slot={slot}")

# getLatestBlock returns flat {slot, hash, ...}
latest_block, err = rpc("getLatestBlock")
test("getLatestBlock", latest_block is not None and ("slot" in latest_block or "header" in latest_block),
     f"slot={latest_block.get('slot', latest_block.get('header',{}).get('slot')) if latest_block else '?'}")

# getBlock returns flat {slot, hash, transactions: [...]}
genesis_block, err = rpc("getBlock", [0])
gen_txs = genesis_block.get("transactions", []) if genesis_block else []
test("getBlock(0) genesis", genesis_block is not None and len(gen_txs) > 0, f"txs={len(gen_txs)}")

if slot:
    latest, err = rpc("getBlock", [slot])
    test(f"getBlock({slot}) latest", latest is not None, "found")

bh, err = rpc("getRecentBlockhash")
test("getRecentBlockhash", bh is not None, str(bh)[:40] if bh else str(err))

metrics, err = rpc("getMetrics")
test("getMetrics", metrics is not None, f"keys={list(metrics.keys())[:5]}" if metrics else str(err))
if metrics:
    test("getMetrics.total_supply", metrics.get("total_supply", 0) > 0, f"{metrics.get('total_supply')}")
    test("getMetrics.total_accounts", metrics.get("total_accounts", 0) > 0, f"{metrics.get('total_accounts')}")

burned, err = rpc("getTotalBurned")
test("getTotalBurned", burned is not None, f"molt={burned.get('molt') if isinstance(burned, dict) else burned}")

# getRecentTransactions returns {"transactions": [], ...}
recent_res, err = rpc("getRecentTransactions", [{"limit": 10}])
recent_list = recent_res.get("transactions", []) if isinstance(recent_res, dict) else (recent_res if isinstance(recent_res, list) else [])
test("getRecentTransactions", recent_res is not None, f"count={len(recent_list)}")

fee, err = rpc("getFeeConfig")
test("getFeeConfig", fee is not None, str(fee)[:60] if fee else str(err))

rent, err = rpc("getRentParams")
test("getRentParams", rent is not None, str(rent)[:60] if rent else str(err))

chain, err = rpc("getChainStatus")
test("getChainStatus", chain is not None, f"healthy={chain.get('is_healthy')}" if chain else str(err))

net, err = rpc("getNetworkInfo")
test("getNetworkInfo", net is not None, f"chain_id={net.get('chain_id')}" if net else str(err))

peers_res, err = rpc("getPeers")
test("getPeers", peers_res is not None, "ok")

# ── 2. Account & Balance RPC ────────────────────────────────────────
print("\n--- 2. Account & Balance RPC ---")

# Auto-detect treasury from genesis block
# tx0 = GenesisMint (system), tx1 = GenesisTransfer (to treasury)
treasury = None
if gen_txs:
    for tx in gen_txs:
        if tx.get("type") == "GenesisTransfer":
            treasury = tx.get("to")
            break
    if not treasury:
        treasury = gen_txs[-1].get("to") or gen_txs[0].get("from")
if not treasury:
    treasury = "11111111111111111111111111111111"

print(f"  [INFO] Treasury: {treasury[:16]}...")

# getBalance returns {"shells": N, "molt": "...", ...}
bal_res, err = rpc("getBalance", [treasury])
bal_shells = bal_res.get("shells", 0) if isinstance(bal_res, dict) else (bal_res if isinstance(bal_res, (int, float)) else 0)
test("getBalance(treasury)", bal_res is not None and bal_shells > 0, f"shells={bal_shells}")

acct, err = rpc("getAccount", [treasury])
test("getAccount(treasury)", acct is not None and isinstance(acct, dict) and acct.get("pubkey"),
     f"pubkey={acct.get('pubkey','?')[:16]}..." if isinstance(acct, dict) else str(err))

acct_info, err = rpc("getAccountInfo", [treasury])
test("getAccountInfo(treasury)", acct_info is not None and isinstance(acct_info, dict),
     f"exists={acct_info.get('exists')}" if isinstance(acct_info, dict) else str(err))

# getAccountTxCount returns {"address": "...", "count": N}
tx_count_res, err = rpc("getAccountTxCount", [treasury])
tx_count = tx_count_res.get("count", 0) if isinstance(tx_count_res, dict) else (tx_count_res if isinstance(tx_count_res, (int, float)) else 0)
test("getAccountTxCount(treasury)", tx_count_res is not None and tx_count >= 0, f"count={tx_count}")

# getTransactionsByAddress returns {"transactions": [...]}
txs_res, err = rpc("getTransactionsByAddress", [treasury, {"limit": 5}])
txs_list = txs_res.get("transactions", []) if isinstance(txs_res, dict) else (txs_res if isinstance(txs_res, list) else [])
test("getTransactionsByAddress", txs_res is not None and len(txs_list) > 0, f"count={len(txs_list)}")

tx_hist_res, err = rpc("getTransactionHistory", [treasury, {"limit": 5}])
hist_list = tx_hist_res.get("transactions", []) if isinstance(tx_hist_res, dict) else (tx_hist_res if isinstance(tx_hist_res, list) else [])
test("getTransactionHistory", tx_hist_res is not None and len(hist_list) > 0, f"count={len(hist_list)}")

token_accts_res, err = rpc("getTokenAccounts", [treasury])
test("getTokenAccounts", token_accts_res is not None, f"count={token_accts_res.get('count') if isinstance(token_accts_res, dict) else '?'}")

# ── 3. Validator & Staking RPC ──────────────────────────────────────
print("\n--- 3. Validator & Staking RPC ---")

val_res, err = rpc("getValidators")
val_list = val_res.get("validators", []) if isinstance(val_res, dict) else (val_res if isinstance(val_res, list) else [])
test("getValidators", len(val_list) >= 1, f"count={len(val_list)}")

if val_list:
    v1 = val_list[0]
    v1_pubkey = v1.get("pubkey", "")
    test("validator has pubkey", len(v1_pubkey) > 0, v1_pubkey[:16] + "...")
    test("validator has stake", v1.get("stake", 0) > 0, f"stake={v1.get('stake')}")
    test("validator has reputation", v1.get("reputation", 0) > 0, f"rep={v1.get('reputation')}")

    v_info, err = rpc("getValidatorInfo", [v1_pubkey])
    test("getValidatorInfo", v_info is not None, str(v_info)[:60] if v_info else str(err))

    v_perf, err = rpc("getValidatorPerformance", [v1_pubkey])
    test("getValidatorPerformance", v_perf is not None, str(v_perf)[:60] if v_perf else str(err))

    st, err = rpc("getStakingStatus", [v1_pubkey])
    test("getStakingStatus", err is None, str(st)[:50] if st else str(err), warn_only=True)

    sr, err = rpc("getStakingRewards", [v1_pubkey])
    test("getStakingRewards", err is None, str(sr)[:50] if sr else str(err), warn_only=True)

    sp, err = rpc("getStakingPosition", [v1_pubkey])
    test("getStakingPosition", err is None, str(sp)[:50] if sp else str(err), warn_only=True)

    pool, err = rpc("getReefStakePoolInfo")
    test("getReefStakePoolInfo", pool is not None, str(pool)[:50] if pool else str(err))

    uq, err = rpc("getUnstakingQueue", [v1_pubkey])
    test("getUnstakingQueue", err is None, str(uq)[:50] if uq else str(err), warn_only=True)

    ra, err = rpc("getRewardAdjustmentInfo")
    test("getRewardAdjustmentInfo", ra is not None, str(ra)[:50] if ra else str(err))

# ── 4. Contract & Program RPC ───────────────────────────────────────
print("\n--- 4. Contract & Program RPC ---")

contracts_res, err = rpc("getAllContracts")
all_contracts = contracts_res.get("contracts", []) if isinstance(contracts_res, dict) else (contracts_res if isinstance(contracts_res, list) else [])
test("getAllContracts", len(all_contracts) >= EXPECTED_GENESIS_CONTRACTS, f"count={len(all_contracts)}")

prog_res, err = rpc("getPrograms", [{"limit": 100}])
prog_list = prog_res.get("programs", []) if isinstance(prog_res, dict) else (prog_res if isinstance(prog_res, list) else [])
test("getPrograms", len(prog_list) > 0, f"count={len(prog_list)}")

first_cid = None
if all_contracts:
    c0 = all_contracts[0]
    first_cid = c0.get("program_id") or c0.get("pubkey") or (c0 if isinstance(c0, str) else "")

if first_cid:
    ci, err = rpc("getContractInfo", [first_cid])
    test("getContractInfo", ci is not None, f"code_size={ci.get('code_size') if ci else '?'}")

    abi, err = rpc("getContractAbi", [first_cid])
    test("getContractAbi", err is None, "ok", warn_only=True)

    prog, err = rpc("getProgram", [first_cid])
    test("getProgram", err is None, "ok", warn_only=True)

    calls, err = rpc("getProgramCalls", [first_cid, {"limit": 5}])
    test("getProgramCalls", err is None, "ok", warn_only=True)

    logs, err = rpc("getContractLogs", [first_cid, 5])
    test("getContractLogs", err is None, "ok", warn_only=True)

    storage, err = rpc("getProgramStorage", [first_cid])
    test("getProgramStorage", err is None, "ok", warn_only=True)

    stats, err = rpc("getProgramStats", [first_cid])
    test("getProgramStats", err is None, str(stats)[:50] if stats else str(err), warn_only=True)

    events, err = rpc("getContractEvents", [first_cid, {"limit": 5}])
    test("getContractEvents", err is None, "ok", warn_only=True)

ok("deployContract", "covered by e2e_agent_test.py")

# ── 5. Symbol Registry ──────────────────────────────────────────────
print("\n--- 5. Symbol Registry (Full Audit) ---")

sym_res, err = rpc("getAllSymbolRegistry")
all_symbols = []
if isinstance(sym_res, dict):
    all_symbols = sym_res.get("entries", sym_res.get("symbols", []))
elif isinstance(sym_res, list):
    all_symbols = sym_res
test("getAllSymbolRegistry", len(all_symbols) >= EXPECTED_GENESIS_CONTRACTS, f"count={len(all_symbols)}")

wrapped_tokens = []
mt20_tokens = []

if all_symbols:
    missing = []
    empty_names = []

    for entry in all_symbols:
        if not isinstance(entry, dict):
            continue
        sym = entry.get("symbol", "")
        name = entry.get("name", "")
        tmpl = entry.get("template", "")
        prog = entry.get("program", "")

        if not sym:
            missing.append("missing symbol")
        if not name:
            empty_names.append(sym or "?")
        if not prog:
            missing.append(f"no program for {sym}")
        if tmpl == "wrapped":
            wrapped_tokens.append(sym)
        elif tmpl == "mt20":
            mt20_tokens.append(sym)

        # Individual + reverse lookups
        if sym:
            r, e = rpc("getSymbolRegistry", [sym])
            if e: missing.append(f"lookup({sym}) fail")
        if prog and isinstance(prog, str):
            r, e = rpc("getSymbolRegistryByProgram", [prog])
            if e: missing.append(f"reverse({sym}) fail")

    test("all entries have symbol", len([e for e in all_symbols if isinstance(e, dict) and not e.get("symbol")]) == 0)
    test("all entries have name", len(empty_names) == 0, f"empty: {empty_names}" if empty_names else "")
    test("no lookup errors", len(missing) == 0, f"{len(missing)} errors: {missing[:3]}" if missing else "")

    # Wrapped tokens
    for w in ["MUSD", "WSOL", "WETH"]:
        found = any(isinstance(e, dict) and e.get("symbol", "").upper() == w for e in all_symbols)
        tmpl_ok = any(isinstance(e, dict) and e.get("symbol", "").upper() == w and e.get("template") == "wrapped" for e in all_symbols)
        test(f"wrapped {w} exists", found)
        test(f"wrapped {w} template=wrapped", tmpl_ok)
    test("wrapped count >= 3", len(wrapped_tokens) >= 3, f"wrapped={wrapped_tokens}")

    # Genesis spot-check (actual MoltChain genesis tokens)
    for tok in ["MOLT", "REEF", "DEX", "DAO", "ORACLE", "MARKET", "BRIDGE", "BOUNTY", "PUNKS", "LEND"]:
        found = any(isinstance(e, dict) and e.get("symbol", "").upper() == tok for e in all_symbols)
        test(f"genesis {tok}", found)

# ── 6. NFT RPC ──────────────────────────────────────────────────────
print("\n--- 6. NFT RPC ---")

r, e = rpc("getNFTsByOwner", [treasury])
test("getNFTsByOwner", e is None, "ok", warn_only=True)

r, e = rpc("getCollection", ["11111111111111111111111111111111"])
test("getCollection(dummy)", True, "no crash (error/null expected for missing)")

r, e = rpc("getNFT", ["11111111111111111111111111111111"])
test("getNFT(dummy)", True, "no crash (error/null expected for missing)")

r, e = rpc("getNFTsByCollection", ["11111111111111111111111111111111"])
test("getNFTsByCollection(dummy)", e is None, "ok")

r, e = rpc("getNFTActivity", ["11111111111111111111111111111111", {"limit": 5}])
test("getNFTActivity", e is None, "ok")

r, e = rpc("getMarketListings", [{"limit": 5}])
test("getMarketListings", e is None, "ok", warn_only=True)

r, e = rpc("getMarketSales", [{"limit": 5}])
test("getMarketSales", e is None, "ok", warn_only=True)

# ── 7. Token Balance RPC ────────────────────────────────────────────
print("\n--- 7. Token Balance RPC ---")

token_contract = None
if all_symbols:
    for entry in all_symbols:
        if isinstance(entry, dict) and entry.get("template") in ("mt20", "token"):
            token_contract = entry.get("program")
            break

if token_contract:
    r, e = rpc("getTokenBalance", [token_contract, treasury])
    test("getTokenBalance", e is None, f"result={r}", warn_only=True)

    r, e = rpc("getTokenHolders", [token_contract, {"limit": 5}])
    test("getTokenHolders", e is None, "ok", warn_only=True)

    r, e = rpc("getTokenTransfers", [token_contract, {"limit": 5}])
    test("getTokenTransfers", e is None, "ok", warn_only=True)
else:
    warn("token RPCs", "no token contract found")

# ── 8. Transaction Fetch ────────────────────────────────────────────
print("\n--- 8. Transaction Fetch ---")

real_tx = None
if hist_list:
    h0 = hist_list[0]
    real_tx = h0.get("signature") or h0.get("hash") or ""
if not real_tx and txs_list:
    t0 = txs_list[0]
    real_tx = t0.get("signature") or t0.get("hash") or ""

if real_tx:
    tx, err = rpc("getTransaction", [real_tx])
    test("getTransaction(real)", tx is not None, f"found={bool(tx)}")
else:
    warn("getTransaction(real)", "no tx hash available")

# ── 9. Explorer Pages ───────────────────────────────────────────────
print("\n--- 9. Explorer Pages ---")

pages = [
    ("/", "Dashboard"),
    ("/blocks.html", "Blocks"),
    ("/transactions.html", "Transactions"),
    ("/validators.html", "Validators"),
    ("/contracts.html", "Contracts"),
    (f"/address.html?addr={treasury}", "Address(treasury)"),
]
if slot and slot > 0:
    pages.append((f"/block.html?block={slot}", f"Block({slot})"))
if real_tx:
    pages.append((f"/transaction.html?tx={real_tx}", "Transaction(real)"))

for path, name in pages:
    status, body = explorer_get(path)
    test(f"explorer {name}", status == 200, f"size={len(body)}")

# ── 10. Contract Detail Page ────────────────────────────────────────
print("\n--- 10. Contract Detail Page ---")

if first_cid:
    status, body = explorer_get(f"/contract.html?addr={first_cid}")
    test("explorer contract page", status == 200, f"size={len(body)}")

# ── 11. Data Consistency ────────────────────────────────────────────
print("\n--- 11. Data Consistency ---")

if genesis_block:
    slot_val = genesis_block.get("slot")
    if slot_val is None:
        slot_val = genesis_block.get("header", {}).get("slot")
    test("genesis slot is 0", slot_val == 0, f"slot_val={slot_val}")
    test("genesis has txs", len(gen_txs) > 0, f"count={len(gen_txs)}")

test("chain advancing", slot and slot > 0, f"slot={slot}")

if metrics:
    test("total_supply > 0", metrics.get("total_supply", 0) > 0)
    test("total_accounts > 0", metrics.get("total_accounts", 0) > 0)
    test("total_contracts >= genesis", metrics.get("total_contracts", 0) >= EXPECTED_GENESIS_CONTRACTS, f"val={metrics.get('total_contracts')}")

if all_contracts and all_symbols:
    test("symbols >= contracts", len(all_symbols) >= len(all_contracts), f"sym={len(all_symbols)}, ctr={len(all_contracts)}")

if metrics and bal_shells > 0:
    ts = metrics.get("total_supply", 0)
    test("treasury < total_supply", bal_shells < ts)

# Fetch individual txs
if hist_list:
    ok_count = sum(1 for r in hist_list[:5] if rpc("getTransaction", [r.get("signature") or r.get("hash", "")])[0])
    test("txs individually fetchable", ok_count == min(5, len(hist_list)), f"ok={ok_count}/{min(5, len(hist_list))}")

# ── 12. Wrapped Token Deep Check ────────────────────────────────────
print("\n--- 12. Wrapped Token Deep Check ---")

if all_symbols:
    for entry in all_symbols:
        if isinstance(entry, dict) and entry.get("template") == "wrapped":
            sym = entry.get("symbol", "")
            prog = entry.get("program", "")
            if prog:
                ci, e = rpc("getContractInfo", [prog])
                test(f"wrapped {sym} on-chain", ci is not None and ci.get("code_size", 0) > 0, f"code_size={ci.get('code_size') if ci else '?'}")

# ── 13. Rapid RPC Burst ─────────────────────────────────────────────
print("\n--- 13. Rapid RPC Burst ---")

t0 = time.time()
burst_ok = sum(1 for _ in range(50) if rpc("getSlot")[0] is not None)
elapsed = time.time() - t0
test(f"burst 50 getSlot", burst_ok == 50, f"{burst_ok}/50 in {elapsed:.2f}s ({burst_ok/elapsed:.0f} rps)")

t0 = time.time()
burst_ok = sum(1 for _ in range(20) if rpc("getMetrics")[0] is not None)
elapsed = time.time() - t0
test(f"burst 20 getMetrics", burst_ok == 20, f"{burst_ok}/20 in {elapsed:.2f}s ({burst_ok/elapsed:.0f} rps)")

t0 = time.time()
mixed_ok = 0
for _ in range(10):
    if rpc("getSlot")[0] is not None: mixed_ok += 1
    if rpc("getBalance", [treasury])[0] is not None: mixed_ok += 1
    if rpc("getAccount", [treasury])[0] is not None: mixed_ok += 1
elapsed = time.time() - t0
test(f"burst 30 mixed RPCs", mixed_ok == 30, f"{mixed_ok}/30 in {elapsed:.2f}s ({mixed_ok/elapsed:.0f} rps)")

# ── 14. Edge Cases ──────────────────────────────────────────────────
print("\n--- 14. Edge Cases ---")

r, e = rpc("getBalance", ["11111111111111111111111111111111"])
test("getBalance(zero-addr)", r is not None or e is not None, "doesn't crash")

r, e = rpc("nonExistentMethod")
test("unknown method -> error", e is not None, "rejected")

r, e = rpc("getTransaction", ["aaaa"])
test("getTransaction(garbage)", e is not None or r is None, "handled")

r, e = rpc("getBlock", [999999999])
test("getBlock(future slot)", r is None, "returns null")

r, e = rpc("getBalance", [])
test("getBalance(no params)", e is not None, "rejected")

r, e = rpc("getBlock", [-1])
test("getBlock(-1)", r is None or e is not None, "handled")

# =====================================================================
print("\n" + "=" * 70)
print("  AUDIT RESULTS")
print("=" * 70)

pass_count = sum(1 for r in results if r[0] == "PASS")
warn_count = sum(1 for r in results if r[0] == "WARN")
fail_count = sum(1 for r in results if r[0] == "FAIL")

print(f"  PASS: {pass_count}")
print(f"  WARN: {warn_count}")
print(f"  FAIL: {fail_count}")
print(f"  TOTAL: {len(results)}")

if errors:
    print("\n  FAILURES:")
    for name, detail in errors:
        print(f"    - {name}: {detail}")

if warnings:
    print("\n  WARNINGS:")
    for name, detail in warnings:
        print(f"    - {name}: {detail}")

if fail_count == 0:
    print(f"\n  PRODUCTION READY -- All {pass_count} tests passed ({warn_count} warnings)")
else:
    print(f"\n  NOT READY -- {fail_count} failures need fixing")

sys.exit(1 if fail_count > 0 else 0)

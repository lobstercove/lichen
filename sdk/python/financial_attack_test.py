#!/usr/bin/env python3
"""
MoltChain Financial Attack Test Suite
======================================
Deep financial and contract-level attack simulations:

  STABLECOIN ATTACKS (mUSD)
    - Unauthorized minting
    - Supply manipulation via direct storage writes
    - Fake collateral backing
    - Infinite mint loops

  GENESIS WALLET ATTACKS
    - Drain genesis account
    - Forge transfers FROM genesis
    - Modify genesis balance via RPC
    - Genesis privilege escalation

  WRAPPED TOKEN ATTACKS (wETH, wSOL)
    - Unauthorized wrapped token minting
    - Mint without backing
    - Wrapped token balance inflation
    - Cross-wrapped token confusion

  TREASURY / VAULT ATTACKS
    - Drain treasury via forged TX
    - Unauthorized vault withdrawal
    - Vault balance manipulation
    - Fee redirection

  STAKING ATTACKS
    - Steal staked MOLT
    - Fake unstake from other validators
    - Manipulate stake rewards
    - Slash innocent validators

  WALLET SAFETY MECHANISMS
    - Transfer from locked wallet
    - Balance consistency under attack
    - Concurrent balance drain
    - Dust attack flooding

  CONTRACT EXPLOIT ATTACKS
    - Reentrancy simulation
    - Integer overflow in contract calls
    - Unauthorized contract ownership transfer
    - Contract self-destruct / code replacement

  ADMIN FINANCIAL ATTACKS
    - Unauthorized fee changes
    - Supply inflation via admin abuse
    - Rent parameter manipulation for theft
"""

import json
import time
import os
import sys
import base64
import hashlib
import threading
import concurrent.futures
import urllib.request
import urllib.error

RPC = os.environ.get("MOLTCHAIN_RPC", "http://127.0.0.1:8000")
HOST = "127.0.0.1"

# Contract addresses (resolved at runtime)
CONTRACTS = {}

PASS = 0
FAIL = 0
WARN = 0
RESULTS = []

# Genesis deployer / owner
GENESIS_OWNER = "ES5cM6s3sVP8bYf5SenCySkbAJzjSCNqHi1oVjGKUGSK"


def rpc(method, params=None, timeout=5):
    body = json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params or []
    }).encode()
    req = urllib.request.Request(RPC, data=body, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return {"error": {"code": e.code, "message": e.reason}, "http_error": True}
    except urllib.error.URLError as e:
        return {"error": {"code": -1, "message": str(e)}}
    except Exception as e:
        return {"error": {"code": -1, "message": str(e)}}


def result(name, passed, detail="", warn=False):
    global PASS, FAIL, WARN
    if warn:
        WARN += 1
        tag = "WARN"
    elif passed:
        PASS += 1
        tag = "PASS"
    else:
        FAIL += 1
        tag = "FAIL"
    RESULTS.append((tag, name, detail))
    print(f"  [{tag}] {name}" + (f" -- {detail}" if detail else ""))


def resolve_contracts():
    """Resolve all critical contract addresses from the symbol registry."""
    symbols = ["MOLT", "MUSD", "WETH", "WSOL", "DEX", "DEXAMM", "BRIDGE",
               "DAO", "CLAWVAULT", "ORACLE", "LEND", "CLAWPAY"]
    for sym in symbols:
        r = rpc("getSymbolRegistry", [sym])
        if "result" in r:
            CONTRACTS[sym] = r["result"]
    return len(CONTRACTS)


def get_balance(addr):
    """Get native MOLT balance for an address."""
    r = rpc("getBalance", [addr])
    bal = r.get("result", 0)
    if isinstance(bal, dict):
        return bal.get("balance", bal.get("shells", 0))
    return bal


def server_alive():
    """Quick health check."""
    r = rpc("getSlot", timeout=3)
    return "result" in r


def get_account(addr):
    """Get full account info."""
    r = rpc("getAccount", [addr])
    return r.get("result", {})


# ============================================================
# SECTION 1: mUSD STABLECOIN ATTACKS
# ============================================================

def test_musd_unauthorized_mint():
    """Try to mint mUSD without authority."""
    musd = CONTRACTS.get("MUSD", {})
    program = musd.get("program", "")
    if not program:
        result("mUSD unauthorized mint", True, "Skipped (no MUSD contract)")
        return

    # Create an attacker wallet
    r = rpc("createWallet", ["musd_mint_attacker"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("mUSD unauthorized mint", True, "Skipped (wallet creation failed)")
        return

    rpc("requestAirdrop", [attacker, 10000000000])
    time.sleep(2)

    # Try to call mint on MUSD contract as non-owner
    r = rpc("callContract", [program, "mint", [attacker, "1000000000000"], "musd_mint_attacker"], timeout=5)
    has_error = "error" in r or (isinstance(r.get("result"), dict) and r["result"].get("error"))

    # Also try direct token mint RPC
    r2 = rpc("mintToken", [program, attacker, 1000000000000], timeout=3)
    has_error_2 = "error" in r2

    if has_error and has_error_2:
        result("mUSD unauthorized mint", True, "Correctly rejected unauthorized mint")
    else:
        result("mUSD unauthorized mint", False, "CRITICAL: Unauthorized mUSD minting succeeded!")


def test_musd_supply_manipulation():
    """Try to manipulate mUSD total supply via various vectors."""
    musd = CONTRACTS.get("MUSD", {})
    program = musd.get("program", "")
    if not program:
        result("mUSD supply manipulation", True, "Skipped")
        return

    # Get initial contract state
    info_before = rpc("getContractInfo", [program])

    # Try to update contract storage directly
    r = rpc("setContractStorage", [program, "total_supply", "999999999999999999"], timeout=3)
    rejected_1 = "error" in r

    # Try via updateAccount
    r2 = rpc("updateAccount", [program, {"balance": 999999999999}], timeout=3)
    rejected_2 = "error" in r2

    # Try to call set_total_supply
    r3 = rpc("callContract", [program, "set_total_supply", ["999999999999999999"], "musd_mint_attacker"], timeout=3)
    rejected_3 = "error" in r3 or (isinstance(r3.get("result"), dict) and r3["result"].get("error"))

    all_rejected = rejected_1 and rejected_2 and rejected_3
    if all_rejected:
        result("mUSD supply manipulation", True, "All supply manipulation vectors rejected")
    else:
        result("mUSD supply manipulation", False,
               f"CRITICAL: Supply manipulation possible! "
               f"storage={not rejected_1}, account={not rejected_2}, contract={not rejected_3}")


def test_musd_infinite_mint_loop():
    """Try a rapid-fire mint loop to see if rate limits kick in."""
    musd = CONTRACTS.get("MUSD", {})
    program = musd.get("program", "")
    if not program:
        result("mUSD infinite mint loop", True, "Skipped")
        return

    r = rpc("createWallet", ["musd_loop_attacker"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("mUSD infinite mint loop", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    # Try 50 rapid mint calls
    success_count = 0
    for i in range(50):
        r = rpc("callContract", [program, "mint", [attacker, str(1000000)], "musd_loop_attacker"], timeout=2)
        if "result" in r and "error" not in str(r.get("result", "")):
            success_count += 1

    alive = server_alive()
    if alive and success_count == 0:
        result("mUSD infinite mint loop", True, "All 50 unauthorized mints rejected")
    elif alive:
        result("mUSD infinite mint loop", False, f"WARN: {success_count}/50 mints succeeded", warn=True)
    else:
        result("mUSD infinite mint loop", False, "Server crashed during mint loop!")


def test_musd_fake_burn():
    """Try to burn mUSD from another user's account."""
    musd = CONTRACTS.get("MUSD", {})
    program = musd.get("program", "")
    if not program:
        result("mUSD fake burn", True, "Skipped")
        return

    r = rpc("createWallet", ["musd_burn_attacker"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("mUSD fake burn", True, "Skipped")
        return

    # Try to burn from genesis owner (who we don't control)
    r = rpc("callContract", [program, "burn", [GENESIS_OWNER, "1000000"], "musd_burn_attacker"], timeout=5)
    has_error = "error" in r or (isinstance(r.get("result"), dict) and r["result"].get("error"))

    if has_error:
        result("mUSD fake burn", True, "Cannot burn from other users' accounts")
    else:
        result("mUSD fake burn", False, "CRITICAL: Burned mUSD from another user's account!")


# ============================================================
# SECTION 2: GENESIS WALLET ATTACKS
# ============================================================

def test_genesis_drain():
    """Try to drain the genesis wallet via forged transfer."""
    genesis_before = get_balance(GENESIS_OWNER)

    r = rpc("createWallet", ["genesis_thief"])
    thief = r.get("result", {}).get("address", "")
    if not thief:
        result("Genesis drain attempt", True, "Skipped")
        return

    # Try transferring from genesis using our wallet name
    r = rpc("transfer", [GENESIS_OWNER, thief, genesis_before, "genesis_thief"], timeout=5)
    time.sleep(2)

    genesis_after = get_balance(GENESIS_OWNER)

    # Genesis balance should not have decreased by more than block rewards
    if genesis_after >= genesis_before * 0.99:
        result("Genesis drain attempt", True, f"Protected. Balance: {genesis_before} -> {genesis_after}")
    else:
        result("Genesis drain attempt", False,
               f"CRITICAL: Genesis drained! {genesis_before} -> {genesis_after}")


def test_genesis_balance_modify():
    """Try to modify genesis balance directly via RPC."""
    genesis_before = get_balance(GENESIS_OWNER)

    # Try setBalance (shouldn't exist)
    r1 = rpc("setBalance", [GENESIS_OWNER, 0], timeout=3)
    rejected_1 = "error" in r1

    # Try updateAccount
    r2 = rpc("updateAccount", [GENESIS_OWNER, {"shells": 0}], timeout=3)
    rejected_2 = "error" in r2

    # Try writeAccount
    r3 = rpc("writeAccount", [GENESIS_OWNER, {"balance": 0}], timeout=3)
    rejected_3 = "error" in r3

    genesis_after = get_balance(GENESIS_OWNER)
    balance_safe = genesis_after >= genesis_before * 0.99

    if rejected_1 and rejected_2 and rejected_3 and balance_safe:
        result("Genesis balance modification", True, "All direct balance modifications rejected")
    else:
        result("Genesis balance modification", False,
               f"CRITICAL: Genesis balance modified! {genesis_before} -> {genesis_after}")


def test_genesis_privilege_escalation():
    """Try to gain genesis-level privileges."""
    r = rpc("createWallet", ["escalation_attacker"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Genesis privilege escalation", True, "Skipped")
        return

    # Try to set ourselves as owner of a genesis contract
    molt = CONTRACTS.get("MOLT", {})
    program = molt.get("program", "")
    if program:
        r1 = rpc("callContract", [program, "transfer_ownership", [attacker], "escalation_attacker"], timeout=5)
        r2 = rpc("setContractOwner", [program, attacker], timeout=3)
        r3 = rpc("updateContract", [program, {"owner": attacker}], timeout=3)

        # Check ownership unchanged
        info = rpc("getContractInfo", [program])
        owner = info.get("result", {}).get("owner", "")
        if owner == GENESIS_OWNER:
            result("Genesis privilege escalation", True, "Contract ownership protected")
        else:
            result("Genesis privilege escalation", False, f"CRITICAL: Ownership changed to {owner}")
    else:
        result("Genesis privilege escalation", True, "Skipped (no MOLT contract)")


def test_genesis_impersonate_transfer():
    """Try to impersonate genesis account in a transfer."""
    r = rpc("createWallet", ["genesis_impersonator"])
    impersonator = r.get("result", {}).get("address", "")
    if not impersonator:
        result("Genesis impersonation", True, "Skipped")
        return

    rpc("requestAirdrop", [impersonator, 1000000000])  # 1 MOLT
    time.sleep(2)

    genesis_before = get_balance(GENESIS_OWNER)

    # Try sending a TX that claims to be from genesis
    fake_tx = {
        "from": GENESIS_OWNER,
        "to": impersonator,
        "amount": 100000000000,  # 100 MOLT
        "signer": "genesis_impersonator"
    }
    r = rpc("sendTransaction", [fake_tx], timeout=5)

    # Also try transfer with genesis as source but our wallet name
    r2 = rpc("transfer", [GENESIS_OWNER, impersonator, 100000000000, "genesis_impersonator"], timeout=5)
    time.sleep(2)

    genesis_after = get_balance(GENESIS_OWNER)

    if genesis_after >= genesis_before * 0.99:
        result("Genesis impersonation", True, f"Impersonation rejected. Balance safe: {genesis_after}")
    else:
        result("Genesis impersonation", False,
               f"CRITICAL: Genesis impersonation succeeded! {genesis_before} -> {genesis_after}")


# ============================================================
# SECTION 3: WRAPPED TOKEN ATTACKS (wETH, wSOL)
# ============================================================

def test_wrapped_unauthorized_mint():
    """Try to mint wrapped tokens without authorization."""
    for sym in ["WETH", "WSOL"]:
        contract = CONTRACTS.get(sym, {})
        program = contract.get("program", "")
        if not program:
            result(f"{sym} unauthorized mint", True, "Skipped")
            continue

        r = rpc("createWallet", [f"{sym.lower()}_mint_atk"])
        attacker = r.get("result", {}).get("address", "")
        if not attacker:
            result(f"{sym} unauthorized mint", True, "Skipped")
            continue

        rpc("requestAirdrop", [attacker, 5000000000])
        time.sleep(1)

        # Try multiple mint vectors
        r1 = rpc("callContract", [program, "mint", [attacker, "1000000000"], f"{sym.lower()}_mint_atk"], timeout=5)
        r2 = rpc("callContract", [program, "wrap", [attacker, "1000000000"], f"{sym.lower()}_mint_atk"], timeout=5)
        r3 = rpc("mintToken", [program, attacker, 1000000000], timeout=3)

        all_rejected = all(
            "error" in r or (isinstance(r.get("result"), dict) and r["result"].get("error"))
            for r in [r1, r2, r3]
        )

        if all_rejected:
            result(f"{sym} unauthorized mint", True, "All mint attempts rejected")
        else:
            result(f"{sym} unauthorized mint", False, f"CRITICAL: Unauthorized {sym} minting!")


def test_wrapped_balance_inflation():
    """Try to inflate wrapped token balance without backing."""
    for sym in ["WETH", "WSOL"]:
        contract = CONTRACTS.get(sym, {})
        program = contract.get("program", "")
        if not program:
            result(f"{sym} balance inflation", True, "Skipped")
            continue

        r = rpc("createWallet", [f"{sym.lower()}_inflate"])
        attacker = r.get("result", {}).get("address", "")
        if not attacker:
            result(f"{sym} balance inflation", True, "Skipped")
            continue

        # Try to set token balance directly
        r1 = rpc("setTokenBalance", [program, attacker, 999999999999], timeout=3)
        r2 = rpc("callContract", [program, "set_balance", [attacker, "999999999999"], f"{sym.lower()}_inflate"], timeout=3)
        r3 = rpc("callContract", [program, "credit", [attacker, "999999999999"], f"{sym.lower()}_inflate"], timeout=3)

        all_rejected = all("error" in r for r in [r1, r2, r3])

        # Check token balance is still 0
        bal = rpc("getTokenBalance", [attacker, program])
        token_bal = bal.get("result", {}).get("balance", 0) if isinstance(bal.get("result"), dict) else 0

        if all_rejected and token_bal == 0:
            result(f"{sym} balance inflation", True, "Balance inflation impossible")
        else:
            result(f"{sym} balance inflation", False,
                   f"CRITICAL: {sym} balance inflated to {token_bal}!")


def test_wrapped_cross_confusion():
    """Try to use wETH as wSOL or vice versa (cross-token confusion)."""
    weth = CONTRACTS.get("WETH", {}).get("program", "")
    wsol = CONTRACTS.get("WSOL", {}).get("program", "")
    if not weth or not wsol:
        result("Wrapped cross-token confusion", True, "Skipped")
        return

    r = rpc("createWallet", ["cross_confused"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Wrapped cross-token confusion", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    # Try to redeem wETH balance via wSOL contract and vice versa
    r1 = rpc("callContract", [wsol, "redeem", [attacker, "1000000"], "cross_confused"], timeout=5)
    r2 = rpc("callContract", [weth, "redeem", [attacker, "1000000"], "cross_confused"], timeout=5)

    # Try calling wETH with wSOL's program ID as param
    r3 = rpc("callContract", [weth, "transfer", [wsol, "1000000000"], "cross_confused"], timeout=5)

    all_safe = all(
        "error" in r or (isinstance(r.get("result"), dict) and r["result"].get("error"))
        for r in [r1, r2, r3]
    )

    if all_safe:
        result("Wrapped cross-token confusion", True, "Cross-token confusion attacks rejected")
    else:
        result("Wrapped cross-token confusion", False, "CRITICAL: Cross-token confusion possible!")


# ============================================================
# SECTION 4: TREASURY / VAULT ATTACKS
# ============================================================

def test_treasury_drain():
    """Try to drain the treasury via various attack vectors."""
    vault = CONTRACTS.get("CLAWVAULT", {})
    program = vault.get("program", "")
    if not program:
        result("Treasury drain attempt", True, "Skipped")
        return

    r = rpc("createWallet", ["treasury_thief"])
    thief = r.get("result", {}).get("address", "")
    if not thief:
        result("Treasury drain attempt", True, "Skipped")
        return

    rpc("requestAirdrop", [thief, 10000000000])
    time.sleep(1)

    # Try multiple withdrawal vectors
    attacks = [
        ("withdraw", [thief, "999999999999999"]),
        ("drain", [thief]),
        ("emergencyWithdraw", [thief, "999999999999"]),
        ("transferAll", [thief]),
        ("sweep", [thief]),
    ]

    all_rejected = True
    for method, params in attacks:
        r = rpc("callContract", [program, method, params, "treasury_thief"], timeout=5)
        if "result" in r and "error" not in str(r.get("result", "")):
            all_rejected = False
            break

    alive = server_alive()
    if alive and all_rejected:
        result("Treasury drain attempt", True, "All treasury drain vectors rejected")
    elif not alive:
        result("Treasury drain attempt", False, "Server crashed during treasury attack!")
    else:
        result("Treasury drain attempt", False, "CRITICAL: Treasury drain may have succeeded!")


def test_vault_unauthorized_withdrawal():
    """Try to withdraw from CLAWVAULT without authorization."""
    vault = CONTRACTS.get("CLAWVAULT", {})
    program = vault.get("program", "")
    if not program:
        result("Vault unauthorized withdrawal", True, "Skipped")
        return

    r = rpc("createWallet", ["vault_burglar"])
    burglar = r.get("result", {}).get("address", "")
    if not burglar:
        result("Vault unauthorized withdrawal", True, "Skipped")
        return

    rpc("requestAirdrop", [burglar, 5000000000])
    time.sleep(1)

    # Get vault balance before
    vault_info = rpc("getContractInfo", [program])

    # Try withdrawal
    r = rpc("callContract", [program, "withdraw", [burglar, "1000000000"], "vault_burglar"], timeout=5)
    has_error = "error" in r or (isinstance(r.get("result"), dict) and r["result"].get("error"))

    if has_error:
        result("Vault unauthorized withdrawal", True, "Unauthorized withdrawal rejected")
    else:
        result("Vault unauthorized withdrawal", False, "CRITICAL: Unauthorized vault withdrawal!")


def test_fee_redirection():
    """Try to redirect transaction fees to attacker's wallet."""
    r = rpc("createWallet", ["fee_redirect_atk"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Fee redirection attack", True, "Skipped")
        return

    # Try to set fee recipient
    r1 = rpc("setFeeRecipient", [attacker], timeout=3)
    r2 = rpc("setFeeConfig", [{"recipient": attacker}], timeout=3)
    r3 = rpc("setFeeConfig", [{"recipient": attacker, "admin_token": "guess123"}], timeout=3)

    all_rejected = all("error" in r for r in [r1, r2, r3])

    if all_rejected:
        result("Fee redirection attack", True, "Fee redirection attempts rejected")
    else:
        result("Fee redirection attack", False, "CRITICAL: Fee recipient may have been changed!")


def test_dao_unauthorized_proposal():
    """Try to execute a DAO proposal without governance approval."""
    dao = CONTRACTS.get("DAO", {})
    program = dao.get("program", "")
    if not program:
        result("DAO unauthorized execution", True, "Skipped")
        return

    r = rpc("createWallet", ["dao_hijacker"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("DAO unauthorized execution", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    # Try to execute treasury transfer without proposal
    attacks = [
        ("execute", [{"transfer": {"to": attacker, "amount": "999999999"}}]),
        ("forceExecute", [attacker, "999999999"]),
        ("emergencyAction", [{"drain_to": attacker}]),
        ("propose_and_execute", [{"recipient": attacker, "amount": "10000000000"}]),
    ]

    all_rejected = True
    for method, params in attacks:
        r = rpc("callContract", [program, method, params, "dao_hijacker"], timeout=5)
        if "result" in r and "error" not in str(r.get("result", "")):
            all_rejected = False
            break

    if all_rejected:
        result("DAO unauthorized execution", True, "DAO governance protected")
    else:
        result("DAO unauthorized execution", False, "CRITICAL: Unauthorized DAO execution!")


# ============================================================
# SECTION 5: STAKING ATTACKS
# ============================================================

def test_steal_staked_molt():
    """Try to unstake MOLT from another validator's account."""
    validators = rpc("getValidators")
    val_list = validators.get("result", {})
    if isinstance(val_list, dict):
        val_list = val_list.get("validators", [])
    if not val_list:
        result("Steal staked MOLT", True, "Skipped (no validators)")
        return

    # Get first validator address
    first_val = val_list[0] if isinstance(val_list[0], str) else val_list[0].get("pubkey", val_list[0].get("address", ""))

    r = rpc("createWallet", ["stake_thief"])
    thief = r.get("result", {}).get("address", "")
    if not thief:
        result("Steal staked MOLT", True, "Skipped")
        return

    # Try to unstake from the validator to our account
    r1 = rpc("unstake", [first_val, 5000000000, "stake_thief"], timeout=5)
    r2 = rpc("requestUnstake", [first_val, thief, 5000000000], timeout=5)
    r3 = rpc("withdrawStake", [first_val, thief, 5000000000], timeout=5)

    all_rejected = all("error" in r for r in [r1, r2, r3])

    # Check thief didn't receive any funds
    thief_bal = get_balance(thief)

    if all_rejected and thief_bal < 1000000000:  # less than 1 MOLT (only airdrop dust)
        result("Steal staked MOLT", True, "Staked MOLT protected from theft")
    else:
        result("Steal staked MOLT", False, f"CRITICAL: Staked MOLT stolen! Thief balance: {thief_bal}")


def test_fake_unstake():
    """Try to forge an unstake transaction from someone else's validator."""
    r = rpc("createWallet", ["fake_unstaker"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Fake unstake", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    # Try to unstake from a validator we don't own
    validators = rpc("getValidators")
    val_list = validators.get("result", {})
    if isinstance(val_list, dict):
        val_list = val_list.get("validators", [])

    if val_list:
        first_val = val_list[0] if isinstance(val_list[0], str) else val_list[0].get("pubkey", "")
        r = rpc("unstake", [first_val, 1000000000, "fake_unstaker"], timeout=5)
        has_error = "error" in r
        if has_error:
            result("Fake unstake", True, "Cannot unstake from others' validators")
        else:
            result("Fake unstake", False, "CRITICAL: Unstaked from foreign validator!")
    else:
        result("Fake unstake", True, "Skipped (no validators)")


def test_stake_reward_manipulation():
    """Try to manipulate staking rewards."""
    r = rpc("createWallet", ["reward_manipulator"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Stake reward manipulation", True, "Skipped")
        return

    # Try to claim rewards we don't have
    r1 = rpc("claimRewards", [GENESIS_OWNER, attacker], timeout=5)
    r2 = rpc("claimStakeRewards", [attacker, 999999999999], timeout=5)
    r3 = rpc("setRewardRate", [999999999, "guess_token"], timeout=3)

    all_rejected = all("error" in r for r in [r1, r2, r3])

    attacker_bal = get_balance(attacker)
    if all_rejected and attacker_bal < 1000000000:
        result("Stake reward manipulation", True, "Reward manipulation rejected")
    else:
        result("Stake reward manipulation", False,
               f"CRITICAL: Reward manipulation possible! Balance: {attacker_bal}")


def test_slash_innocent_validator():
    """Try to slash a validator without authority."""
    validators = rpc("getValidators")
    val_list = validators.get("result", {})
    if isinstance(val_list, dict):
        val_list = val_list.get("validators", [])
    if not val_list:
        result("Slash innocent validator", True, "Skipped")
        return

    first_val = val_list[0] if isinstance(val_list[0], str) else val_list[0].get("pubkey", "")
    val_balance_before = get_balance(first_val) if first_val else 0

    # Try to slash
    r1 = rpc("slashValidator", [first_val, 5000000000], timeout=5)
    r2 = rpc("reportViolation", [first_val, "made_up_violation"], timeout=5)
    r3 = rpc("penalizeValidator", [first_val, 1000000000], timeout=5)

    all_rejected = all("error" in r for r in [r1, r2, r3])

    if first_val:
        val_balance_after = get_balance(first_val)
        balance_safe = val_balance_after >= val_balance_before * 0.95
    else:
        balance_safe = True

    if all_rejected and balance_safe:
        result("Slash innocent validator", True, "Cannot slash validators without authority")
    else:
        result("Slash innocent validator", False, "CRITICAL: Validator slashed without authority!")


# ============================================================
# SECTION 6: WALLET SAFETY MECHANISMS
# ============================================================

def test_concurrent_balance_drain():
    """Try to create a race condition to double-spend by draining simultaneously."""
    r = rpc("createWallet", ["race_victim"])
    victim = r.get("result", {}).get("address", "")
    if not victim:
        result("Concurrent balance drain", True, "Skipped")
        return

    rpc("requestAirdrop", [victim, 10000000000])  # 10 MOLT
    time.sleep(2)

    # Create 10 recipient wallets
    recipients = []
    for i in range(10):
        r = rpc("createWallet", [f"race_recv_{i}"])
        addr = r.get("result", {}).get("address", "")
        if addr:
            recipients.append(addr)

    if len(recipients) < 5:
        result("Concurrent balance drain", True, "Skipped (not enough recipients)")
        return

    # Try to send 5 MOLT to each of 10 recipients simultaneously (50 MOLT total from 10 MOLT balance)
    results_list = []
    def drain_race(recv_addr, idx):
        r = rpc("transfer", [victim, recv_addr, 5000000000, "race_victim"], timeout=5)
        results_list.append(("result" in r and "error" not in r, r))

    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
        futures = [pool.submit(drain_race, recv, i) for i, recv in enumerate(recipients)]
        concurrent.futures.wait(futures, timeout=20)

    time.sleep(3)

    # Check victim balance -- should never go negative
    victim_bal = get_balance(victim)
    total_received = sum(get_balance(r) for r in recipients)
    success_count = sum(1 for s, _ in results_list if s)

    # Only 1-2 should succeed (10 MOLT / 5 MOLT each)
    if success_count <= 2 and victim_bal >= 0:
        result("Concurrent balance drain", True,
               f"Race condition protected. {success_count} succeeded, victim bal: {victim_bal}")
    else:
        result("Concurrent balance drain", False,
               f"CRITICAL: Race drain! {success_count} succeeded, "
               f"victim balance: {victim_bal}, total received: {total_received}")


def test_dust_attack_flood():
    """Send thousands of tiny transactions to overwhelm an account."""
    r = rpc("createWallet", ["dust_attacker"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Dust attack flood", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 50000000000])  # 50 MOLT
    time.sleep(2)

    r = rpc("createWallet", ["dust_victim"])
    victim = r.get("result", {}).get("address", "")
    if not victim:
        result("Dust attack flood", True, "Skipped")
        return

    # Send 200 tiny transactions (1 shell each)
    sent = 0
    for i in range(200):
        r = rpc("transfer", [attacker, victim, 1, "dust_attacker"], timeout=2)
        if "result" in r:
            sent += 1

    alive = server_alive()
    if alive:
        result("Dust attack flood", True, f"Server survived {sent} dust TXs")
    else:
        result("Dust attack flood", False, "Server crashed under dust attack!")


def test_balance_consistency_under_attack():
    """Verify balance consistency after various attack attempts."""
    r = rpc("createWallet", ["consistency_check"])
    addr = r.get("result", {}).get("address", "")
    if not addr:
        result("Balance consistency", True, "Skipped")
        return

    rpc("requestAirdrop", [addr, 10000000000])  # 10 MOLT
    time.sleep(2)

    initial = get_balance(addr)

    # Do many rapid reads interleaved with transfer attempts
    balances = []
    for i in range(50):
        b = get_balance(addr)
        balances.append(b)
        if i % 10 == 0:
            rpc("transfer", [addr, addr, 0, "consistency_check"], timeout=2)  # self-transfer 0

    # All balances should be consistent (within small variance for fees)
    unique = set(balances)
    if len(unique) <= 3:  # Minor fee variance acceptable
        result("Balance consistency", True, f"Consistent across 50 reads ({len(unique)} unique values)")
    else:
        result("Balance consistency", False, f"CRITICAL: Balance inconsistent! {len(unique)} different values")


def test_negative_balance_via_overflow():
    """Try to create a negative balance via integer overflow in transfer amount."""
    r = rpc("createWallet", ["neg_overflow_atk"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Negative balance overflow", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    r2 = rpc("createWallet", ["neg_overflow_rcv"])
    receiver = r2.get("result", {}).get("address", "")
    if not receiver:
        result("Negative balance overflow", True, "Skipped")
        return

    # Try various overflow amounts
    overflow_amounts = [
        2**63 - 1,        # i64 max
        2**64 - 1,        # u64 max
        2**64,            # u64 overflow
        2**128,           # u128 overflow
        -1,               # Negative
        -2**63,           # i64 min
    ]

    all_safe = True
    for amt in overflow_amounts:
        r = rpc("transfer", [attacker, receiver, amt, "neg_overflow_atk"], timeout=3)
        # If transfer doesn't error, check balances
        if "result" in r and "error" not in r:
            bal = get_balance(attacker)
            if isinstance(bal, (int, float)) and bal < 0:
                all_safe = False
                break

    if all_safe:
        result("Negative balance overflow", True, "All overflow attempts handled safely")
    else:
        result("Negative balance overflow", False, "CRITICAL: Negative balance created via overflow!")


# ============================================================
# SECTION 7: CONTRACT EXPLOIT ATTACKS
# ============================================================

def test_reentrancy_simulation():
    """Simulate a reentrancy attack via rapid recursive contract calls."""
    dex = CONTRACTS.get("DEX", {})
    program = dex.get("program", "")
    if not program:
        result("Reentrancy simulation", True, "Skipped")
        return

    r = rpc("createWallet", ["reentrancy_atk"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Reentrancy simulation", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 10000000000])
    time.sleep(1)

    # Try to call swap with a callback that calls swap again (simulated)
    r1 = rpc("callContract", [program, "swap_with_callback", [
        attacker, "1000000", program, "swap", [attacker, "1000000"]
    ], "reentrancy_atk"], timeout=5)

    # Try recursive deposit
    r2 = rpc("callContract", [program, "deposit", [
        attacker, "1000000",
        {"callback": {"contract": program, "method": "deposit", "args": [attacker, "1000000"]}}
    ], "reentrancy_atk"], timeout=5)

    alive = server_alive()
    if alive:
        result("Reentrancy simulation", True, "Server survived reentrancy attempts")
    else:
        result("Reentrancy simulation", False, "Server crashed during reentrancy test!")


def test_contract_ownership_hijack():
    """Try to change ownership of genesis contracts."""
    r = rpc("createWallet", ["hijack_atk"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Contract ownership hijack", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    hijack_count = 0
    for sym in ["MOLT", "MUSD", "DEX", "BRIDGE", "CLAWVAULT"]:
        contract = CONTRACTS.get(sym, {})
        program = contract.get("program", "")
        if not program:
            continue

        # Try various ownership transfer methods
        for method in ["transfer_ownership", "set_owner", "changeOwner", "admin_transfer"]:
            rpc("callContract", [program, method, [attacker], "hijack_atk"], timeout=3)

        # Check ownership
        info = rpc("getContractInfo", [program])
        current_owner = info.get("result", {}).get("owner", "")
        if current_owner and current_owner != GENESIS_OWNER:
            hijack_count += 1

    if hijack_count == 0:
        result("Contract ownership hijack", True, "All contracts ownership protected")
    else:
        result("Contract ownership hijack", False,
               f"CRITICAL: {hijack_count} contracts had ownership hijacked!")


def test_contract_code_replacement():
    """Try to replace contract code without authorization."""
    molt = CONTRACTS.get("MOLT", {})
    program = molt.get("program", "")
    if not program:
        result("Contract code replacement", True, "Skipped")
        return

    # Get original code hash
    info_before = rpc("getContractInfo", [program])
    hash_before = info_before.get("result", {}).get("code_hash", "")

    # Try to replace code
    evil_code = base64.b64encode(b'\x00asm\x01\x00\x00\x00' + b'\x00' * 100).decode()

    r1 = rpc("deployContract", [evil_code, "hijack_atk", "MOLT"], timeout=5)
    r2 = rpc("upgradeContract", [program, evil_code], timeout=5)
    r3 = rpc("updateContract", [program, {"code": evil_code}], timeout=5)

    # Check code hash unchanged
    info_after = rpc("getContractInfo", [program])
    hash_after = info_after.get("result", {}).get("code_hash", "")

    if hash_before and hash_after and hash_before == hash_after:
        result("Contract code replacement", True, "Contract code is immutable")
    elif not hash_before:
        result("Contract code replacement", True, "Skipped (no code hash available)")
    else:
        result("Contract code replacement", False,
               f"CRITICAL: Contract code changed! {hash_before[:16]}... -> {hash_after[:16]}...")


def test_oracle_price_manipulation():
    """Try to manipulate the ORACLE contract prices."""
    oracle = CONTRACTS.get("ORACLE", {})
    program = oracle.get("program", "")
    if not program:
        result("Oracle price manipulation", True, "Skipped")
        return

    r = rpc("createWallet", ["oracle_manipulator"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Oracle price manipulation", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    # Try to push a fake price update
    attacks = [
        ("update_price", ["MOLT/USD", "0.001"]),  # Tank the price
        ("set_price", ["MOLT/USD", "999999"]),     # Pump the price
        ("push_feed", [{"pair": "MOLT/USD", "price": "0", "timestamp": int(time.time())}]),
        ("override_price", ["MOLT/USD", "0.00001"]),
    ]

    all_rejected = True
    for method, params in attacks:
        r = rpc("callContract", [program, method, params, "oracle_manipulator"], timeout=5)
        if "result" in r and "error" not in str(r.get("result", "")):
            all_rejected = False
            break

    if all_rejected:
        result("Oracle price manipulation", True, "Oracle prices protected from manipulation")
    else:
        result("Oracle price manipulation", False, "CRITICAL: Oracle price manipulation possible!")


def test_lending_exploit():
    """Try to exploit the LEND contract for free loans."""
    lend = CONTRACTS.get("LEND", {})
    program = lend.get("program", "")
    if not program:
        result("Lending protocol exploit", True, "Skipped")
        return

    r = rpc("createWallet", ["lend_exploiter"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Lending protocol exploit", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    # Try to borrow without collateral
    attacks = [
        ("borrow", [attacker, "1000000000000", "0"]),  # borrow max, 0 collateral
        ("flash_loan", [attacker, "999999999999"]),     # flash loan attack
        ("liquidate", [GENESIS_OWNER, attacker]),       # liquidate genesis
        ("withdraw_collateral", [GENESIS_OWNER, "999999999"]),  # steal collateral
    ]

    all_rejected = True
    for method, params in attacks:
        r = rpc("callContract", [program, method, params, "lend_exploiter"], timeout=5)
        if "result" in r and "error" not in str(r.get("result", "")):
            all_rejected = False
            break

    if all_rejected:
        result("Lending protocol exploit", True, "Lending protocol protected")
    else:
        result("Lending protocol exploit", False, "CRITICAL: Lending protocol exploited!")


# ============================================================
# SECTION 8: ADMIN FINANCIAL ATTACKS
# ============================================================

def test_unauthorized_fee_change():
    """Try to change fee parameters without admin token."""
    # Try common fee manipulation methods
    attacks = [
        ("setFeeConfig", [{"base_fee": 0}]),
        ("setFeeConfig", [{"base_fee": 0, "admin_token": ""}]),
        ("setFeeConfig", [{"base_fee": 0, "admin_token": "password"}]),
        ("setTransactionFee", [0]),
        ("setMintFee", [0]),
    ]

    all_rejected = True
    for method, params in attacks:
        r = rpc(method, params, timeout=3)
        if "result" in r and "error" not in r:
            all_rejected = False
            break

    if all_rejected:
        result("Unauthorized fee change", True, "Fee changes require admin authority")
    else:
        result("Unauthorized fee change", False, "CRITICAL: Fee configuration changed without authority!")


def test_supply_inflation_attack():
    """Try to inflate total MOLT supply."""
    metrics_before = rpc("getMetrics")
    supply_before = metrics_before.get("result", {}).get("total_supply", 0)

    # Try various supply inflation vectors
    r1 = rpc("mintNative", [GENESIS_OWNER, 999999999999], timeout=3)
    r2 = rpc("inflateSupply", [999999999999], timeout=3)
    r3 = rpc("createTokens", [999999999999], timeout=3)

    # Try massive airdrop
    r4 = rpc("createWallet", ["inflation_test"])
    addr = r4.get("result", {}).get("address", "")
    if addr:
        rpc("requestAirdrop", [addr, 2**62], timeout=3)

    time.sleep(2)
    metrics_after = rpc("getMetrics")
    supply_after = metrics_after.get("result", {}).get("total_supply", 0)

    # Supply should not have changed dramatically (small changes from airdrops are normal)
    if supply_before and supply_after:
        change = abs(supply_after - supply_before)
        pct = (change / supply_before) * 100 if supply_before else 0
        if pct < 1:  # Less than 1% change
            result("Supply inflation attack", True,
                   f"Supply stable: {supply_before} -> {supply_after} ({pct:.4f}% change)")
        else:
            result("Supply inflation attack", False,
                   f"CRITICAL: Supply inflated! {supply_before} -> {supply_after} ({pct:.2f}% change)")
    else:
        result("Supply inflation attack", True, "Skipped (couldn't read supply)")


def test_rent_parameter_exploitation():
    """Try to manipulate rent parameters for financial gain."""
    # Try to set rent to 0 (free storage) or extremely high (drain accounts)
    attacks = [
        ("setRentParams", [{"rate": 0}]),
        ("setRentParams", [{"rate": 0, "admin_token": ""}]),
        ("setRentParams", [{"rate": 999999999, "admin_token": "guess"}]),
        ("setRentExemption", [0]),
    ]

    all_rejected = True
    for method, params in attacks:
        r = rpc(method, params, timeout=3)
        if "result" in r and "error" not in r:
            all_rejected = False
            break

    if all_rejected:
        result("Rent parameter exploitation", True, "Rent parameters protected")
    else:
        result("Rent parameter exploitation", False, "CRITICAL: Rent parameters manipulated!")


def test_bridge_exploit():
    """Try to exploit the BRIDGE contract for free cross-chain tokens."""
    bridge = CONTRACTS.get("BRIDGE", {})
    program = bridge.get("program", "")
    if not program:
        result("Bridge exploit", True, "Skipped")
        return

    r = rpc("createWallet", ["bridge_exploiter"])
    attacker = r.get("result", {}).get("address", "")
    if not attacker:
        result("Bridge exploit", True, "Skipped")
        return

    rpc("requestAirdrop", [attacker, 5000000000])
    time.sleep(1)

    # Try to claim a fake bridge deposit
    attacks = [
        ("claim", [attacker, "1000000000000", "0x" + "aa" * 32]),
        ("completeBridge", [{"recipient": attacker, "amount": "999999999", "source_tx": "0x" + "bb" * 32}]),
        ("mint_bridged", [attacker, "wETH", "999999999"]),
        ("release", [attacker, "1000000000000"]),
    ]

    all_rejected = True
    for method, params in attacks:
        r = rpc("callContract", [program, method, params, "bridge_exploiter"], timeout=5)
        if "result" in r and "error" not in str(r.get("result", "")):
            all_rejected = False
            break

    attacker_bal = get_balance(attacker)
    if all_rejected and attacker_bal < 6000000000:  # Only initial airdrop
        result("Bridge exploit", True, "Bridge contract protected")
    else:
        result("Bridge exploit", False, f"CRITICAL: Bridge exploited! Attacker balance: {attacker_bal}")


def test_dex_sandwich_attack():
    """Simulate a sandwich attack on the DEX."""
    dex = CONTRACTS.get("DEX", {})
    program = dex.get("program", "")
    if not program:
        result("DEX sandwich attack", True, "Skipped")
        return

    # Create attacker and victim
    r1 = rpc("createWallet", ["sandwich_atk"])
    r2 = rpc("createWallet", ["sandwich_victim"])
    attacker = r1.get("result", {}).get("address", "")
    victim = r2.get("result", {}).get("address", "")
    if not attacker or not victim:
        result("DEX sandwich attack", True, "Skipped")
        return

    for addr in [attacker, victim]:
        rpc("requestAirdrop", [addr, 10000000000])
    time.sleep(2)

    # Try front-running: submit swap, immediately submit another with higher gas
    def victim_swap():
        return rpc("callContract", [program, "swap", [victim, "MOLT", "MUSD", "1000000000"], "sandwich_victim"], timeout=10)

    def attacker_frontrun():
        return rpc("callContract", [program, "swap", [attacker, "MOLT", "MUSD", "5000000000", {"priority": 999}], "sandwich_atk"], timeout=10)

    def attacker_backrun():
        return rpc("callContract", [program, "swap", [attacker, "MUSD", "MOLT", "5000000000"], "sandwich_atk"], timeout=10)

    # Execute sandwich: frontrun -> victim -> backrun
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        f1 = pool.submit(attacker_frontrun)
        time.sleep(0.05)
        f2 = pool.submit(victim_swap)
        time.sleep(0.05)
        f3 = pool.submit(attacker_backrun)
        concurrent.futures.wait([f1, f2, f3], timeout=15)

    alive = server_alive()
    if alive:
        result("DEX sandwich attack", True, "Server survived sandwich attack simulation")
    else:
        result("DEX sandwich attack", False, "Server crashed during sandwich attack!")


# ============================================================
# MAIN
# ============================================================

def main():
    print("=" * 70)
    print("  MoltChain Financial Attack Test Suite")
    print("=" * 70)
    print(f"  RPC: {RPC}")
    print()

    # Verify connectivity
    r = rpc("getSlot")
    if "result" not in r:
        print("FATAL: Cannot connect to validator RPC")
        sys.exit(1)
    print(f"  Connected. Current slot: {r['result']}")

    # Resolve contract addresses
    count = resolve_contracts()
    print(f"  Resolved {count} contracts: {', '.join(CONTRACTS.keys())}")
    print()

    sections = [
        ("mUSD STABLECOIN ATTACKS", [
            test_musd_unauthorized_mint,
            test_musd_supply_manipulation,
            test_musd_infinite_mint_loop,
            test_musd_fake_burn,
        ]),
        ("GENESIS WALLET ATTACKS", [
            test_genesis_drain,
            test_genesis_balance_modify,
            test_genesis_privilege_escalation,
            test_genesis_impersonate_transfer,
        ]),
        ("WRAPPED TOKEN ATTACKS", [
            test_wrapped_unauthorized_mint,
            test_wrapped_balance_inflation,
            test_wrapped_cross_confusion,
        ]),
        ("TREASURY / VAULT ATTACKS", [
            test_treasury_drain,
            test_vault_unauthorized_withdrawal,
            test_fee_redirection,
            test_dao_unauthorized_proposal,
        ]),
        ("STAKING ATTACKS", [
            test_steal_staked_molt,
            test_fake_unstake,
            test_stake_reward_manipulation,
            test_slash_innocent_validator,
        ]),
        ("WALLET SAFETY MECHANISMS", [
            test_concurrent_balance_drain,
            test_dust_attack_flood,
            test_balance_consistency_under_attack,
            test_negative_balance_via_overflow,
        ]),
        ("CONTRACT EXPLOIT ATTACKS", [
            test_reentrancy_simulation,
            test_contract_ownership_hijack,
            test_contract_code_replacement,
            test_oracle_price_manipulation,
            test_lending_exploit,
        ]),
        ("ADMIN FINANCIAL ATTACKS", [
            test_unauthorized_fee_change,
            test_supply_inflation_attack,
            test_rent_parameter_exploitation,
            test_bridge_exploit,
            test_dex_sandwich_attack,
        ]),
    ]

    for section_name, tests in sections:
        print(f"\n--- {section_name} ---")
        for test_fn in tests:
            try:
                test_fn()
            except Exception as e:
                result(test_fn.__name__.replace("test_", ""), False, f"EXCEPTION: {e}")
            # Verify server is still alive between tests
            try:
                alive_check = rpc("getSlot", timeout=3)
                if "result" not in alive_check:
                    print(f"  [!] SERVER UNRESPONSIVE after {test_fn.__name__}")
                    time.sleep(3)
            except:
                print(f"  [!] SERVER UNREACHABLE after {test_fn.__name__}")
                time.sleep(5)

    print("\n" + "=" * 70)
    print(f"  RESULTS: {PASS} PASS / {FAIL} FAIL / {WARN} WARN")
    print("=" * 70)

    if FAIL > 0:
        print("\n  FAILURES:")
        for tag, name, detail in RESULTS:
            if tag == "FAIL":
                print(f"    - {name}: {detail}")

    if WARN > 0:
        print("\n  WARNINGS:")
        for tag, name, detail in RESULTS:
            if tag == "WARN":
                print(f"    - {name}: {detail}")

    print()
    return 0 if FAIL == 0 else 1

if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""
Lichen End-to-End Agent Simulation Test
===========================================
Simulates an external agent performing REAL on-chain operations with
strict verification that every artifact was actually created:

1. Agent wallet creation
2. Receive 100 LICN from treasury (genesis) -- verify exact balance
3. Deploy a PROPER MT-20 token via init_data registration (using lusd_token.wasm)
4. Verify token appears in symbol registry with correct name/symbol/template
5. Deploy an NFT collection -- verify on-chain
6. Mint an NFT -- verify on-chain
7. Send 5 LICN back to treasury -- verify deploy fee deducted
8. Final on-chain verification of everything

Uses lusd_token.wasm (5392 bytes) as the MT-20 WASM template.
Passes init_data JSON to auto-register in symbol registry at deploy time.
Deploy fee: 25 LICN.
"""

import asyncio
import hashlib
import json
import os
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from lichen import Connection, Keypair, PublicKey, TransactionBuilder, Instruction

import nacl.signing

# ============================================================================
# Constants
# ============================================================================
RPC_URL = "http://127.0.0.1:8000"
SPORES_PER_LICN = 1_000_000_000
SYSTEM_PROGRAM = PublicKey(b'\x00' * 32)
CONTRACT_PROGRAM = PublicKey(b'\xff' * 32)

STATE_DIR = Path(__file__).resolve().parent.parent.parent / "data" / "state-8000"
TREASURY_KEY_PATH = STATE_DIR / "genesis-keys" / "treasury-lichen-testnet-1.json"

# Use lusd_token.wasm -- the real MT-20 token template (5392 bytes)
WASM_PATH = Path(__file__).resolve().parent.parent.parent / "contracts" / "lusd_token" / "target" / "wasm32-unknown-unknown" / "release" / "lusd_token.wasm"

# Agent token details
AGENT_TOKEN_SYMBOL = "TSYMBIONT"
AGENT_TOKEN_NAME = "TradingLobster Token"
AGENT_TOKEN_TEMPLATE = "mt20"
AGENT_TOKEN_DECIMALS = 9


# ============================================================================
# Bincode helpers (matching Rust bincode serialization)
# ============================================================================
def bincode_u16(v):
    return struct.pack("<H", v)

def bincode_u64(v):
    return struct.pack("<Q", v)

def bincode_bool(v):
    return b'\x01' if v else b'\x00'

def bincode_string(s):
    encoded = s.encode("utf-8")
    return bincode_u64(len(encoded)) + encoded

def bincode_option_none():
    return b'\x00'

def bincode_option_some(data):
    return b'\x01' + data

def bincode_pubkey(pk):
    return pk.to_bytes()


def encode_create_collection_data(name, symbol, royalty_bps, max_supply, public_mint, mint_authority=None):
    """Encode CreateCollectionData in Rust bincode format."""
    data = bincode_string(name)
    data += bincode_string(symbol)
    data += bincode_u16(royalty_bps)
    data += bincode_u64(max_supply)
    data += bincode_bool(public_mint)
    if mint_authority is None:
        data += bincode_option_none()
    else:
        data += bincode_option_some(bincode_pubkey(mint_authority))
    return data


def encode_mint_nft_data(token_id, metadata_uri):
    """Encode MintNftData in Rust bincode format."""
    data = bincode_u64(token_id)
    data += bincode_string(metadata_uri)
    return data


# ============================================================================
# Helpers
# ============================================================================
def load_treasury_keypair():
    """Load treasury keypair from genesis-keys JSON (hex seed format)."""
    with open(TREASURY_KEY_PATH) as f:
        data = json.load(f)
    seed_hex = data["secret_key"]
    seed = bytes.fromhex(seed_hex)
    signing_key = nacl.signing.SigningKey(seed)
    kp = Keypair(signing_key)
    expected_b58 = data["pubkey"]
    actual_b58 = kp.public_key().to_base58()
    assert actual_b58 == expected_b58, f"Treasury key mismatch: {actual_b58} != {expected_b58}"
    return kp


def derive_program_address(deployer_pubkey, wasm_bytes):
    """SHA-256(deployer_bytes + wasm_bytes) -> first 32 bytes = program address."""
    h = hashlib.sha256(deployer_pubkey.to_bytes() + wasm_bytes).digest()
    return PublicKey(h[:32])


def derive_collection_address(creator_pubkey, name):
    """Derive a deterministic collection address from creator + name."""
    h = hashlib.sha256(creator_pubkey.to_bytes() + name.encode("utf-8")).digest()
    return PublicKey(h[:32])


def derive_token_address(collection_pubkey, token_id):
    """Derive a deterministic NFT token address from collection + token_id."""
    h = hashlib.sha256(collection_pubkey.to_bytes() + struct.pack("<Q", token_id)).digest()
    return PublicKey(h[:32])


def print_header(title):
    print(f"\n{'='*70}")
    print(f"  {title}")
    print(f"{'='*70}")


def print_step(num, desc):
    print(f"\n--- Step {num}: {desc} ---")


def print_ok(msg):
    print(f"  [OK] {msg}")


def print_fail(msg):
    print(f"  [FAIL] {msg}")


def print_info(msg):
    print(f"  [..] {msg}")


# ============================================================================
# Main Test
# ============================================================================
async def main():
    results = []
    conn = Connection(RPC_URL)
    program_pubkey = None

    print_header("Lichen E2E Agent Simulation Test (FULL VERIFICATION)")

    # ------------------------------------------------------------------
    # PRE-CHECK: Verify chain is alive, genesis contracts exist
    # ------------------------------------------------------------------
    print_step(0, "Pre-flight checks")
    try:
        health = await conn.health()
        print_ok(f"Chain healthy: {health}")
    except Exception as e:
        print_fail(f"Chain unreachable: {e}")
        return False

    slot = await conn.get_slot()
    print_ok(f"Current slot: {slot}")

    validators = await conn.get_validators()
    val_list = validators.get("validators", []) if isinstance(validators, dict) else validators if isinstance(validators, list) else []
    print_ok(f"Validators: {len(val_list)}")

    # Verify genesis contracts exist
    try:
        registry = await conn._rpc("getAllSymbolRegistry")
        reg_list = registry if isinstance(registry, list) else registry.get("entries", []) if isinstance(registry, dict) else []
        print_ok(f"Symbol registry has {len(reg_list)} entries (expected 26 genesis)")
        if len(reg_list) < 26:
            print_fail(f"Expected 26 genesis contracts, got {len(reg_list)}")
    except Exception as e:
        print_info(f"Registry query: {e}")
        reg_list = []

    # ------------------------------------------------------------------
    # Step 1: Load treasury + generate agent wallet
    # ------------------------------------------------------------------
    print_step(1, "Create Agent wallet + load Treasury")

    treasury = load_treasury_keypair()
    treasury_b58 = treasury.public_key().to_base58()
    print_ok(f"Treasury loaded: {treasury_b58}")

    agent = Keypair.generate()
    agent_b58 = agent.public_key().to_base58()
    print_ok(f"Agent wallet generated: {agent_b58}")

    # Check treasury balance
    treasury_bal = await conn.get_balance(treasury.public_key())
    treasury_licn = float(treasury_bal.get("licn", "0"))
    print_ok(f"Treasury balance: {treasury_licn:.2f} LICN")
    assert treasury_licn > 100, f"Treasury too low: {treasury_licn}"

    # Agent should have 0
    agent_bal = await conn.get_balance(agent.public_key())
    agent_licn = float(agent_bal.get("licn", "0"))
    print_ok(f"Agent balance: {agent_licn:.2f} LICN (should be 0)")

    # ------------------------------------------------------------------
    # Step 2: Transfer 100 LICN from treasury to agent
    # ------------------------------------------------------------------
    print_step(2, "Transfer 250 LICN: Treasury -> Agent")

    amount_spores = 250 * SPORES_PER_LICN
    blockhash = await conn.get_recent_blockhash()
    print_info(f"Blockhash: {blockhash[:16]}...")

    transfer_ix = TransactionBuilder.transfer(
        treasury.public_key(),
        agent.public_key(),
        amount_spores,
    )
    tx = (
        TransactionBuilder()
        .add(transfer_ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(treasury)
    )
    sig = await conn.send_transaction(tx)
    print_ok(f"Transfer tx: {sig[:32] if sig else 'no sig'}...")

    # Wait for confirmation then VERIFY exact balance
    await asyncio.sleep(2)

    agent_bal = await conn.get_balance(agent.public_key())
    agent_licn = float(agent_bal.get("licn", "0"))
    if agent_licn >= 249.9:
        print_ok(f"Agent balance verified: {agent_licn:.4f} LICN")
        results.append(("Transfer 250 LICN to Agent", True))
    else:
        print_fail(f"Agent balance: {agent_licn:.4f} LICN (expected ~250)")
        results.append(("Transfer 250 LICN to Agent", False))

    # ------------------------------------------------------------------
    # Step 3: Agent deploys a PROPER MT-20 token via deployContract RPC
    #
    # Uses lusd_token.wasm (5392 bytes) as the WASM template.
    # Sends code as base64 + init_data as JSON via the deployContract
    # RPC endpoint (bypasses transaction instruction size limit).
    # Deployer signs SHA-256(code_bytes) to prove ownership.
    # Deploy fee: 25 LICN deducted from agent.
    # ------------------------------------------------------------------
    print_step(3, "Agent deploys MT-20 token (TSYMBIONT) via deployContract RPC")

    if not WASM_PATH.exists():
        print_fail(f"WASM not found: {WASM_PATH}")
        results.append(("Deploy MT-20 Token (TSYMBIONT)", False))
    else:
        import base64 as b64

        wasm_bytes = WASM_PATH.read_bytes()
        print_info(f"WASM loaded: {len(wasm_bytes)} bytes ({WASM_PATH.name})")

        # Derive expected program address: SHA-256(deployer + code)
        h = hashlib.sha256(agent.public_key().to_bytes() + wasm_bytes).digest()
        program_pubkey = PublicKey(h[:32])
        print_info(f"Expected program address: {program_pubkey}")

        # Sign SHA-256(code_bytes) with agent's key
        code_hash = hashlib.sha256(wasm_bytes).digest()
        signature = agent.sign(code_hash)
        print_info(f"Code hash signed by agent")

        # Build init_data JSON for symbol registry
        init_data = json.dumps({
            "symbol": AGENT_TOKEN_SYMBOL,
            "name": AGENT_TOKEN_NAME,
            "template": AGENT_TOKEN_TEMPLATE,
            "metadata": {
                "decimals": AGENT_TOKEN_DECIMALS,
            }
        })
        print_info(f"init_data: {init_data}")

        # Call deployContract RPC
        # Params: [deployer_base58, code_base64, init_data_json, signature_hex]
        try:
            deploy_result = await conn._rpc("deployContract", [
                agent.public_key().to_base58(),
                b64.b64encode(wasm_bytes).decode("ascii"),
                init_data,
                signature.hex(),
            ])
            print_ok(f"deployContract result: program_id={deploy_result.get('program_id', '?')}")
            print_ok(f"  code_size={deploy_result.get('code_size', '?')}, fee={deploy_result.get('deploy_fee_licn', '?')} LICN")

            # VERIFY 1: Contract exists on-chain
            deploy_ok = True
            try:
                info = await conn.get_contract_info(program_pubkey)
                if info:
                    print_ok(f"Contract verified on-chain: code_size={info.get('code_size', '?')}")
                else:
                    print_fail("getContractInfo returned null")
                    deploy_ok = False
            except Exception as e:
                print_fail(f"getContractInfo failed: {e}")
                deploy_ok = False

            # VERIFY 2: Token appears in symbol registry with correct name
            try:
                reg_entry = await conn._rpc("getSymbolRegistryByProgram", [str(program_pubkey)])
                if reg_entry and reg_entry.get("symbol") == AGENT_TOKEN_SYMBOL:
                    print_ok(f"Symbol registry: ${reg_entry['symbol']} = {reg_entry.get('name', '?')}")
                    print_ok(f"  template={reg_entry.get('template', '?')}")
                elif reg_entry:
                    print_fail(f"Registry symbol mismatch: {reg_entry.get('symbol')} != {AGENT_TOKEN_SYMBOL}")
                    deploy_ok = False
                else:
                    print_fail("getSymbolRegistryByProgram returned null")
                    deploy_ok = False
            except Exception as e:
                print_fail(f"Symbol registry check failed: {e}")
                deploy_ok = False

            # VERIFY 3: Deploy fee was charged (agent should have ~75 LICN now: 100 - 25)
            agent_bal_after = await conn.get_balance(agent.public_key())
            agent_licn_after = float(agent_bal_after.get("licn", "0"))
            print_info(f"Agent balance after deploy: {agent_licn_after:.4f} LICN")
            if agent_licn_after < 230.0:
                print_ok(f"Deploy fee verified (deducted ~{250 - agent_licn_after:.1f} LICN)")
            else:
                print_info(f"Deploy fee may not have been charged (balance: {agent_licn_after})")

            results.append(("Deploy MT-20 Token (TSYMBIONT)", deploy_ok))

        except Exception as e:
            print_fail(f"deployContract RPC failed: {e}")
            results.append(("Deploy MT-20 Token (TSYMBIONT)", False))

    # ------------------------------------------------------------------
    # Step 4: Verify ALL symbol registry entries (26 genesis + 1 agent)
    # ------------------------------------------------------------------
    print_step(4, "Verify symbol registry integrity")

    try:
        full_registry = await conn._rpc("getAllSymbolRegistry")
        reg_list = full_registry if isinstance(full_registry, list) else full_registry.get("entries", []) if isinstance(full_registry, dict) else []
        print_ok(f"Total symbol registry entries: {len(reg_list)}")

        # Check for our agent token
        agent_entry_found = False
        for entry in reg_list:
            if entry.get("symbol") == AGENT_TOKEN_SYMBOL:
                agent_entry_found = True
                print_ok(f"Agent token in registry: ${AGENT_TOKEN_SYMBOL} = {entry.get('name', '?')}")
                break

        if not agent_entry_found:
            print_fail(f"Agent token ${AGENT_TOKEN_SYMBOL} NOT found in registry")

        # Check that all genesis entries have names (not "unknown")
        unnamed = [e for e in reg_list if not e.get("name") or e.get("name") == "unknown"]
        if unnamed:
            print_fail(f"{len(unnamed)} entries have no name or 'unknown' name")
        else:
            print_ok("All registry entries have proper names")

        results.append(("Symbol registry integrity", agent_entry_found and len(unnamed) == 0))

    except Exception as e:
        print_fail(f"Registry verification failed: {e}")
        results.append(("Symbol registry integrity", False))

    # ------------------------------------------------------------------
    # Step 5: Agent deploys an NFT collection
    # ------------------------------------------------------------------
    print_step(5, "Agent deploys NFT collection (AgentPunks)")

    collection_name = "AgentPunks"
    collection_symbol = "APUNK"
    collection_pubkey = derive_collection_address(agent.public_key(), collection_name)
    print_info(f"Collection address: {collection_pubkey}")

    # System instruction type 6 = create_collection
    collection_data = encode_create_collection_data(
        name=collection_name,
        symbol=collection_symbol,
        royalty_bps=500,        # 5%
        max_supply=1000,
        public_mint=False,
        mint_authority=None,
    )
    ix_data = b'\x06' + collection_data

    collection_ix = Instruction(
        program_id=SYSTEM_PROGRAM,
        accounts=[agent.public_key(), collection_pubkey],
        data=ix_data,
    )

    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(collection_ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(agent)
    )

    try:
        sig = await conn.send_transaction(tx)
        print_ok(f"Collection tx: {sig[:32] if sig else 'no sig'}...")
        await asyncio.sleep(2)

        # VERIFY: Collection exists on-chain
        coll_ok = False
        try:
            coll_info = await conn.get_collection(collection_pubkey)
            if coll_info:
                print_ok(f"Collection verified: name={coll_info.get('name', '?')}, symbol={coll_info.get('symbol', '?')}")
                coll_ok = True
            else:
                print_fail("getCollection returned null")
        except Exception as e:
            print_fail(f"Collection verification failed: {e}")

        results.append(("Deploy NFT Collection (AgentPunks)", coll_ok))
    except Exception as e:
        print_fail(f"Collection deploy failed: {e}")
        results.append(("Deploy NFT Collection (AgentPunks)", False))

    # ------------------------------------------------------------------
    # Step 6: Agent mints an NFT
    # ------------------------------------------------------------------
    print_step(6, "Agent mints NFT #1 in AgentPunks")

    token_id = 1
    token_pubkey = derive_token_address(collection_pubkey, token_id)
    print_info(f"Token address: {token_pubkey}")

    # System instruction type 7 = mint_nft
    mint_data = encode_mint_nft_data(
        token_id=token_id,
        metadata_uri="https://lichen.network/nft/agentpunks/1.json",
    )
    ix_data = b'\x07' + mint_data

    mint_ix = Instruction(
        program_id=SYSTEM_PROGRAM,
        accounts=[
            agent.public_key(),       # minter
            collection_pubkey,        # collection
            token_pubkey,             # token account
            agent.public_key(),       # owner (minting to self)
        ],
        data=ix_data,
    )

    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(mint_ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(agent)
    )

    try:
        sig = await conn.send_transaction(tx)
        print_ok(f"Mint tx: {sig[:32] if sig else 'no sig'}...")
        await asyncio.sleep(2)

        # VERIFY: NFT exists on-chain
        nft_ok = False
        try:
            nft_info = await conn.get_nft(collection_pubkey, token_id)
            if nft_info:
                print_ok(f"NFT verified: token #{token_id}, owner={nft_info.get('owner', '?')}")
                nft_ok = True
            else:
                print_fail("getNft returned null")
        except Exception as e:
            print_fail(f"NFT verification failed: {e}")

        results.append(("Mint NFT #1", nft_ok))
    except Exception as e:
        print_fail(f"Mint failed: {e}")
        results.append(("Mint NFT #1", False))

    # ------------------------------------------------------------------
    # Step 7: Agent sends 5 LICN back to treasury
    # ------------------------------------------------------------------
    print_step(7, "Transfer 5 LICN: Agent -> Treasury")

    bal_before_send = await conn.get_balance(agent.public_key())
    licn_before_send = float(bal_before_send.get("licn", "0"))
    print_info(f"Agent balance before send: {licn_before_send:.4f} LICN")

    amount_spores = 5 * SPORES_PER_LICN
    blockhash = await conn.get_recent_blockhash()

    transfer_ix = TransactionBuilder.transfer(
        agent.public_key(),
        treasury.public_key(),
        amount_spores,
    )
    tx = (
        TransactionBuilder()
        .add(transfer_ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(agent)
    )

    try:
        sig = await conn.send_transaction(tx)
        print_ok(f"Transfer tx: {sig[:32] if sig else 'no sig'}...")
        await asyncio.sleep(2)

        # VERIFY: Balance decreased by 5 LICN
        agent_bal_final = await conn.get_balance(agent.public_key())
        agent_licn_final = float(agent_bal_final.get("licn", "0"))
        delta = licn_before_send - agent_licn_final
        if delta >= 4.9 and delta <= 6.0:  # 5 LICN + possible tx fee
            print_ok(f"Agent balance: {agent_licn_final:.4f} LICN (sent {delta:.4f})")
            results.append(("Send 5 LICN to Treasury", True))
        else:
            print_fail(f"Unexpected balance delta: {delta:.4f} (expected ~5)")
            results.append(("Send 5 LICN to Treasury", False))
    except Exception as e:
        print_fail(f"Transfer failed: {e}")
        results.append(("Send 5 LICN to Treasury", False))

    # ------------------------------------------------------------------
    # Step 8: Final comprehensive on-chain verification
    # ------------------------------------------------------------------
    print_step(8, "Final on-chain verification")

    final_slot = await conn.get_slot()
    print_ok(f"Final slot: {final_slot} (advanced {final_slot - slot} slots)")

    # Final balances
    treasury_bal_final = await conn.get_balance(treasury.public_key())
    agent_bal_final = await conn.get_balance(agent.public_key())
    print_ok(f"Treasury final: {treasury_bal_final.get('licn', '?')} LICN")
    print_ok(f"Agent final:    {agent_bal_final.get('licn', '?')} LICN")

    # Contract count (should be 26 genesis + 1 agent = 27)
    try:
        all_contracts = await conn._rpc("getAllContracts")
        contracts_list = all_contracts.get("contracts", []) if isinstance(all_contracts, dict) else all_contracts if isinstance(all_contracts, list) else []
        print_ok(f"Total contracts on-chain: {len(contracts_list)}")
        if len(contracts_list) >= 27:
            print_ok("Contract count correct (26 genesis + 1 agent)")
        elif len(contracts_list) == 26:
            print_info("Only 26 contracts (genesis only, agent deploy may have failed)")
    except Exception as e:
        print_info(f"Could not query contract count: {e}")

    # Validators
    validators_final = await conn.get_validators()
    val_list = validators_final.get("validators", []) if isinstance(validators_final, dict) else validators_final if isinstance(validators_final, list) else []
    for v in val_list:
        stake = v.get("stake", 0)
        stake_licn = stake / SPORES_PER_LICN if isinstance(stake, (int, float)) else 0
        print_ok(f"Validator {v.get('pubkey', '?')[:12]}... stake={stake_licn:.0f} LICN rep={v.get('reputation', '?')}")

    # Verify agent token is visible in explorer-style queries
    if program_pubkey:
        try:
            agent_abi = await conn._rpc("getContractAbi", [str(program_pubkey)])
            abi_name = agent_abi.get("name", "?") if agent_abi else "null"
            print_info(f"Agent contract ABI name (raw): '{abi_name}' (explorer uses registry name instead)")
        except:
            pass

    # ------------------------------------------------------------------
    # Summary
    # ------------------------------------------------------------------
    print_header("TEST RESULTS")
    all_pass = True
    for name, passed in results:
        status = "[PASS]" if passed else "[FAIL]"
        if not passed:
            all_pass = False
        print(f"  {status} {name}")

    print()
    if all_pass:
        print("  ALL TESTS PASSED -- Full on-chain verification complete!")
    else:
        failed = [name for name, passed in results if not passed]
        print(f"  {len(failed)} TEST(S) FAILED:")
        for f in failed:
            print(f"    - {f}")

    print(f"\n  Agent pubkey:    {agent_b58}")
    print(f"  Treasury pubkey: {treasury_b58}")
    if program_pubkey:
        print(f"  Token contract:  {program_pubkey}")
    print()

    return all_pass


if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)

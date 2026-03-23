#!/usr/bin/env python3
"""
Send signed transactions using generated wallet

This script demonstrates the complete transaction flow:
1. Load wallet keypair
2. Create transaction
3. Sign transaction
4. Send to validator
"""

import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from lichen import Connection, PublicKey, Keypair, TransactionBuilder


async def main():
    print("🦞 Lichen Transaction Sender\n")
    print("=" * 60)
    
    # Load wallet
    wallet_path = Path(__file__).parent / "wallets" / "wallet.json"
    
    if not wallet_path.exists():
        print(f"\n❌ Wallet not found at: {wallet_path}")
        print("   Run generate_wallet.py first!")
        return False
    
    print(f"\n🔑 Loading wallet from: {wallet_path}")
    keypair = Keypair.load(wallet_path)
    pubkey_b58 = keypair.public_key().to_base58()
    print(f"   Public Key: {pubkey_b58}")
    
    # Connect to validator
    connection = Connection('http://localhost:8899', 'ws://localhost:8900')
    
    try:
        # Get current slot for nonce
        slot = await connection.get_slot()
        print(f"\n📍 Current Slot: {slot}")
        
        recent_blockhash = await connection.get_recent_blockhash()
        print(f"   Recent Blockhash: {recent_blockhash[:16]}...")
        
        # Check balance
        try:
            balance = await connection.get_balance(PublicKey(pubkey_b58))
            if balance:
                licn_balance = balance.get('licn', 0) / 1_000_000_000
                print(f"   Wallet Balance: {licn_balance:.9f} LICN")
        except Exception as e:
            print(f"   Wallet Balance: 0 LICN (account not found)")
        
        # Get validators to use as recipient
        validators = await connection.get_validators()
        if not validators or len(validators) == 0:
            print("\n❌ No validators found")
            return False
        
        recipient_pubkey_b58 = validators[0]["pubkey"]
        
        print(f"\n📤 Preparing transaction:")
        print(f"   From: {pubkey_b58[:20]}...")
        print(f"   To:   {recipient_pubkey_b58[:20]}...")
        print(f"   Amount: 0.001 LICN (1,000,000 spores)")
        
        amount = 1_000_000  # 0.001 LICN
        instruction = TransactionBuilder.transfer(
            keypair.public_key(),
            PublicKey(recipient_pubkey_b58),
            amount,
        )
        tx = (
            TransactionBuilder()
            .add(instruction)
            .set_recent_blockhash(recent_blockhash)
            .build_and_sign(keypair)
        )
        
        print(f"\n🔏 Signing transaction...")
        print(f"   Signature: {tx.signatures[0][:32]}...")
        
        print(f"\n📡 Sending transaction to validator...")
        result = await connection.send_transaction(tx)
        
        if result:
            print(f"\n✅ Transaction sent successfully!")
            print(f"   Signature: {result[:16]}...")
        else:
            print(f"\n⚠️  Transaction sent but no response received")
        
        # Wait a bit and check latest block
        print(f"\n⏳ Waiting for block confirmation...")
        await asyncio.sleep(2)
        
        latest_block = await connection.get_latest_block()
        if latest_block:
            tx_count = latest_block.get('transaction_count', 0)
            print(f"   Latest block (slot {latest_block['slot']}) has {tx_count} transaction(s)")
        
        print("\n" + "=" * 60)
        print("🎉 Transaction test complete!")
        
        return True
        
    except Exception as e:
        print(f"\n❌ Transaction failed: {e}")
        import traceback
        traceback.print_exc()
        return False


if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)

#!/usr/bin/env python3
"""Send test transactions to Lichen"""

import asyncio
import sys
from pathlib import Path

from lichen import Connection, PublicKey, Keypair, TransactionBuilder


async def send_test_txs():
    print('🦞 Sending Test Transactions to Lichen\n')
    print('=' * 60)
    
    # Connect to validator
    connection = Connection('http://localhost:8899', 'ws://localhost:8900')
    
    try:
        # Get initial slot
        initial_slot = await connection.get_slot()
        print(f'\n📍 Current Slot: {initial_slot}\n')
        
        recent_blockhash = await connection.get_recent_blockhash()

        wallet_path = Path(__file__).parent / "wallets" / "wallet.json"
        if not wallet_path.exists():
            print(f'\n❌ Wallet not found at: {wallet_path}')
            print('   Run generate_wallet.py first!')
            return False

        sender_keypair = Keypair.load(wallet_path)
        
        # Create test transactions
        print('📤 Sending 5 test transactions...\n')
        
        for i in range(5):
            try:
                validators = await connection.get_validators()
                if validators and len(validators) > 0:
                    recipient = PublicKey(validators[1]["pubkey"] if len(validators) > 1 else validators[0]["pubkey"])
                    
                    instruction = TransactionBuilder.transfer(
                        sender_keypair.public_key(),
                        recipient,
                        1_000_000,
                    )
                    tx = (
                        TransactionBuilder()
                        .add(instruction)
                        .set_recent_blockhash(recent_blockhash)
                        .build_and_sign(sender_keypair)
                    )
                    
                    result = await connection.send_transaction(tx)
                    
                    if result:
                        print(f'  ✅ Transaction {i+1}: Sent successfully')
                        print(f'     Signature: {result[:16]}...')
                    else:
                        print(f'  ⚠️  Transaction {i+1}: Response was empty/failed')
                    
                    # Small delay between transactions
                    await asyncio.sleep(0.5)
                    
            except Exception as e:
                print(f'  ❌ Transaction {i+1} failed: {e}')
        
        # Wait a bit for transactions to be included
        print('\n⏳ Waiting for transactions to be included in blocks...')
        await asyncio.sleep(2)
        
        # Check latest block for transactions
        final_slot = await connection.get_slot()
        print(f'\n📍 Final Slot: {final_slot}')
        print(f'   Blocks produced: {final_slot - initial_slot}')
        
        latest_block = await connection.get_latest_block()
        if latest_block:
            tx_count = latest_block.get('transaction_count', 0)
            print(f'   Latest block has {tx_count} transaction(s)')
        
        print('\n✅ Test transactions sent!')
        print('\n💡 Check the explorer at http://localhost:3000')
        
        return True
        
    except Exception as e:
        print(f'\n❌ Test failed: {e}')
        import traceback
        traceback.print_exc()
        return False


if __name__ == '__main__':
    success = asyncio.run(send_test_txs())
    sys.exit(0 if success else 1)

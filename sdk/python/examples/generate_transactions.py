#!/usr/bin/env python3
"""Generate test transactions"""

import asyncio
import sys
sys.path.insert(0, '..')

from lichen import Connection, Keypair, TransactionBuilder, Instruction, PublicKey

async def main():
    print("🐍 Python SDK: Generating transactions...\n")
    
    client = Connection('http://localhost:8899')
    
    # Generate 5 test keypairs
    keypairs = [Keypair.generate() for _ in range(5)]
    
    print("📝 Generated 5 test keypairs:")
    for i, kp in enumerate(keypairs):
        print(f"   {i + 1}: {kp.public_key().to_base58()}")
    print()
    
    # Get recent blockhash
    blockhash = await client.get_recent_blockhash()
    print(f"🔗 Blockhash: {blockhash[:16]}...\n")
    
    # System program
    system_program = PublicKey([0] * 32)
    
    # Generate transactions
    print("📤 Building transactions:")
    transactions = []
    
    for i in range(5):
        sender = keypairs[i]
        recipient = keypairs[(i + 1) % 5]
        
        # Create transfer instruction
        amount = 100 * (i + 1)
        data = bytes([0]) + amount.to_bytes(8, 'little')
        
        instruction = Instruction(
            program_id=system_program,
            accounts=[sender.public_key(), recipient.public_key()],
            data=data
        )
        
        tx = (TransactionBuilder()
            .add_instruction(instruction)
            .recent_blockhash(blockhash)
            .sign(sender))
        
        transactions.append(tx)
        print(f"   ✅ Transaction {i + 1} built")
    
    print(f"\n📊 Summary:")
    print(f"   Transactions built: {len(transactions)}")
    print(f"   Total instructions: {sum(len(tx.message.instructions) for tx in transactions)}")
    
    print("\n✅ Python SDK transaction generation complete!")

asyncio.run(main())

"""Transaction example for MoltChain Python SDK"""

import asyncio
from moltchain import Connection, PublicKey, TransactionBuilder


async def main():
    connection = Connection('http://localhost:8899')
    
    print('🦞 MoltChain Transaction Example\n')
    
    # Create a transfer transaction
    from_pubkey = PublicKey('FromPublicKeyHere...')
    to_pubkey = PublicKey('ToPublicKeyHere...')
    amount = 1_000_000_000  # 1 MOLT
    
    print('Building transaction...')
    
    # Get recent blockhash
    latest_block = await connection.get_latest_block()
    
    # Build transaction
    message = (TransactionBuilder()
        .add(TransactionBuilder.transfer(from_pubkey, to_pubkey, amount))
        .set_recent_blockhash(latest_block['hash'])
        .build())
    
    print(f"Transfer: {amount / 1e9} MOLT")
    print(f"From: {from_pubkey}")
    print(f"To: {to_pubkey}")
    print(f"Blockhash: {latest_block['hash'][:16]}...")
    
    # Note: In a real application, you would sign the transaction here
    # For example: transaction = sign_transaction(message, keypair)
    
    print('\n⚠️  Transaction built but not signed or sent')
    print('In a real app, you would sign with a keypair and send with connection.send_transaction()')


if __name__ == '__main__':
    asyncio.run(main())

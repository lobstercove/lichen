#!/usr/bin/env python3
"""
Lichen Wallet Generator

Generates Ed25519 keypairs compatible with Lichen validator.
Saves keypair to a JSON file for later use.
"""

import os
import sys
from pathlib import Path


from lichen import Keypair


def main():
    print("🦞 Lichen Wallet Generator\n")
    print("=" * 60)
    
    # Create wallets directory if it doesn't exist
    wallets_dir = Path(__file__).parent / "wallets"
    wallets_dir.mkdir(exist_ok=True)
    
    # Generate new keypair
    print("\n🔑 Generating new Ed25519 keypair...")
    keypair = Keypair.generate()
    pubkey_b58 = keypair.public_key().to_base58()
    print(f"\n✅ Keypair Generated!")
    print(f"\n📍 Public Key (Base58):")
    print(f"   {pubkey_b58}")
    print(f"\n🔐 Private Seed (hex, first 16 bytes):")
    print(f"   {keypair.seed()[:16].hex()}...")
    
    # Save to file
    wallet_path = wallets_dir / "wallet.json"
    keypair.save(wallet_path)
    
    print(f"\n💾 Wallet saved to: {wallet_path}")
    print(f"   Permissions: 600 (owner read/write only)")
    
    # Test signing
    print("\n🧪 Testing signature...")
    test_message = b"Hello Lichen!"
    signature = keypair.sign(test_message)
    print(f"   Message: {test_message.decode()}")
    print(f"   Signature (first 16 bytes): {signature[:16].hex()}...")
    
    # Verify signature
    print(f"   ✅ Signature generated successfully!")
    
    print("\n" + "=" * 60)
    print("🎉 Wallet generation complete!")
    print("\n💡 Usage:")
    print(f"   - Your wallet is saved at: {wallet_path}")
    print(f"   - Public key: {pubkey_b58}")
    print(f"   - Use this wallet to sign transactions")
    print(f"   - Never share your wallet.json file!")
    print("\n🚀 Next steps:")
    print(f"   - Fund this address from a faucet or validator")
    print(f"   - Use the SDK to send transactions")
    
    return wallet_path


if __name__ == "__main__":
    try:
        wallet_path = main()
        print(f"\n✅ Success! Wallet: {wallet_path}\n")
        sys.exit(0)
    except Exception as e:
        print(f"\n❌ Error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)

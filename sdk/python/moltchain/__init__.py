"""
MoltChain Python SDK

Official Python SDK for interacting with MoltChain blockchain.
"""

__version__ = "0.1.0"

from .publickey import PublicKey
from .keypair import Keypair
from .connection import Connection
from .transaction import Transaction, TransactionBuilder, Instruction
from .shielded import shield_instruction, unshield_instruction, transfer_instruction

__all__ = [
    "PublicKey",
    "Keypair",
    "Connection", 
    "Transaction",
    "TransactionBuilder",
    "Instruction",
    "shield_instruction",
    "unshield_instruction",
    "transfer_instruction",
]

# Default URLs
DEFAULT_RPC_URL = "http://localhost:8899"
DEFAULT_WS_URL = "ws://localhost:8900"

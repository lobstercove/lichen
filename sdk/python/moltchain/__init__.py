"""
MoltChain Python SDK

Official Python SDK for interacting with MoltChain blockchain.
"""

__version__ = "0.1.0"

from .publickey import PublicKey
from .keypair import Keypair
from .connection import Connection
from .transaction import Transaction, TransactionBuilder, Instruction

__all__ = [
    "PublicKey",
    "Keypair",
    "Connection", 
    "Transaction",
    "TransactionBuilder",
    "Instruction",
]

# Default URLs
DEFAULT_RPC_URL = "http://localhost:8899"
DEFAULT_WS_URL = "ws://localhost:8900"

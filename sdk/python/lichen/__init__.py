"""
Lichen Python SDK

Official Python SDK for interacting with Lichen blockchain.
"""

__version__ = "0.1.0"

from .publickey import PublicKey
from .keypair import Keypair
from .connection import Connection
from .transaction import Transaction, TransactionBuilder, Instruction
from .pq import (
    ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SIGNATURE_BYTES,
    PQ_SCHEME_ML_DSA_65,
    PqPublicKey,
    PqSignature,
)
from .shielded import shield_instruction, unshield_instruction, transfer_instruction

__all__ = [
    "PublicKey",
    "PqPublicKey",
    "PqSignature",
    "PQ_SCHEME_ML_DSA_65",
    "ML_DSA_65_PUBLIC_KEY_BYTES",
    "ML_DSA_65_SIGNATURE_BYTES",
    "Keypair",
    "Connection", 
    "Transaction",
    "TransactionBuilder",
    "Instruction",
    "shield_instruction",
    "unshield_instruction",
    "transfer_instruction",
]

# Default URLs (override with LICHEN_RPC_URL / LICHEN_WS_URL env vars)
import os as _os
DEFAULT_RPC_URL = _os.environ.get("LICHEN_RPC_URL", "http://localhost:8899")
DEFAULT_WS_URL = _os.environ.get("LICHEN_WS_URL", "ws://localhost:8900")

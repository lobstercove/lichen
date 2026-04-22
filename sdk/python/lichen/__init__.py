"""
Lichen Python SDK

Official Python SDK for interacting with Lichen blockchain.
"""

__version__ = "1.0.0"

from .publickey import PublicKey
from .keypair import Keypair
from .connection import Connection
from .lichenswap import (
    AddLiquidityParams,
    CreatePoolParams,
    LichenSwapClient,
    SwapParams,
    SwapWithDeadlineParams,
)
from .thalllend import LiquidateParams, ThallLendClient
from .lichenid import (
    AddSkillParams,
    ApproveRecoveryParams,
    AttestSkillParams,
    BidNameAuctionParams,
    CreateNameAuctionParams,
    ExecuteRecoveryParams,
    FinalizeNameAuctionParams,
    LichenIdClient,
    LICHEN_ID_DELEGATE_PERMISSIONS,
    RegisterIdentityParams,
    RegisterNameParams,
    RevokeAttestationParams,
    SetAvailabilityParams,
    SetAvailabilityAsParams,
    SetDelegateParams,
    SetEndpointParams,
    SetEndpointAsParams,
    SetMetadataParams,
    SetMetadataAsParams,
    SetRateParams,
    SetRateAsParams,
    SetRecoveryGuardiansParams,
    UpdateAgentTypeAsParams,
    estimate_lichenid_name_registration_cost,
)
from .sporepay import (
    CreateStreamParams,
    CreateStreamWithCliffParams,
    SporePayClient,
    TransferStreamParams,
    WithdrawFromStreamParams,
)
from .sporevault import SporeVaultClient
from .bountyboard import (
    BountyBoardClient,
    BOUNTY_STATUS_CANCELLED,
    BOUNTY_STATUS_COMPLETED,
    BOUNTY_STATUS_OPEN,
)
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
    "AddLiquidityParams",
    "AddSkillParams",
    "ApproveRecoveryParams",
    "AttestSkillParams",
    "BidNameAuctionParams",
    "CreatePoolParams",
    "CreateNameAuctionParams",
    "CreateStreamParams",
    "CreateStreamWithCliffParams",
    "ExecuteRecoveryParams",
    "FinalizeNameAuctionParams",
    "LichenSwapClient",
    "LichenIdClient",
    "LICHEN_ID_DELEGATE_PERMISSIONS",
    "LiquidateParams",
    "RegisterIdentityParams",
    "RegisterNameParams",
    "RevokeAttestationParams",
    "SetAvailabilityParams",
    "SetAvailabilityAsParams",
    "SetDelegateParams",
    "SetEndpointParams",
    "SetEndpointAsParams",
    "SetMetadataParams",
    "SetMetadataAsParams",
    "SetRateParams",
    "SetRateAsParams",
    "SetRecoveryGuardiansParams",
    "SporePayClient",
    "SporeVaultClient",
    "BountyBoardClient",
    "BOUNTY_STATUS_CANCELLED",
    "BOUNTY_STATUS_COMPLETED",
    "BOUNTY_STATUS_OPEN",
    "SwapParams",
    "SwapWithDeadlineParams",
    "ThallLendClient",
    "TransferStreamParams",
    "UpdateAgentTypeAsParams",
    "WithdrawFromStreamParams",
    "estimate_lichenid_name_registration_cost",
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

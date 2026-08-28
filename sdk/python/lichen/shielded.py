"""
Lichen Shielded Transaction Helpers

Build legacy shield (type 23), unshield (type 24), and transfer (type 25)
instruction wire formats for compatibility tests and historical tooling.

Proof scheme 0x01 is disabled because its verifier did not constrain private
witnesses. Validators reject instructions built by these helpers. Proof
generation is not performed here; the caller supplies legacy payload bytes.
"""

import struct
from typing import List

from .publickey import PublicKey
from .transaction import Instruction

# System program ID (all zeros)
SYSTEM_PROGRAM_ID = PublicKey(b'\x00' * 32)


def shield_instruction(
    sender: PublicKey,
    amount: int,
    commitment: bytes,
    proof: bytes,
) -> Instruction:
    """
    Encode a legacy shield (type 23) instruction for compatibility tooling.

    Validators reject this instruction while proof scheme 0x01 remains disabled.
    The historical format represented a deposit of ``amount`` spores from
    ``sender`` and a ``commitment`` (Poseidon(value, blinding)).

    Data layout (variable length):
        [0]     = 23  (type tag)
        [1..9]  = amount  (u64 LE)
        [9..41] = commitment  (32 bytes)
        [41..]  = proof  (variable-length Plonky3 STARK proof)

    Args:
        sender: The public key whose balance is debited.
        amount: Amount in spores (1 LICN = 1_000_000_000 spores).
        commitment: 32-byte Poseidon commitment.
        proof: Variable-length Plonky3 STARK proof bytes.

    Returns:
        A legacy ``Instruction`` wire object. Submitting it is expected to fail.
    """
    if len(commitment) != 32:
        raise ValueError(f"commitment must be 32 bytes, got {len(commitment)}")

    data = bytes([23]) + struct.pack("<Q", amount) + commitment + proof
    return Instruction(
        program_id=SYSTEM_PROGRAM_ID,
        accounts=[sender],
        data=data,
    )


def unshield_instruction(
    recipient: PublicKey,
    amount: int,
    nullifier: bytes,
    merkle_root: bytes,
    recipient_hash: bytes,
    proof: bytes,
) -> Instruction:
    """
    Encode a legacy unshield (type 24) instruction for compatibility tooling.

    Validators reject this instruction while proof scheme 0x01 remains disabled.

    Data layout (variable length):
        [0]       = 24  (type tag)
        [1..9]    = amount  (u64 LE)
        [9..41]   = nullifier  (32 bytes)
        [41..73]  = merkle_root  (32 bytes)
        [73..105] = recipient_hash  (Poseidon(Fr(pubkey), 0))
        [105..]   = proof  (variable-length Plonky3 STARK proof)

    Args:
        recipient: Public key credited.
        amount: Amount in spores.
        nullifier: 32-byte nullifier (Poseidon(serial, spending_key)).
        merkle_root: 32-byte current Merkle root from the pool state.
        recipient_hash: 32-byte Poseidon(Fr(recipient_pubkey), 0).
        proof: Variable-length Plonky3 STARK proof bytes.
    """
    if len(nullifier) != 32:
        raise ValueError(f"nullifier must be 32 bytes, got {len(nullifier)}")
    if len(merkle_root) != 32:
        raise ValueError(f"merkle_root must be 32 bytes, got {len(merkle_root)}")
    if len(recipient_hash) != 32:
        raise ValueError(f"recipient_hash must be 32 bytes, got {len(recipient_hash)}")

    data = (
        bytes([24])
        + struct.pack("<Q", amount)
        + nullifier
        + merkle_root
        + recipient_hash
        + proof
    )
    return Instruction(
        program_id=SYSTEM_PROGRAM_ID,
        accounts=[recipient],
        data=data,
    )


def transfer_instruction(
    fee_payer: PublicKey,
    nullifiers: List[bytes],
    output_commitments: List[bytes],
    merkle_root: bytes,
    proof: bytes,
) -> Instruction:
    """
    Encode a legacy shielded transfer (type 25) for compatibility tooling.

    Validators reject this instruction while proof scheme 0x01 remains disabled.
    The historical format represented a 2-input, 2-output pool transfer.

    Data layout (variable length):
        [0]       = 25  (type tag)
        [1..33]   = nullifier_1      (32 bytes)
        [33..65]  = nullifier_2      (32 bytes)
        [65..97]  = output_commit_1  (32 bytes)
        [97..129] = output_commit_2  (32 bytes)
        [129..161]= merkle_root      (32 bytes)
        [161..]   = proof            (variable-length Plonky3 STARK proof)

    The ``fee_payer`` account is included so the runtime can deduct
    the transaction fee (no shielded-balance change for that account).

    Args:
        fee_payer: Public key that pays the transaction fee.
        nullifiers: Two 32-byte nullifiers (input notes consumed).
        output_commitments: Two 32-byte output commitments (new notes).
        merkle_root: 32-byte current Merkle root from the pool state.
        proof: Variable-length Plonky3 STARK proof bytes.
    """
    if len(nullifiers) != 2:
        raise ValueError(f"exactly 2 nullifiers required, got {len(nullifiers)}")
    if len(output_commitments) != 2:
        raise ValueError(f"exactly 2 output commitments required, got {len(output_commitments)}")
    for i, n in enumerate(nullifiers):
        if len(n) != 32:
            raise ValueError(f"nullifier[{i}] must be 32 bytes")
    for i, c in enumerate(output_commitments):
        if len(c) != 32:
            raise ValueError(f"output_commitment[{i}] must be 32 bytes")
    if len(merkle_root) != 32:
        raise ValueError(f"merkle_root must be 32 bytes")

    data = (
        bytes([25])
        + nullifiers[0]
        + nullifiers[1]
        + output_commitments[0]
        + output_commitments[1]
        + merkle_root
        + proof
    )
    return Instruction(
        program_id=SYSTEM_PROGRAM_ID,
        accounts=[fee_payer],
        data=data,
    )

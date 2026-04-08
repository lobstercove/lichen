"""K4-02: Cross-SDK compatibility test.

Validates Python SDK bincode encoding matches Rust golden vectors exactly.
The authoritative hex values come from core/src/transaction.rs
test_cross_sdk_message_golden_vector and test_cross_sdk_transaction_golden_vector.
"""

import sys
import os
import hashlib

# Add parent directory so we can import lichen package
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from lichen.publickey import PublicKey
from lichen.bincode import encode_message, encode_transaction, EncodedInstruction
from lichen.pq import PqPublicKey, PqSignature
from lichen.transaction import TransactionBuilder


# --- Deterministic test data (same as Rust golden vector tests) ---
program_id = PublicKey(bytes([0x01] * 32))
account0   = PublicKey(bytes([0x02] * 32))
data       = bytes([0x00, 0x01, 0x02, 0x03])
blockhash  = "aa" * 32   # 32 bytes as hex string
signature  = PqSignature(
    scheme_version=0x01,
    public_key=PqPublicKey(0x01, bytes([0xBB]) * 1952),
    sig=bytes([0xBB]) * 3309,
)

ix = EncodedInstruction(program_id=program_id, accounts=[account0], data=data)


def test_message_golden_vector():
    """Message encoding must match Rust bincode output exactly."""
    msg_bytes = encode_message([ix], blockhash)
    got = msg_bytes.hex()

    # Authoritative value from Rust test_cross_sdk_message_golden_vector
    expected = (
        "0100000000000000"                                            # Vec<Ix> len = 1
        "0101010101010101010101010101010101010101010101010101010101010101"  # program_id
        "0100000000000000"                                            # Vec<Pubkey> len = 1
        "0202020202020202020202020202020202020202020202020202020202020202"  # accounts[0]
        "040000000000000000010203"                                    # Vec<u8> len=4 + data
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"  # blockhash
        "0000"                                                        # compute_budget: None + compute_unit_price: None
    )

    assert got == expected, (
        f"K4-02 PYTHON MESSAGE GOLDEN VECTOR MISMATCH!\n"
        f"Got:      {got}\n"
        f"Expected: {expected}"
    )
    print("  ✓ Message golden vector matches Rust")


def test_transaction_golden_vector():
    """Transaction encoding must match Rust bincode output length and hash."""
    msg_bytes = encode_message([ix], blockhash)
    tx_bytes = encode_transaction([signature], msg_bytes)
    tx_hash = hashlib.sha256(tx_bytes).hexdigest()

    assert len(tx_bytes) == 5417, (
        f"K4-02 PYTHON TX LENGTH MISMATCH!\n"
        f"Got:      {len(tx_bytes)}\n"
        f"Expected: 5417"
    )
    assert tx_hash == "9d0eec7b657276b828c265995ce78b41a3e19b17ab354b11f37254bbc4ee2a91", (
        f"K4-02 PYTHON TX HASH MISMATCH!\n"
        f"Got:      {tx_hash}\n"
        f"Expected: 9d0eec7b657276b828c265995ce78b41a3e19b17ab354b11f37254bbc4ee2a91"
    )
    print("  ✓ Transaction golden vector matches Rust length + hash")


def test_message_optional_compute_fields():
    """Optional compute fields must encode as bincode Option<u64>."""
    msg_bytes = encode_message([ix], blockhash, compute_budget=500_000, compute_unit_price=250)
    got = msg_bytes.hex()
    expected_suffix = (
        "01"                                                        # compute_budget: Some
        "20a1070000000000"                                          # 500_000 LE
        "01"                                                        # compute_unit_price: Some
        "fa00000000000000"                                          # 250 LE
    )

    assert got.endswith(expected_suffix), (
        f"K4-02 PYTHON COMPUTE OPTION MISMATCH!\n"
        f"Got suffix:      {got[-len(expected_suffix):]}\n"
        f"Expected suffix: {expected_suffix}"
    )
    print("  ✓ Optional compute fields encode correctly")


def test_transaction_builder_sets_compute_fields():
    """TransactionBuilder should preserve explicit compute settings on the message."""
    message = (
        TransactionBuilder()
        .add(TransactionBuilder.transfer(account0, program_id, 1))
        .set_recent_blockhash(blockhash)
        .set_compute_budget(500_000)
        .set_compute_unit_price(250)
        .build()
    )

    assert message.compute_budget == 500_000
    assert message.compute_unit_price == 250
    print("  ✓ TransactionBuilder preserves compute settings")


if __name__ == "__main__":
    test_message_golden_vector()
    test_transaction_golden_vector()
    test_message_optional_compute_fields()
    test_transaction_builder_sets_compute_fields()
    print("K4-02: All Python cross-SDK compatibility tests passed")

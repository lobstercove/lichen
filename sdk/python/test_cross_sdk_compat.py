"""K4-02: Cross-SDK compatibility test.

Validates Python SDK bincode encoding matches Rust golden vectors exactly.
The authoritative hex values come from core/src/transaction.rs
test_cross_sdk_message_golden_vector and test_cross_sdk_transaction_golden_vector.
"""

import sys
import os

# Add parent directory so we can import lichen package
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from lichen.publickey import PublicKey
from lichen.bincode import encode_message, encode_transaction, EncodedInstruction


# --- Deterministic test data (same as Rust golden vector tests) ---
program_id = PublicKey(bytes([0x01] * 32))
account0   = PublicKey(bytes([0x02] * 32))
data       = bytes([0x00, 0x01, 0x02, 0x03])
blockhash  = "aa" * 32   # 32 bytes as hex string
sig_hex    = "bb" * 64   # 64 bytes as hex string

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
    """Transaction encoding must match Rust bincode output exactly."""
    msg_bytes = encode_message([ix], blockhash)
    tx_bytes = encode_transaction([sig_hex], msg_bytes)
    got = tx_bytes.hex()

    sig_expected = "bb" * 64  # 64 bytes
    expected = (
        "0100000000000000"                                            # Vec<[u8;64]> len = 1
        + sig_expected                                                # sig
        + "0100000000000000"                                          # Vec<Ix> len = 1
        "0101010101010101010101010101010101010101010101010101010101010101"  # program_id
        "0100000000000000"                                            # Vec<Pubkey> len = 1
        "0202020202020202020202020202020202020202020202020202020202020202"  # accounts[0]
        "040000000000000000010203"                                    # Vec<u8> len=4 + data
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"  # blockhash
        "0000"                                                        # compute_budget: None + compute_unit_price: None
        "00000000"                                                    # tx_type: Native (u32 LE)
    )

    assert got == expected, (
        f"K4-02 PYTHON TX GOLDEN VECTOR MISMATCH!\n"
        f"Got:      {got}\n"
        f"Expected: {expected}"
    )
    print("  ✓ Transaction golden vector matches Rust")


if __name__ == "__main__":
    test_message_golden_vector()
    test_transaction_golden_vector()
    print("K4-02: All Python cross-SDK compatibility tests passed")

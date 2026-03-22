"""Tests for bincode encoding — verifies signature format matches Rust bincode Vec<[u8; 64]>."""

import struct
from moltchain.bincode import encode_transaction, _encode_u64

def test_encode_transaction_signature_format():
    """Signatures must be encoded as raw 64-byte arrays (no per-element length prefix)."""
    # Create a fake 64-byte signature as hex
    sig_bytes = bytes(range(64))
    sig_hex = sig_bytes.hex()

    # Fake message bytes (just a placeholder)
    message_bytes = b"\x00" * 40

    result = encode_transaction([sig_hex], message_bytes)

    # Expected layout: u64(1) + 64 raw sig bytes + 40 message bytes + u32(tx_type)
    # Total: 8 + 64 + 40 + 4 = 116 bytes
    assert len(result) == 116, f"Expected 116 bytes, got {len(result)}"

    # First 8 bytes: vector length = 1 (little-endian u64)
    vec_len = struct.unpack("<Q", result[:8])[0]
    assert vec_len == 1, f"Expected vec len 1, got {vec_len}"

    # Next 64 bytes: raw signature (no length prefix)
    assert result[8:72] == sig_bytes, "Signature bytes mismatch"

    # Next: message bytes
    assert result[72:112] == message_bytes, "Message bytes mismatch"

    # Last 4 bytes: tx_type = 0 (Native) as u32 LE
    assert result[112:] == b"\x00\x00\x00\x00", "tx_type mismatch"


def test_encode_transaction_rejects_wrong_signature_length():
    """Signatures that aren't 64 bytes should raise ValueError."""
    short_sig = ("ab" * 32)  # 32 bytes, not 64
    try:
        encode_transaction([short_sig], b"\x00")
        assert False, "Should have raised ValueError"
    except ValueError as e:
        assert "64 bytes" in str(e)


def test_encode_transaction_multiple_signatures():
    """Multiple signatures are packed sequentially (no per-element length prefix)."""
    sig1 = bytes(range(64)).hex()
    sig2 = bytes(range(64, 128)).hex()
    message = b"\xff" * 10

    result = encode_transaction([sig1, sig2], message)

    # Layout: u64(2) + 64 + 64 + 10 + u32(tx_type) = 150 bytes
    assert len(result) == 150
    vec_len = struct.unpack("<Q", result[:8])[0]
    assert vec_len == 2


if __name__ == "__main__":
    test_encode_transaction_signature_format()
    test_encode_transaction_rejects_wrong_signature_length()
    test_encode_transaction_multiple_signatures()
    print("All Python bincode tests passed!")

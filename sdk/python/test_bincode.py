"""Tests for bincode encoding — verifies PQ signature format matches Rust bincode Vec<PqSignature>."""

import struct
from lichen.bincode import encode_transaction_wire
from lichen.pq import PqPublicKey, PqSignature


def _fixture_signature(fill: int) -> PqSignature:
    return PqSignature(
        scheme_version=0x01,
        public_key=PqPublicKey(0x01, bytes([fill]) * 1952),
        sig=bytes([fill]) * 3309,
    )

def test_encode_transaction_signature_format():
    """Signatures must be encoded as PQ objects with public key and blob lengths."""
    signature = _fixture_signature(0xBB)

    message_bytes = b"\x00" * 40

    result = encode_transaction_wire([signature], message_bytes)

    # V1 envelope + u64(1) + encoded PQ signature + message + payload tx_type.
    assert len(result) == 5335, f"Expected 5335 bytes, got {len(result)}"
    assert result[:4] == b"MT\x01\x00"

    vec_len = struct.unpack("<Q", result[4:12])[0]
    assert vec_len == 1, f"Expected vec len 1, got {vec_len}"

    assert result[12] == 0x01
    assert result[13] == 0x01
    assert struct.unpack("<Q", result[14:22])[0] == 1952
    assert struct.unpack("<Q", result[1974:1982])[0] == 3309
    assert result[-4:] == b"\x00\x00\x00\x00", "tx_type mismatch"


def test_encode_transaction_rejects_wrong_signature_length():
    """Signatures that aren't the full ML-DSA-65 size should raise ValueError."""
    try:
        encode_transaction_wire(
            [
                PqSignature(
                    scheme_version=0x01,
                    public_key=PqPublicKey(0x01, bytes([0xAA]) * 1952),
                    sig=b"\xAB" * 64,
                )
            ],
            b"\x00",
        )
        assert False, "Should have raised ValueError"
    except ValueError as e:
        assert "3309" in str(e)


def test_encode_transaction_multiple_signatures():
    """Multiple PQ signatures are packed sequentially."""
    sig1 = _fixture_signature(0xBB)
    sig2 = _fixture_signature(0xAA)
    message = b"\xff" * 10

    result = encode_transaction_wire([sig1, sig2], message)

    # Layout: u64(2) + 5279 + 5279 + 10 + u32(tx_type) = 10580 bytes
    assert len(result) == 10584
    vec_len = struct.unpack("<Q", result[4:12])[0]
    assert vec_len == 2


if __name__ == "__main__":
    test_encode_transaction_signature_format()
    test_encode_transaction_rejects_wrong_signature_length()
    test_encode_transaction_multiple_signatures()
    print("All Python bincode tests passed!")

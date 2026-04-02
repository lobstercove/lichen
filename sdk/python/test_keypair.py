"""Keypair smoke tests for the native PQ Python SDK surface."""

from lichen import (
    Keypair,
    ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SIGNATURE_BYTES,
    PQ_SCHEME_ML_DSA_65,
)
from lichen.pq import PqSignature


def test_keypair_sign_and_verify_roundtrip():
    seed = bytes(range(32))
    keypair = Keypair.from_seed(seed)
    twin = Keypair.from_seed(seed)

    public_key = keypair.public_key()
    assert public_key.scheme_version == PQ_SCHEME_ML_DSA_65
    assert len(public_key.bytes) == ML_DSA_65_PUBLIC_KEY_BYTES
    assert public_key.bytes == twin.public_key().bytes
    assert keypair.pubkey().to_bytes() == twin.pubkey().to_bytes()

    message = b"hello pq"
    signature = keypair.sign(message)

    assert isinstance(signature, PqSignature)
    assert signature.scheme_version == PQ_SCHEME_ML_DSA_65
    assert len(signature.sig) == ML_DSA_65_SIGNATURE_BYTES
    assert Keypair.verify(keypair.pubkey(), message, signature)
    assert not Keypair.verify(Keypair.generate().pubkey(), message, signature)


if __name__ == "__main__":
    test_keypair_sign_and_verify_roundtrip()
    print("Python keypair PQ tests passed!")
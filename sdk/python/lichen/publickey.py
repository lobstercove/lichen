"""PublicKey utilities for Lichen"""

import base58
from typing import Union


class PublicKey:
    """A 32-byte public key"""
    
    def __init__(self, value: Union[str, bytes, list]):
        """
        Create a PublicKey from base58 string, bytes, or list
        
        Args:
            value: Base58 string, bytes, or list of integers
        """
        if isinstance(value, str):
            # Decode from base58
            self._bytes = base58.b58decode(value)
        elif isinstance(value, list):
            self._bytes = bytes(value)
        else:
            self._bytes = value
            
        if len(self._bytes) != 32:
            raise ValueError(f"Invalid public key length: {len(self._bytes)}, expected 32")
    
    def to_base58(self) -> str:
        """Convert to base58 string"""
        return base58.b58encode(self._bytes).decode('ascii')
    
    def to_bytes(self) -> bytes:
        """Convert to bytes"""
        return self._bytes
    
    def __str__(self) -> str:
        """String representation (base58)"""
        return self.to_base58()
    
    def __repr__(self) -> str:
        """Developer representation"""
        return f"PublicKey('{self.to_base58()}')"
    
    def __eq__(self, other) -> bool:
        """Check equality"""
        if not isinstance(other, PublicKey):
            return False
        return self._bytes == other._bytes
    
    def __hash__(self) -> int:
        """Hash for use in sets and dicts"""
        return hash(self._bytes)
    
    @classmethod
    def from_base58(cls, s: str) -> 'PublicKey':
        """Create from base58 string"""
        return cls(s)
    
    @classmethod
    def from_bytes(cls, b: bytes) -> 'PublicKey':
        """Create from bytes"""
        return cls(b)
    
    @classmethod
    def new_unique(cls) -> 'PublicKey':
        """Create a unique public key (for testing)"""
        import secrets
        return cls(secrets.token_bytes(32))

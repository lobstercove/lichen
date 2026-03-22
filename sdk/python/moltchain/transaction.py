"""Transaction types and builder for MoltChain"""

from typing import List, Optional
from dataclasses import dataclass
import binascii

from .bincode import EncodedInstruction, encode_message, encode_transaction
from .keypair import Keypair
from .publickey import PublicKey


@dataclass
class Instruction:
    """Transaction instruction"""
    program_id: PublicKey
    accounts: List[PublicKey]
    data: bytes


@dataclass
class Message:
    """Transaction message (before signing)"""
    instructions: List[Instruction]
    recent_blockhash: str
    compute_budget: Optional[int] = None
    compute_unit_price: Optional[int] = None


@dataclass
class Transaction:
    """Signed transaction"""
    signatures: List[str]
    message: Message


class TransactionBuilder:
    """Build transactions with a fluent interface"""
    
    def __init__(self):
        self._instructions: List[Instruction] = []
        self._recent_blockhash: Optional[str] = None
    
    def add(self, instruction: Instruction) -> 'TransactionBuilder':
        """Add an instruction"""
        self._instructions.append(instruction)
        return self
    
    def set_recent_blockhash(self, blockhash: str) -> 'TransactionBuilder':
        """Set recent blockhash"""
        self._recent_blockhash = blockhash
        return self
    
    def build(self) -> Message:
        """Build the message (ready for signing)"""
        if not self._recent_blockhash:
            raise ValueError("Recent blockhash not set")
        if not self._instructions:
            raise ValueError("No instructions added")
        
        return Message(
            instructions=self._instructions,
            recent_blockhash=self._recent_blockhash
        )

    def build_and_sign(self, keypair: Keypair) -> Transaction:
        message = self.build()
        encoded_instructions = [
            EncodedInstruction(ix.program_id, ix.accounts, ix.data)
            for ix in message.instructions
        ]
        message_bytes = encode_message(
            encoded_instructions,
            message.recent_blockhash,
            message.compute_budget,
            message.compute_unit_price,
        )
        signature = keypair.sign(message_bytes)
        sig_hex = binascii.hexlify(signature).decode("ascii")
        return Transaction(signatures=[sig_hex], message=message)

    @staticmethod
    def message_to_bincode(message: Message) -> bytes:
        encoded_instructions = [
            EncodedInstruction(ix.program_id, ix.accounts, ix.data)
            for ix in message.instructions
        ]
        return encode_message(
            encoded_instructions,
            message.recent_blockhash,
            message.compute_budget,
            message.compute_unit_price,
        )

    @staticmethod
    def transaction_to_bincode(transaction: Transaction) -> bytes:
        message_bytes = TransactionBuilder.message_to_bincode(transaction.message)
        return encode_transaction(transaction.signatures, message_bytes)
    
    @staticmethod
    def transfer(from_pubkey: PublicKey, to_pubkey: PublicKey, amount: int) -> Instruction:
        """
        Create a transfer instruction
        
        Args:
            from_pubkey: Source account
            to_pubkey: Destination account
            amount: Amount in shells (1 MOLT = 1_000_000_000 shells)
        """
        # Encode transfer data (instruction type + 8 bytes amount)
        data = b"\x00" + amount.to_bytes(8, byteorder='little')
        
        # System program ID (all 1s)
        system_program = PublicKey(b'\x00' * 32)
        
        return Instruction(
            program_id=system_program,
            accounts=[from_pubkey, to_pubkey],
            data=data
        )

    @staticmethod
    def stake(from_pubkey: PublicKey, validator: PublicKey, amount: int) -> Instruction:
        """Create a stake instruction"""
        data = b"\x09" + amount.to_bytes(8, byteorder='little')
        system_program = PublicKey(b'\x00' * 32)
        return Instruction(
            program_id=system_program,
            accounts=[from_pubkey, validator],
            data=data
        )

    @staticmethod
    def unstake(from_pubkey: PublicKey, validator: PublicKey, amount: int) -> Instruction:
        """Create an unstake request instruction"""
        data = b"\x0a" + amount.to_bytes(8, byteorder='little')
        system_program = PublicKey(b'\x00' * 32)
        return Instruction(
            program_id=system_program,
            accounts=[from_pubkey, validator],
            data=data
        )

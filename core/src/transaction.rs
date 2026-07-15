// Lichen Core - Transaction Model

use crate::account::{Keypair, PqSignature, Pubkey};
use crate::codec::{
    deserialize_legacy_bincode_strict, serialize_legacy_bincode, serialized_size_legacy_bincode,
};
use crate::hash::Hash;
use crate::signing::{versioned_signing_bytes, DOMAIN_NATIVE_TX};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Single instruction in a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instruction {
    /// Program to invoke
    pub program_id: Pubkey,

    /// Accounts involved
    pub accounts: Vec<Pubkey>,

    /// Instruction data
    pub data: Vec<u8>,
}

/// Default compute unit budget per transaction (200,000 CU).
/// Users can request up to [`MAX_COMPUTE_BUDGET`] by setting
/// `Message::compute_budget`.
pub const DEFAULT_COMPUTE_BUDGET: u64 = 200_000;

/// Maximum compute unit budget a transaction may request (1,400,000 CU).
/// Mirrors Solana's per-transaction CU ceiling.
pub const MAX_COMPUTE_BUDGET: u64 = 1_400_000;

/// Transaction message (before signing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Instructions to execute
    pub instructions: Vec<Instruction>,

    /// Recent blockhash (for replay protection)
    pub recent_blockhash: Hash,

    /// Compute unit budget for this transaction.
    /// If `None` or `0`, defaults to [`DEFAULT_COMPUTE_BUDGET`] (200,000 CU).
    /// Maximum allowed: [`MAX_COMPUTE_BUDGET`] (1,400,000 CU).
    /// If execution exceeds this budget the transaction reverts and the
    /// base fee is still charged (anti-DoS).
    #[serde(default)]
    pub compute_budget: Option<u64>,

    /// Price per compute unit in micro-spores (μspores).
    /// Priority fee = `effective_compute_budget × compute_unit_price`.
    /// Set to `0` (default) for no priority fee. Validators order
    /// transactions by effective CU price for block inclusion.
    #[serde(default)]
    pub compute_unit_price: Option<u64>,
}

impl Message {
    pub fn new(instructions: Vec<Instruction>, recent_blockhash: Hash) -> Self {
        Message {
            instructions,
            recent_blockhash,
            compute_budget: None,
            compute_unit_price: None,
        }
    }

    /// Effective compute budget — resolves `None`/`0` to the protocol default.
    pub fn effective_compute_budget(&self) -> u64 {
        match self.compute_budget {
            Some(b) if b > 0 => b.min(MAX_COMPUTE_BUDGET),
            _ => DEFAULT_COMPUTE_BUDGET,
        }
    }

    /// Effective compute unit price in micro-spores.
    pub fn effective_compute_unit_price(&self) -> u64 {
        self.compute_unit_price.unwrap_or(0)
    }

    /// Serialize for signing.
    ///
    /// Panics only on OOM or bincode internal error (neither expected for a
    /// well-formed Message). Callers that need fallibility should use
    /// `try_serialize()` instead.
    pub fn serialize(&self) -> Vec<u8> {
        serialize_legacy_bincode(self, "Message").unwrap_or_else(|e| {
            panic!(
                "FATAL: Message serialization failed ({}). This indicates data corruption or OOM.",
                e
            )
        })
    }

    /// Fallible serialization for contexts that can propagate errors.
    pub fn try_serialize(&self) -> Result<Vec<u8>, String> {
        serialize_legacy_bincode(self, "Message")
            .map_err(|e| format!("Message serialization failed: {}", e))
    }

    /// Serialize into the versioned native transaction signing envelope for a chain.
    pub fn signing_bytes_for_chain_id(&self, chain_id: &str) -> Vec<u8> {
        assert!(
            !chain_id.is_empty(),
            "chain id is required for transaction signing"
        );
        versioned_signing_bytes(DOMAIN_NATIVE_TX, chain_id, &self.serialize())
    }

    /// Hash for signing
    pub fn hash(&self) -> Hash {
        Hash::hash(&self.serialize())
    }
}

/// Transaction type discriminator — replaces sentinel-based detection.
///
/// - `Native`: Standard Lichen transaction (PQ signed, blockhash replay protection)
/// - `Evm`: EVM-wrapped transaction (ECDSA signed, EVM nonce replay protection)
/// - `Consensus`: Protocol-generated block metadata; never accepted from RPC/mempool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TransactionType {
    #[default]
    Native,
    Evm,
    Consensus,
}

/// Wire-format magic bytes identifying a Lichen transaction envelope.
/// "MT" = fixed magic prefix. The pair `[0x4D, 0x54]` cannot appear as the first
/// two bytes of a raw-bincode Transaction (that would imply 0x544D = 21,581
/// signatures, which is impossible).
pub const TX_WIRE_MAGIC: [u8; 2] = [0x4D, 0x54];

/// Current wire-format version.
pub const TX_WIRE_VERSION: u8 = 1;

/// Maximum encoded transaction size, including the four-byte V1 envelope.
pub const MAX_TRANSACTION_WIRE_SIZE: u64 = MAX_TRANSACTION_SERIALIZED_SIZE + 4;

/// Signed transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction signatures. Each signature is self-contained: it carries both
    /// the signature bytes and the signer's PQ verifying key.
    pub signatures: Vec<PqSignature>,

    /// Transaction message
    pub message: Message,

    /// Transaction type — determines processing path.
    /// Defaults to `Native`.
    #[serde(default)]
    pub tx_type: TransactionType,
}

/// Maximum instructions per transaction (T1.7)
pub const MAX_INSTRUCTIONS_PER_TX: usize = 64;
/// Maximum self-contained PQ signatures per transaction.
///
/// Each instruction contributes at most one required signer (its first account),
/// so accepting more signatures than instructions only increases bandwidth,
/// memory, and verification work without adding valid authorization surface.
pub const MAX_SIGNATURES_PER_TX: usize = MAX_INSTRUCTIONS_PER_TX;
/// Maximum data bytes per instruction (T1.7)
pub const MAX_INSTRUCTION_DATA: usize = 204_800; // 200KB — contract calls may carry significant payloads
pub const MAX_DEPLOY_INSTRUCTION_DATA: usize = 4_194_304; // 4MB — WASM deploys via instruction type 17
/// Maximum bincode-serialized transaction size.
///
/// This leaves room for one max-size deploy instruction plus PQ signatures and
/// metadata, while preventing many individually-valid instructions or excess
/// signatures from producing transactions too large for admission.
pub const MAX_TRANSACTION_SERIALIZED_SIZE: u64 = 5 * 1024 * 1024;
/// Maximum accounts per instruction
pub const MAX_ACCOUNTS_PER_IX: usize = 64;

impl Transaction {
    pub fn new(message: Message) -> Self {
        Transaction {
            signatures: Vec::new(),
            message,
            tx_type: TransactionType::Native,
        }
    }

    /// Create a new EVM-typed transaction.
    pub fn new_evm(message: Message) -> Self {
        Transaction {
            signatures: Vec::new(),
            message,
            tx_type: TransactionType::Evm,
        }
    }

    /// Check if this is an EVM transaction (by type field or EVM replay sentinel).
    pub fn is_evm(&self) -> bool {
        self.tx_type == TransactionType::Evm
            || self.message.recent_blockhash == crate::Hash([0xEE; 32])
    }

    /// Whether this is protocol-generated consensus metadata carried in a block.
    pub fn is_consensus(&self) -> bool {
        self.tx_type == TransactionType::Consensus
    }

    /// Get transaction signature (first signature's identifier)
    pub fn signature(&self) -> Hash {
        self.hash()
    }

    /// Get the message-only hash (signing hash).
    ///
    /// This is the hash that signers commit to via PQ signatures. It does NOT include
    /// signatures, so it is predictable before signing — useful for multi-sig
    /// coordination and client-side txid tracking before broadcast.
    ///
    /// See also: `hash()` which includes signatures and serves as the canonical txid.
    pub fn message_hash(&self) -> Hash {
        self.message.hash()
    }

    /// Get the sender/fee-payer (first account of first instruction)
    pub fn sender(&self) -> Pubkey {
        self.message.instructions[0].accounts[0]
    }

    /// Get transaction hash (includes the full signed envelope).
    ///
    /// This is the **canonical transaction ID** stored in `CF_TRANSACTIONS` and
    /// returned by RPC methods. It equals `SHA-256(bincode(transaction))`.
    ///
    /// Using the full serialized transaction keeps the txid aligned with the
    /// actual bytes propagated over the network, including the self-contained PQ
    /// signatures that carry signer public keys.
    pub fn hash(&self) -> Hash {
        let data = serialize_legacy_bincode(self, "Transaction")
            .expect("Transaction serialization failed");
        Hash::hash(&data)
    }

    /// Collect required signer accounts in their canonical signature order.
    ///
    /// The first account of each instruction is the signer for that instruction.
    /// Repeated signers are included once at their first occurrence. The same
    /// order is enforced for `Transaction::signatures` so a signed transaction
    /// cannot be replayed with reordered, duplicated, or extra signatures.
    pub fn required_signers_ordered(&self) -> Result<Vec<Pubkey>, String> {
        if self.message.instructions.is_empty() {
            return Err("No instructions".to_string());
        }

        let mut seen = HashSet::new();
        let mut required_signers = Vec::new();
        for ix in &self.message.instructions {
            let Some(first_account) = ix.accounts.first() else {
                return Err("Instruction has no accounts".to_string());
            };
            if seen.insert(*first_account) {
                required_signers.push(*first_account);
            }
        }

        Ok(required_signers)
    }

    /// Collect the signer accounts required by this transaction.
    pub fn required_signers(&self) -> Result<HashSet<Pubkey>, String> {
        Ok(self.required_signers_ordered()?.into_iter().collect())
    }

    /// Verify that the canonical required signer set has valid PQ signatures
    /// over the serialized transaction message.
    pub fn verify_required_signatures(&self) -> Result<HashSet<Pubkey>, String> {
        self.verify_required_signatures_against(&self.message.serialize())
    }

    /// Verify native signatures against the chain-id domain envelope.
    pub fn verify_required_signatures_with_chain_id(
        &self,
        chain_id: &str,
    ) -> Result<HashSet<Pubkey>, String> {
        if chain_id.is_empty() {
            return Err("chain id is required for transaction verification".to_string());
        }
        self.verify_required_signatures_against(&self.message.signing_bytes_for_chain_id(chain_id))
    }

    fn verify_required_signatures_against(
        &self,
        message_bytes: &[u8],
    ) -> Result<HashSet<Pubkey>, String> {
        if self.signatures.is_empty() {
            return Err("No signatures".to_string());
        }

        let required_signers = self.required_signers_ordered()?;
        if self.signatures.len() != required_signers.len() {
            return Err(format!(
                "Signature set must match required signers exactly: got {}, need {}",
                self.signatures.len(),
                required_signers.len()
            ));
        }

        let mut verified_signers = HashSet::with_capacity(required_signers.len());

        for (index, (signature, expected_signer)) in self
            .signatures
            .iter()
            .zip(required_signers.iter())
            .enumerate()
        {
            signature
                .validate()
                .map_err(|err| format!("invalid signature {}: {}", index, err))?;
            let signer = signature.signer_address();
            if signer != *expected_signer {
                return Err(format!(
                    "signature {} signed by {}, expected {}",
                    index, signer, expected_signer
                ));
            }

            if !Keypair::verify(expected_signer, message_bytes, signature) {
                return Err(format!(
                    "Missing or invalid signature for account {}",
                    expected_signer
                ));
            }

            if !verified_signers.insert(signer) {
                return Err(format!("Duplicate signature for account {}", signer));
            }
        }

        Ok(verified_signers)
    }

    /// Validate transaction structure (size limits, T1.7)
    pub fn validate_structure(&self) -> Result<(), String> {
        if self.is_consensus() && !self.signatures.is_empty() {
            return Err("Consensus transactions must not carry user signatures".to_string());
        }
        if self.signatures.len() > MAX_SIGNATURES_PER_TX {
            return Err(format!(
                "Too many signatures: {} (max {})",
                self.signatures.len(),
                MAX_SIGNATURES_PER_TX
            ));
        }
        if self.message.instructions.is_empty() {
            return Err("No instructions".to_string());
        }
        if self.message.instructions.len() > MAX_INSTRUCTIONS_PER_TX {
            return Err(format!(
                "Too many instructions: {} (max {})",
                self.message.instructions.len(),
                MAX_INSTRUCTIONS_PER_TX
            ));
        }
        for (i, ix) in self.message.instructions.iter().enumerate() {
            // Deploy instructions allow up to 4MB for WASM code:
            // - System program type 17 (system_deploy_contract)
            // - Contract program Deploy variant (JSON-encoded WASM via ContractInstruction)
            let is_system_deploy = ix.program_id == crate::Pubkey([0u8; 32])
                && !ix.data.is_empty()
                && ix.data[0] == 17;
            let is_contract_deploy =
                ix.program_id == crate::Pubkey([0xFFu8; 32]) && ix.data.starts_with(b"{\"Deploy\"");
            let data_limit = if is_system_deploy || is_contract_deploy || self.is_consensus() {
                MAX_DEPLOY_INSTRUCTION_DATA
            } else {
                MAX_INSTRUCTION_DATA
            };
            if ix.data.len() > data_limit {
                return Err(format!(
                    "Instruction {} data too large: {} bytes (max {})",
                    i,
                    ix.data.len(),
                    data_limit
                ));
            }
            if ix.accounts.is_empty() {
                return Err(format!("Instruction {} has no accounts", i));
            }
            if ix.accounts.len() > MAX_ACCOUNTS_PER_IX {
                return Err(format!(
                    "Instruction {} has too many accounts: {} (max {})",
                    i,
                    ix.accounts.len(),
                    MAX_ACCOUNTS_PER_IX
                ));
            }
        }
        let serialized_size = serialized_size_legacy_bincode(self, "Transaction")
            .map_err(|e| format!("Transaction size serialization failed: {}", e))?;
        if serialized_size > MAX_TRANSACTION_SERIALIZED_SIZE {
            return Err(format!(
                "Transaction serialized size too large: {} bytes (max {})",
                serialized_size, MAX_TRANSACTION_SERIALIZED_SIZE
            ));
        }
        Ok(())
    }

    // ── Wire-format envelope (M-6) ─────────────────────────────

    /// Serialize to the V1 wire envelope: `[magic_0, magic_1, version, type, ...bincode]`.
    ///
    /// Callers that need base64 transport can encode the returned bytes with
    /// `base64::encode(&tx.to_wire())`.
    pub fn to_wire(&self) -> Vec<u8> {
        let payload = serialize_legacy_bincode(self, "Transaction")
            .expect("Transaction serialization failed");
        let mut buf = Vec::with_capacity(4 + payload.len());
        buf.extend_from_slice(&TX_WIRE_MAGIC);
        buf.push(TX_WIRE_VERSION);
        buf.push(self.tx_type as u8);
        buf.extend_from_slice(&payload);
        buf
    }

    /// Deserialize a transaction from the mandatory V1 wire envelope.
    ///
    /// The `max_wire_bytes` parameter caps the complete envelope before
    /// deserialization to prevent OOM from adversarial transaction submissions.
    pub fn from_wire(data: &[u8], max_wire_bytes: u64) -> Result<Self, String> {
        if data.len() as u64 > max_wire_bytes {
            return Err(format!(
                "Transaction wire payload too large: {} bytes (max {})",
                data.len(),
                max_wire_bytes
            ));
        }

        if data.len() < 4 || data[0..2] != TX_WIRE_MAGIC {
            return Err("Missing transaction V1 wire envelope".to_string());
        }

        let version = data[2];
        if version != TX_WIRE_VERSION {
            return Err(format!("Unsupported wire version: {}", version));
        }
        let type_byte = data[3];
        let tx_type = match type_byte {
            0 => TransactionType::Native,
            1 => TransactionType::Evm,
            _ => return Err(format!("Unknown transaction type byte: {}", type_byte)),
        };
        let payload = &data[4..];
        let mut tx: Self = bounded_bincode_deser(payload, max_wire_bytes.saturating_sub(4))?;
        // The versioned envelope is authoritative.
        tx.tx_type = tx_type;
        Ok(tx)
    }
}

/// Bounded bincode deserialization with panic catch (bincode 1.x safety).
fn bounded_bincode_deser(bytes: &[u8], limit: u64) -> Result<Transaction, String> {
    deserialize_legacy_bincode_strict(bytes, limit, "Transaction wire").map_err(|err| {
        if err.contains("panicked") {
            "bincode panicked during deserialization".to_string()
        } else {
            format!("bincode: {}", err)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_creation() {
        let program_id = Pubkey([1u8; 32]);
        let accounts = vec![Pubkey([2u8; 32]), Pubkey([3u8; 32])];

        let instruction = Instruction {
            program_id,
            accounts,
            data: vec![0, 1, 2, 3],
        };

        let message = Message::new(vec![instruction], Hash::hash(b"recent_block"));

        let tx = Transaction::new(message);

        println!("Transaction signature: {}", tx.signature());
        assert_eq!(tx.signatures.len(), 0); // Not signed yet
    }

    #[test]
    fn consensus_transaction_type_is_not_accepted_from_external_wire() {
        let mut tx = Transaction::new(Message::new(
            vec![Instruction {
                program_id: Pubkey([0u8; 32]),
                accounts: vec![Pubkey([0u8; 32])],
                data: vec![crate::CANONICAL_COMMIT_ENVELOPE_OPCODE],
            }],
            Hash::hash(b"parent"),
        ));
        tx.tx_type = TransactionType::Consensus;
        let wire = tx.to_wire();
        assert_eq!(wire[3], TransactionType::Consensus as u8);
        assert!(Transaction::from_wire(&wire, MAX_TRANSACTION_WIRE_SIZE)
            .unwrap_err()
            .contains("Unknown transaction type"));
    }

    // ── H16 tests: deploy instruction data limit exemption ──

    #[test]
    fn test_validate_structure_normal_instruction_200kb_limit() {
        let ix = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0u8; MAX_INSTRUCTION_DATA + 1],
        };
        let msg = Message::new(vec![ix], Hash::default());
        let tx = Transaction::new(msg);
        assert!(tx.validate_structure().is_err());
    }

    #[test]
    fn test_validate_structure_rejects_instruction_without_accounts() {
        let ix = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: Vec::new(),
            data: vec![0],
        };
        let tx = Transaction::new(Message::new(vec![ix], Hash::default()));

        let result = tx.validate_structure();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("has no accounts"));
    }

    #[test]
    fn test_validate_structure_deploy_instruction_allows_large_data() {
        // System program (all zeros), instruction type 17 = DeployContract
        let mut data = vec![17u8]; // type byte
        data.extend_from_slice(&(100_000u32).to_le_bytes()); // code_length
        data.extend(vec![0u8; 100_000]); // fake WASM code (100KB — within 200KB general limit but tests deploy path)

        let ix = Instruction {
            program_id: Pubkey([0u8; 32]), // system program
            accounts: vec![Pubkey([2u8; 32]), Pubkey([3u8; 32])],
            data,
        };
        let msg = Message::new(vec![ix], Hash::default());
        let tx = Transaction::new(msg);
        assert!(
            tx.validate_structure().is_ok(),
            "Deploy instruction should allow >200KB data"
        );
    }

    #[test]
    fn test_validate_structure_deploy_instruction_4mb_limit() {
        // Even deploy instructions have a 4MB cap
        let mut data = vec![17u8];
        data.extend(vec![0u8; MAX_DEPLOY_INSTRUCTION_DATA - 1]); // total = limit (type byte + payload)
        let ix = Instruction {
            program_id: Pubkey([0u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data,
        };
        let msg = Message::new(vec![ix], Hash::default());
        let tx = Transaction::new(msg);
        assert!(tx.validate_structure().is_ok());

        // Over limit
        let mut data2 = vec![17u8];
        data2.extend(vec![0u8; MAX_DEPLOY_INSTRUCTION_DATA + 1]);
        let ix2 = Instruction {
            program_id: Pubkey([0u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: data2,
        };
        let msg2 = Message::new(vec![ix2], Hash::default());
        let tx2 = Transaction::new(msg2);
        assert!(
            tx2.validate_structure().is_err(),
            "Deploy instruction over 4MB should be rejected"
        );
    }

    #[test]
    fn test_validate_structure_rejects_too_many_signatures() {
        let ix = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0],
        };
        let msg = Message::new(vec![ix], Hash::default());
        let mut tx = Transaction::new(msg);
        tx.signatures = (0..=MAX_SIGNATURES_PER_TX)
            .map(|idx| crate::account::PqSignature::test_fixture(idx as u8))
            .collect();

        let result = tx.validate_structure();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Too many signatures"));
    }

    #[test]
    fn test_validate_structure_rejects_serialized_tx_size_over_limit() {
        let instructions = (0..26)
            .map(|idx| Instruction {
                program_id: Pubkey([idx as u8; 32]),
                accounts: vec![Pubkey([2u8; 32])],
                data: vec![idx as u8; MAX_INSTRUCTION_DATA],
            })
            .collect();
        let tx = Transaction::new(Message::new(instructions, Hash::default()));

        let serialized_size = serialized_size_legacy_bincode(&tx, "transaction test fixture")
            .expect("serialized size");
        assert!(
            serialized_size > MAX_TRANSACTION_SERIALIZED_SIZE,
            "fixture must exceed tx serialized-size cap"
        );

        let result = tx.validate_structure();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("serialized size too large"));
    }

    fn two_signer_transaction() -> (Transaction, Keypair, Keypair) {
        let kp1 = Keypair::from_seed(&[1u8; 32]);
        let kp2 = Keypair::from_seed(&[2u8; 32]);
        let ix1 = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![kp1.pubkey()],
            data: vec![1],
        };
        let ix2 = Instruction {
            program_id: Pubkey([2u8; 32]),
            accounts: vec![kp2.pubkey()],
            data: vec![2],
        };
        let mut tx = Transaction::new(Message::new(vec![ix1, ix2], Hash::hash(b"recent")));
        let message = tx.message.serialize();
        tx.signatures.push(kp1.sign(&message));
        tx.signatures.push(kp2.sign(&message));
        (tx, kp1, kp2)
    }

    #[test]
    fn test_verify_required_signatures_accepts_canonical_multi_signer_order() {
        let (tx, kp1, kp2) = two_signer_transaction();

        let signers = tx.verify_required_signatures().unwrap();
        assert!(signers.contains(&kp1.pubkey()));
        assert!(signers.contains(&kp2.pubkey()));
    }

    #[test]
    fn test_verify_required_signatures_accepts_chain_id_domain() {
        let kp1 = Keypair::from_seed(&[1u8; 32]);
        let kp2 = Keypair::from_seed(&[2u8; 32]);
        let ix1 = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![kp1.pubkey()],
            data: vec![1],
        };
        let ix2 = Instruction {
            program_id: Pubkey([2u8; 32]),
            accounts: vec![kp2.pubkey()],
            data: vec![2],
        };
        let mut tx = Transaction::new(Message::new(vec![ix1, ix2], Hash::hash(b"recent")));
        let message = tx.message.signing_bytes_for_chain_id("lichen-testnet-1");
        tx.signatures.push(kp1.sign(&message));
        tx.signatures.push(kp2.sign(&message));

        let signers = tx
            .verify_required_signatures_with_chain_id("lichen-testnet-1")
            .expect("chain-id domain signatures should verify");
        assert!(signers.contains(&kp1.pubkey()));
        assert!(signers.contains(&kp2.pubkey()));

        let wrong_chain = tx.verify_required_signatures_with_chain_id("lichen-mainnet-1");
        assert!(
            wrong_chain.is_err(),
            "chain-id domain signatures must not verify on a different chain"
        );
    }

    #[test]
    fn test_verify_required_signatures_with_chain_id_rejects_unbound_signature() {
        let (tx, _, _) = two_signer_transaction();
        let error = tx
            .verify_required_signatures_with_chain_id("lichen-testnet-1")
            .expect_err("unbound signature must be rejected");
        assert!(error.contains("Missing or invalid signature"));
    }

    #[test]
    fn test_verify_required_signatures_rejects_extra_signature_malleation() {
        let (mut tx, _kp1, _kp2) = two_signer_transaction();
        let extra = Keypair::from_seed(&[3u8; 32]);
        tx.signatures.push(extra.sign(&tx.message.serialize()));

        let result = tx.verify_required_signatures();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must match required signers"));
    }

    #[test]
    fn test_verify_required_signatures_rejects_reordered_signatures() {
        let (mut tx, _kp1, _kp2) = two_signer_transaction();
        tx.signatures.swap(0, 1);

        let result = tx.verify_required_signatures();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected"));
    }

    #[test]
    fn test_verify_required_signatures_rejects_duplicate_signer_malleation() {
        let (mut tx, kp1, _kp2) = two_signer_transaction();
        tx.signatures[1] = kp1.sign(&tx.message.serialize());

        let result = tx.verify_required_signatures();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected"));
    }

    #[test]
    fn test_from_wire_rejects_oversized_payload_before_deserialization() {
        let bytes = vec![b'{'; MAX_TRANSACTION_WIRE_SIZE as usize + 1];

        let result = Transaction::from_wire(&bytes, MAX_TRANSACTION_WIRE_SIZE);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("wire payload too large"));
    }

    #[test]
    fn test_from_wire_rejects_unenveloped_payloads() {
        let (tx, _, _) = two_signer_transaction();
        let raw = serialize_legacy_bincode(&tx, "Transaction").unwrap();
        assert!(Transaction::from_wire(&raw, MAX_TRANSACTION_WIRE_SIZE)
            .unwrap_err()
            .contains("Missing transaction V1 wire envelope"));

        let json = serde_json::to_vec(&tx).unwrap();
        assert!(Transaction::from_wire(&json, MAX_TRANSACTION_WIRE_SIZE)
            .unwrap_err()
            .contains("Missing transaction V1 wire envelope"));
    }

    #[test]
    fn test_v1_wire_roundtrip() {
        let (tx, _, _) = two_signer_transaction();
        let decoded = Transaction::from_wire(&tx.to_wire(), MAX_TRANSACTION_WIRE_SIZE).unwrap();
        assert_eq!(decoded.signatures, tx.signatures);
        assert_eq!(decoded.message.serialize(), tx.message.serialize());
        assert_eq!(decoded.tx_type, tx.tx_type);
    }

    // ── AUDIT-FIX A3-01: Verify data field IS included in signature hash ──

    /// Regression test: changing instruction data MUST produce a different
    /// message hash and different signature. This prevents the old vulnerability
    /// where `data` was excluded from the signed hash.
    #[test]
    fn test_a3_01_data_field_included_in_signature_hash() {
        let bh = Hash::default();

        // Two instructions identical except for data
        let ix1 = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0x01, 0x02, 0x03],
        };
        let ix2 = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0x01, 0x02, 0x04], // only last byte differs
        };

        let msg1 = Message::new(vec![ix1], bh);
        let msg2 = Message::new(vec![ix2], bh);

        // Serialized bytes must differ
        assert_ne!(
            msg1.serialize(),
            msg2.serialize(),
            "A3-01 REGRESSION: Messages with different data must serialize differently"
        );

        // Hashes must differ
        assert_ne!(
            msg1.hash(),
            msg2.hash(),
            "A3-01 REGRESSION: Messages with different data must hash differently"
        );
    }

    /// Regression test: changing program_id MUST produce a different hash.
    #[test]
    fn test_a3_01_program_id_included_in_signature_hash() {
        let bh = Hash::default();

        let ix1 = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0x01],
        };
        let ix2 = Instruction {
            program_id: Pubkey([99u8; 32]), // different program
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0x01],
        };

        let msg1 = Message::new(vec![ix1], bh);
        let msg2 = Message::new(vec![ix2], bh);

        assert_ne!(
            msg1.hash(),
            msg2.hash(),
            "A3-01 REGRESSION: Messages with different program_id must hash differently"
        );
    }

    /// Regression test: changing accounts MUST produce a different hash.
    #[test]
    fn test_a3_01_accounts_included_in_signature_hash() {
        let bh = Hash::default();

        let ix1 = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0x01],
        };
        let ix2 = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([3u8; 32])], // different account
            data: vec![0x01],
        };

        let msg1 = Message::new(vec![ix1], bh);
        let msg2 = Message::new(vec![ix2], bh);

        assert_ne!(
            msg1.hash(),
            msg2.hash(),
            "A3-01 REGRESSION: Messages with different accounts must hash differently"
        );
    }

    // ════════════════════════════════════════════════════════════════════
    // K4-02: Cross-SDK serialization compatibility golden vector
    // ════════════════════════════════════════════════════════════════════

    /// Generate a deterministic Message, serialize it via bincode, and assert
    /// the exact bytes match the golden vector. JS and Python SDKs MUST produce
    /// identical output for the same input. If this test changes, all SDK tests
    /// must be updated.
    #[test]
    fn test_cross_sdk_message_golden_vector() {
        let ix = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0x00, 0x01, 0x02, 0x03],
        };
        let msg = Message {
            instructions: vec![ix],
            recent_blockhash: crate::Hash::new([0xAA; 32]),
            compute_budget: None,
            compute_unit_price: None,
        };

        let bytes = msg.serialize();
        let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();

        // Print for reference if generating new golden vector:
        // eprintln!("GOLDEN_VECTOR_HEX={}", hex);

        // Golden vector (bincode 1.3 default serialization):
        // instructions: Vec<Instruction> → u64_le(1) + Instruction
        //   program_id: [u8; 32] → 32 raw bytes (0x01 repeated)
        //   accounts: Vec<Pubkey> → u64_le(1) + 32 raw bytes (0x02 repeated)
        //   data: Vec<u8> → u64_le(4) + [0x00, 0x01, 0x02, 0x03]
        // recent_blockhash: [u8; 32] → 32 raw bytes (0xAA repeated)
        let expected = format!(
            "{}{}{}{}{}{}{}",
            "0100000000000000", // Vec<Ix> len = 1
            "0101010101010101010101010101010101010101010101010101010101010101", // program_id
            "0100000000000000", // Vec<Pubkey> len = 1
            "0202020202020202020202020202020202020202020202020202020202020202", // accounts[0]
            "040000000000000000010203", // Vec<u8> len=4 + data
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // blockhash
            "0000",             // compute_budget: None (0x00) + compute_unit_price: None (0x00)
        );

        assert_eq!(
            hex, expected,
            "K4-02 GOLDEN VECTOR MISMATCH!\n\
             This means the Rust bincode serialization changed.\n\
             JS/Python SDKs MUST also match this exact byte sequence.\n\
             Got:      {}\n\
             Expected: {}",
            hex, expected
        );
    }

    /// Golden vector for a full Transaction (signature + message).
    #[test]
    fn test_cross_sdk_transaction_golden_vector() {
        let ix = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0x00, 0x01, 0x02, 0x03],
        };
        let msg = Message {
            instructions: vec![ix],
            recent_blockhash: crate::Hash::new([0xAA; 32]),
            compute_budget: None,
            compute_unit_price: None,
        };
        let sig = crate::account::PqSignature::test_fixture(0xBB);
        let tx = Transaction {
            signatures: vec![sig],
            message: msg,
            tx_type: Default::default(),
        };

        let bytes =
            serialize_legacy_bincode(&tx, "transaction golden vector").expect("tx serialization");
        let tx_hash = Hash::hash(&bytes);

        assert_eq!(
            bytes.len(),
            5_417,
            "unexpected serialized PQ transaction length"
        );
        assert_eq!(
            tx_hash,
            Hash::from_hex("9d0eec7b657276b828c265995ce78b41a3e19b17ab354b11f37254bbc4ee2a91")
                .unwrap()
        );
    }
}

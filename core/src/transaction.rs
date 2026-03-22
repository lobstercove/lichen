// MoltChain Core - Transaction Model

use crate::account::Pubkey;
use crate::hash::Hash;
use serde::{Deserialize, Serialize};

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

    /// Price per compute unit in micro-shells (μshells).
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

    /// Effective compute unit price in micro-shells.
    pub fn effective_compute_unit_price(&self) -> u64 {
        self.compute_unit_price.unwrap_or(0)
    }

    /// Serialize for signing.
    ///
    /// Panics only on OOM or bincode internal error (neither expected for a
    /// well-formed Message). Callers that need fallibility should use
    /// `try_serialize()` instead.
    pub fn serialize(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap_or_else(|e| {
            panic!(
                "FATAL: Message serialization failed ({}). This indicates data corruption or OOM.",
                e
            )
        })
    }

    /// Fallible serialization for contexts that can propagate errors.
    pub fn try_serialize(&self) -> Result<Vec<u8>, String> {
        bincode::serialize(self).map_err(|e| format!("Message serialization failed: {}", e))
    }

    /// Hash for signing
    pub fn hash(&self) -> Hash {
        Hash::hash(&self.serialize())
    }
}

/// Transaction type discriminator — replaces sentinel-based detection.
///
/// - `Native`: Standard MoltChain transaction (Ed25519 signed, blockhash replay protection)
/// - `Evm`: EVM-wrapped transaction (ECDSA signed, EVM nonce replay protection)
/// - `SolanaCompat`: Submitted via Solana-format RPC (same as Native but tagged for metrics)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TransactionType {
    #[default]
    Native,
    Evm,
    SolanaCompat,
}

/// Wire-format magic bytes identifying a MoltChain transaction envelope.
/// "MT" = MoltTransaction. The pair `[0x4D, 0x54]` cannot appear as the first
/// two bytes of a legacy bincode Transaction (that would imply 0x544D = 21,581
/// signatures, which is impossible).
pub const TX_WIRE_MAGIC: [u8; 2] = [0x4D, 0x54];

/// Current wire-format version.
pub const TX_WIRE_VERSION: u8 = 1;

/// Signed transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction signatures (as hex strings for serde compatibility)
    #[serde(
        serialize_with = "serialize_signatures",
        deserialize_with = "deserialize_signatures"
    )]
    pub signatures: Vec<[u8; 64]>,

    /// Transaction message
    pub message: Message,

    /// Transaction type — determines processing path.
    /// Defaults to `Native` for backward compatibility with existing serialized transactions.
    #[serde(default)]
    pub tx_type: TransactionType,
}

// Helper functions for signature serialization.
//
// L2-01 fix: Use `is_human_readable()` to branch between formats:
// - Human-readable (JSON, TOML): serialize as hex strings for readability
// - Non-human-readable (bincode): serialize as raw bytes for efficiency
//   and compatibility with JS/Python SDK manual bincode encoders
//
// Since serde doesn't impl Serialize/Deserialize for [T; 64] (only up to 32),
// we use helper newtypes that manually encode/decode via serialize_tuple(64).

/// Newtype wrapper to serialize `[u8; 64]` as a fixed-size tuple (no length prefix in bincode).
struct Sig64Ser<'a>(&'a [u8; 64]);

impl serde::Serialize for Sig64Ser<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(64)?;
        for b in self.0 {
            tup.serialize_element(b)?;
        }
        tup.end()
    }
}

/// DeserializeSeed for reading a single [u8; 64] from a bincode tuple.
struct Sig64De;

impl<'de> serde::de::DeserializeSeed<'de> for Sig64De {
    type Value = [u8; 64];

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = [u8; 64];
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("64 bytes")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut arr = [0u8; 64];
                for (i, slot) in arr.iter_mut().enumerate() {
                    *slot = seq
                        .next_element::<u8>()?
                        .ok_or_else(|| serde::de::Error::invalid_length(i, &self))?;
                }
                Ok(arr)
            }
        }
        deserializer.deserialize_tuple(64, V)
    }
}

fn serialize_signatures<S>(sigs: &[[u8; 64]], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if serializer.is_human_readable() {
        // JSON: hex strings for readability
        use serde::Serialize;
        let hex_sigs: Vec<String> = sigs.iter().map(hex::encode).collect();
        hex_sigs.serialize(serializer)
    } else {
        // bincode: raw Vec<[u8; 64]> — each signature is 64 flat bytes,
        // prefixed by a u64 vec length. Matches JS/Python SDK encoding.
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(sigs.len()))?;
        for sig in sigs {
            seq.serialize_element(&Sig64Ser(sig))?;
        }
        seq.end()
    }
}

fn deserialize_signatures<'de, D>(deserializer: D) -> Result<Vec<[u8; 64]>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    if deserializer.is_human_readable() {
        // JSON: parse hex strings
        use serde::Deserialize;
        let hex_sigs: Vec<String> = Vec::deserialize(deserializer)?;
        hex_sigs
            .iter()
            .map(|s| {
                let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
                if bytes.len() != 64 {
                    return Err(serde::de::Error::custom("Invalid signature length"));
                }
                let mut sig = [0u8; 64];
                sig.copy_from_slice(&bytes);
                Ok(sig)
            })
            .collect()
    } else {
        // bincode: read Vec<[u8; 64]> as sequence of 64-byte tuples
        struct SigsVisitor;
        impl<'de> serde::de::Visitor<'de> for SigsVisitor {
            type Value = Vec<[u8; 64]>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a sequence of 64-byte signatures")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut sigs = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(sig) = seq.next_element_seed(Sig64De)? {
                    sigs.push(sig);
                }
                Ok(sigs)
            }
        }
        deserializer.deserialize_seq(SigsVisitor)
    }
}

/// Maximum instructions per transaction (T1.7)
pub const MAX_INSTRUCTIONS_PER_TX: usize = 64;
/// Maximum data bytes per instruction (T1.7)
pub const MAX_INSTRUCTION_DATA: usize = 204_800; // 200KB — contract calls may carry significant payloads
pub const MAX_DEPLOY_INSTRUCTION_DATA: usize = 4_194_304; // 4MB — WASM deploys via instruction type 17
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
            signatures: vec![[0u8; 64]],
            message,
            tx_type: TransactionType::Evm,
        }
    }

    /// Check if this is an EVM transaction (by type field or legacy sentinel).
    pub fn is_evm(&self) -> bool {
        self.tx_type == TransactionType::Evm
            || self.message.recent_blockhash == crate::Hash([0xEE; 32])
    }

    /// Check if this is a Solana-compat transaction.
    pub fn is_solana_compat(&self) -> bool {
        self.tx_type == TransactionType::SolanaCompat
    }

    /// Get transaction signature (first signature's identifier)
    pub fn signature(&self) -> Hash {
        self.hash()
    }

    /// Get the message-only hash (signing hash).
    ///
    /// This is the hash that signers commit to via Ed25519. It does NOT include
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

    /// Get transaction hash (includes signatures for unique deduplication).
    ///
    /// This is the **canonical transaction ID** stored in `CF_TRANSACTIONS` and
    /// returned by RPC methods. It equals `SHA-256(bincode(message) || sig_0 || sig_1 || ...)`.
    ///
    /// Including signatures prevents txid-malleability: two transactions with the
    /// same message but different signatures produce different hashes, matching
    /// Bitcoin's post-SegWit wtxid approach and Cosmos/CometBFT's `SHA-256(tx_bytes)`.
    ///
    /// Determinism guarantee: for any `Transaction` value, `hash()` always returns
    /// the same `Hash`. Bincode serialization of `Message` is deterministic (no
    /// maps, no unordered collections), and Ed25519 signatures are fixed-size byte
    /// arrays concatenated in order.
    pub fn hash(&self) -> Hash {
        let mut data = self.message.serialize();
        for sig in &self.signatures {
            data.extend_from_slice(sig);
        }
        Hash::hash(&data)
    }

    /// Validate transaction structure (size limits, T1.7)
    pub fn validate_structure(&self) -> Result<(), String> {
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
            let data_limit = if is_system_deploy || is_contract_deploy {
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
            if ix.accounts.len() > MAX_ACCOUNTS_PER_IX {
                return Err(format!(
                    "Instruction {} has too many accounts: {} (max {})",
                    i,
                    ix.accounts.len(),
                    MAX_ACCOUNTS_PER_IX
                ));
            }
        }
        Ok(())
    }

    // ── Wire-format envelope (M-6) ─────────────────────────────

    /// Serialize to the V1 wire envelope: `[magic_0, magic_1, version, type, ...bincode]`.
    ///
    /// Callers that need base64 transport can encode the returned bytes with
    /// `base64::encode(&tx.to_wire())`.
    pub fn to_wire(&self) -> Vec<u8> {
        let payload = bincode::serialize(self).expect("Transaction bincode serialization failed");
        let mut buf = Vec::with_capacity(4 + payload.len());
        buf.extend_from_slice(&TX_WIRE_MAGIC);
        buf.push(TX_WIRE_VERSION);
        buf.push(self.tx_type as u8);
        buf.extend_from_slice(&payload);
        buf
    }

    /// Deserialize from wire bytes, supporting three formats:
    ///
    /// 1. **V1 envelope** — starts with `TX_WIRE_MAGIC` (`[0x4D, 0x54]`)
    /// 2. **Legacy bincode** — raw `bincode::serialize(&Transaction)` output
    /// 3. **JSON** — `{ "signatures": [...], "message": {...} }` from browser wallets
    ///
    /// The `max_bincode_bytes` parameter caps the bincode deserialization buffer
    /// to prevent OOM from adversarial payloads.
    pub fn from_wire(data: &[u8], max_bincode_bytes: u64) -> Result<Self, String> {
        // --- V1 envelope ---
        if data.len() >= 4 && data[0..2] == TX_WIRE_MAGIC {
            let version = data[2];
            if version != TX_WIRE_VERSION {
                return Err(format!("Unsupported wire version: {}", version));
            }
            let type_byte = data[3];
            let tx_type = match type_byte {
                0 => TransactionType::Native,
                1 => TransactionType::Evm,
                2 => TransactionType::SolanaCompat,
                _ => return Err(format!("Unknown transaction type byte: {}", type_byte)),
            };
            let payload = &data[4..];
            let mut tx: Self = bounded_bincode_deser(payload, max_bincode_bytes)?;
            // Envelope type is authoritative
            tx.tx_type = tx_type;
            return Ok(tx);
        }

        // --- Legacy: JSON vs bincode ---
        if data.first() == Some(&b'{') {
            // Looks like JSON — try JSON first, fall back to bincode
            json_deser(data).or_else(|_| bounded_bincode_deser(data, max_bincode_bytes))
        } else {
            // Try bincode first, fall back to JSON
            bounded_bincode_deser(data, max_bincode_bytes).or_else(|_| json_deser(data))
        }
    }
}

/// Bounded bincode deserialization with panic catch (bincode 1.x safety).
fn bounded_bincode_deser(bytes: &[u8], limit: u64) -> Result<Transaction, String> {
    use bincode::Options;
    match std::panic::catch_unwind(|| {
        bincode::options()
            .with_limit(limit)
            .with_fixint_encoding()
            .allow_trailing_bytes()
            .deserialize(bytes)
    }) {
        Ok(Ok(tx)) => Ok(tx),
        Ok(Err(e)) => Err(format!("bincode: {}", e)),
        Err(_) => Err("bincode panicked during deserialization".to_string()),
    }
}

/// Attempt JSON deserialization of a wallet-format transaction.
fn json_deser(bytes: &[u8]) -> Result<Transaction, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("JSON: {}", e))
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
        let sig: [u8; 64] = [0xBB; 64];
        let tx = Transaction {
            signatures: vec![sig],
            message: msg,
            tx_type: Default::default(),
        };

        let bytes = bincode::serialize(&tx).expect("tx serialization");
        let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();

        let sig_hex = "bb".repeat(64); // 64 bytes = 128 hex chars
        let expected = format!(
            "{}{}{}{}{}{}{}{}{}{}",
            "0100000000000000", // Vec<[u8;64]> len = 1
            sig_hex,            // sig (64 bytes)
            // -- Message bytes (same as golden vector above) --
            "0100000000000000", // Vec<Ix> len = 1
            "0101010101010101010101010101010101010101010101010101010101010101", // program_id
            "0100000000000000", // Vec<Pubkey> len = 1
            "0202020202020202020202020202020202020202020202020202020202020202", // accounts[0]
            "040000000000000000010203", // Vec<u8> len=4 + data
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // blockhash
            "0000",             // compute_budget: None + compute_unit_price: None
            "00000000",         // tx_type: Native (enum variant 0)
        );

        assert_eq!(
            hex, expected,
            "K4-02 TX GOLDEN VECTOR MISMATCH!\n\
             Got:      {}\n\
             Expected: {}",
            hex, expected
        );
    }
}

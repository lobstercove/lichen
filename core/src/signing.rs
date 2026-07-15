//! Versioned signing-domain envelopes.
//!
//! These helpers domain-separate signatures without changing the serialized
//! payload being committed. Legacy callers can keep signing the payload bytes;
//! chain-aware callers wrap the same payload in this envelope before signing.

pub const SIGNING_ENVELOPE_MAGIC: &[u8; 10] = b"LICHEN-SIG";
pub const SIGNING_ENVELOPE_VERSION: u8 = 1;
pub const CHAIN_ID_METADATA_KEY: &str = "chain_id";
pub const CONSENSUS_V1_ACTIVATION_SLOT_METADATA_KEY: &str = "post_block_effects_activation_slot_v1";

pub const DOMAIN_NATIVE_TX: &str = "native-tx";
pub const DOMAIN_BLOCK: &str = "block";
pub const DOMAIN_PROPOSAL: &str = "proposal";
pub const DOMAIN_PREVOTE: &str = "prevote";
pub const DOMAIN_PRECOMMIT: &str = "precommit";

pub fn versioned_signing_bytes(domain: &str, chain_id: &str, payload: &[u8]) -> Vec<u8> {
    let domain_bytes = domain.as_bytes();
    let chain_bytes = chain_id.as_bytes();

    let domain_len = u16::try_from(domain_bytes.len()).expect("signing domain length exceeds u16");
    let chain_len = u16::try_from(chain_bytes.len()).expect("chain id length exceeds u16");
    let payload_len = u64::try_from(payload.len()).expect("payload length exceeds u64");

    let mut out = Vec::with_capacity(
        SIGNING_ENVELOPE_MAGIC.len()
            + 1
            + 2
            + domain_bytes.len()
            + 2
            + chain_bytes.len()
            + 8
            + payload.len(),
    );
    out.extend_from_slice(SIGNING_ENVELOPE_MAGIC);
    out.push(SIGNING_ENVELOPE_VERSION);
    out.extend_from_slice(&domain_len.to_le_bytes());
    out.extend_from_slice(domain_bytes);
    out.extend_from_slice(&chain_len.to_le_bytes());
    out.extend_from_slice(chain_bytes);
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn maybe_versioned_signing_bytes(domain: &str, chain_id: &str, payload: &[u8]) -> Vec<u8> {
    if chain_id.is_empty() {
        payload.to_vec()
    } else {
        versioned_signing_bytes(domain, chain_id, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_signing_bytes_bind_domain_and_chain() {
        let payload = b"payload";
        let a = versioned_signing_bytes(DOMAIN_NATIVE_TX, "lichen-testnet-1", payload);
        let b = versioned_signing_bytes(DOMAIN_NATIVE_TX, "lichen-mainnet-1", payload);
        let c = versioned_signing_bytes(DOMAIN_PREVOTE, "lichen-testnet-1", payload);

        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with(SIGNING_ENVELOPE_MAGIC));
        assert_eq!(a[SIGNING_ENVELOPE_MAGIC.len()], SIGNING_ENVELOPE_VERSION);
    }
}

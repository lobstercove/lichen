#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::codec::deserialize_legacy_bincode;
use lichen_core::consensus::{Precommit, Prevote, Proposal, SlashingEvidence, Vote};

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<Vote>(data);
    let _ = deserialize_legacy_bincode::<Vote>(data, "fuzz vote");
    let _ = serde_json::from_slice::<Proposal>(data);
    let _ = deserialize_legacy_bincode::<Proposal>(data, "fuzz proposal");
    let _ = serde_json::from_slice::<Prevote>(data);
    let _ = deserialize_legacy_bincode::<Prevote>(data, "fuzz prevote");
    let _ = serde_json::from_slice::<Precommit>(data);
    let _ = deserialize_legacy_bincode::<Precommit>(data, "fuzz precommit");
    let _ = serde_json::from_slice::<SlashingEvidence>(data);
    let _ = deserialize_legacy_bincode::<SlashingEvidence>(data, "fuzz slashing evidence");

    if let Ok(vote) = serde_json::from_slice::<Vote>(data) {
        let _ = vote.verify();
    }
    if let Ok(proposal) = serde_json::from_slice::<Proposal>(data) {
        let _ = proposal.verify_signature_with_chain_id("lichen-testnet-1");
    }
    if let Ok(prevote) = serde_json::from_slice::<Prevote>(data) {
        let _ = prevote.verify_signature_with_chain_id("lichen-testnet-1");
    }
    if let Ok(precommit) = serde_json::from_slice::<Precommit>(data) {
        let _ = precommit.verify_signature_with_chain_id("lichen-testnet-1");
    }
});

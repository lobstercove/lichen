//! EventStream — Structured event logging for chain activity
//! Provides typed events that indexers and UIs can subscribe to

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChainEvent {
    Transfer {
        from: [u8; 32],
        to: [u8; 32],
        amount: u64,
    },
    ContractDeploy {
        deployer: [u8; 32],
        contract: [u8; 32],
        code_hash: [u8; 32],
    },
    ContractCall {
        caller: [u8; 32],
        contract: [u8; 32],
        method: String,
    },
    StakeDeposit {
        staker: [u8; 32],
        amount: u64,
    },
    StakeWithdraw {
        staker: [u8; 32],
        amount: u64,
    },
    GovernanceVote {
        voter: [u8; 32],
        proposal_id: u64,
        vote: bool,
    },
    BridgeLock {
        sender: [u8; 32],
        recipient: [u8; 32],
        amount: u64,
        asset: String,
        dest_chain: String,
    },
    BridgeMint {
        recipient: [u8; 32],
        amount: u64,
        asset: String,
        source_chain: String,
        tx_hash: String,
    },
    IdentityRegistered {
        address: [u8; 32],
        agent_type: u8,
    },
    SkillAttested {
        identity: [u8; 32],
        skill: String,
        attester: [u8; 32],
    },
}

/// Event buffer for the current block being processed
pub struct EventBuffer {
    events: Vec<ChainEvent>,
    slot: u64,
}

impl EventBuffer {
    pub fn new(slot: u64) -> Self {
        Self {
            events: Vec::new(),
            slot,
        }
    }

    pub fn emit(&mut self, event: ChainEvent) {
        self.events.push(event);
    }

    pub fn drain(&mut self) -> Vec<ChainEvent> {
        core::mem::take(&mut self.events)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn slot(&self) -> u64 {
        self.slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_buffer_emit_and_drain() {
        let mut buf = EventBuffer::new(42);
        buf.emit(ChainEvent::Transfer {
            from: [1u8; 32],
            to: [2u8; 32],
            amount: 100,
        });
        buf.emit(ChainEvent::StakeDeposit {
            staker: [1u8; 32],
            amount: 500,
        });
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.slot(), 42);
        let events = buf.drain();
        assert_eq!(events.len(), 2);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_event_buffer_empty() {
        let buf = EventBuffer::new(0);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_event_buffer_all_event_types() {
        let mut buf = EventBuffer::new(100);
        buf.emit(ChainEvent::Transfer {
            from: [1u8; 32],
            to: [2u8; 32],
            amount: 100,
        });
        buf.emit(ChainEvent::ContractDeploy {
            deployer: [1u8; 32],
            contract: [2u8; 32],
            code_hash: [3u8; 32],
        });
        buf.emit(ChainEvent::ContractCall {
            caller: [1u8; 32],
            contract: [2u8; 32],
            method: "transfer".to_string(),
        });
        buf.emit(ChainEvent::StakeDeposit {
            staker: [1u8; 32],
            amount: 500,
        });
        buf.emit(ChainEvent::StakeWithdraw {
            staker: [1u8; 32],
            amount: 200,
        });
        buf.emit(ChainEvent::GovernanceVote {
            voter: [1u8; 32],
            proposal_id: 1,
            vote: true,
        });
        buf.emit(ChainEvent::BridgeLock {
            sender: [1u8; 32],
            recipient: [2u8; 32],
            amount: 1000,
            asset: "usdc".to_string(),
            dest_chain: "ethereum".to_string(),
        });
        buf.emit(ChainEvent::BridgeMint {
            recipient: [1u8; 32],
            amount: 1000,
            asset: "usdc".to_string(),
            source_chain: "ethereum".to_string(),
            tx_hash: "0xabc123".to_string(),
        });
        buf.emit(ChainEvent::IdentityRegistered {
            address: [1u8; 32],
            agent_type: 1,
        });
        buf.emit(ChainEvent::SkillAttested {
            identity: [1u8; 32],
            skill: "Rust".to_string(),
            attester: [2u8; 32],
        });
        assert_eq!(buf.len(), 10);
    }
}

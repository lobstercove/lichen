// Lichen Block Receiver / Validator
//
// Validates incoming blocks before they are accepted by the consensus
// engine (via proposals) or the sync manager (via block range responses).
// This module contains pure validation logic — no state mutation.

use lichen_core::{Block, Pubkey, StakePool, ValidatorSet, MIN_VALIDATOR_STAKE};

/// Errors from block validation.
#[derive(Debug)]
pub enum BlockValidationError {
    InvalidSignature,
    InvalidStructure(String),
    TimestampTooFarAhead { timestamp: u64, now: u64 },
    ProducerNotInValidatorSet(Pubkey),
    ProducerBelowMinStake { pubkey: Pubkey, stake: u64 },
}

impl std::fmt::Display for BlockValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSignature => write!(f, "invalid block signature"),
            Self::InvalidStructure(msg) => write!(f, "invalid structure: {msg}"),
            Self::TimestampTooFarAhead { timestamp, now } => {
                write!(
                    f,
                    "timestamp {timestamp} is {}s in the future",
                    timestamp - now
                )
            }
            Self::ProducerNotInValidatorSet(pk) => {
                write!(f, "producer {} not in validator set", pk.to_base58())
            }
            Self::ProducerBelowMinStake { pubkey, stake } => {
                write!(
                    f,
                    "producer {} stake {} below minimum",
                    pubkey.to_base58(),
                    stake
                )
            }
        }
    }
}

impl std::error::Error for BlockValidationError {}

/// Maximum allowed clock drift for incoming blocks (120 seconds).
const MAX_FUTURE_TIMESTAMP_SECS: u64 = 120;

/// Validate an incoming block's signature, structure, and timestamp.
///
/// This is the minimal validation done for ALL incoming blocks (sync and BFT).
pub fn validate_block_basic(block: &Block) -> Result<(), BlockValidationError> {
    // Genesis block gets a pass
    if block.header.slot == 0 {
        return Ok(());
    }

    // Verify Ed25519 signature
    if !block.verify_signature() {
        return Err(BlockValidationError::InvalidSignature);
    }

    // Verify structural limits
    if let Err(e) = block.validate_structure() {
        return Err(BlockValidationError::InvalidStructure(e.to_string()));
    }

    // Reject blocks with timestamps too far in the future
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if block.header.timestamp > now_secs + MAX_FUTURE_TIMESTAMP_SECS {
        return Err(BlockValidationError::TimestampTooFarAhead {
            timestamp: block.header.timestamp,
            now: now_secs,
        });
    }

    Ok(())
}

/// Validate that the block's producer is in the active validator set
/// with sufficient stake. Skip for genesis blocks (slot 0).
pub fn validate_block_producer(
    block: &Block,
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
) -> Result<(), BlockValidationError> {
    if block.header.slot == 0 {
        return Ok(());
    }

    let producer = Pubkey(block.header.validator);

    // Check validator set membership
    if validator_set.get_validator(&producer).is_none() {
        return Err(BlockValidationError::ProducerNotInValidatorSet(producer));
    }

    // Check minimum stake
    let stake = stake_pool
        .get_stake(&producer)
        .map(|s| s.total_stake())
        .unwrap_or(0);
    if stake < MIN_VALIDATOR_STAKE {
        return Err(BlockValidationError::ProducerBelowMinStake {
            pubkey: producer,
            stake,
        });
    }

    Ok(())
}

/// Full validation: basic checks + producer membership + stake.
pub fn validate_block_full(
    block: &Block,
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
) -> Result<(), BlockValidationError> {
    validate_block_basic(block)?;
    validate_block_producer(block, validator_set, stake_pool)?;
    Ok(())
}

// Lichen BFT Consensus Engine
//
// Tendermint-style consensus: Propose → Prevote → Precommit → Commit.
//
// Each height (slot number) runs one or more rounds. In each round,
// a deterministic proposer broadcasts a block; validators prevote and
// precommit. 2/3+ stake-weighted precommits for the same block hash
// commit the block and advance to the next height. If a round fails
// (timeout or nil votes), the engine advances to round+1 with a new
// proposer.
//
// Safety invariant: locked-value rule — once a validator precommits for
// value V in round R, it will only prevote V in all future rounds unless
// it observes 2/3+ prevotes for a different value at a round > R (POL
// unlock). This guarantees that two honest validators never commit
// different values at the same height.

use lichen_core::consensus::{
    DEFAULT_BFT_MAX_PHASE_TIMEOUT_MS, DEFAULT_BFT_PRECOMMIT_TIMEOUT_BASE_MS,
    DEFAULT_BFT_PREVOTE_TIMEOUT_BASE_MS, DEFAULT_BFT_PROPOSE_TIMEOUT_BASE_MS,
};
use lichen_core::{
    Block, CommitSignature, Hash, Keypair, PqSignature, Precommit, Prevote, Proposal, Pubkey,
    RoundStep, StakePool, ValidatorSet, MIN_VALIDATOR_STAKE,
};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsensusTimeoutConfig {
    pub propose_timeout_base_ms: u64,
    pub prevote_timeout_base_ms: u64,
    pub precommit_timeout_base_ms: u64,
    pub max_phase_timeout_ms: u64,
}

impl Default for ConsensusTimeoutConfig {
    fn default() -> Self {
        Self {
            propose_timeout_base_ms: DEFAULT_BFT_PROPOSE_TIMEOUT_BASE_MS,
            prevote_timeout_base_ms: DEFAULT_BFT_PREVOTE_TIMEOUT_BASE_MS,
            precommit_timeout_base_ms: DEFAULT_BFT_PRECOMMIT_TIMEOUT_BASE_MS,
            max_phase_timeout_ms: DEFAULT_BFT_MAX_PHASE_TIMEOUT_MS,
        }
    }
}

/// Maximum number of heights ahead to buffer future BFT messages.
/// Messages beyond this range are dropped to prevent memory exhaustion.
const FUTURE_MSG_BUFFER_HEIGHTS: u64 = 10;

/// Actions emitted by the consensus engine for the caller to execute.
///
/// The engine is a pure state machine — it never touches I/O directly.
/// The caller (main loop) executes broadcasts, state writes, and timeouts.
#[derive(Debug)]
pub enum ConsensusAction {
    /// No action needed.
    None,
    /// Schedule a timeout for the current step.
    ScheduleTimeout(RoundStep, Duration),
    /// Broadcast a proposal to the network.
    BroadcastProposal(Proposal),
    /// Broadcast a prevote to the network.
    BroadcastPrevote(Prevote),
    /// Broadcast a precommit to the network.
    BroadcastPrecommit(Precommit),
    /// A block has been committed — apply it to state and advance height.
    CommitBlock {
        height: u64,
        round: u32,
        block: Block,
        block_hash: Hash,
    },
    /// Multiple actions (processed in order).
    Multiple(Vec<ConsensusAction>),
    /// Equivocation detected: a validator signed conflicting votes at the same (height, round).
    EquivocationDetected {
        height: u64,
        round: u32,
        validator: Pubkey,
        /// "prevote" or "precommit"
        vote_type: &'static str,
        hash_1: Option<Hash>,
        hash_2: Option<Hash>,
    },
}

/// Tendermint-style BFT consensus engine.
///
/// Pure state machine: call methods with incoming messages / timeout events,
/// receive `ConsensusAction` values to execute externally.
pub struct ConsensusEngine {
    // ── Identity ────────────────────────────────────────────────────
    keypair: Keypair,
    pub validator_pubkey: Pubkey,
    min_validator_stake: u64,
    timeouts: ConsensusTimeoutConfig,

    // ── Round state ─────────────────────────────────────────────────
    /// Current block height (tip_slot + 1).
    pub height: u64,
    /// Current round within this height (starts at 0).
    pub round: u32,
    /// Current consensus step.
    pub step: RoundStep,

    // ── Locking (Tendermint safety) ─────────────────────────────────
    /// Round at which we locked on a value (None = not locked).
    locked_round: Option<u32>,
    /// Block hash we are locked on.
    locked_value: Option<Hash>,
    /// Round at which we observed a polka (2/3+ prevotes) for a value.
    valid_round: Option<u32>,
    /// Block that has a polka.
    valid_value: Option<Block>,

    // ── Vote tracking ───────────────────────────────────────────────
    /// Proposals received per round: round → Proposal.
    proposals: HashMap<u32, Proposal>,
    /// Prevotes per (round, block_hash_or_nil) → list of validators.
    prevotes: HashMap<(u32, Option<Hash>), Vec<Pubkey>>,
    /// Precommits per (round, block_hash_or_nil) → list of validators.
    precommits: HashMap<(u32, Option<Hash>), Vec<Pubkey>>,
    /// Blocks received via proposals, keyed by hash.
    proposal_blocks: HashMap<Hash, Block>,

    // ── Duplicate suppression & equivocation detection ─────────────
    /// Prevotes we've already processed for the current height:
    /// (round, validator) → voted hash.
    /// Height is implicit because `start_height()` clears all per-height vote
    /// state and future-height votes are buffered until they become current.
    seen_prevotes: HashMap<(u32, Pubkey), Option<Hash>>,
    /// Precommits we've already processed for the current height:
    /// (round, validator) → voted hash.
    /// Height is implicit because `start_height()` clears all per-height vote
    /// state and future-height votes are buffered until they become current.
    seen_precommits: HashMap<(u32, Pubkey), Option<Hash>>,
    /// Precommit signatures retained for commit certificates: (round, validator) → (signature, timestamp).
    precommit_sigs: HashMap<(u32, Pubkey), (PqSignature, u64)>,
    /// Rounds for which we already signed a prevote, to prevent equivocation.
    signed_prevote_rounds: HashMap<u32, Option<Hash>>,
    /// Rounds for which we already signed a precommit, to prevent equivocation.
    signed_precommit_rounds: HashMap<u32, Option<Hash>>,
    /// Timestamp of the last committed block header so new proposals can be
    /// rejected if they do not advance monotonically.
    last_committed_block_timestamp: Option<u64>,

    // ── Future message buffers (G-10 fix) ───────────────────────────
    /// Proposals for heights > self.height, replayed when we advance.
    future_proposals: BTreeMap<u64, Vec<Proposal>>,
    /// Prevotes for heights > self.height.
    future_prevotes: BTreeMap<u64, Vec<Prevote>>,
    /// Precommits for heights > self.height.
    future_precommits: BTreeMap<u64, Vec<Precommit>>,
}

impl ConsensusEngine {
    /// Create a new consensus engine for the given validator identity.
    pub fn new(keypair: Keypair, validator_pubkey: Pubkey) -> Self {
        Self::new_with_min_stake(keypair, validator_pubkey, MIN_VALIDATOR_STAKE)
    }

    /// Create a new consensus engine with a network-specific minimum stake.
    pub fn new_with_min_stake(
        keypair: Keypair,
        validator_pubkey: Pubkey,
        min_validator_stake: u64,
    ) -> Self {
        Self::new_with_min_stake_and_timeouts(
            keypair,
            validator_pubkey,
            min_validator_stake,
            ConsensusTimeoutConfig::default(),
        )
    }

    /// Create a new consensus engine with a network-specific minimum stake
    /// and timeout configuration.
    pub fn new_with_min_stake_and_timeouts(
        keypair: Keypair,
        validator_pubkey: Pubkey,
        min_validator_stake: u64,
        timeouts: ConsensusTimeoutConfig,
    ) -> Self {
        Self {
            keypair,
            validator_pubkey,
            min_validator_stake,
            timeouts,
            height: 0,
            round: 0,
            step: RoundStep::Commit, // Not active until start_height()
            locked_round: None,
            locked_value: None,
            valid_round: None,
            valid_value: None,
            proposals: HashMap::new(),
            prevotes: HashMap::new(),
            precommits: HashMap::new(),
            proposal_blocks: HashMap::new(),
            seen_prevotes: HashMap::new(),
            seen_precommits: HashMap::new(),
            precommit_sigs: HashMap::new(),
            signed_prevote_rounds: HashMap::new(),
            signed_precommit_rounds: HashMap::new(),
            last_committed_block_timestamp: None,
            future_proposals: BTreeMap::new(),
            future_prevotes: BTreeMap::new(),
            future_precommits: BTreeMap::new(),
        }
    }

    /// Begin consensus for a new height. Resets all per-height state.
    pub fn start_height(&mut self, height: u64) {
        self.height = height;
        self.round = 0;
        self.step = RoundStep::Propose;
        self.locked_round = None;
        self.locked_value = None;
        self.valid_round = None;
        self.valid_value = None;
        self.proposals.clear();
        self.prevotes.clear();
        self.precommits.clear();
        self.proposal_blocks.clear();
        self.seen_prevotes.clear();
        self.seen_precommits.clear();
        self.precommit_sigs.clear();
        self.signed_prevote_rounds.clear();
        self.signed_precommit_rounds.clear();
        // Prune future message buffers: discard entries below the new height
        self.future_proposals.retain(|h, _| *h >= height);
        self.future_prevotes.retain(|h, _| *h >= height);
        self.future_precommits.retain(|h, _| *h >= height);
        info!("🔷 BFT: Starting height {} round 0", height);
    }

    /// Advance to the next round within the current height.
    fn start_round(&mut self, round: u32) -> ConsensusAction {
        self.round = round;
        self.step = RoundStep::Propose;
        info!(
            "🔷 BFT: Height {} advancing to round {}",
            self.height, round
        );
        ConsensusAction::ScheduleTimeout(RoundStep::Propose, self.propose_timeout())
    }

    // ═══════════════════════════════════════════════════════════════
    //  STATE MACHINE TRANSITION GUARD (G-7 fix)
    // ═══════════════════════════════════════════════════════════════

    /// Validate and execute a state transition. Logs invalid transitions
    /// (which indicates a logic bug) and returns false if rejected.
    ///
    /// Valid transitions:
    ///   Propose  → Prevote
    ///   Prevote  → Precommit
    ///   Precommit → Commit
    ///   Commit   → Propose   (new height via start_height/start_round)
    ///
    /// Note: start_round() sets step directly because it's the canonical
    /// entry point for a new round. This guard is for mid-round transitions.
    fn transition_to(&mut self, new_step: RoundStep) -> bool {
        let valid = matches!(
            (self.step, new_step),
            (RoundStep::Propose, RoundStep::Prevote)
                | (RoundStep::Prevote, RoundStep::Precommit)
                | (RoundStep::Precommit, RoundStep::Commit)
                // These allow re-entering the same step (idempotent)
                | (RoundStep::Prevote, RoundStep::Prevote)
                | (RoundStep::Precommit, RoundStep::Precommit)
        );
        if valid {
            self.step = new_step;
        } else {
            warn!(
                "⚠️ BFT: Invalid state transition {:?} → {:?} at h={} r={}",
                self.step, new_step, self.height, self.round
            );
        }
        valid
    }

    // ═══════════════════════════════════════════════════════════════
    //  PROPOSAL HANDLING
    // ═══════════════════════════════════════════════════════════════

    /// Called when this node is the designated proposer for (height, round).
    ///
    /// If we have a `valid_value` from a prior round (a block that received
    /// a polka), re-propose it with the `valid_round` set. Otherwise,
    /// propose the freshly built block.
    pub fn create_proposal(
        &mut self,
        fresh_block: Block,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        if self.step != RoundStep::Propose {
            return ConsensusAction::None;
        }

        let (block, valid_round) = if let Some(ref vb) = self.valid_value {
            (vb.clone(), self.valid_round.map(|r| r as i32).unwrap_or(-1))
        } else {
            (fresh_block, -1)
        };

        let block_hash = block.hash();
        let sig_bytes =
            Proposal::signable_bytes_static(self.height, self.round, &block_hash, valid_round);
        let signature = self.keypair.sign(&sig_bytes);

        let proposal = Proposal {
            height: self.height,
            round: self.round,
            block,
            valid_round,
            proposer: self.validator_pubkey,
            signature,
        };

        self.proposal_blocks
            .insert(block_hash, proposal.block.clone());
        self.proposals.insert(self.round, proposal.clone());

        info!(
            "📦 BFT: Proposing block at height={} round={} hash={}",
            self.height,
            self.round,
            hex::encode(&block_hash.0[..4])
        );

        // After proposing, we immediately prevote for our own proposal
        let prevote_action = self.do_prevote(Some(block_hash), validator_set, stake_pool);
        ConsensusAction::Multiple(vec![
            ConsensusAction::BroadcastProposal(self.proposals[&self.round].clone()),
            prevote_action,
        ])
    }

    /// Handle an incoming proposal from the network.
    pub fn on_proposal(
        &mut self,
        proposal: Proposal,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        // Buffer proposals for future heights (G-10 fix)
        if proposal.height > self.height {
            if proposal.height <= self.height + FUTURE_MSG_BUFFER_HEIGHTS {
                debug!(
                    "📥 BFT: Buffering future proposal h={} (current h={})",
                    proposal.height, self.height
                );
                self.future_proposals
                    .entry(proposal.height)
                    .or_default()
                    .push(proposal);
            }
            return ConsensusAction::None;
        }
        // Ignore proposals for past heights
        if proposal.height < self.height {
            return ConsensusAction::None;
        }
        // Ignore proposals for rounds we've already passed
        if proposal.round < self.round {
            return ConsensusAction::None;
        }
        // Verify signature
        if !proposal.verify_signature() {
            warn!(
                "🚨 BFT: Invalid proposal signature from {:?}",
                proposal.proposer
            );
            return ConsensusAction::None;
        }
        // Verify proposer is the correct leader for (height, round)
        let parent_hash = proposal.block.header.parent_hash;
        let leader_slot = self.height * 1000 + proposal.round as u64;
        let expected_leader = validator_set.select_leader_weighted(
            leader_slot,
            stake_pool,
            &parent_hash.0,
            self.min_validator_stake,
        );
        if expected_leader != Some(proposal.proposer) {
            warn!(
                "🚨 BFT: Proposal from non-leader {:?} (expected {:?})",
                proposal.proposer, expected_leader
            );
            return ConsensusAction::None;
        }
        // Verify block signature
        if !proposal.block.verify_signature() {
            warn!("🚨 BFT: Invalid block signature in proposal");
            return ConsensusAction::None;
        }

        // BFT timestamp validation: reject blocks with timestamps too far in the future.
        // Tolerance: 30 seconds (matches CometBFT PBTS precision + message delay).
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let proposed_ts = proposal.block.header.timestamp;
        if let Some(parent_ts) = self.last_committed_block_timestamp {
            if proposed_ts <= parent_ts {
                warn!(
                    "🚨 BFT: Proposal timestamp {} does not advance past parent timestamp {}",
                    proposed_ts, parent_ts
                );
                return ConsensusAction::None;
            }
        }
        if proposed_ts > now_secs + 30 {
            warn!(
                "🚨 BFT: Proposal timestamp {} is too far in the future (now={}, delta={}s)",
                proposed_ts,
                now_secs,
                proposed_ts - now_secs
            );
            return ConsensusAction::None;
        }

        let block_hash = proposal.block.hash();
        self.proposal_blocks
            .insert(block_hash, proposal.block.clone());
        self.proposals.insert(proposal.round, proposal.clone());

        // If this was for a future round, just store it — don't prevote yet
        if proposal.round > self.round {
            return ConsensusAction::None;
        }

        // Already past Propose step for this round
        if self.step != RoundStep::Propose {
            return ConsensusAction::None;
        }

        // Tendermint prevote rule:
        // prevote(h, r, block_hash) if:
        //   - locked_round == None (not locked) OR
        //   - locked_value == block_hash (locked on same value) OR
        //   - proposal.valid_round >= 0 AND proposal.valid_round > locked_round
        //     AND we've seen 2/3+ prevotes for block_hash at valid_round (POL unlock)
        let should_prevote_block =
            if self.locked_round.is_none() || self.locked_value == Some(block_hash) {
                true
            } else if proposal.valid_round >= 0 {
                let vr = proposal.valid_round as u32;
                if let Some(lr) = self.locked_round {
                    vr > lr && self.has_polka_for(vr, &Some(block_hash), validator_set, stake_pool)
                } else {
                    self.has_polka_for(vr, &Some(block_hash), validator_set, stake_pool)
                }
            } else {
                false
            };

        if should_prevote_block {
            self.do_prevote(Some(block_hash), validator_set, stake_pool)
        } else {
            self.do_prevote(None, validator_set, stake_pool)
        }
    }

    // ═══════════════════════════════════════════════════════════════
    //  PREVOTE HANDLING
    // ═══════════════════════════════════════════════════════════════

    /// Handle an incoming prevote from the network.
    pub fn on_prevote(
        &mut self,
        prevote: Prevote,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        // Buffer prevotes for future heights (G-10 fix)
        if prevote.height > self.height {
            if prevote.height <= self.height + FUTURE_MSG_BUFFER_HEIGHTS {
                self.future_prevotes
                    .entry(prevote.height)
                    .or_default()
                    .push(prevote);
            }
            return ConsensusAction::None;
        }
        if prevote.height < self.height {
            return ConsensusAction::None;
        }
        if !prevote.verify_signature() {
            warn!("🚨 BFT: Invalid prevote signature");
            return ConsensusAction::None;
        }
        // Verify voter is in the validator set
        if validator_set.get_validator(&prevote.validator).is_none() {
            return ConsensusAction::None;
        }
        // Deduplicate and detect equivocation (G-9 evidence reactor fix)
        let dedup_key = (prevote.round, prevote.validator);
        if let Some(existing_hash) = self.seen_prevotes.get(&dedup_key) {
            if *existing_hash != prevote.block_hash {
                // EQUIVOCATION: same validator sent conflicting prevotes for (height, round)
                warn!(
                    "🚨 BFT EQUIVOCATION: Double-prevote from {} at h={} r={} (hash1={} vs hash2={})",
                    prevote.validator.to_base58(),
                    self.height,
                    prevote.round,
                    existing_hash.map(|h| hex::encode(&h.0[..4])).unwrap_or_else(|| "nil".into()),
                    prevote.block_hash.map(|h| hex::encode(&h.0[..4])).unwrap_or_else(|| "nil".into()),
                );
                return ConsensusAction::EquivocationDetected {
                    height: self.height,
                    round: prevote.round,
                    validator: prevote.validator,
                    vote_type: "prevote",
                    hash_1: *existing_hash,
                    hash_2: prevote.block_hash,
                };
            }
            // Exact duplicate — ignore
            return ConsensusAction::None;
        }
        self.seen_prevotes.insert(dedup_key, prevote.block_hash);

        // Record the prevote
        self.prevotes
            .entry((prevote.round, prevote.block_hash))
            .or_default()
            .push(prevote.validator);

        let round = prevote.round;
        let mut actions = Vec::new();

        // Rule 1: Upon 2/3+ prevotes for a specific block_hash at current round
        if round == self.round && self.step == RoundStep::Prevote {
            // Find the polka hash (if any) without holding a borrow on self
            let polka_hash = {
                let mut found = None;
                for (key, voters) in &self.prevotes {
                    if key.0 != round {
                        continue;
                    }
                    if let Some(bh) = &key.1 {
                        if self.has_supermajority_voters(voters, validator_set, stake_pool) {
                            found = Some(*bh);
                            break;
                        }
                    }
                }
                found
            };
            if let Some(bh) = polka_hash {
                info!(
                    "🔒 BFT: Polka at height={} round={} for {}",
                    self.height,
                    round,
                    hex::encode(&bh.0[..4])
                );
                self.valid_round = Some(round);
                if let Some(block) = self.proposal_blocks.get(&bh) {
                    self.valid_value = Some(block.clone());
                }
                self.locked_round = Some(round);
                self.locked_value = Some(bh);
                self.transition_to(RoundStep::Precommit);
                actions.push(self.do_precommit(Some(bh), validator_set, stake_pool));
            }
        }

        // Rule 2: Upon 2/3+ prevotes for nil at current round
        if round == self.round && self.step == RoundStep::Prevote {
            let nil_voters = self
                .prevotes
                .get(&(round, None))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if self.has_supermajority_voters(nil_voters, validator_set, stake_pool) {
                info!(
                    "⭕ BFT: Nil polka at height={} round={}",
                    self.height, round
                );
                self.transition_to(RoundStep::Precommit);
                actions.push(self.do_precommit(None, validator_set, stake_pool));
            }
        }

        // Rule 3: Upon 2/3+ prevotes for anything (start prevote timeout)
        if round == self.round
            && self.step == RoundStep::Prevote
            && self.has_any_supermajority_prevotes(round, validator_set, stake_pool)
        {
            actions.push(ConsensusAction::ScheduleTimeout(
                RoundStep::Prevote,
                self.prevote_timeout(),
            ));
        }

        // Tendermint round-skip: if this prevote is for a future round and
        // >1/3 voting power has voted for that round, skip to it.
        if round > self.round {
            let skip = self.check_round_skip(round, validator_set, stake_pool);
            if !matches!(skip, ConsensusAction::None) {
                actions.push(skip);
            }
        }

        if actions.is_empty() {
            ConsensusAction::None
        } else if actions.len() == 1 {
            actions.remove(0)
        } else {
            ConsensusAction::Multiple(actions)
        }
    }

    // ═══════════════════════════════════════════════════════════════
    //  PRECOMMIT HANDLING
    // ═══════════════════════════════════════════════════════════════

    /// Handle an incoming precommit from the network.
    pub fn on_precommit(
        &mut self,
        precommit: Precommit,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        // Buffer precommits for future heights (G-10 fix)
        if precommit.height > self.height {
            if precommit.height <= self.height + FUTURE_MSG_BUFFER_HEIGHTS {
                self.future_precommits
                    .entry(precommit.height)
                    .or_default()
                    .push(precommit);
            }
            return ConsensusAction::None;
        }
        if precommit.height < self.height {
            return ConsensusAction::None;
        }
        if !precommit.verify_signature() {
            warn!("🚨 BFT: Invalid precommit signature");
            return ConsensusAction::None;
        }
        if validator_set.get_validator(&precommit.validator).is_none() {
            return ConsensusAction::None;
        }
        // Deduplicate and detect equivocation (G-9 evidence reactor fix)
        let dedup_key = (precommit.round, precommit.validator);
        if let Some(existing_hash) = self.seen_precommits.get(&dedup_key) {
            if *existing_hash != precommit.block_hash {
                // EQUIVOCATION: same validator sent conflicting precommits for (height, round)
                warn!(
                    "🚨 BFT EQUIVOCATION: Double-precommit from {} at h={} r={} (hash1={} vs hash2={})",
                    precommit.validator.to_base58(),
                    self.height,
                    precommit.round,
                    existing_hash.map(|h| hex::encode(&h.0[..4])).unwrap_or_else(|| "nil".into()),
                    precommit.block_hash.map(|h| hex::encode(&h.0[..4])).unwrap_or_else(|| "nil".into()),
                );
                return ConsensusAction::EquivocationDetected {
                    height: self.height,
                    round: precommit.round,
                    validator: precommit.validator,
                    vote_type: "precommit",
                    hash_1: *existing_hash,
                    hash_2: precommit.block_hash,
                };
            }
            // Exact duplicate — ignore
            return ConsensusAction::None;
        }
        self.seen_precommits.insert(dedup_key, precommit.block_hash);

        // Record the precommit
        self.precommits
            .entry((precommit.round, precommit.block_hash))
            .or_default()
            .push(precommit.validator);

        // Retain precommit signature + timestamp for commit certificate
        self.precommit_sigs.insert(
            (precommit.round, precommit.validator),
            (precommit.signature.clone(), precommit.timestamp),
        );

        let round = precommit.round;
        let mut actions = Vec::new();

        // Rule 1: 2/3+ precommits for a specific block → COMMIT
        // Find the committed hash without holding a borrow on self
        let commit_hash = {
            let mut found = None;
            for (key, voters) in &self.precommits {
                if key.0 != round {
                    continue;
                }
                if let Some(bh) = &key.1 {
                    if self.has_supermajority_voters(voters, validator_set, stake_pool) {
                        found = Some(*bh);
                        break;
                    }
                }
            }
            found
        };
        if let Some(bh) = commit_hash {
            let block_clone = self.proposal_blocks.get(&bh).cloned();
            if let Some(block) = block_clone {
                info!(
                    "✅ BFT: COMMIT at height={} round={} hash={}",
                    self.height,
                    round,
                    hex::encode(&bh.0[..4])
                );
                self.transition_to(RoundStep::Commit);
                self.last_committed_block_timestamp = Some(block.header.timestamp);
                let mut committed = block;
                committed.commit_round = round;
                committed.commit_signatures = self.collect_commit_signatures(round, &bh);
                return ConsensusAction::CommitBlock {
                    height: self.height,
                    round,
                    block: committed,
                    block_hash: bh,
                };
            }
            // We have 2/3+ precommits but don't have the block.
            warn!(
                "⚠️ BFT: 2/3+ precommits for {} but block not found",
                hex::encode(&bh.0[..4])
            );
        }

        // Rule 2: 2/3+ precommits for nil → advance to next round
        let nil_voters = self
            .precommits
            .get(&(round, None))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        if round == self.round
            && self.has_supermajority_voters(nil_voters, validator_set, stake_pool)
        {
            info!(
                "⭕ BFT: Nil commit at height={} round={}, advancing",
                self.height, round
            );
            return self.start_round(round + 1);
        }

        // Rule 3: 2/3+ precommits for anything → start precommit timeout
        if round == self.round
            && self.step == RoundStep::Precommit
            && self.has_any_supermajority_precommits(round, validator_set, stake_pool)
        {
            actions.push(ConsensusAction::ScheduleTimeout(
                RoundStep::Precommit,
                self.precommit_timeout(),
            ));
        }

        // Tendermint round-skip: if this precommit is for a future round and
        // >1/3 voting power has voted for that round, skip to it.
        if round > self.round {
            let skip = self.check_round_skip(round, validator_set, stake_pool);
            if !matches!(skip, ConsensusAction::None) {
                actions.push(skip);
            }
        }

        if actions.is_empty() {
            ConsensusAction::None
        } else if actions.len() == 1 {
            actions.remove(0)
        } else {
            ConsensusAction::Multiple(actions)
        }
    }

    // ═══════════════════════════════════════════════════════════════
    //  FUTURE MESSAGE REPLAY (G-10 fix)
    // ═══════════════════════════════════════════════════════════════

    /// Replay any buffered proposals, prevotes, and precommits for the current
    /// height. Called after `start_height()` to process messages that arrived
    /// while we were still at a previous height. This is critical for fast
    /// catch-up: without it, a validator that falls one height behind would
    /// miss the proposal and all votes, forcing a full round timeout.
    pub fn drain_future_messages(
        &mut self,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        let height = self.height;
        let mut actions = Vec::new();

        // Proposals first (so the block is registered before votes reference it)
        if let Some(proposals) = self.future_proposals.remove(&height) {
            info!(
                "📥 BFT: Replaying {} buffered proposals for height {}",
                proposals.len(),
                height
            );
            for p in proposals {
                let a = self.on_proposal(p, validator_set, stake_pool);
                if !matches!(a, ConsensusAction::None) {
                    actions.push(a);
                }
            }
        }

        // Prevotes
        if let Some(prevotes) = self.future_prevotes.remove(&height) {
            info!(
                "📥 BFT: Replaying {} buffered prevotes for height {}",
                prevotes.len(),
                height
            );
            for pv in prevotes {
                let a = self.on_prevote(pv, validator_set, stake_pool);
                if !matches!(a, ConsensusAction::None) {
                    actions.push(a);
                }
            }
        }

        // Precommits
        if let Some(precommits) = self.future_precommits.remove(&height) {
            info!(
                "📥 BFT: Replaying {} buffered precommits for height {}",
                precommits.len(),
                height
            );
            for pc in precommits {
                let a = self.on_precommit(pc, validator_set, stake_pool);
                if !matches!(a, ConsensusAction::None) {
                    actions.push(a);
                }
            }
        }

        match actions.len() {
            0 => ConsensusAction::None,
            1 => actions.remove(0),
            _ => ConsensusAction::Multiple(actions),
        }
    }

    // ═══════════════════════════════════════════════════════════════
    //  TIMEOUT HANDLING
    // ═══════════════════════════════════════════════════════════════

    /// Called when a timeout fires for the given step at the current round.
    pub fn on_timeout(
        &mut self,
        step: RoundStep,
        timeout_round: u32,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        // Only process timeouts for the current round
        if timeout_round != self.round {
            return ConsensusAction::None;
        }

        match step {
            RoundStep::Propose => {
                if self.step == RoundStep::Propose {
                    info!(
                        "⏰ BFT: Propose timeout at height={} round={}",
                        self.height, self.round
                    );
                    // No proposal received — prevote nil
                    self.do_prevote(None, validator_set, stake_pool)
                } else {
                    ConsensusAction::None
                }
            }
            RoundStep::Prevote => {
                if self.step == RoundStep::Prevote {
                    info!(
                        "⏰ BFT: Prevote timeout at height={} round={}",
                        self.height, self.round
                    );
                    // Didn't reach polka — precommit nil
                    self.transition_to(RoundStep::Precommit);
                    self.do_precommit(None, validator_set, stake_pool)
                } else {
                    ConsensusAction::None
                }
            }
            RoundStep::Precommit => {
                if self.step == RoundStep::Precommit {
                    info!(
                        "⏰ BFT: Precommit timeout at height={} round={}",
                        self.height, self.round
                    );
                    // Didn't reach decision — advance to next round
                    self.start_round(self.round + 1)
                } else {
                    ConsensusAction::None
                }
            }
            RoundStep::Commit => ConsensusAction::None,
        }
    }

    // ═══════════════════════════════════════════════════════════════
    //  INTERNAL HELPERS
    // ═══════════════════════════════════════════════════════════════

    /// Sign and return a prevote. Enforces single-sign per (height, round).
    ///
    /// After recording the self-vote, checks if our own vote creates a polka
    /// (2/3+ prevotes). If so, immediately locks and produces a precommit — this
    /// is critical for single-validator operation and prevents deadlocks.
    fn do_prevote(
        &mut self,
        block_hash: Option<Hash>,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        if self.signed_prevote_rounds.contains_key(&self.round) {
            debug!(
                "BFT: Already signed prevote for round {}, skipping",
                self.round
            );
            return ConsensusAction::None;
        }

        self.transition_to(RoundStep::Prevote);
        let msg = Prevote::signable_bytes(self.height, self.round, &block_hash);
        let signature = self.keypair.sign(&msg);

        let prevote = Prevote {
            height: self.height,
            round: self.round,
            block_hash,
            validator: self.validator_pubkey,
            signature,
        };

        // Record locally so we count our own vote
        self.signed_prevote_rounds.insert(self.round, block_hash);
        self.seen_prevotes
            .insert((self.round, self.validator_pubkey), block_hash);
        self.prevotes
            .entry((self.round, block_hash))
            .or_default()
            .push(self.validator_pubkey);

        debug!(
            "🗳️ BFT: Prevote height={} round={} hash={:?}",
            self.height,
            self.round,
            block_hash.map(|h| hex::encode(&h.0[..4]))
        );

        let broadcast = ConsensusAction::BroadcastPrevote(prevote);

        // Check if our self-vote creates a polka (supermajority of prevotes).
        // This is essential: without it, a solo validator would broadcast
        // its prevote and wait forever for it to come back from the network.
        let round = self.round;
        if let Some(bh) = block_hash {
            let voters = self
                .prevotes
                .get(&(round, Some(bh)))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if self.has_supermajority_voters(voters, validator_set, stake_pool) {
                info!(
                    "🔒 BFT: Polka at height={} round={} for {}",
                    self.height,
                    round,
                    hex::encode(&bh.0[..4])
                );
                self.valid_round = Some(round);
                if let Some(block) = self.proposal_blocks.get(&bh) {
                    self.valid_value = Some(block.clone());
                }
                self.locked_round = Some(round);
                self.locked_value = Some(bh);
                self.transition_to(RoundStep::Precommit);
                let precommit_action = self.do_precommit(Some(bh), validator_set, stake_pool);
                return ConsensusAction::Multiple(vec![broadcast, precommit_action]);
            }
        } else {
            let nil_voters = self
                .prevotes
                .get(&(round, None))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if self.has_supermajority_voters(nil_voters, validator_set, stake_pool) {
                info!(
                    "⭕ BFT: Nil polka at height={} round={}",
                    self.height, round
                );
                self.transition_to(RoundStep::Precommit);
                let precommit_action = self.do_precommit(None, validator_set, stake_pool);
                return ConsensusAction::Multiple(vec![broadcast, precommit_action]);
            }
        }

        broadcast
    }

    /// Sign and return a precommit. Enforces single-sign per (height, round).
    ///
    /// After recording the self-vote, checks if our own precommit creates a
    /// commit (2/3+ precommits for the same block). If so, returns CommitBlock
    /// immediately — critical for single-validator operation.
    fn do_precommit(
        &mut self,
        block_hash: Option<Hash>,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        if self.signed_precommit_rounds.contains_key(&self.round) {
            debug!(
                "BFT: Already signed precommit for round {}, skipping",
                self.round
            );
            return ConsensusAction::None;
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let msg = Precommit::signable_bytes(self.height, self.round, &block_hash, timestamp);
        let signature = self.keypair.sign(&msg);

        let precommit = Precommit {
            height: self.height,
            round: self.round,
            block_hash,
            validator: self.validator_pubkey,
            signature: signature.clone(),
            timestamp,
        };

        self.signed_precommit_rounds.insert(self.round, block_hash);
        self.seen_precommits
            .insert((self.round, self.validator_pubkey), block_hash);
        self.precommits
            .entry((self.round, block_hash))
            .or_default()
            .push(self.validator_pubkey);
        // Retain own signature + timestamp for commit certificate
        self.precommit_sigs
            .insert((self.round, self.validator_pubkey), (signature, timestamp));

        debug!(
            "🗳️ BFT: Precommit height={} round={} hash={:?}",
            self.height,
            self.round,
            block_hash.map(|h| hex::encode(&h.0[..4]))
        );

        let broadcast = ConsensusAction::BroadcastPrecommit(precommit);

        // Check if our self-precommit creates a commit (2/3+ for a block).
        let round = self.round;
        if let Some(bh) = block_hash {
            let voters = self
                .precommits
                .get(&(round, Some(bh)))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let has_commit = self.has_supermajority_voters(voters, validator_set, stake_pool);
            let block_clone = if has_commit {
                self.proposal_blocks.get(&bh).cloned()
            } else {
                None
            };
            if let Some(block) = block_clone {
                info!(
                    "✅ BFT: COMMIT at height={} round={} hash={}",
                    self.height,
                    round,
                    hex::encode(&bh.0[..4])
                );
                self.transition_to(RoundStep::Commit);
                let mut committed = block;
                committed.commit_round = round;
                committed.commit_signatures = self.collect_commit_signatures(round, &bh);
                return ConsensusAction::Multiple(vec![
                    broadcast,
                    ConsensusAction::CommitBlock {
                        height: self.height,
                        round,
                        block: committed,
                        block_hash: bh,
                    },
                ]);
            }
        } else {
            let nil_voters = self
                .precommits
                .get(&(round, None))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if self.has_supermajority_voters(nil_voters, validator_set, stake_pool) {
                info!(
                    "⭕ BFT: Nil commit at height={} round={}, advancing",
                    self.height, round
                );
                let advance = self.start_round(round + 1);
                return ConsensusAction::Multiple(vec![broadcast, advance]);
            }
        }

        broadcast
    }

    /// Check if a set of voters has 2/3+ of total eligible stake.
    fn has_supermajority_voters(
        &self,
        voters: &[Pubkey],
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> bool {
        // Only count votes from active (non-pending) validators.
        // Pending validators are in warmup and must not affect quorum.
        let vote_stake: u128 = voters
            .iter()
            .filter(|pk| {
                validator_set
                    .get_validator(pk)
                    .map(|v| !v.pending_activation)
                    .unwrap_or(false)
            })
            .filter_map(|pk| stake_pool.get_stake(pk))
            .map(|s| s.total_stake() as u128)
            .sum();

        let total_eligible_stake: u128 = validator_set
            .sorted_validators()
            .iter()
            .filter(|v| {
                if v.pending_activation {
                    return false;
                }
                let s = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(0);
                s >= self.min_validator_stake
            })
            .map(|v| {
                stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(0) as u128
            })
            .sum();

        if total_eligible_stake == 0 {
            return false;
        }

        // 2/3 threshold: vote_stake * 3 >= total_eligible_stake * 2
        vote_stake * 3 >= total_eligible_stake * 2
    }

    /// Collect commit signatures for the given round and block hash.
    ///
    /// Gathers all retained precommit signatures from validators that voted
    /// for `block_hash` in `round`, returning them as `CommitSignature` entries
    /// suitable for inclusion in the committed block.
    fn collect_commit_signatures(&self, round: u32, block_hash: &Hash) -> Vec<CommitSignature> {
        let voters = match self.precommits.get(&(round, Some(*block_hash))) {
            Some(v) => v,
            None => return Vec::new(),
        };

        voters
            .iter()
            .filter_map(|pk| {
                self.precommit_sigs
                    .get(&(round, *pk))
                    .map(|(sig, ts)| CommitSignature {
                        validator: pk.0,
                        signature: sig.clone(),
                        timestamp: *ts,
                    })
            })
            .collect()
    }

    /// Check if there's a polka (2/3+ prevotes) for a given value at a given round.
    fn has_polka_for(
        &self,
        round: u32,
        block_hash: &Option<Hash>,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> bool {
        let voters = self.prevotes.get(&(round, *block_hash));
        match voters {
            Some(v) => self.has_supermajority_voters(v, validator_set, stake_pool),
            None => false,
        }
    }

    /// Check if 2/3+ of total stake has prevoted for *any* value in this round.
    fn has_any_supermajority_prevotes(
        &self,
        round: u32,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> bool {
        let total_eligible_stake: u128 = validator_set
            .sorted_validators()
            .iter()
            .filter(|v| {
                if v.pending_activation {
                    return false;
                }
                let s = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(0);
                s >= self.min_validator_stake
            })
            .map(|v| {
                stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(0) as u128
            })
            .sum();

        if total_eligible_stake == 0 {
            return false;
        }

        let total_voted_stake: u128 = self
            .seen_prevotes
            .keys()
            .filter(|(r, _)| *r == round)
            .filter(|(_, pk)| {
                validator_set
                    .get_validator(pk)
                    .map(|v| !v.pending_activation)
                    .unwrap_or(false)
            })
            .filter_map(|(_, pk)| stake_pool.get_stake(pk))
            .map(|s| s.total_stake() as u128)
            .sum();

        total_voted_stake * 3 >= total_eligible_stake * 2
    }

    /// Check if 2/3+ of total stake has precommitted for *any* value in this round.
    fn has_any_supermajority_precommits(
        &self,
        round: u32,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> bool {
        let total_eligible_stake: u128 = validator_set
            .sorted_validators()
            .iter()
            .filter(|v| {
                if v.pending_activation {
                    return false;
                }
                let s = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(0);
                s >= self.min_validator_stake
            })
            .map(|v| {
                stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(0) as u128
            })
            .sum();

        if total_eligible_stake == 0 {
            return false;
        }

        let total_voted_stake: u128 = self
            .seen_precommits
            .keys()
            .filter(|(r, _)| *r == round)
            .filter(|(_, pk)| {
                validator_set
                    .get_validator(pk)
                    .map(|v| !v.pending_activation)
                    .unwrap_or(false)
            })
            .filter_map(|(_, pk)| stake_pool.get_stake(pk))
            .map(|s| s.total_stake() as u128)
            .sum();

        total_voted_stake * 3 >= total_eligible_stake * 2
    }

    /// Tendermint round-skip: if we see votes from >1/3 voting power for
    /// round R' > our round, skip to R'. This prevents permanent deadlocks
    /// when nodes diverge in round numbers.
    /// Tendermint-style round-skip with aggregate future-round counting.
    ///
    /// CometBFT's f+1 rule: if >1/3 of voting power has voted for a round
    /// higher than ours, our round can't reach 2/3 anyway — skip ahead.
    ///
    /// Unlike the basic per-round check, this counts ALL unique voters across
    /// ALL rounds > self.round.  This is critical for convergence after
    /// staggered restarts: if validators are at rounds 7, 8, 9 respectively,
    /// each round has only one vote (25% < 33%).  By aggregating, a validator
    /// at round 7 sees 2 voters in future rounds (50% > 33%) and skips to
    /// the highest round, enabling consensus.
    ///
    /// Safety: if >1/3 of stake has moved past round R, round R can never
    /// gather the required 2/3 supermajority — skipping is always safe.
    fn check_round_skip(
        &mut self,
        _vote_round: u32,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        let total_eligible_stake: u128 = validator_set
            .sorted_validators()
            .iter()
            .filter(|v| {
                if v.pending_activation {
                    return false;
                }
                let s = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(0);
                s >= self.min_validator_stake
            })
            .map(|v| {
                stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(0) as u128
            })
            .sum();

        if total_eligible_stake == 0 {
            return ConsensusAction::None;
        }

        // Collect unique voters who sent prevotes OR precommits for ANY
        // round > self.round, and track the highest round seen.
        let mut future_voters = std::collections::HashSet::new();
        let mut max_round = self.round;
        for (r, pk) in self.seen_prevotes.keys() {
            if *r > self.round {
                future_voters.insert(*pk);
                if *r > max_round {
                    max_round = *r;
                }
            }
        }
        for (r, pk) in self.seen_precommits.keys() {
            if *r > self.round {
                future_voters.insert(*pk);
                if *r > max_round {
                    max_round = *r;
                }
            }
        }

        if max_round == self.round {
            return ConsensusAction::None;
        }

        let future_stake: u128 = future_voters
            .iter()
            .filter(|pk| {
                validator_set
                    .get_validator(pk)
                    .map(|v| !v.pending_activation)
                    .unwrap_or(false)
            })
            .filter_map(|pk| stake_pool.get_stake(pk))
            .map(|s| s.total_stake() as u128)
            .sum();

        // f+1 threshold: future_stake * 3 > total_eligible_stake (i.e., >1/3)
        if future_stake * 3 > total_eligible_stake {
            info!(
                "🔄 BFT: Round skip h={} r={} → r={} (>1/3 stake has voted in future rounds, {} voters)",
                self.height, self.round, max_round, future_voters.len()
            );
            let skip_action = self.start_round(max_round);
            let mut all_actions = vec![skip_action];

            // Fast catch-up loop: rapidly advance through rounds where we
            // already have sufficient vote data (stored proposals, nil polka,
            // nil commit), without waiting for timeouts.  This is critical
            // for late-joining validators that received nil votes from peers
            // while still at a lower round.  Without this, a joining node
            // waits for exponentially-increasing propose timeouts at each
            // skipped round, causing minutes-long stalls.
            for _ in 0..100 {
                let round = self.round;

                // 1. Stored proposal → prevote for it and stop.
                if let Some(proposal) = self.proposals.get(&round).cloned() {
                    let block_hash = proposal.block.hash();
                    let should_prevote_block =
                        if self.locked_round.is_none() || self.locked_value == Some(block_hash) {
                            true
                        } else if proposal.valid_round >= 0 {
                            let vr = proposal.valid_round as u32;
                            if let Some(lr) = self.locked_round {
                                vr > lr
                                    && self.has_polka_for(
                                        vr,
                                        &Some(block_hash),
                                        validator_set,
                                        stake_pool,
                                    )
                            } else {
                                self.has_polka_for(vr, &Some(block_hash), validator_set, stake_pool)
                            }
                        } else {
                            false
                        };
                    let prevote_action = if should_prevote_block {
                        self.do_prevote(Some(block_hash), validator_set, stake_pool)
                    } else {
                        self.do_prevote(None, validator_set, stake_pool)
                    };
                    all_actions.push(prevote_action);
                    break;
                }

                // 2. Nil polka (≥2/3 nil prevotes) → prevote nil and cascade.
                //    do_prevote(None) automatically chains: nil polka detected
                //    → do_precommit(None) → if nil commit → start_round(+1).
                //    This lets the loop advance through multiple nil rounds
                //    in a single call.
                let has_nil_polka = self
                    .prevotes
                    .get(&(round, None))
                    .is_some_and(|v| self.has_supermajority_voters(v, validator_set, stake_pool));
                if has_nil_polka {
                    info!(
                        "🔄 BFT: Fast catch-up: nil polka at h={} r={}, advancing",
                        self.height, round
                    );
                    let prevote_action = self.do_prevote(None, validator_set, stake_pool);
                    all_actions.push(prevote_action);
                    if self.round > round {
                        continue; // Cascaded through nil commit → check next round
                    }
                    break; // At Precommit step, wait for more precommits
                }

                // 3. No stored proposal, no nil polka — wait for proposal.
                break;
            }

            return if all_actions.len() == 1 {
                all_actions.remove(0)
            } else {
                ConsensusAction::Multiple(all_actions)
            };
        }

        ConsensusAction::None
    }

    // ── Timeouts (exponential backoff with 1.5x multiplier, capped at 60s) ──

    /// Compute exponential timeout: base × 1.5^round, capped at MAX_TIMEOUT_MS.
    /// Uses integer arithmetic (×3/2 per round) to avoid floating-point.
    fn exponential_timeout(base_ms: u64, round: u32, max_timeout_ms: u64) -> Duration {
        let mut timeout = base_ms;
        for _ in 0..round.min(20) {
            timeout = (timeout * 3 / 2).min(max_timeout_ms);
        }
        Duration::from_millis(timeout.min(max_timeout_ms))
    }

    fn propose_timeout(&self) -> Duration {
        Self::exponential_timeout(
            self.timeouts.propose_timeout_base_ms,
            self.round,
            self.timeouts.max_phase_timeout_ms,
        )
    }

    pub fn prevote_timeout(&self) -> Duration {
        Self::exponential_timeout(
            self.timeouts.prevote_timeout_base_ms,
            self.round,
            self.timeouts.max_phase_timeout_ms,
        )
    }

    pub fn precommit_timeout(&self) -> Duration {
        Self::exponential_timeout(
            self.timeouts.precommit_timeout_base_ms,
            self.round,
            self.timeouts.max_phase_timeout_ms,
        )
    }

    /// Determine if this validator is the proposer for (height, round)
    /// using the shared leader-election deterministic algorithm.
    pub fn is_proposer(
        &self,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
        parent_hash: &Hash,
    ) -> bool {
        let leader_slot = self.height * 1000 + self.round as u64;
        let leader = validator_set.select_leader_weighted(
            leader_slot,
            stake_pool,
            &parent_hash.0,
            self.min_validator_stake,
        );
        let is_us = leader == Some(self.validator_pubkey);
        if is_us {
            info!(
                "🔑 BFT: Leader election h={} r={} seed={} eligible={} → US",
                self.height,
                self.round,
                hex::encode(&parent_hash.0[..8]),
                validator_set
                    .sorted_validators()
                    .iter()
                    .filter(|v| {
                        if v.pending_activation {
                            return false;
                        }
                        let s = stake_pool
                            .get_stake(&v.pubkey)
                            .map(|s| s.total_stake())
                            .unwrap_or(0);
                        s >= self.min_validator_stake
                    })
                    .count()
            );
        }
        is_us
    }

    /// Get the proposer timeout for the initial start of a round.
    pub fn initial_propose_timeout(&self) -> Duration {
        self.propose_timeout()
    }

    /// Restore locked state from WAL recovery (G-1/G-2 fix).
    /// Called after start_height() if the WAL indicates we were locked
    /// before a crash. This preserves the Tendermint safety invariant.
    pub fn restore_lock(&mut self, height: u64, round: u32, block_hash: Hash) {
        if height == self.height {
            info!(
                "🔐 WAL: Restoring lock from crash recovery: h={} r={} hash={}",
                height,
                round,
                hex::encode(&block_hash.0[..4])
            );
            self.locked_round = Some(round);
            self.locked_value = Some(block_hash);
        }
    }

    /// Get the current locked state (for WAL persistence).
    pub fn locked_state(&self) -> Option<(u32, Hash)> {
        match (self.locked_round, self.locked_value) {
            (Some(r), Some(h)) => Some((r, h)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lichen_core::{Hash, Keypair, Pubkey, StakeInfo, StakePool, ValidatorInfo, ValidatorSet};

    fn make_validator(seed: u8) -> (Keypair, Pubkey) {
        let mut s = [0u8; 32];
        s[0] = seed;
        let kp = Keypair::from_seed(&s);
        let pk = kp.pubkey();
        (kp, pk)
    }

    fn make_test_env(n: usize) -> (Vec<(Keypair, Pubkey)>, ValidatorSet, StakePool) {
        let validators: Vec<(Keypair, Pubkey)> = (1..=n as u8).map(make_validator).collect();
        let mut vs = ValidatorSet::new();
        let mut sp = StakePool::new();
        for (_, pk) in &validators {
            let mut info = ValidatorInfo::new(*pk, 0);
            info.stake = MIN_VALIDATOR_STAKE;
            vs.add_validator(info);
            sp.stake(*pk, MIN_VALIDATOR_STAKE, 0).ok();
        }
        (validators, vs, sp)
    }

    fn make_custom_test_env(stakes: &[u64]) -> (Vec<(Keypair, Pubkey)>, ValidatorSet, StakePool) {
        let validators: Vec<(Keypair, Pubkey)> =
            (1..=stakes.len() as u8).map(make_validator).collect();
        let mut vs = ValidatorSet::new();
        let mut sp = StakePool::new();
        for ((_, pk), stake) in validators.iter().zip(stakes.iter().copied()) {
            let mut info = ValidatorInfo::new(*pk, 0);
            info.stake = stake;
            vs.add_validator(info);
            let entry = StakeInfo::new(*pk, stake, 0);
            sp.upsert_stake_full(entry);
        }
        (validators, vs, sp)
    }

    #[test]
    fn test_prevote_signature_roundtrip() {
        let (kp, pk) = make_validator(1);
        let block_hash = Some(Hash::hash(b"test block"));
        let msg = Prevote::signable_bytes(100, 0, &block_hash);
        let sig = kp.sign(&msg);
        let prevote = Prevote {
            height: 100,
            round: 0,
            block_hash,
            validator: pk,
            signature: sig,
        };
        assert!(prevote.verify_signature());
    }

    #[test]
    fn test_precommit_signature_roundtrip() {
        let (kp, pk) = make_validator(2);
        let block_hash = Some(Hash::hash(b"another block"));
        let ts = 5000u64;
        let msg = Precommit::signable_bytes(50, 1, &block_hash, ts);
        let sig = kp.sign(&msg);
        let precommit = Precommit {
            height: 50,
            round: 1,
            block_hash,
            validator: pk,
            signature: sig,
            timestamp: ts,
        };
        assert!(precommit.verify_signature());
    }

    #[test]
    fn test_nil_prevote_different_from_block_prevote() {
        let bytes_nil = Prevote::signable_bytes(10, 0, &None);
        let bytes_block = Prevote::signable_bytes(10, 0, &Some(Hash::hash(b"block")));
        assert_ne!(bytes_nil, bytes_block);
    }

    #[test]
    fn test_prevote_precommit_different_tags() {
        let h = Some(Hash::hash(b"block"));
        let prevote_bytes = Prevote::signable_bytes(10, 0, &h);
        let precommit_bytes = Precommit::signable_bytes(10, 0, &h, 0);
        // They should differ because of the tag byte (0x01 vs 0x02)
        assert_ne!(prevote_bytes, precommit_bytes);
    }

    #[test]
    fn test_engine_start_height_resets_state() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(42);
        assert_eq!(engine.height, 42);
        assert_eq!(engine.round, 0);
        assert_eq!(engine.step, RoundStep::Propose);
        assert!(engine.locked_round.is_none());
    }

    #[test]
    fn test_supermajority_with_3_validators() {
        let (validators, vs, sp) = make_test_env(3);
        let (kp, pk) = make_validator(1);
        let engine = ConsensusEngine::new(kp, pk);

        // 2 out of 3 with equal stake should be supermajority (66.7%)
        let voters = vec![validators[0].1, validators[1].1];
        assert!(engine.has_supermajority_voters(&voters, &vs, &sp));

        // 1 out of 3 should NOT be supermajority
        let one_voter = vec![validators[0].1];
        assert!(!engine.has_supermajority_voters(&one_voter, &vs, &sp));
    }

    #[test]
    fn test_supermajority_uses_runtime_min_stake() {
        let (validators, vs, sp) = make_custom_test_env(&[60, 60, 60]);
        let (kp, pk) = make_validator(1);
        let engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);

        let voters = vec![validators[0].1, validators[1].1];
        assert!(engine.has_supermajority_voters(&voters, &vs, &sp));
    }

    #[test]
    fn test_supermajority_ignores_cached_validator_stake_without_pool_entry() {
        let (validators, vs, _) = make_custom_test_env(&[
            MIN_VALIDATOR_STAKE,
            MIN_VALIDATOR_STAKE,
            MIN_VALIDATOR_STAKE,
        ]);
        let (kp, pk) = make_validator(1);
        let engine = ConsensusEngine::new_with_min_stake(kp, pk, MIN_VALIDATOR_STAKE);
        let empty_pool = StakePool::new();

        let voters = vec![validators[0].1, validators[1].1];
        assert!(!engine.has_supermajority_voters(&voters, &vs, &empty_pool));
    }

    #[test]
    fn test_round_skip_uses_runtime_min_stake() {
        let (validators, vs, sp) = make_custom_test_env(&[60, 60, 60]);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);
        engine.start_height(1);

        engine.seen_prevotes.insert((2, validators[1].1), None);
        engine.seen_prevotes.insert((2, validators[2].1), None);

        let action = engine.check_round_skip(2, &vs, &sp);
        assert_eq!(engine.round, 2);
        assert!(matches!(
            action,
            ConsensusAction::ScheduleTimeout(RoundStep::Propose, _)
        ));
    }

    #[test]
    fn test_prevote_equivocation_ignored_across_heights() {
        let (_, vs, sp) = make_test_env(2);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);
        let (validator_kp, validator_pk) = make_validator(2);

        engine.start_height(10);

        let block_hash_10 = Some(Hash::hash(b"height-10"));
        let prevote_10 = Prevote {
            height: 10,
            round: 0,
            block_hash: block_hash_10,
            validator: validator_pk,
            signature: validator_kp.sign(&Prevote::signable_bytes(10, 0, &block_hash_10)),
        };
        assert!(matches!(
            engine.on_prevote(prevote_10, &vs, &sp),
            ConsensusAction::None
        ));
        assert_eq!(
            engine.seen_prevotes.get(&(0, validator_pk)),
            Some(&block_hash_10)
        );

        engine.start_height(11);

        let block_hash_11 = Some(Hash::hash(b"height-11"));
        let prevote_11 = Prevote {
            height: 11,
            round: 0,
            block_hash: block_hash_11,
            validator: validator_pk,
            signature: validator_kp.sign(&Prevote::signable_bytes(11, 0, &block_hash_11)),
        };
        assert!(matches!(
            engine.on_prevote(prevote_11, &vs, &sp),
            ConsensusAction::None
        ));
        assert_eq!(
            engine.seen_prevotes.get(&(0, validator_pk)),
            Some(&block_hash_11)
        );
        assert_eq!(engine.seen_prevotes.len(), 1);
    }

    #[test]
    fn test_prevote_equivocation_detected_within_height_round() {
        let (_, vs, sp) = make_test_env(2);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);
        let (validator_kp, validator_pk) = make_validator(2);

        engine.start_height(10);

        let block_hash_a = Some(Hash::hash(b"prevote-a"));
        let first_prevote = Prevote {
            height: 10,
            round: 0,
            block_hash: block_hash_a,
            validator: validator_pk,
            signature: validator_kp.sign(&Prevote::signable_bytes(10, 0, &block_hash_a)),
        };
        assert!(matches!(
            engine.on_prevote(first_prevote, &vs, &sp),
            ConsensusAction::None
        ));

        let block_hash_b = Some(Hash::hash(b"prevote-b"));
        let conflicting_prevote = Prevote {
            height: 10,
            round: 0,
            block_hash: block_hash_b,
            validator: validator_pk,
            signature: validator_kp.sign(&Prevote::signable_bytes(10, 0, &block_hash_b)),
        };

        match engine.on_prevote(conflicting_prevote, &vs, &sp) {
            ConsensusAction::EquivocationDetected {
                height,
                round,
                validator,
                vote_type,
                hash_1,
                hash_2,
            } => {
                assert_eq!(height, 10);
                assert_eq!(round, 0);
                assert_eq!(validator, validator_pk);
                assert_eq!(vote_type, "prevote");
                assert_eq!(hash_1, block_hash_a);
                assert_eq!(hash_2, block_hash_b);
            }
            other => panic!("expected prevote equivocation, got {:?}", other),
        }
    }

    #[test]
    fn test_precommit_equivocation_ignored_across_heights() {
        let (_, vs, sp) = make_test_env(2);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);
        let (validator_kp, validator_pk) = make_validator(2);

        engine.start_height(10);

        let block_hash_10 = Some(Hash::hash(b"precommit-height-10"));
        let precommit_10 = Precommit {
            height: 10,
            round: 0,
            block_hash: block_hash_10,
            validator: validator_pk,
            signature: validator_kp.sign(&Precommit::signable_bytes(10, 0, &block_hash_10, 1)),
            timestamp: 1,
        };
        assert!(matches!(
            engine.on_precommit(precommit_10, &vs, &sp),
            ConsensusAction::None
        ));
        assert_eq!(
            engine.seen_precommits.get(&(0, validator_pk)),
            Some(&block_hash_10)
        );

        engine.start_height(11);

        let block_hash_11 = Some(Hash::hash(b"precommit-height-11"));
        let precommit_11 = Precommit {
            height: 11,
            round: 0,
            block_hash: block_hash_11,
            validator: validator_pk,
            signature: validator_kp.sign(&Precommit::signable_bytes(11, 0, &block_hash_11, 2)),
            timestamp: 2,
        };
        assert!(matches!(
            engine.on_precommit(precommit_11, &vs, &sp),
            ConsensusAction::None
        ));
        assert_eq!(
            engine.seen_precommits.get(&(0, validator_pk)),
            Some(&block_hash_11)
        );
        assert_eq!(engine.seen_precommits.len(), 1);
    }

    #[test]
    fn test_precommit_equivocation_detected_within_height_round() {
        let (_, vs, sp) = make_test_env(2);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);
        let (validator_kp, validator_pk) = make_validator(2);

        engine.start_height(10);

        let block_hash_a = Some(Hash::hash(b"precommit-a"));
        let first_precommit = Precommit {
            height: 10,
            round: 0,
            block_hash: block_hash_a,
            validator: validator_pk,
            signature: validator_kp.sign(&Precommit::signable_bytes(10, 0, &block_hash_a, 1)),
            timestamp: 1,
        };
        assert!(matches!(
            engine.on_precommit(first_precommit, &vs, &sp),
            ConsensusAction::None
        ));

        let block_hash_b = Some(Hash::hash(b"precommit-b"));
        let conflicting_precommit = Precommit {
            height: 10,
            round: 0,
            block_hash: block_hash_b,
            validator: validator_pk,
            signature: validator_kp.sign(&Precommit::signable_bytes(10, 0, &block_hash_b, 2)),
            timestamp: 2,
        };

        match engine.on_precommit(conflicting_precommit, &vs, &sp) {
            ConsensusAction::EquivocationDetected {
                height,
                round,
                validator,
                vote_type,
                hash_1,
                hash_2,
            } => {
                assert_eq!(height, 10);
                assert_eq!(round, 0);
                assert_eq!(validator, validator_pk);
                assert_eq!(vote_type, "precommit");
                assert_eq!(hash_1, block_hash_a);
                assert_eq!(hash_2, block_hash_b);
            }
            other => panic!("expected precommit equivocation, got {:?}", other),
        }
    }

    #[test]
    fn test_exponential_timeout_propose() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);

        // Round 0: base = 2000ms
        engine.round = 0;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(2000));

        // Round 1: 2000 * 1.5 = 3000ms
        engine.round = 1;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(3000));

        // Round 2: 3000 * 1.5 = 4500ms
        engine.round = 2;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(4500));

        // Round 3: 4500 * 1.5 = 6750ms
        engine.round = 3;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(6750));
    }

    #[test]
    fn test_exponential_timeout_prevote() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);

        // Round 0: base = 1000ms
        engine.round = 0;
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(1000));

        // Round 1: 1000 * 1.5 = 1500ms
        engine.round = 1;
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(1500));

        // Round 2: 1500 * 1.5 = 2250ms
        engine.round = 2;
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(2250));
    }

    #[test]
    fn test_exponential_timeout_caps_at_max() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);

        // At very high rounds, should cap at 60 seconds
        engine.round = 50;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(60_000));
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(60_000));
        assert_eq!(engine.precommit_timeout(), Duration::from_millis(60_000));
    }

    #[test]
    fn test_custom_timeout_config_overrides_defaults() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake_and_timeouts(
            kp,
            pk,
            MIN_VALIDATOR_STAKE,
            ConsensusTimeoutConfig {
                propose_timeout_base_ms: 500,
                prevote_timeout_base_ms: 250,
                precommit_timeout_base_ms: 400,
                max_phase_timeout_ms: 1000,
            },
        );

        engine.round = 0;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(500));
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(250));
        assert_eq!(engine.precommit_timeout(), Duration::from_millis(400));

        engine.round = 2;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(1000));
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(562));
        assert_eq!(engine.precommit_timeout(), Duration::from_millis(900));
    }

    // ─── Commit certificate tests (Task 1.2) ────────────────────────

    #[test]
    fn test_commit_block_includes_commit_signatures() {
        // Setup: 3 validators, equal stake. Validators vote until 2/3+ triggers commit.
        let (kp1, pk1) = make_validator(1);
        let (kp2, pk2) = make_validator(2);
        let (kp3, pk3) = make_validator(3);
        // Recreate kp1 from seed so we can still sign with it after moving into engine
        let mut seed1 = [0u8; 32];
        seed1[0] = 1;
        let kp1_sign = Keypair::from_seed(&seed1);

        let mut vs = ValidatorSet::new();
        let mut sp = StakePool::new();
        for (_kp, pk) in [(&kp1, &pk1), (&kp2, &pk2), (&kp3, &pk3)] {
            let vi = lichen_core::ValidatorInfo {
                pubkey: *pk,
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                stake: 100_000_000_000_000,
                joined_slot: 0,
                last_active_slot: 0,
                last_observed_at_ms: 0,
                last_observed_block_at_ms: 0,
                last_observed_block_slot: 0,
                commission_rate: 500,
                transactions_processed: 0,
                pending_activation: false,
            };
            vs.add_validator(vi);
            sp.stake(*pk, 100_000_000_000_000, 0).ok();
        }

        let mut engine = ConsensusEngine::new(kp1, pk1);
        engine.start_height(1);

        // Build a block and register it
        let block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"state"),
            pk1.0,
            Vec::new(),
            1000,
        );
        let block_hash = block.hash();
        engine.proposal_blocks.insert(block_hash, block);

        // kp2 precommits
        let ts2 = 1000u64;
        let signable = Precommit::signable_bytes(1, 0, &Some(block_hash), ts2);
        let pc2 = Precommit {
            height: 1,
            round: 0,
            block_hash: Some(block_hash),
            validator: pk2,
            signature: kp2.sign(&signable),
            timestamp: ts2,
        };
        let _ = engine.on_precommit(pc2, &vs, &sp);

        // kp3 precommits — should trigger commit (kp1's self-vote isn't in yet)
        let ts3 = 1001u64;
        let signable3 = Precommit::signable_bytes(1, 0, &Some(block_hash), ts3);
        let pc3 = Precommit {
            height: 1,
            round: 0,
            block_hash: Some(block_hash),
            validator: pk3,
            signature: kp3.sign(&signable3),
            timestamp: ts3,
        };
        // First, let engine vote itself (step must be Precommit)
        engine.step = RoundStep::Precommit;
        engine
            .precommits
            .entry((0, Some(block_hash)))
            .or_default()
            .push(pk1);
        engine.seen_precommits.insert((0, pk1), Some(block_hash));
        let ts1 = 999u64;
        let signable1 = Precommit::signable_bytes(1, 0, &Some(block_hash), ts1);
        engine
            .precommit_sigs
            .insert((0, pk1), (kp1_sign.sign(&signable1), ts1));
        engine.signed_precommit_rounds.insert(0, Some(block_hash));

        let action = engine.on_precommit(pc3, &vs, &sp);

        // Should produce CommitBlock with commit_signatures
        match action {
            ConsensusAction::CommitBlock { block, .. } => {
                assert!(
                    !block.commit_signatures.is_empty(),
                    "CommitBlock should include commit signatures"
                );
                assert_eq!(block.commit_round, 0);
                // Should have 3 signatures (kp1 + kp2 + kp3)
                assert_eq!(block.commit_signatures.len(), 3);
            }
            other => panic!("Expected CommitBlock, got {:?}", other),
        }
    }

    #[test]
    fn test_on_proposal_rejects_non_monotonic_parent_timestamp() {
        let (kp, pk) = make_validator(1);

        let mut vs = ValidatorSet::new();
        let mut sp = StakePool::new();
        let vi = lichen_core::ValidatorInfo {
            pubkey: pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 100_000_000_000_000,
            joined_slot: 0,
            last_active_slot: 0,
            last_observed_at_ms: 0,
            last_observed_block_at_ms: 0,
            last_observed_block_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        };
        vs.add_validator(vi);
        sp.stake(pk, 100_000_000_000_000, 0).ok();

        let mut block = Block::new_with_timestamp(
            2,
            Hash::default(),
            Hash::hash(b"state"),
            pk.0,
            Vec::new(),
            1_000,
        );
        block.sign(&kp);

        let block_hash = block.hash();
        let signature = kp.sign(&Proposal::signable_bytes_static(2, 0, &block_hash, -1));
        let proposal = Proposal {
            height: 2,
            round: 0,
            block,
            valid_round: -1,
            proposer: pk,
            signature,
        };

        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(2);
        engine.last_committed_block_timestamp = Some(1_000);

        let action = engine.on_proposal(proposal, &vs, &sp);
        match action {
            ConsensusAction::None => {}
            other => panic!("expected proposal rejection, got {:?}", other),
        }
        assert!(engine.proposals.is_empty());
    }

    #[test]
    fn test_precommit_sigs_cleared_on_new_height() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(1);

        // Insert a fake signature + timestamp
        engine
            .precommit_sigs
            .insert((0, pk), (make_validator(42).0.sign(b"fixture"), 1000));
        assert!(!engine.precommit_sigs.is_empty());

        // Start new height
        engine.start_height(2);
        assert!(
            engine.precommit_sigs.is_empty(),
            "Precommit signatures should be cleared on new height"
        );
    }

    #[test]
    fn test_self_precommit_retains_signature() {
        let (kp1, pk1) = make_validator(1);

        let mut vs = ValidatorSet::new();
        let mut sp = StakePool::new();
        let vi = lichen_core::ValidatorInfo {
            pubkey: pk1,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 100_000_000_000_000,
            joined_slot: 0,
            last_active_slot: 0,
            last_observed_at_ms: 0,
            last_observed_block_at_ms: 0,
            last_observed_block_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        };
        vs.add_validator(vi);
        sp.stake(pk1, 100_000_000_000_000, 0).ok();

        let mut engine = ConsensusEngine::new(kp1, pk1);
        engine.start_height(1);
        engine.step = RoundStep::Precommit;

        let block_hash = Hash::hash(b"test_block");
        engine.do_precommit(Some(block_hash), &vs, &sp);

        // Verify our own signature was retained
        assert!(
            engine.precommit_sigs.contains_key(&(0, pk1)),
            "Self-precommit signature should be retained"
        );
        // Verify timestamp is present in the retained entry
        let (_, ts) = engine.precommit_sigs.get(&(0, pk1)).unwrap();
        assert!(*ts > 0, "Precommit timestamp should be non-zero");
    }
}

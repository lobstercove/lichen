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

/// Maximum proposer-selection cache entries retained within one height.
const LEADER_CACHE_MAX_ENTRIES: usize = 256;

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
    /// Consensus observed a commit certificate for a block that is missing
    /// locally. The caller should fetch the committed slot from peers.
    RequestBlockRange {
        start_slot: u64,
        end_slot: u64,
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

#[derive(Debug, Clone, Default)]
struct ConsensusPowerSnapshot {
    eligible_power: HashMap<Pubkey, u64>,
    total_eligible_stake: u128,
}

impl ConsensusPowerSnapshot {
    fn build(
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
        min_validator_stake: u64,
    ) -> Self {
        let mut eligible_power = HashMap::new();
        let mut total_eligible_stake = 0u128;

        for validator in validator_set.sorted_validators() {
            if validator.pending_activation {
                continue;
            }

            let Some(stake) = stake_pool
                .get_stake(&validator.pubkey)
                .map(|stake| stake.total_stake())
            else {
                continue;
            };

            if stake < min_validator_stake {
                continue;
            }

            eligible_power.insert(validator.pubkey, stake);
            total_eligible_stake = total_eligible_stake.saturating_add(u128::from(stake));
        }

        Self {
            eligible_power,
            total_eligible_stake,
        }
    }

    fn eligible_stake(&self, validator: &Pubkey) -> Option<u64> {
        self.eligible_power.get(validator).copied()
    }
}

/// Tendermint-style BFT consensus engine.
///
/// Pure state machine: call methods with incoming messages / timeout events,
/// receive `ConsensusAction` values to execute externally.
pub struct ConsensusEngine {
    // ── Identity ────────────────────────────────────────────────────
    keypair: Keypair,
    pub validator_pubkey: Pubkey,
    signing_chain_id: String,
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
    /// Cached voting power per prevote bucket.
    prevote_power: HashMap<(u32, Option<Hash>), u128>,
    /// Cached voting power per precommit bucket.
    precommit_power: HashMap<(u32, Option<Hash>), u128>,
    /// Cached unique prevote power per round, regardless of value.
    prevote_any_power: HashMap<u32, u128>,
    /// Cached unique precommit power per round, regardless of value.
    precommit_any_power: HashMap<u32, u128>,
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
    /// rejected if they move time backwards.
    last_committed_block_timestamp: Option<u64>,

    // ── Future message buffers (G-10 fix) ───────────────────────────
    // Future proposals are intentionally not buffered here. A proposal must be
    // replayed against application state before this pure consensus state
    // machine can safely prevote for it, so the validator loop owns the
    // future-proposal buffer and validates each proposal before calling
    // `on_proposal`.
    /// Prevotes for heights > self.height.
    future_prevotes: BTreeMap<u64, Vec<Prevote>>,
    /// Precommits for heights > self.height.
    future_precommits: BTreeMap<u64, Vec<Precommit>>,
    /// Frozen voting power for the current height. The validator loop builds
    /// this from the same height-frozen validator set/stake pool used by BFT,
    /// avoiding repeated stake scans on vote hot paths.
    power_snapshot: Option<ConsensusPowerSnapshot>,
    /// Deterministic proposer selections keyed by (leader slot, parent hash).
    /// Cleared at height/snapshot boundaries so validator-set or stake changes
    /// cannot leak into the next height.
    leader_cache: HashMap<(u64, [u8; 32]), Option<Pubkey>>,
}

fn leader_selection_slot(height: u64, round: u32) -> u64 {
    height.saturating_add(round as u64)
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
        Self::new_with_chain_id_min_stake_and_timeouts(
            keypair,
            validator_pubkey,
            "",
            min_validator_stake,
            timeouts,
        )
    }

    /// Create a new consensus engine with chain-id signing domains,
    /// network-specific minimum stake, and timeout configuration.
    pub fn new_with_chain_id_min_stake_and_timeouts(
        keypair: Keypair,
        validator_pubkey: Pubkey,
        signing_chain_id: impl Into<String>,
        min_validator_stake: u64,
        timeouts: ConsensusTimeoutConfig,
    ) -> Self {
        Self {
            keypair,
            validator_pubkey,
            signing_chain_id: signing_chain_id.into(),
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
            prevote_power: HashMap::new(),
            precommit_power: HashMap::new(),
            prevote_any_power: HashMap::new(),
            precommit_any_power: HashMap::new(),
            proposal_blocks: HashMap::new(),
            seen_prevotes: HashMap::new(),
            seen_precommits: HashMap::new(),
            precommit_sigs: HashMap::new(),
            signed_prevote_rounds: HashMap::new(),
            signed_precommit_rounds: HashMap::new(),
            last_committed_block_timestamp: None,
            future_prevotes: BTreeMap::new(),
            future_precommits: BTreeMap::new(),
            power_snapshot: None,
            leader_cache: HashMap::new(),
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
        self.prevote_power.clear();
        self.precommit_power.clear();
        self.prevote_any_power.clear();
        self.precommit_any_power.clear();
        self.proposal_blocks.clear();
        self.seen_prevotes.clear();
        self.seen_precommits.clear();
        self.precommit_sigs.clear();
        self.signed_prevote_rounds.clear();
        self.signed_precommit_rounds.clear();
        self.power_snapshot = None;
        self.leader_cache.clear();
        // Prune future message buffers: discard entries below the new height
        self.future_prevotes.retain(|h, _| *h >= height);
        self.future_precommits.retain(|h, _| *h >= height);
        info!("🔷 BFT: Starting height {} round 0", height);
    }

    /// Cache the current height's deterministic voting power snapshot.
    ///
    /// Callers must pass the same frozen validator set and stake pool used for
    /// this height's proposal/vote handling.
    pub fn rebuild_power_snapshot(&mut self, validator_set: &ValidatorSet, stake_pool: &StakePool) {
        self.leader_cache.clear();
        self.power_snapshot = Some(ConsensusPowerSnapshot::build(
            validator_set,
            stake_pool,
            self.min_validator_stake,
        ));
        self.rebuild_vote_power_tallies();
    }

    fn rebuild_vote_power_tallies(&mut self) {
        self.prevote_power.clear();
        self.precommit_power.clear();
        self.prevote_any_power.clear();
        self.precommit_any_power.clear();

        let Some(snapshot) = &self.power_snapshot else {
            return;
        };

        for (key, voters) in &self.prevotes {
            let power = voters
                .iter()
                .filter_map(|pk| snapshot.eligible_stake(pk))
                .map(u128::from)
                .sum();
            self.prevote_power.insert(*key, power);
        }
        for (round, pk) in self.seen_prevotes.keys() {
            if let Some(power) = snapshot.eligible_stake(pk) {
                *self.prevote_any_power.entry(*round).or_default() += u128::from(power);
            }
        }

        for (key, voters) in &self.precommits {
            let power = voters
                .iter()
                .filter_map(|pk| snapshot.eligible_stake(pk))
                .map(u128::from)
                .sum();
            self.precommit_power.insert(*key, power);
        }
        for (round, pk) in self.seen_precommits.keys() {
            if let Some(power) = snapshot.eligible_stake(pk) {
                *self.precommit_any_power.entry(*round).or_default() += u128::from(power);
            }
        }
    }

    fn expected_leader_cached(
        &mut self,
        leader_slot: u64,
        parent_hash: &Hash,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> Option<Pubkey> {
        let key = (leader_slot, parent_hash.0);
        if let Some(leader) = self.leader_cache.get(&key) {
            return *leader;
        }

        let leader = validator_set.select_leader_weighted(
            leader_slot,
            stake_pool,
            &parent_hash.0,
            self.min_validator_stake,
        );
        if self.leader_cache.len() >= LEADER_CACHE_MAX_ENTRIES {
            self.leader_cache.clear();
        }
        self.leader_cache.insert(key, leader);
        leader
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

    /// Resume after WAL recovery at the first round where this validator has
    /// not already signed. Signed-vote records remain restored for slashing
    /// protection; this only avoids replaying already-exhausted rounds.
    pub fn resume_after_recovered_round(&mut self, recovered_round: u32) {
        let next_round = recovered_round.saturating_add(1);
        if next_round <= self.round {
            return;
        }
        self.round = next_round;
        self.step = RoundStep::Propose;
        info!(
            "🔐 WAL: Resuming height {} at round {} after recovered round {}",
            self.height, self.round, recovered_round
        );
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
    ///   Propose/Prevote → Commit (late commit certificate while catching up)
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
                | (RoundStep::Propose, RoundStep::Commit)
                | (RoundStep::Prevote, RoundStep::Commit)
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

        let (block, valid_round) = if let Some(locked_hash) = self.locked_value {
            let locked_block = self
                .valid_value
                .as_ref()
                .filter(|block| block.hash() == locked_hash)
                .cloned()
                .or_else(|| self.proposal_blocks.get(&locked_hash).cloned());
            let Some(block) = locked_block else {
                warn!(
                    "⚠️ BFT: refusing to propose a new value at h={} r={} while locked on unrecovered {}",
                    self.height,
                    self.round,
                    hex::encode(&locked_hash.0[..4])
                );
                return ConsensusAction::ScheduleTimeout(
                    RoundStep::Propose,
                    self.propose_timeout(),
                );
            };
            (block, self.locked_round.map(|r| r as i32).unwrap_or(-1))
        } else if let Some(ref vb) = self.valid_value {
            (vb.clone(), self.valid_round.map(|r| r as i32).unwrap_or(-1))
        } else {
            (fresh_block, -1)
        };

        let block_hash = block.hash();
        let sig_bytes = Proposal::signing_bytes_static_for_chain_id(
            &self.signing_chain_id,
            self.height,
            self.round,
            &block_hash,
            valid_round,
        );
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

        debug!(
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
        if proposal.height > self.height {
            debug!(
                "📥 BFT: Ignoring future proposal h={} in pure engine; caller must buffer and validate before replay (current h={})",
                proposal.height, self.height
            );
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
        if !proposal.verify_signature_with_chain_id(&self.signing_chain_id) {
            warn!(
                "🚨 BFT: Invalid proposal signature from {:?}",
                proposal.proposer
            );
            return ConsensusAction::None;
        }
        // Verify proposer is the correct leader for (height, round)
        let parent_hash = proposal.block.header.parent_hash;
        let leader_slot = leader_selection_slot(self.height, proposal.round);
        let expected_leader =
            self.expected_leader_cached(leader_slot, &parent_hash, validator_set, stake_pool);
        if expected_leader != Some(proposal.proposer) {
            warn!(
                "🚨 BFT: Proposal from non-leader {:?} (expected {:?})",
                proposal.proposer, expected_leader
            );
            return ConsensusAction::None;
        }
        // Verify block signature
        if !proposal
            .block
            .verify_signature_with_chain_id(&self.signing_chain_id)
        {
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
            if proposed_ts < parent_ts {
                warn!(
                    "🚨 BFT: Proposal timestamp {} is older than parent timestamp {}",
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
        if !prevote.verify_signature_with_chain_id(&self.signing_chain_id) {
            warn!("🚨 BFT: Invalid prevote signature");
            return ConsensusAction::None;
        }
        // Verify voter is eligible for this height's BFT voting power.
        let Some(voter_power) = self.eligible_stake(&prevote.validator, validator_set, stake_pool)
        else {
            return ConsensusAction::None;
        };
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
        self.add_prevote_power(prevote.round, prevote.block_hash, voter_power);

        let round = prevote.round;
        let mut actions = Vec::new();

        // Rule 1: Upon 2/3+ prevotes for a specific block_hash at current round
        if round == self.round && self.step == RoundStep::Prevote {
            // Find the polka hash (if any) without holding a borrow on self
            let polka_hash = {
                let mut found = None;
                for key in self.prevotes.keys() {
                    if key.0 != round {
                        continue;
                    }
                    if let Some(bh) = key.1 {
                        let vote_power =
                            self.prevote_power.get(key).copied().unwrap_or_else(|| {
                                self.prevotes
                                    .get(key)
                                    .map(|voters| {
                                        self.voter_stake(voters.iter(), validator_set, stake_pool)
                                    })
                                    .unwrap_or(0)
                            });
                        if self.has_supermajority_power(vote_power, validator_set, stake_pool) {
                            found = Some(bh);
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
            let nil_key = (round, None);
            let nil_power = self
                .prevote_power
                .get(&nil_key)
                .copied()
                .unwrap_or_else(|| {
                    self.prevotes
                        .get(&nil_key)
                        .map(|voters| self.voter_stake(voters.iter(), validator_set, stake_pool))
                        .unwrap_or(0)
                });
            if self.has_supermajority_power(nil_power, validator_set, stake_pool) {
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
        if !precommit.verify_signature_with_chain_id(&self.signing_chain_id) {
            warn!("🚨 BFT: Invalid precommit signature");
            return ConsensusAction::None;
        }
        let Some(voter_power) =
            self.eligible_stake(&precommit.validator, validator_set, stake_pool)
        else {
            return ConsensusAction::None;
        };
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
        if self.step == RoundStep::Commit {
            return ConsensusAction::None;
        }
        self.seen_precommits.insert(dedup_key, precommit.block_hash);

        // Record the precommit
        self.precommits
            .entry((precommit.round, precommit.block_hash))
            .or_default()
            .push(precommit.validator);
        self.add_precommit_power(precommit.round, precommit.block_hash, voter_power);

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
            for key in self.precommits.keys() {
                if key.0 != round {
                    continue;
                }
                if let Some(bh) = key.1 {
                    let vote_power = self.precommit_power.get(key).copied().unwrap_or_else(|| {
                        self.precommits
                            .get(key)
                            .map(|voters| {
                                self.voter_stake(voters.iter(), validator_set, stake_pool)
                            })
                            .unwrap_or(0)
                    });
                    if self.has_supermajority_power(vote_power, validator_set, stake_pool) {
                        found = Some(bh);
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
                committed.commit_signatures =
                    self.collect_commit_signatures(round, &bh, validator_set, stake_pool);
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
            actions.push(ConsensusAction::RequestBlockRange {
                start_slot: self.height,
                end_slot: self.height,
                block_hash: bh,
            });
        }

        // Rule 2: 2/3+ precommits for nil → advance to next round
        let nil_key = (round, None);
        let nil_power = self
            .precommit_power
            .get(&nil_key)
            .copied()
            .unwrap_or_else(|| {
                self.precommits
                    .get(&nil_key)
                    .map(|voters| self.voter_stake(voters.iter(), validator_set, stake_pool))
                    .unwrap_or(0)
            });
        if round == self.round && self.has_supermajority_power(nil_power, validator_set, stake_pool)
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

    /// Replay any buffered prevotes and precommits for the current height.
    /// Called after `start_height()` to process messages that arrived
    /// while we were still at a previous height. This is critical for fast
    /// catch-up. Future proposals are buffered in the validator loop instead,
    /// because they require application-state validation before prevote.
    pub fn drain_future_messages(
        &mut self,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        let height = self.height;
        let mut actions = Vec::new();

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
        let msg = Prevote::signing_bytes_for_chain_id(
            &self.signing_chain_id,
            self.height,
            self.round,
            &block_hash,
        );
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
        if let Some(voter_power) =
            self.eligible_stake(&self.validator_pubkey, validator_set, stake_pool)
        {
            self.add_prevote_power(self.round, block_hash, voter_power);
        }

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
            let vote_power = self
                .prevote_power
                .get(&(round, Some(bh)))
                .copied()
                .unwrap_or_else(|| self.voter_stake(voters.iter(), validator_set, stake_pool));
            if self.has_supermajority_power(vote_power, validator_set, stake_pool) {
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
            let nil_power = self
                .prevote_power
                .get(&(round, None))
                .copied()
                .unwrap_or_else(|| self.voter_stake(nil_voters.iter(), validator_set, stake_pool));
            if self.has_supermajority_power(nil_power, validator_set, stake_pool) {
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

        let msg = Precommit::signing_bytes_for_chain_id(
            &self.signing_chain_id,
            self.height,
            self.round,
            &block_hash,
            timestamp,
        );
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
        if let Some(voter_power) =
            self.eligible_stake(&self.validator_pubkey, validator_set, stake_pool)
        {
            self.add_precommit_power(self.round, block_hash, voter_power);
        }

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
            let vote_power = self
                .precommit_power
                .get(&(round, Some(bh)))
                .copied()
                .unwrap_or_else(|| self.voter_stake(voters.iter(), validator_set, stake_pool));
            let has_commit = self.has_supermajority_power(vote_power, validator_set, stake_pool);
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
                committed.commit_signatures =
                    self.collect_commit_signatures(round, &bh, validator_set, stake_pool);
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
            let nil_power = self
                .precommit_power
                .get(&(round, None))
                .copied()
                .unwrap_or_else(|| self.voter_stake(nil_voters.iter(), validator_set, stake_pool));
            if self.has_supermajority_power(nil_power, validator_set, stake_pool) {
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
        let vote_stake = self.voter_stake(voters.iter(), validator_set, stake_pool);
        let total_eligible_stake = self.total_eligible_stake(validator_set, stake_pool);

        if total_eligible_stake == 0 {
            return false;
        }

        // 2/3 threshold: vote_stake * 3 >= total_eligible_stake * 2
        vote_stake * 3 >= total_eligible_stake * 2
    }

    fn eligible_stake(
        &self,
        validator: &Pubkey,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> Option<u64> {
        if let Some(snapshot) = &self.power_snapshot {
            return snapshot.eligible_stake(validator);
        }

        let info = validator_set.get_validator(validator)?;
        if info.pending_activation {
            return None;
        }
        let stake = stake_pool.get_stake(validator)?.total_stake();
        if stake < self.min_validator_stake {
            return None;
        }
        Some(stake)
    }

    fn is_eligible_validator(
        &self,
        validator: &Pubkey,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> bool {
        self.eligible_stake(validator, validator_set, stake_pool)
            .is_some()
    }

    fn voter_stake<'a>(
        &self,
        voters: impl IntoIterator<Item = &'a Pubkey>,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> u128 {
        voters
            .into_iter()
            .filter_map(|pk| self.eligible_stake(pk, validator_set, stake_pool))
            .map(u128::from)
            .sum()
    }

    fn total_eligible_stake(&self, validator_set: &ValidatorSet, stake_pool: &StakePool) -> u128 {
        if let Some(snapshot) = &self.power_snapshot {
            return snapshot.total_eligible_stake;
        }

        validator_set
            .sorted_validators()
            .iter()
            .filter_map(|v| self.eligible_stake(&v.pubkey, validator_set, stake_pool))
            .map(u128::from)
            .sum()
    }

    fn add_prevote_power(&mut self, round: u32, block_hash: Option<Hash>, voting_power: u64) {
        let power = u128::from(voting_power);
        *self.prevote_power.entry((round, block_hash)).or_default() += power;
        *self.prevote_any_power.entry(round).or_default() += power;
    }

    fn add_precommit_power(&mut self, round: u32, block_hash: Option<Hash>, voting_power: u64) {
        let power = u128::from(voting_power);
        *self.precommit_power.entry((round, block_hash)).or_default() += power;
        *self.precommit_any_power.entry(round).or_default() += power;
    }

    fn has_supermajority_power(
        &self,
        vote_power: u128,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> bool {
        let total_eligible_stake = self.total_eligible_stake(validator_set, stake_pool);
        total_eligible_stake > 0 && vote_power * 3 >= total_eligible_stake * 2
    }

    /// Collect commit signatures for the given round and block hash.
    ///
    /// Gathers all retained precommit signatures from validators that voted
    /// for `block_hash` in `round`, returning them as `CommitSignature` entries
    /// suitable for inclusion in the committed block.
    fn collect_commit_signatures(
        &self,
        round: u32,
        block_hash: &Hash,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> Vec<CommitSignature> {
        let voters = match self.precommits.get(&(round, Some(*block_hash))) {
            Some(v) => v,
            None => return Vec::new(),
        };

        voters
            .iter()
            .filter(|pk| self.is_eligible_validator(pk, validator_set, stake_pool))
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
        let total_eligible_stake = self.total_eligible_stake(validator_set, stake_pool);

        if total_eligible_stake == 0 {
            return false;
        }

        if let Some(vote_power) = self.prevote_any_power.get(&round).copied() {
            return vote_power * 3 >= total_eligible_stake * 2;
        }

        let total_voted_stake: u128 = self
            .seen_prevotes
            .keys()
            .filter(|(r, _)| *r == round)
            .filter_map(|(_, pk)| self.eligible_stake(pk, validator_set, stake_pool))
            .map(u128::from)
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
        let total_eligible_stake = self.total_eligible_stake(validator_set, stake_pool);

        if total_eligible_stake == 0 {
            return false;
        }

        if let Some(vote_power) = self.precommit_any_power.get(&round).copied() {
            return vote_power * 3 >= total_eligible_stake * 2;
        }

        let total_voted_stake: u128 = self
            .seen_precommits
            .keys()
            .filter(|(r, _)| *r == round)
            .filter_map(|(_, pk)| self.eligible_stake(pk, validator_set, stake_pool))
            .map(u128::from)
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
        let total_eligible_stake = self.total_eligible_stake(validator_set, stake_pool);

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
            .filter_map(|pk| self.eligible_stake(pk, validator_set, stake_pool))
            .map(u128::from)
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
        &mut self,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
        parent_hash: &Hash,
    ) -> bool {
        let leader_slot = leader_selection_slot(self.height, self.round);
        let leader =
            self.expected_leader_cached(leader_slot, parent_hash, validator_set, stake_pool);
        let is_us = leader == Some(self.validator_pubkey);
        if is_us {
            debug!(
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

    /// Restore a proposal block from WAL recovery.
    pub fn restore_proposal_block(&mut self, height: u64, round: u32, block: Block) {
        if height != self.height {
            return;
        }
        let block_hash = block.hash();
        info!(
            "🔐 WAL: Restoring proposal block: h={} r={} hash={}",
            height,
            round,
            hex::encode(&block_hash.0[..4])
        );
        self.proposal_blocks.insert(block_hash, block.clone());
        if self.locked_value == Some(block_hash) {
            self.valid_round = Some(round);
            self.valid_value = Some(block);
        }
    }

    /// Release a restored lock only when its proposal value is unrecoverable.
    ///
    /// Signed vote records remain restored, so the validator still cannot
    /// double-sign any recovered height/round. This is only for legacy or
    /// corrupted WALs that contain a lock hash but not the corresponding
    /// proposal block; a Tendermint lock without its value cannot make progress.
    pub fn release_unrecoverable_lock_without_value(
        &mut self,
        height: u64,
        block_hash: Hash,
    ) -> bool {
        if height != self.height || self.locked_value != Some(block_hash) {
            return false;
        }
        if self.proposal_blocks.contains_key(&block_hash) {
            return false;
        }
        warn!(
            "⚠️ BFT: releasing unrecoverable restored lock at h={} hash={} because the proposal value is absent from WAL",
            height,
            hex::encode(&block_hash.0[..4])
        );
        self.locked_round = None;
        self.locked_value = None;
        if self
            .valid_value
            .as_ref()
            .is_some_and(|block| block.hash() == block_hash)
        {
            self.valid_value = None;
            self.valid_round = None;
        }
        true
    }

    /// Restore a signed local prevote from WAL slashing-protection state.
    pub fn restore_signed_prevote(
        &mut self,
        height: u64,
        round: u32,
        block_hash: Option<Hash>,
        signature: PqSignature,
    ) -> Result<(), String> {
        if height != self.height {
            return Ok(());
        }
        if let Some(existing) = self.signed_prevote_rounds.get(&round) {
            if *existing != block_hash {
                return Err(format!(
                    "conflicting recovered prevote for height={} round={}",
                    height, round
                ));
            }
            return Ok(());
        }
        let signable =
            Prevote::signing_bytes_for_chain_id(&self.signing_chain_id, height, round, &block_hash);
        if !Keypair::verify(&self.validator_pubkey, &signable, &signature) {
            let legacy = Prevote::signable_bytes(height, round, &block_hash);
            if !Keypair::verify(&self.validator_pubkey, &legacy, &signature) {
                return Err(format!(
                    "invalid recovered prevote signature for height={} round={}",
                    height, round
                ));
            }
        }

        info!(
            "🔐 WAL: Restoring signed prevote: h={} r={} hash={:?}",
            height,
            round,
            block_hash.map(|h| hex::encode(&h.0[..4]))
        );
        self.signed_prevote_rounds.insert(round, block_hash);
        self.seen_prevotes
            .insert((round, self.validator_pubkey), block_hash);
        let voters = self.prevotes.entry((round, block_hash)).or_default();
        if !voters.contains(&self.validator_pubkey) {
            voters.push(self.validator_pubkey);
            if let Some(power) = self
                .power_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.eligible_stake(&self.validator_pubkey))
            {
                self.add_prevote_power(round, block_hash, power);
            }
        }
        Ok(())
    }

    /// Restore a signed local precommit from WAL slashing-protection state.
    pub fn restore_signed_precommit(
        &mut self,
        height: u64,
        round: u32,
        block_hash: Option<Hash>,
        signature: PqSignature,
        timestamp: u64,
    ) -> Result<(), String> {
        if height != self.height {
            return Ok(());
        }
        if let Some(existing) = self.signed_precommit_rounds.get(&round) {
            if *existing != block_hash {
                return Err(format!(
                    "conflicting recovered precommit for height={} round={}",
                    height, round
                ));
            }
            return Ok(());
        }
        let signable = Precommit::signing_bytes_for_chain_id(
            &self.signing_chain_id,
            height,
            round,
            &block_hash,
            timestamp,
        );
        if !Keypair::verify(&self.validator_pubkey, &signable, &signature) {
            let legacy = Precommit::signable_bytes(height, round, &block_hash, timestamp);
            if !Keypair::verify(&self.validator_pubkey, &legacy, &signature) {
                return Err(format!(
                    "invalid recovered precommit signature for height={} round={}",
                    height, round
                ));
            }
        }

        info!(
            "🔐 WAL: Restoring signed precommit: h={} r={} hash={:?}",
            height,
            round,
            block_hash.map(|h| hex::encode(&h.0[..4]))
        );
        self.signed_precommit_rounds.insert(round, block_hash);
        self.seen_precommits
            .insert((round, self.validator_pubkey), block_hash);
        let voters = self.precommits.entry((round, block_hash)).or_default();
        if !voters.contains(&self.validator_pubkey) {
            voters.push(self.validator_pubkey);
            if let Some(power) = self
                .power_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.eligible_stake(&self.validator_pubkey))
            {
                self.add_precommit_power(round, block_hash, power);
            }
        }
        self.precommit_sigs
            .insert((round, self.validator_pubkey), (signature, timestamp));
        if let Some(hash) = block_hash {
            if self
                .locked_round
                .is_none_or(|locked_round| round >= locked_round)
            {
                self.locked_round = Some(round);
                self.locked_value = Some(hash);
            }
        }
        Ok(())
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
    fn test_bft_leader_slot_mapping_covers_four_round_zero_validators() {
        let (validators, vs, sp) = make_test_env(4);
        let expected: std::collections::BTreeSet<_> =
            validators.iter().map(|(_, pk)| *pk).collect();
        let mut observed = std::collections::BTreeSet::new();

        for height in 1..=16 {
            let leader_slot = leader_selection_slot(height, 0);
            if let Some(leader) =
                vs.select_leader_weighted(leader_slot, &sp, &[], MIN_VALIDATOR_STAKE)
            {
                observed.insert(leader);
            }
        }

        assert_eq!(
            observed, expected,
            "round-0 leader mapping must not starve one validator in a four-validator set"
        );
    }

    #[test]
    fn test_bft_leader_slot_mapping_advances_on_round_change() {
        let height = 42;
        assert_eq!(leader_selection_slot(height, 0), height);
        assert_eq!(leader_selection_slot(height, 1), height + 1);
        assert_eq!(leader_selection_slot(height, 2), height + 2);
    }

    #[test]
    fn test_bft_leader_cache_matches_weighted_selection() {
        let stakes = [
            MIN_VALIDATOR_STAKE,
            MIN_VALIDATOR_STAKE * 2,
            MIN_VALIDATOR_STAKE * 3,
            MIN_VALIDATOR_STAKE * 4,
        ];
        let (validators, vs, sp) = make_custom_test_env(&stakes);
        let (kp, pk) = &validators[0];
        let mut engine = ConsensusEngine::new_with_min_stake(
            Keypair::from_seed(kp.secret_key()),
            *pk,
            MIN_VALIDATOR_STAKE,
        );
        engine.start_height(123);

        for parent_hash in [Hash::hash(b"parent-a"), Hash::hash(b"parent-b")] {
            for round in 0..32 {
                let leader_slot = leader_selection_slot(engine.height, round);
                let direct = vs.select_leader_weighted(
                    leader_slot,
                    &sp,
                    &parent_hash.0,
                    MIN_VALIDATOR_STAKE,
                );
                let cached = engine.expected_leader_cached(leader_slot, &parent_hash, &vs, &sp);
                assert_eq!(cached, direct);
                let cached_again =
                    engine.expected_leader_cached(leader_slot, &parent_hash, &vs, &sp);
                assert_eq!(cached_again, direct);
            }
        }

        assert!(!engine.leader_cache.is_empty());
    }

    #[test]
    fn test_bft_leader_cache_clears_on_height_and_snapshot_rebuild() {
        let (validators, vs, sp) = make_test_env(4);
        let (kp, pk) = &validators[0];
        let mut engine = ConsensusEngine::new_with_min_stake(
            Keypair::from_seed(kp.secret_key()),
            *pk,
            MIN_VALIDATOR_STAKE,
        );
        engine.start_height(7);
        let parent_hash = Hash::hash(b"parent");
        let leader_slot = leader_selection_slot(engine.height, engine.round);

        engine.expected_leader_cached(leader_slot, &parent_hash, &vs, &sp);
        assert_eq!(engine.leader_cache.len(), 1);

        engine.rebuild_power_snapshot(&vs, &sp);
        assert!(engine.leader_cache.is_empty());

        engine.expected_leader_cached(leader_slot, &parent_hash, &vs, &sp);
        assert_eq!(engine.leader_cache.len(), 1);

        engine.start_height(8);
        assert!(engine.leader_cache.is_empty());
    }

    #[test]
    fn test_bft_leader_cache_snapshot_rebuild_prevents_stale_context() {
        let stakes = [MIN_VALIDATOR_STAKE, MIN_VALIDATOR_STAKE * 8];
        let (validators, vs, sp) = make_custom_test_env(&stakes);
        let mut active_only_vs = vs.clone();
        active_only_vs
            .get_validator_mut(&validators[1].1)
            .expect("second validator exists")
            .pending_activation = true;

        let (kp, pk) = &validators[0];
        let mut engine = ConsensusEngine::new_with_min_stake(
            Keypair::from_seed(kp.secret_key()),
            *pk,
            MIN_VALIDATOR_STAKE,
        );
        engine.start_height(77);
        let leader_slot = leader_selection_slot(engine.height, engine.round);

        let mut chosen_parent = None;
        for seed in 0..4096u64 {
            let parent_hash = Hash::hash(&seed.to_le_bytes());
            let before =
                vs.select_leader_weighted(leader_slot, &sp, &parent_hash.0, MIN_VALIDATOR_STAKE);
            let after = active_only_vs.select_leader_weighted(
                leader_slot,
                &sp,
                &parent_hash.0,
                MIN_VALIDATOR_STAKE,
            );
            if before != after {
                chosen_parent = Some((parent_hash, before, after));
                break;
            }
        }

        let (parent_hash, first_leader, second_leader) =
            chosen_parent.expect("test fixture should expose a leader change");
        assert_eq!(second_leader, Some(validators[0].1));

        let cached_first = engine.expected_leader_cached(leader_slot, &parent_hash, &vs, &sp);
        assert_eq!(cached_first, first_leader);
        assert!(!engine.leader_cache.is_empty());

        engine.rebuild_power_snapshot(&active_only_vs, &sp);
        assert!(engine.leader_cache.is_empty());

        let cached_second =
            engine.expected_leader_cached(leader_slot, &parent_hash, &active_only_vs, &sp);
        assert_eq!(cached_second, second_leader);
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
    fn test_resume_after_recovered_round_advances_to_first_unsigned_round() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(42);

        let hash = Hash::hash(b"recovered");
        let prevote_sig = engine
            .keypair
            .sign(&Prevote::signable_bytes(42, 3, &Some(hash)));
        engine
            .restore_signed_prevote(42, 3, Some(hash), prevote_sig)
            .unwrap();

        engine.resume_after_recovered_round(3);

        assert_eq!(engine.height, 42);
        assert_eq!(engine.round, 4);
        assert_eq!(engine.step, RoundStep::Propose);
        assert_eq!(engine.signed_prevote_rounds.get(&3), Some(&Some(hash)));
    }

    #[test]
    fn test_restore_signed_prevote_blocks_conflicting_self_vote() {
        let (validators, vs, sp) = make_test_env(1);
        let (kp, pk) = validators.into_iter().next().unwrap();
        let restored_hash = Hash::hash(b"restored");
        let restored_sig = kp.sign(&Prevote::signable_bytes(5, 0, &Some(restored_hash)));
        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(5);

        engine
            .restore_signed_prevote(5, 0, Some(restored_hash), restored_sig)
            .unwrap();

        let action = engine.do_prevote(Some(Hash::hash(b"conflict")), &vs, &sp);
        assert!(matches!(action, ConsensusAction::None));
        assert_eq!(
            engine.signed_prevote_rounds.get(&0),
            Some(&Some(restored_hash))
        );
    }

    #[test]
    fn test_restore_signed_precommit_blocks_conflicting_self_vote() {
        let (validators, vs, sp) = make_test_env(1);
        let (kp, pk) = validators.into_iter().next().unwrap();
        let restored_hash = Hash::hash(b"restored-precommit");
        let timestamp = 1_700_000_000;
        let restored_sig = kp.sign(&Precommit::signable_bytes(
            6,
            0,
            &Some(restored_hash),
            timestamp,
        ));
        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(6);

        engine
            .restore_signed_precommit(6, 0, Some(restored_hash), restored_sig, timestamp)
            .unwrap();

        let action = engine.do_precommit(Some(Hash::hash(b"conflict")), &vs, &sp);
        assert!(matches!(action, ConsensusAction::None));
        assert_eq!(
            engine.signed_precommit_rounds.get(&0),
            Some(&Some(restored_hash))
        );
        assert_eq!(engine.locked_state(), Some((0, restored_hash)));
    }

    #[test]
    fn test_locked_proposer_requires_recovered_block_value() {
        let (validators, vs, sp) = make_test_env(1);
        let (kp, pk) = validators.into_iter().next().unwrap();
        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(77);

        let locked_hash = Hash::hash(b"locked-but-missing");
        engine.restore_lock(77, 2, locked_hash);
        engine.round = 3;
        let fresh_block = Block::new_with_timestamp(
            77,
            Hash::default(),
            Hash::hash(b"fresh"),
            pk.0,
            Vec::new(),
            1_700_000_010,
        );

        let action = engine.create_proposal(fresh_block, &vs, &sp);
        assert!(matches!(
            action,
            ConsensusAction::ScheduleTimeout(RoundStep::Propose, _)
        ));
        assert!(!engine.signed_prevote_rounds.contains_key(&3));
    }

    #[test]
    fn test_release_unrecoverable_lock_keeps_signed_vote_protection() {
        let (validators, vs, sp) = make_test_env(1);
        let (kp, pk) = validators.into_iter().next().unwrap();
        let restored_hash = Hash::hash(b"missing-proposal");
        let restored_sig = kp.sign(&Prevote::signable_bytes(80, 2, &Some(restored_hash)));
        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(80);
        engine
            .restore_signed_prevote(80, 2, Some(restored_hash), restored_sig)
            .unwrap();
        engine.restore_lock(80, 2, restored_hash);

        assert!(engine.release_unrecoverable_lock_without_value(80, restored_hash));
        assert_eq!(engine.locked_state(), None);
        assert_eq!(
            engine.signed_prevote_rounds.get(&2),
            Some(&Some(restored_hash))
        );

        engine.round = 3;
        let fresh_block = Block::new_with_timestamp(
            80,
            Hash::default(),
            Hash::hash(b"fresh-after-release"),
            pk.0,
            Vec::new(),
            1_700_000_040,
        );
        let fresh_hash = fresh_block.hash();
        let action = engine.create_proposal(fresh_block, &vs, &sp);
        assert!(matches!(action, ConsensusAction::Multiple(_)));
        assert_eq!(
            engine.signed_prevote_rounds.get(&3),
            Some(&Some(fresh_hash))
        );
        assert_eq!(
            engine.signed_prevote_rounds.get(&2),
            Some(&Some(restored_hash))
        );
    }

    #[test]
    fn test_locked_proposer_reproposes_recovered_block_value() {
        let (validators, vs, sp) = make_test_env(1);
        let (kp, pk) = validators.into_iter().next().unwrap();
        let mut engine = ConsensusEngine::new(kp, pk);
        engine.start_height(78);

        let locked_block = Block::new_with_timestamp(
            78,
            Hash::default(),
            Hash::hash(b"locked-state"),
            pk.0,
            Vec::new(),
            1_700_000_020,
        );
        let locked_hash = locked_block.hash();
        engine.restore_lock(78, 2, locked_hash);
        engine.restore_proposal_block(78, 2, locked_block.clone());
        engine.round = 3;

        let fresh_block = Block::new_with_timestamp(
            78,
            Hash::default(),
            Hash::hash(b"fresh-state"),
            pk.0,
            Vec::new(),
            1_700_000_030,
        );
        let action = engine.create_proposal(fresh_block, &vs, &sp);

        let ConsensusAction::Multiple(actions) = action else {
            panic!("expected proposal and prevote actions");
        };
        let proposal = actions.iter().find_map(|action| {
            if let ConsensusAction::BroadcastProposal(proposal) = action {
                Some(proposal)
            } else {
                None
            }
        });
        let proposal = proposal.expect("proposal action");
        assert_eq!(proposal.block.hash(), locked_hash);
        assert_eq!(proposal.valid_round, 2);
        assert_eq!(
            engine.signed_prevote_rounds.get(&3),
            Some(&Some(locked_hash))
        );
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
    fn test_power_snapshot_matches_runtime_min_stake_supermajority() {
        let (validators, vs, sp) = make_custom_test_env(&[60, 60, 49]);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);
        engine.start_height(1);
        engine.rebuild_power_snapshot(&vs, &sp);

        assert_eq!(engine.total_eligible_stake(&vs, &sp), 120);
        assert_eq!(engine.eligible_stake(&validators[0].1, &vs, &sp), Some(60));
        assert_eq!(engine.eligible_stake(&validators[2].1, &vs, &sp), None);

        let voters = vec![validators[0].1, validators[1].1];
        assert!(engine.has_supermajority_voters(&voters, &vs, &sp));

        let below_min_voters = vec![validators[0].1, validators[2].1];
        assert!(!engine.has_supermajority_voters(&below_min_voters, &vs, &sp));
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
    fn test_power_snapshot_ignores_cached_validator_stake_without_pool_entry() {
        let (validators, vs, _) = make_custom_test_env(&[
            MIN_VALIDATOR_STAKE,
            MIN_VALIDATOR_STAKE,
            MIN_VALIDATOR_STAKE,
        ]);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, MIN_VALIDATOR_STAKE);
        let empty_pool = StakePool::new();

        engine.start_height(1);
        engine.rebuild_power_snapshot(&vs, &empty_pool);

        let voters = vec![validators[0].1, validators[1].1];
        assert_eq!(engine.total_eligible_stake(&vs, &empty_pool), 0);
        assert!(!engine.has_supermajority_voters(&voters, &vs, &empty_pool));
    }

    #[test]
    fn test_power_snapshot_excludes_pending_validators() {
        let (validators, mut vs, sp) = make_custom_test_env(&[100, 100, 100]);
        vs.get_validator_mut(&validators[2].1)
            .expect("validator exists")
            .pending_activation = true;

        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);
        engine.start_height(1);
        engine.rebuild_power_snapshot(&vs, &sp);

        assert_eq!(engine.total_eligible_stake(&vs, &sp), 200);
        assert_eq!(engine.eligible_stake(&validators[2].1, &vs, &sp), None);
        assert!(engine.has_supermajority_voters(&[validators[0].1, validators[1].1], &vs, &sp));
        assert!(!engine.has_supermajority_voters(&[validators[0].1, validators[2].1], &vs, &sp));
    }

    #[test]
    fn test_prevote_power_tally_dedupes_votes() {
        let (validators, vs, sp) = make_custom_test_env(&[100, 100, 100]);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);
        engine.start_height(10);
        engine.rebuild_power_snapshot(&vs, &sp);
        engine.step = RoundStep::Prevote;

        let vote_hash = Some(Hash::hash(b"cached-prevote"));
        let signable = Prevote::signable_bytes(10, 0, &vote_hash);
        let prevote = Prevote {
            height: 10,
            round: 0,
            block_hash: vote_hash,
            validator: validators[1].1,
            signature: validators[1].0.sign(&signable),
        };

        let _ = engine.on_prevote(prevote.clone(), &vs, &sp);
        assert_eq!(
            engine.prevote_power.get(&(0, vote_hash)).copied(),
            Some(100)
        );
        assert_eq!(engine.prevote_any_power.get(&0).copied(), Some(100));

        let _ = engine.on_prevote(prevote, &vs, &sp);
        assert_eq!(
            engine.prevote_power.get(&(0, vote_hash)).copied(),
            Some(100),
            "duplicate prevote must not inflate block-specific power"
        );
        assert_eq!(
            engine.prevote_any_power.get(&0).copied(),
            Some(100),
            "duplicate prevote must not inflate any-value power"
        );
    }

    #[test]
    fn test_precommit_power_tally_dedupes_votes() {
        let (validators, vs, sp) = make_custom_test_env(&[100, 100, 100]);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);
        engine.start_height(10);
        engine.rebuild_power_snapshot(&vs, &sp);
        engine.step = RoundStep::Precommit;

        let vote_hash = Some(Hash::hash(b"cached-precommit"));
        let timestamp = 123;
        let signable = Precommit::signable_bytes(10, 0, &vote_hash, timestamp);
        let precommit = Precommit {
            height: 10,
            round: 0,
            block_hash: vote_hash,
            validator: validators[1].1,
            signature: validators[1].0.sign(&signable),
            timestamp,
        };

        let _ = engine.on_precommit(precommit.clone(), &vs, &sp);
        assert_eq!(
            engine.precommit_power.get(&(0, vote_hash)).copied(),
            Some(100)
        );
        assert_eq!(engine.precommit_any_power.get(&0).copied(), Some(100));

        let _ = engine.on_precommit(precommit, &vs, &sp);
        assert_eq!(
            engine.precommit_power.get(&(0, vote_hash)).copied(),
            Some(100),
            "duplicate precommit must not inflate block-specific power"
        );
        assert_eq!(
            engine.precommit_any_power.get(&0).copied(),
            Some(100),
            "duplicate precommit must not inflate any-value power"
        );
    }

    #[test]
    fn test_power_snapshot_rebuild_tallies_restored_wal_votes() {
        let (_, vs, sp) = make_custom_test_env(&[100, 100, 100]);
        let (local_kp, local_pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(local_kp, local_pk, 50);
        engine.start_height(10);

        let vote_hash = Some(Hash::hash(b"restored-prevote-tally"));
        let (signer_kp, _) = make_validator(1);
        let prevote_sig = signer_kp.sign(&Prevote::signable_bytes(10, 0, &vote_hash));
        engine
            .restore_signed_prevote(10, 0, vote_hash, prevote_sig)
            .expect("restore signed prevote");

        let timestamp = 456;
        let precommit_sig =
            signer_kp.sign(&Precommit::signable_bytes(10, 0, &vote_hash, timestamp));
        engine
            .restore_signed_precommit(10, 0, vote_hash, precommit_sig, timestamp)
            .expect("restore signed precommit");

        assert!(engine.prevote_power.is_empty());
        assert!(engine.precommit_power.is_empty());

        engine.rebuild_power_snapshot(&vs, &sp);

        assert_eq!(
            engine.prevote_power.get(&(0, vote_hash)).copied(),
            Some(100)
        );
        assert_eq!(engine.prevote_any_power.get(&0).copied(), Some(100));
        assert_eq!(
            engine.precommit_power.get(&(0, vote_hash)).copied(),
            Some(100)
        );
        assert_eq!(engine.precommit_any_power.get(&0).copied(), Some(100));
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
    fn test_below_min_voter_does_not_satisfy_supermajority() {
        let (validators, vs, sp) = make_custom_test_env(&[100, 100, 49]);
        let (kp, pk) = make_validator(1);
        let engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);

        let voters = vec![validators[0].1, validators[2].1];
        assert!(!engine.has_supermajority_voters(&voters, &vs, &sp));

        let eligible_voters = vec![validators[0].1, validators[1].1];
        assert!(engine.has_supermajority_voters(&eligible_voters, &vs, &sp));
    }

    #[test]
    fn test_below_min_votes_are_not_admitted_or_counted_for_any_supermajority() {
        let (validators, vs, sp) = make_custom_test_env(&[100, 100, 49]);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);
        engine.start_height(10);

        let below_min_kp = &validators[2].0;
        let below_min_pk = validators[2].1;
        let vote_hash = Some(Hash::hash(b"below-min-vote"));
        let prevote = Prevote {
            height: 10,
            round: 0,
            block_hash: vote_hash,
            validator: below_min_pk,
            signature: below_min_kp.sign(&Prevote::signable_bytes(10, 0, &vote_hash)),
        };
        assert!(matches!(
            engine.on_prevote(prevote, &vs, &sp),
            ConsensusAction::None
        ));
        assert!(!engine.seen_prevotes.contains_key(&(0, below_min_pk)));

        let precommit = Precommit {
            height: 10,
            round: 0,
            block_hash: vote_hash,
            validator: below_min_pk,
            signature: below_min_kp.sign(&Precommit::signable_bytes(10, 0, &vote_hash, 123)),
            timestamp: 123,
        };
        assert!(matches!(
            engine.on_precommit(precommit, &vs, &sp),
            ConsensusAction::None
        ));
        assert!(!engine.seen_precommits.contains_key(&(0, below_min_pk)));

        engine.seen_prevotes.insert((0, validators[0].1), vote_hash);
        engine.seen_prevotes.insert((0, below_min_pk), vote_hash);
        assert!(!engine.has_any_supermajority_prevotes(0, &vs, &sp));

        engine
            .seen_precommits
            .insert((0, validators[0].1), vote_hash);
        engine.seen_precommits.insert((0, below_min_pk), vote_hash);
        assert!(!engine.has_any_supermajority_precommits(0, &vs, &sp));
    }

    #[test]
    fn test_below_min_future_vote_does_not_trigger_round_skip() {
        let (validators, vs, sp) = make_custom_test_env(&[100, 100, 100, 100, 49]);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);
        engine.start_height(1);

        engine.seen_prevotes.insert((2, validators[0].1), None);
        engine.seen_prevotes.insert((2, validators[4].1), None);

        let action = engine.check_round_skip(2, &vs, &sp);
        assert_eq!(engine.round, 0);
        assert!(matches!(action, ConsensusAction::None));
    }

    #[test]
    fn test_commit_signatures_filter_below_min_and_pending_validators() {
        let (validators, mut vs, sp) = make_custom_test_env(&[100, 100, 49]);
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new_with_min_stake(kp, pk, 50);
        engine.start_height(1);

        vs.get_validator_mut(&validators[1].1)
            .expect("pending validator")
            .pending_activation = true;

        let block_hash = Hash::hash(b"commit-signature-filter");
        let voters = vec![validators[0].1, validators[1].1, validators[2].1];
        engine
            .precommits
            .insert((0, Some(block_hash)), voters.clone());
        for (idx, (_, pubkey)) in validators.iter().take(3).enumerate() {
            engine.precommit_sigs.insert(
                (0, *pubkey),
                (
                    make_validator((idx + 10) as u8).0.sign(b"fixture"),
                    100 + idx as u64,
                ),
            );
        }

        let signatures = engine.collect_commit_signatures(0, &block_hash, &vs, &sp);
        assert_eq!(signatures.len(), 1);
        assert_eq!(signatures[0].validator, validators[0].1 .0);
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

        // Round 0: base = 800ms
        engine.round = 0;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(800));

        // Round 1: 800 * 1.5 = 1200ms
        engine.round = 1;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(1200));

        // Round 2: 1200 * 1.5 = 1800ms
        engine.round = 2;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(1800));

        // Round 3: 1800 * 1.5 = 2700ms
        engine.round = 3;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(2700));
    }

    #[test]
    fn test_exponential_timeout_prevote() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);

        // Round 0: base = 500ms
        engine.round = 0;
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(500));

        // Round 1: 500 * 1.5 = 750ms
        engine.round = 1;
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(750));

        // Round 2: 750 * 1.5 = 1125ms
        engine.round = 2;
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(1125));
    }

    #[test]
    fn test_exponential_timeout_caps_at_max() {
        let (kp, pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(kp, pk);

        // At very high rounds, should cap at 5 seconds
        engine.round = 50;
        assert_eq!(engine.propose_timeout(), Duration::from_millis(5_000));
        assert_eq!(engine.prevote_timeout(), Duration::from_millis(5_000));
        assert_eq!(engine.precommit_timeout(), Duration::from_millis(5_000));
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
        engine.rebuild_power_snapshot(&vs, &sp);

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
    fn test_commit_certificate_can_commit_from_propose_step() {
        let (validators, vs, sp) = make_test_env(4);
        let mut validators = validators.into_iter();
        let (kp1, pk1) = validators.next().unwrap();
        let (kp2, pk2) = validators.next().unwrap();
        let (kp3, pk3) = validators.next().unwrap();
        let mut seed1 = [0u8; 32];
        seed1[0] = 1;
        let kp1_sign = Keypair::from_seed(&seed1);

        let mut engine = ConsensusEngine::new(kp1, pk1);
        engine.start_height(1);
        assert_eq!(engine.step, RoundStep::Propose);

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

        let ts1 = 999u64;
        let signable1 = Precommit::signable_bytes(1, 0, &Some(block_hash), ts1);
        engine
            .precommit_sigs
            .insert((0, pk1), (kp1_sign.sign(&signable1), ts1));
        engine
            .precommits
            .entry((0, Some(block_hash)))
            .or_default()
            .push(pk1);
        engine.seen_precommits.insert((0, pk1), Some(block_hash));
        engine.signed_precommit_rounds.insert(0, Some(block_hash));
        engine.rebuild_power_snapshot(&vs, &sp);

        let ts2 = 1000u64;
        let pc2 = Precommit {
            height: 1,
            round: 0,
            block_hash: Some(block_hash),
            validator: pk2,
            signature: kp2.sign(&Precommit::signable_bytes(1, 0, &Some(block_hash), ts2)),
            timestamp: ts2,
        };
        let _ = engine.on_precommit(pc2, &vs, &sp);

        let ts3 = 1001u64;
        let pc3 = Precommit {
            height: 1,
            round: 0,
            block_hash: Some(block_hash),
            validator: pk3,
            signature: kp3.sign(&Precommit::signable_bytes(1, 0, &Some(block_hash), ts3)),
            timestamp: ts3,
        };

        let action = engine.on_precommit(pc3, &vs, &sp);

        assert!(matches!(action, ConsensusAction::CommitBlock { .. }));
        assert_eq!(engine.step, RoundStep::Commit);
    }

    #[test]
    fn test_commit_certificate_requests_missing_block() {
        let (validators, vs, sp) = make_test_env(4);
        let (local_kp, local_pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(local_kp, local_pk);
        engine.start_height(7);
        engine.step = RoundStep::Propose;

        let block_hash = Hash::hash(b"missing-committed-block");
        let mut action = ConsensusAction::None;
        for (idx, (kp, pk)) in validators.iter().enumerate().take(3) {
            let timestamp = 1000 + idx as u64;
            let precommit = Precommit {
                height: 7,
                round: 0,
                block_hash: Some(block_hash),
                validator: *pk,
                signature: kp.sign(&Precommit::signable_bytes(
                    7,
                    0,
                    &Some(block_hash),
                    timestamp,
                )),
                timestamp,
            };
            action = engine.on_precommit(precommit, &vs, &sp);
        }

        assert!(matches!(
            action,
            ConsensusAction::RequestBlockRange {
                start_slot: 7,
                end_slot: 7,
                block_hash: hash,
            } if hash == block_hash
        ));
    }

    #[test]
    fn test_precommit_after_commit_does_not_emit_duplicate_commit() {
        let (validators, vs, sp) = make_test_env(4);
        let (local_kp, local_pk) = make_validator(1);
        let mut engine = ConsensusEngine::new(local_kp, local_pk);
        engine.start_height(1);
        engine.step = RoundStep::Precommit;

        let block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"state"),
            local_pk.0,
            Vec::new(),
            1000,
        );
        let block_hash = block.hash();
        engine.proposal_blocks.insert(block_hash, block);

        for (idx, (kp, pk)) in validators.iter().enumerate().take(3) {
            let timestamp = 1000 + idx as u64;
            let signature = kp.sign(&Precommit::signable_bytes(
                1,
                0,
                &Some(block_hash),
                timestamp,
            ));
            let precommit = Precommit {
                height: 1,
                round: 0,
                block_hash: Some(block_hash),
                validator: *pk,
                signature,
                timestamp,
            };
            let action = engine.on_precommit(precommit, &vs, &sp);
            if idx < 2 {
                assert!(matches!(action, ConsensusAction::None));
            } else {
                assert!(matches!(action, ConsensusAction::CommitBlock { .. }));
            }
        }

        let (late_kp, late_pk) = &validators[3];
        let late_timestamp = 2000;
        let late_signature = late_kp.sign(&Precommit::signable_bytes(
            1,
            0,
            &Some(block_hash),
            late_timestamp,
        ));
        let late_precommit = Precommit {
            height: 1,
            round: 0,
            block_hash: Some(block_hash),
            validator: *late_pk,
            signature: late_signature,
            timestamp: late_timestamp,
        };

        let action = engine.on_precommit(late_precommit, &vs, &sp);
        assert!(
            matches!(action, ConsensusAction::None),
            "late precommit after commit emitted duplicate action: {:?}",
            action
        );
    }

    #[test]
    fn test_on_proposal_rejects_timestamp_older_than_parent() {
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
            999,
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

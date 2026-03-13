// MoltChain BFT Consensus Engine
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

use moltchain_core::{
    Block, Hash, Keypair, Precommit, Prevote, Proposal, Pubkey, RoundStep, StakePool, ValidatorSet,
    MIN_VALIDATOR_STAKE,
};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Base timeout for the Propose step (ms). Actual = base * (round + 1).
const PROPOSE_TIMEOUT_BASE_MS: u64 = 2000;
/// Base timeout for the Prevote step (ms).
const PREVOTE_TIMEOUT_BASE_MS: u64 = 1000;
/// Base timeout for the Precommit step (ms).
const PRECOMMIT_TIMEOUT_BASE_MS: u64 = 1000;

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
}

/// Tendermint-style BFT consensus engine.
///
/// Pure state machine: call methods with incoming messages / timeout events,
/// receive `ConsensusAction` values to execute externally.
pub struct ConsensusEngine {
    // ── Identity ────────────────────────────────────────────────────
    keypair: Keypair,
    pub validator_pubkey: Pubkey,

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

    // ── Duplicate suppression ───────────────────────────────────────
    /// Prevotes we've already processed: (round, validator) → true.
    seen_prevotes: HashMap<(u32, Pubkey), bool>,
    /// Precommits we've already processed: (round, validator) → true.
    seen_precommits: HashMap<(u32, Pubkey), bool>,
    /// Rounds for which we already signed a prevote, to prevent equivocation.
    signed_prevote_rounds: HashMap<u32, Option<Hash>>,
    /// Rounds for which we already signed a precommit, to prevent equivocation.
    signed_precommit_rounds: HashMap<u32, Option<Hash>>,
}

impl ConsensusEngine {
    /// Create a new consensus engine for the given validator identity.
    pub fn new(keypair: Keypair, validator_pubkey: Pubkey) -> Self {
        Self {
            keypair,
            validator_pubkey,
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
            signed_prevote_rounds: HashMap::new(),
            signed_precommit_rounds: HashMap::new(),
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
        self.signed_prevote_rounds.clear();
        self.signed_precommit_rounds.clear();
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
        // Ignore proposals for wrong height
        if proposal.height != self.height {
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
        let expected_leader =
            validator_set.select_leader_weighted_with_seed(leader_slot, stake_pool, &parent_hash.0);
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
        if prevote.height != self.height {
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
        // Deduplicate
        if self
            .seen_prevotes
            .contains_key(&(prevote.round, prevote.validator))
        {
            return ConsensusAction::None;
        }
        self.seen_prevotes
            .insert((prevote.round, prevote.validator), true);

        // Record the prevote
        self.prevotes
            .entry((prevote.round, prevote.block_hash))
            .or_default()
            .push(prevote.validator);

        let round = prevote.round;
        let mut actions = Vec::new();

        // Rule 1: Upon 2/3+ prevotes for a specific block_hash at current round
        if round == self.round && self.step == RoundStep::Prevote {
            // Check for polka for any specific block hash
            for (key, voters) in &self.prevotes {
                if key.0 != round {
                    continue;
                }
                if let Some(bh) = &key.1 {
                    if self.has_supermajority_voters(voters, validator_set, stake_pool) {
                        // 2/3+ prevotes for block_hash — LOCK and precommit
                        info!(
                            "🔒 BFT: Polka at height={} round={} for {}",
                            self.height,
                            round,
                            hex::encode(&bh.0[..4])
                        );
                        self.valid_round = Some(round);
                        if let Some(block) = self.proposal_blocks.get(bh) {
                            self.valid_value = Some(block.clone());
                        }
                        self.locked_round = Some(round);
                        self.locked_value = Some(*bh);
                        self.step = RoundStep::Precommit;
                        actions.push(self.do_precommit(Some(*bh), validator_set, stake_pool));
                        break;
                    }
                }
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
                self.step = RoundStep::Precommit;
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
        if precommit.height != self.height {
            return ConsensusAction::None;
        }
        if !precommit.verify_signature() {
            warn!("🚨 BFT: Invalid precommit signature");
            return ConsensusAction::None;
        }
        if validator_set.get_validator(&precommit.validator).is_none() {
            return ConsensusAction::None;
        }
        // Deduplicate
        if self
            .seen_precommits
            .contains_key(&(precommit.round, precommit.validator))
        {
            return ConsensusAction::None;
        }
        self.seen_precommits
            .insert((precommit.round, precommit.validator), true);

        // Record the precommit
        self.precommits
            .entry((precommit.round, precommit.block_hash))
            .or_default()
            .push(precommit.validator);

        let round = precommit.round;
        let mut actions = Vec::new();

        // Rule 1: 2/3+ precommits for a specific block → COMMIT
        for (key, voters) in &self.precommits {
            if key.0 != round {
                continue;
            }
            if let Some(bh) = &key.1 {
                if self.has_supermajority_voters(voters, validator_set, stake_pool) {
                    if let Some(block) = self.proposal_blocks.get(bh) {
                        info!(
                            "✅ BFT: COMMIT at height={} round={} hash={}",
                            self.height,
                            round,
                            hex::encode(&bh.0[..4])
                        );
                        self.step = RoundStep::Commit;
                        return ConsensusAction::CommitBlock {
                            height: self.height,
                            round,
                            block: block.clone(),
                            block_hash: *bh,
                        };
                    }
                    // We have 2/3+ precommits but don't have the block.
                    // This shouldn't happen if the proposer broadcast correctly.
                    warn!(
                        "⚠️ BFT: 2/3+ precommits for {} but block not found",
                        hex::encode(&bh.0[..4])
                    );
                }
            }
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
                    self.step = RoundStep::Precommit;
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

        self.step = RoundStep::Prevote;
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
            .insert((self.round, self.validator_pubkey), true);
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
                self.step = RoundStep::Precommit;
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
                self.step = RoundStep::Precommit;
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

        let msg = Precommit::signable_bytes(self.height, self.round, &block_hash);
        let signature = self.keypair.sign(&msg);

        let precommit = Precommit {
            height: self.height,
            round: self.round,
            block_hash,
            validator: self.validator_pubkey,
            signature,
        };

        self.signed_precommit_rounds.insert(self.round, block_hash);
        self.seen_precommits
            .insert((self.round, self.validator_pubkey), true);
        self.precommits
            .entry((self.round, block_hash))
            .or_default()
            .push(self.validator_pubkey);

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
            if self.has_supermajority_voters(voters, validator_set, stake_pool) {
                if let Some(block) = self.proposal_blocks.get(&bh) {
                    info!(
                        "✅ BFT: COMMIT at height={} round={} hash={}",
                        self.height,
                        round,
                        hex::encode(&bh.0[..4])
                    );
                    self.step = RoundStep::Commit;
                    return ConsensusAction::Multiple(vec![
                        broadcast,
                        ConsensusAction::CommitBlock {
                            height: self.height,
                            round,
                            block: block.clone(),
                            block_hash: bh,
                        },
                    ]);
                }
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
        let vote_stake: u128 = voters
            .iter()
            .filter_map(|pk| stake_pool.get_stake(pk))
            .map(|s| s.total_stake() as u128)
            .sum();

        let total_eligible_stake: u128 = validator_set
            .sorted_validators()
            .iter()
            .filter(|v| {
                let s = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake);
                s >= MIN_VALIDATOR_STAKE
            })
            .map(|v| {
                stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake) as u128
            })
            .sum();

        if total_eligible_stake == 0 {
            return false;
        }

        // 2/3 threshold: vote_stake * 3 >= total_eligible_stake * 2
        vote_stake * 3 >= total_eligible_stake * 2
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
                let s = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake);
                s >= MIN_VALIDATOR_STAKE
            })
            .map(|v| {
                stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake) as u128
            })
            .sum();

        if total_eligible_stake == 0 {
            return false;
        }

        let total_voted_stake: u128 = self
            .seen_prevotes
            .keys()
            .filter(|(r, _)| *r == round)
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
                let s = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake);
                s >= MIN_VALIDATOR_STAKE
            })
            .map(|v| {
                stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake) as u128
            })
            .sum();

        if total_eligible_stake == 0 {
            return false;
        }

        let total_voted_stake: u128 = self
            .seen_precommits
            .keys()
            .filter(|(r, _)| *r == round)
            .filter_map(|(_, pk)| stake_pool.get_stake(pk))
            .map(|s| s.total_stake() as u128)
            .sum();

        total_voted_stake * 3 >= total_eligible_stake * 2
    }

    /// Tendermint round-skip: if we see votes from >1/3 voting power for
    /// round R' > our round, skip to R'. This prevents permanent deadlocks
    /// when nodes diverge in round numbers.
    fn check_round_skip(
        &mut self,
        vote_round: u32,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> ConsensusAction {
        if vote_round <= self.round {
            return ConsensusAction::None;
        }

        let total_eligible_stake: u128 = validator_set
            .sorted_validators()
            .iter()
            .filter(|v| {
                let s = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake);
                s >= MIN_VALIDATOR_STAKE
            })
            .map(|v| {
                stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake) as u128
            })
            .sum();

        if total_eligible_stake == 0 {
            return ConsensusAction::None;
        }

        // Collect unique voters who sent prevotes OR precommits for vote_round
        let mut round_voters = std::collections::HashSet::new();
        for (r, pk) in self.seen_prevotes.keys() {
            if *r == vote_round {
                round_voters.insert(*pk);
            }
        }
        for (r, pk) in self.seen_precommits.keys() {
            if *r == vote_round {
                round_voters.insert(*pk);
            }
        }

        let round_stake: u128 = round_voters
            .iter()
            .filter_map(|pk| stake_pool.get_stake(pk))
            .map(|s| s.total_stake() as u128)
            .sum();

        // f+1 threshold: round_stake * 3 > total_eligible_stake (i.e., >1/3)
        if round_stake * 3 > total_eligible_stake {
            info!(
                "🔄 BFT: Round skip h={} r={} → r={} (saw >1/3 votes for higher round)",
                self.height, self.round, vote_round
            );
            let skip_action = self.start_round(vote_round);
            // After skipping, check if we already have a stored proposal
            // for the new round and process it immediately.
            if let Some(proposal) = self.proposals.get(&vote_round).cloned() {
                let block_hash = proposal.block.hash();
                // Tendermint prevote rule after skip
                let should_prevote_block = if self.locked_round.is_none()
                    || self.locked_value == Some(block_hash)
                {
                    true
                } else if proposal.valid_round >= 0 {
                    let vr = proposal.valid_round as u32;
                    if let Some(lr) = self.locked_round {
                        vr > lr
                            && self.has_polka_for(vr, &Some(block_hash), validator_set, stake_pool)
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
                return ConsensusAction::Multiple(vec![skip_action, prevote_action]);
            }
            return skip_action;
        }

        ConsensusAction::None
    }

    // ── Timeouts (linear backoff) ──────────────────────────────────

    fn propose_timeout(&self) -> Duration {
        Duration::from_millis(PROPOSE_TIMEOUT_BASE_MS * (self.round as u64 + 1))
    }

    pub fn prevote_timeout(&self) -> Duration {
        Duration::from_millis(PREVOTE_TIMEOUT_BASE_MS * (self.round as u64 + 1))
    }

    pub fn precommit_timeout(&self) -> Duration {
        Duration::from_millis(PRECOMMIT_TIMEOUT_BASE_MS * (self.round as u64 + 1))
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
        let leader =
            validator_set.select_leader_weighted_with_seed(leader_slot, stake_pool, &parent_hash.0);
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
                        let s = stake_pool
                            .get_stake(&v.pubkey)
                            .map(|s| s.total_stake())
                            .unwrap_or(v.stake);
                        s >= MIN_VALIDATOR_STAKE
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use moltchain_core::{Hash, Keypair, Pubkey, StakePool, ValidatorInfo, ValidatorSet};

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
        let msg = Precommit::signable_bytes(50, 1, &block_hash);
        let sig = kp.sign(&msg);
        let precommit = Precommit {
            height: 50,
            round: 1,
            block_hash,
            validator: pk,
            signature: sig,
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
        let precommit_bytes = Precommit::signable_bytes(10, 0, &h);
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
}

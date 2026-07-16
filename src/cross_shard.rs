//! Cross-shard atomic commit as a barrier over the transaction's participant set —
//! the *coordinated* rung.
//!
//! A single-shard write is coordination-free (that is the structural rung,
//! `sharding`). The moment a transaction touches keys in **more than one shard**, no
//! single node owns the whole write, and safety needs evidence from **every** shard it
//! touches: if even one participating shard cannot commit, none may, or the write tears
//! (some shards apply it, others do not). This rung types that evidence.
//!
//! ## The mechanism — a barrier over the *declared participant set*
//!
//! * [`ShardVote<S, E>`] — a **linear**, unforgeable YES-to-commit from shard `S` at
//!   configuration epoch `E`, minted only by [`prepare`]. A shard emits one after it
//!   has locked the transaction's keys in its own partition and is ready to apply. It
//!   is `#[must_use]` and move-only: a vote is *consumed* by the coordinator, never
//!   copied.
//! * [`AtomicCommit<E>`] — the coordinator's barrier. [`over`](AtomicCommit::over)
//!   declares the transaction's **participant set** (the shards it touches);
//!   [`record`](AtomicCommit::record) consumes one [`ShardVote`]; [`seal`](AtomicCommit::seal)
//!   mints a [`CrossCommitted<E>`] **only if every declared participant has voted** —
//!   unanimity over the participant set. Any missing shard yields `None` (the
//!   transaction must abort).
//! * [`CrossCommitted<E>`] — a witness (a *fact* — hence `Clone`, like
//!   [`membership::Quorum`](crate::membership::Quorum)) that every **declared participant**
//!   of the transaction voted to commit at epoch `E`. Unforgeable (private field, minted
//!   only by `seal`).
//!
//! ## The witness shape: `stability`'s unanimity barrier fed linear, epoch-branded votes
//!
//! This rung's witness is a **composition** of two mechanisms the crate already has, not
//! a new primitive — and worth naming as such. It reuses `stability`'s unanimity
//! **barrier** (every member of a set must report or the certificate is withheld; the
//! seal logic here, an `.all()` over the declared set, is `stability`'s), but feeds it
//! `twophase`'s **linear vote** in place of a bare watermark: a [`ShardVote<S, E>`] is
//! move-only, unforgeable, and epoch-branded, where `stability`'s `ack` was a plain
//! `(NodeId, u64)`. The payload is the participant set itself, not a `min`. So the
//! contribution is the *combination* — an all-of-set barrier over a **per-transaction**
//! participant set, fed linear epoch-branded votes — rather than the barrier shape,
//! which is `stability`'s. (`stability`'s roster is a config's full membership; here the
//! set is chosen per transaction, but nothing type-level distinguishes the two — both
//! take an arbitrary runtime `BTreeSet` at [`over`](AtomicCommit::over).)
//!
//! ## Distinct from [`twophase`](crate::twophase)
//!
//! Both concern atomic commit, and the distinction is exact. `twophase` types the
//! **participant's surrender**: a `Participant<Prepared>` is in-doubt, exposes no
//! `commit`/`abort`, and blocks if the coordinator vanishes — and it leaves
//! *vote-completeness* (did the ballot reflect the full membership?) an **untyped seam**
//! (`Ballot::resolve` commits on any non-empty vote set). This rung types the half
//! `twophase` leaves open: the participant set is **declared up front** and `seal`
//! returns `None` unless **every** declared shard voted. It does *not* re-type the
//! in-doubt blocking (that is `twophase`'s contribution); it types the
//! **completeness barrier** over a known participant set. The two are complementary
//! halves of atomic commit, not the same rung.
//!
//! ## A cross-shard commit certificate cannot be forged
//!
//! ```compile_fail
//! use quorum_types::cross_shard::CrossCommitted;
//! use std::collections::BTreeSet;
//! // Private field: no public literal. A caller cannot assert "every shard committed"
//! // without going through a sealed barrier.
//! let _forged: CrossCommitted<1> = CrossCommitted { shards: BTreeSet::new(), _priv: () };
//! ```
//!
//! ## A vote is linear — it cannot be recorded twice
//!
//! ```compile_fail
//! use quorum_types::cross_shard::{prepare, AtomicCommit};
//! use std::collections::BTreeSet;
//! let vote = prepare::<0, 1>();
//! let a = AtomicCommit::<1>::over(BTreeSet::from([0])).record(vote);
//! let b = AtomicCommit::<1>::over(BTreeSet::from([0])).record(vote); // `vote` moved
//! ```
//!
//! ## A vote from another epoch cannot be recorded
//!
//! ```compile_fail
//! use quorum_types::cross_shard::{prepare, AtomicCommit};
//! use std::collections::BTreeSet;
//! let stale = prepare::<0, 2>();                 // epoch 2
//! let bar = AtomicCommit::<1>::over(BTreeSet::from([0]));
//! let _ = bar.record(stale); // epoch 2 vs 1 do not unify
//! ```
//!
//! ## The happy path — every touched shard votes, the transaction commits
//!
//! ```
//! use quorum_types::cross_shard::{prepare, AtomicCommit};
//! use std::collections::BTreeSet;
//!
//! // A transaction touches shards 0 and 2 at epoch 1.
//! let participants = BTreeSet::from([0u64, 2u64]);
//! let barrier = AtomicCommit::<1>::over(participants)
//!     .record(prepare::<0, 1>())
//!     .record(prepare::<2, 1>());
//!
//! let committed = barrier.seal().expect("every touched shard voted → commit");
//! assert!(committed.commits(0) && committed.commits(2));
//! assert!(!committed.commits(1), "shard 1 was not part of this transaction");
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! What the types own is the **completeness barrier**: a `CrossCommitted<E>` exists only
//! if every *declared* participant voted at epoch `E`, and the vote set cannot be forged
//! or double-counted. Three things are **not** owned:
//!
//! * **Coverage of the declared set.** The barrier certifies that every *declared* shard
//!   voted — not that the declared set actually covers every key the transaction touched.
//!   That coverage is the caller's obligation, inherited from `sharding`'s routing seam:
//!   a transaction that touches shard 3 but forgets to declare it gets a well-typed
//!   `CrossCommitted` that silently excludes shard 3.
//! * **Local durability and liveness.** A [`ShardVote`] attests a shard *reported* it is
//!   ready to commit; that it truly locked and can apply is the shard's own obligation,
//!   and a silent shard blocks the commit forever (the atomic-commit analogue of
//!   `stability`'s one silent node — unanimity's liveness price).
//! * **Epoch is a config *class*, not a config instance.** The brand `E` distinguishes
//!   certificates and votes of different epochs (a stale-epoch vote will not unify), but
//!   there is no `Config<E>` here against which the participant set is checked — the same
//!   type-level-class-not-instance seam `election`/`stability` carry.

use std::collections::BTreeSet;

/// A linear YES-to-commit from shard `S` at configuration epoch `E`.
///
/// Minted only by [`prepare`] (no public constructor — the private field makes it
/// unforgeable), consumed only by [`AtomicCommit::record`]. Move-only and
/// `#[must_use]`: a vote that never reaches the barrier strands its shard, and a vote
/// cannot be counted twice.
#[must_use = "a ShardVote must be recorded on the barrier, or its shard's commit is never counted"]
pub struct ShardVote<const S: u64, const E: u64> {
    _priv: (),
}

impl<const S: u64, const E: u64> ShardVote<S, E> {
    /// The shard this vote came from (mirrors the type-level `S`).
    pub const fn shard(&self) -> u64 {
        S
    }

    /// The configuration epoch this vote was cast at (mirrors the type-level `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }
}

/// Cast shard `S`'s YES-to-commit at epoch `E`.
///
/// Models a shard that has locked the transaction's keys in its own partition and is
/// ready to apply. Emitting the vote is the shard's assertion of readiness; whether it
/// truly locked and can apply is the shard's obligation (the module's durability seam).
pub const fn prepare<const S: u64, const E: u64>() -> ShardVote<S, E> {
    ShardVote { _priv: () }
}

/// The coordinator's completeness barrier for a cross-shard transaction at epoch `E`.
///
/// [`over`](AtomicCommit::over) declares the participant set; [`record`](AtomicCommit::record)
/// collects one [`ShardVote`] per shard; [`seal`](AtomicCommit::seal) commits **only if
/// every declared participant voted**.
#[must_use = "an AtomicCommit must be sealed into a decision, or the transaction dangles in-doubt"]
pub struct AtomicCommit<const E: u64> {
    declared: BTreeSet<u64>,
    prepared: BTreeSet<u64>,
}

impl<const E: u64> AtomicCommit<E> {
    /// Open a barrier over the transaction's participant set — the shards it declares it
    /// touches (coverage of that declaration is the caller's obligation; see the seam).
    ///
    /// The participants are the shards that *must* all agree for the transaction to
    /// commit. An empty participant set is degenerate: it can never [`seal`](AtomicCommit::seal)
    /// to a commit (there is nothing to commit and no shard to attest it).
    pub fn over(participants: BTreeSet<u64>) -> Self {
        AtomicCommit { declared: participants, prepared: BTreeSet::new() }
    }

    /// Record shard `S`'s vote, consuming the linear [`ShardVote`]. The vote's epoch
    /// `E` must match the barrier's (a stale-epoch vote fails to unify). A vote from a
    /// shard *outside* the declared participant set is retained but can never help a
    /// [`seal`](AtomicCommit::seal) succeed — completeness is judged over the declared
    /// set, never over the recorded votes (so a stray vote cannot inflate it).
    pub fn record<const S: u64>(mut self, _vote: ShardVote<S, E>) -> Self {
        self.prepared.insert(S);
        self
    }

    /// How many distinct shards have voted so far — a **progress counter**, not the
    /// commit predicate. A transaction commits on [`seal`](AtomicCommit::seal) only when
    /// this set *covers the declared participants*, which this count alone does not
    /// establish (a stray non-participant vote inflates the count but not the coverage).
    pub fn prepared_count(&self) -> usize {
        self.prepared.len()
    }

    /// The declared participant set (the shards that must all agree).
    pub fn participants(&self) -> &BTreeSet<u64> {
        &self.declared
    }

    /// **Seal the barrier.** Mints a [`CrossCommitted<E>`] **iff** the participant set is
    /// non-empty and **every** declared shard has voted — unanimity over the declared
    /// participants. Any missing participant, or an empty set, yields `None`: the
    /// transaction must abort (no torn commit).
    ///
    /// Completeness is checked over `declared`, never over `prepared`, so a vote from a
    /// shard that is not a participant can never substitute for a missing one.
    pub fn seal(self) -> Option<CrossCommitted<E>> {
        if self.declared.is_empty() {
            return None;
        }
        if self.declared.iter().all(|s| self.prepared.contains(s)) {
            Some(CrossCommitted { shards: self.declared, _priv: () })
        } else {
            None
        }
    }
}

/// A witness that every **declared participant** of a cross-shard transaction voted to
/// commit at epoch `E`.
///
/// A *fact*, not a consumable capability — hence `Clone` (duplicating it duplicates a
/// true statement, like [`membership::Quorum`](crate::membership::Quorum)). Unforgeable:
/// the private field means the only source is [`AtomicCommit::seal`], which mints it only
/// on unanimity. It attests that every declared participant *reported* readiness and the
/// barrier saw them all — not that the declared set actually covered every key the
/// transaction touched (the coverage seam), nor that each shard's local apply is durable
/// (both are the caller's/shard's obligation; see the module seam).
#[derive(Debug, Clone)]
#[must_use = "a CrossCommitted witness records a cross-shard commit; dropping it discards the evidence"]
pub struct CrossCommitted<const E: u64> {
    shards: BTreeSet<u64>,
    _priv: (),
}

impl<const E: u64> CrossCommitted<E> {
    /// The declared participant set this commit covers — the shards the barrier saw vote.
    pub fn shards(&self) -> &BTreeSet<u64> {
        &self.shards
    }

    /// Whether `shard` was one of this commit's declared participants.
    pub fn commits(&self, shard: u64) -> bool {
        self.shards.contains(&shard)
    }

    /// The configuration epoch this commit was certified at (mirrors the type-level `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_participants_voting_commits() {
        let barrier = AtomicCommit::<1>::over(BTreeSet::from([0, 2]))
            .record(prepare::<0, 1>())
            .record(prepare::<2, 1>());
        let committed = barrier.seal().expect("both touched shards voted");
        assert!(committed.commits(0));
        assert!(committed.commits(2));
        assert!(!committed.commits(1));
        assert_eq!(committed.epoch(), 1);
    }

    #[test]
    fn a_missing_participant_aborts() {
        // Declared {0, 1, 2} but shard 1 never votes → no commit (would tear).
        let barrier = AtomicCommit::<1>::over(BTreeSet::from([0, 1, 2]))
            .record(prepare::<0, 1>())
            .record(prepare::<2, 1>());
        assert!(barrier.seal().is_none(), "a silent participant blocks the commit");
    }

    #[test]
    fn a_non_participant_vote_cannot_substitute_for_a_missing_one() {
        // Declared {0, 1}; shard 0 and a *non-participant* shard 5 vote, but shard 1
        // does not. Completeness is over the declared set, so this must NOT commit —
        // the stray vote cannot stand in for the missing participant.
        let barrier = AtomicCommit::<1>::over(BTreeSet::from([0, 1]))
            .record(prepare::<0, 1>())
            .record(prepare::<5, 1>());
        assert_eq!(barrier.prepared_count(), 2, "two votes recorded");
        assert!(barrier.seal().is_none(), "coverage is judged over participants, not vote count");
    }

    #[test]
    fn empty_participant_set_never_commits() {
        let barrier = AtomicCommit::<1>::over(BTreeSet::new());
        assert!(barrier.seal().is_none(), "an empty transaction has nothing to commit");
    }

    #[test]
    fn a_single_declared_shard_commits_on_its_own_vote() {
        // Even a one-shard "cross-shard" barrier requires that shard's vote.
        let committed = AtomicCommit::<7>::over(BTreeSet::from([3]))
            .record(prepare::<3, 7>())
            .seal()
            .expect("the sole participant voted");
        assert_eq!(committed.shards(), &BTreeSet::from([3]));
        assert_eq!(committed.epoch(), 7);
    }

    #[test]
    fn a_committed_witness_is_clone_a_fact_not_a_capability() {
        let committed = AtomicCommit::<1>::over(BTreeSet::from([0]))
            .record(prepare::<0, 1>())
            .seal()
            .unwrap();
        let copy = committed.clone();
        assert_eq!(committed.shards(), copy.shards(), "a fact may be duplicated");
    }

    #[test]
    fn votes_report_their_shard_and_epoch() {
        let v = prepare::<4, 9>();
        assert_eq!(v.shard(), 4);
        assert_eq!(v.epoch(), 9);
    }
}

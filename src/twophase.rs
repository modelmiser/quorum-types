//! Two-phase atomic commit as a linear session typestate — the *blocking* rung.
//!
//! Every rung so far hands you a token you *can* spend. Two-phase commit (2PC) is
//! defined by the **opposite** move: a participant that has voted YES has
//! *surrendered* the right to decide. It is **in-doubt** — it must apply whatever
//! the coordinator decides and may not commit or abort on its own. That surrender
//! is the whole protocol's character (Gray 1978; Bernstein–Hadzilacos–Goodman), and
//! it is what makes 2PC **block**: if the coordinator crashes after a participant
//! prepares, that participant is stuck forever.
//!
//! This module types the surrender directly, and the block falls out for free.
//!
//! ## The mechanism
//!
//! * [`Participant<Working>`] can still act locally and can [`prepare`](Participant::prepare).
//!   Preparing splits into a runtime YES/NO decision: a NO returns an
//!   already-aborted final state *and no vote* (a participant may always unilaterally
//!   abort — the one direction that is always safe); a YES returns a
//!   [`Participant<Prepared>`] **plus a linear [`Vote`]** handed to the coordinator.
//! * [`Participant<Prepared>`] — the in-doubt state — exposes **no** `commit` and
//!   **no** `abort`. Its *only* exit is [`decide`](Participant::decide), which
//!   **consumes** a [`Decision<O>`]. There is no method by which it can choose `O`
//!   itself. This absence *is* the model of "a prepared participant cannot decide
//!   unilaterally."
//! * [`Decision<O>`] (`O` = [`Commit`] or [`Abort`]) is minted **only** by
//!   [`Ballot::resolve`] (given the vote-completeness seam below, all YES ⇒
//!   `Decision<Commit>`, otherwise `Decision<Abort>`). It is `Copy` — *intended* to be
//!   broadcast so every participant applies the same one, though the types do not
//!   enforce that single-source discipline (see "Atomicity" below).
//!
//! ## Atomicity: what the types do — and do not — give
//!
//! A participant reaches [`Final<Commit>`] **only** by consuming a `Decision<Commit>`,
//! and a `Decision<Commit>` exists **only** if a ballot resolved to commit. Two
//! guarantees *are* structural: **AC3** (a commit needs votes — `resolve` mints a
//! `Decision<Commit>` only from a non-empty ballot) and **AC2** (a decision is
//! irreversible — [`Final<Commit>`]/`Final<Abort>` is terminal, with no transition
//! out). But **AC1** — uniform agreement, no two participants decide differently — is
//! **not** enforced by construction. [`decide`](Participant::decide) accepts *any*
//! `Decision<O>`, and nothing binds a decision to a particular transaction: a
//! coordinator that resolves two *different* ballots for one transaction can hand one
//! participant a `Decision<Commit>` and another a `Decision<Abort>`, and both
//! typecheck. Uniform agreement rests on a coordinator obligation — broadcast a
//! *single* decision to exactly that transaction's participants — the same class of
//! trusted seam as vote-completeness below, not a compile-time guarantee.
//!
//! ## A prepared participant cannot self-decide — it is a compile error
//!
//! ```compile_fail
//! use quorum_types::twophase::{Participant, Working};
//! let p = Participant::<Working>::join(0);
//! let prepared = match p.prepare(true) { Ok((pp, _vote)) => pp, Err(_) => unreachable!() };
//! let _ = prepared.commit(); // no such method: an in-doubt participant may not decide
//! ```
//!
//! ## The blocking hazard is *visible in the types*
//!
//! The exit from `Prepared` needs a `Decision`. If the coordinator never produces
//! one (it crashed), the participant is left holding a [`Participant<Prepared>`] —
//! a `#[must_use]` linear token it **cannot discharge**. The type system's inability
//! to let you proceed is a faithful model of 2PC's block: you are holding a resource
//! whose only consumer never arrives. (Loop-5-C model-checks exactly where this
//! lifts: a cooperative-termination protocol restores progress.)
//!
//! ## The happy path — collect votes, resolve once, everyone applies the decision
//!
//! ```
//! use quorum_types::twophase::{Participant, Working, Ballot, Outcome};
//!
//! // Two participants prepare and both vote YES.
//! let (p0, v0) = Participant::<Working>::join(10).prepare(true).ok().unwrap();
//! let (p1, v1) = Participant::<Working>::join(20).prepare(true).ok().unwrap();
//!
//! // The coordinator collects the votes and resolves the ballot exactly once.
//! let decision = match Ballot::new().record(v0).record(v1).resolve() {
//!     Outcome::Commit(d) => d,          // all YES ⇒ a Decision<Commit>
//!     Outcome::Abort(_)  => unreachable!(),
//! };
//!
//! // The single decision is broadcast (Copy) and every participant applies it.
//! let f0 = p0.decide(decision);
//! let f1 = p1.decide(decision);
//! assert!(f0.committed() && f1.committed());
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! What the types own is *local*: an in-doubt participant may not self-decide, and a
//! participant applies *a* coordinator decision it is handed. Three obligations are
//! **not** owned:
//!
//! * **Single decision per transaction.** [`decide`](Participant::decide) accepts any
//!   [`Decision<O>`]; nothing ties a decision to a transaction, so uniform agreement
//!   (AC1) requires the coordinator to broadcast *one* decision to exactly that
//!   transaction's participants (see "Atomicity" above).
//! * **Vote-completeness.** A member that votes NO — or is unreachable — emits *no*
//!   [`Vote`], so a ballot that collected any YES commits regardless. [`Ballot::resolve`]
//!   cannot see an absent member; the obligation is therefore to reflect the *full
//!   membership* and abort on any absent/NO member, not merely to "collect every vote."
//! * **Liveness.** The types make the block *visible* (an undischargeable [`Participant<Prepared>`]
//!   token) but do not remove it.
//!
//! All three are the same root-of-trust shape as [`Config::new`](crate::membership::Config::new):
//! types verify chains; operators choose roots.

use core::marker::PhantomData;

/// Phantom over a type-level state/outcome marker. `fn() -> T` keeps the wrappers
/// covariant and unconditionally `Send`/`Sync` — the markers carry identity, not data.
type Phantom<T> = PhantomData<fn() -> T>;

mod sealed {
    pub trait Sealed {}
}

/// A participant lifecycle state. Sealed: the only states are [`Working`],
/// [`Prepared`], and [`Final<O>`].
pub trait State: sealed::Sealed {}

/// The initial state: the participant may still act locally and may [`prepare`](Participant::prepare).
pub enum Working {}
/// The **in-doubt** state: the participant has voted YES and surrendered unilateral
/// choice. Exposes no `commit`/`abort` — its only exit is [`decide`](Participant::decide).
pub enum Prepared {}
/// The terminal state, tagged with the outcome `O` that was applied.
pub enum Final<O: OutcomeKind> {
    #[doc(hidden)]
    _Never(core::convert::Infallible, Phantom<O>),
}

impl sealed::Sealed for Working {}
impl sealed::Sealed for Prepared {}
impl<O: OutcomeKind> sealed::Sealed for Final<O> {}
impl State for Working {}
impl State for Prepared {}
impl<O: OutcomeKind> State for Final<O> {}

/// A commit-protocol outcome marker. Sealed: only [`Commit`] and [`Abort`].
pub trait OutcomeKind: sealed::Sealed {
    /// Whether this outcome commits (`true` for [`Commit`], `false` for [`Abort`]).
    const COMMITS: bool;
}

/// The outcome in which the transaction commits — reached only if *every* vote was YES.
pub enum Commit {}
/// The outcome in which the transaction aborts.
pub enum Abort {}

impl sealed::Sealed for Commit {}
impl sealed::Sealed for Abort {}
impl OutcomeKind for Commit {
    const COMMITS: bool = true;
}
impl OutcomeKind for Abort {
    const COMMITS: bool = false;
}

/// A participant in the atomic-commit protocol, in lifecycle state `S`.
///
/// Move-only: the state transition is carried by consuming `self`. `id` is a runtime
/// label; the type-level `S` is what the compiler reasons about.
#[must_use = "a Participant is a linear protocol role; drive it to a Final state or the transaction dangles"]
pub struct Participant<S: State> {
    id: u32,
    _s: Phantom<S>,
}

/// A YES vote — a **linear** token minted only by a participant's [`prepare`](Participant::prepare)
/// and consumed only by [`Ballot::record`]. Its existence is evidence that *this*
/// participant reached [`Prepared`]; the coordinator cannot fabricate one (the `voter`
/// field is private, so the only source is `prepare`).
///
/// It certifies a prepared participant *exists*, not that *all* of them do — see the
/// module's vote-completeness seam.
#[must_use = "a Vote must reach the coordinator's Ballot, or the participant is stranded in-doubt"]
pub struct Vote {
    voter: u32,
}

impl Vote {
    /// The id of the participant this vote came from.
    pub const fn voter(&self) -> u32 {
        self.voter
    }
}

impl Participant<Working> {
    /// Enroll a participant, identified by `id`, in the `Working` state.
    pub const fn join(id: u32) -> Self {
        Participant { id, _s: PhantomData }
    }

    /// **Phase 1 (vote).** Decide locally whether this participant can commit.
    ///
    /// * `false` (vote NO): return an aborted `Final<Abort>` participant and **no vote** —
    ///   a unilateral abort, always safe, needs no coordinator.
    /// * `true` (vote YES): move to [`Prepared`] (in-doubt) and emit a linear [`Vote`]
    ///   for the coordinator. From here the participant can no longer decide alone.
    ///
    /// `Err` carries the aborted participant; `Ok` carries the prepared participant
    /// and its vote.
    #[allow(clippy::type_complexity)]
    pub fn prepare(self, can_commit: bool) -> Result<(Participant<Prepared>, Vote), Participant<Final<Abort>>> {
        if can_commit {
            Ok((Participant { id: self.id, _s: PhantomData }, Vote { voter: self.id }))
        } else {
            Err(Participant { id: self.id, _s: PhantomData })
        }
    }
}

impl Participant<Prepared> {
    /// **Phase 2 (apply).** The *only* exit from in-doubt: consume the coordinator's
    /// [`Decision<O>`] and reach [`Final<O>`]. The participant applies `O`; it never
    /// chooses it. There is deliberately no `commit`/`abort` here — that absence is
    /// the model of 2PC's surrender of unilateral choice.
    pub fn decide<O: OutcomeKind>(self, _decision: Decision<O>) -> Participant<Final<O>> {
        Participant { id: self.id, _s: PhantomData }
    }
}

impl<O: OutcomeKind> Participant<Final<O>> {
    /// Whether this participant committed (mirrors the type-level outcome `O`).
    pub const fn committed(&self) -> bool {
        O::COMMITS
    }
}

impl<S: State> Participant<S> {
    /// The participant's runtime id.
    pub const fn id(&self) -> u32 {
        self.id
    }
}

/// The coordinator's vote tally. Votes are [`record`](Ballot::record)ed (each consuming
/// a linear [`Vote`]); [`resolve`](Ballot::resolve) turns the tally into the
/// [`Decision`] the transaction's participants should apply.
#[must_use = "a Ballot must be resolved into a Decision, or the participants block in-doubt"]
pub struct Ballot {
    votes: u32,
}

impl Ballot {
    /// Open an empty ballot.
    pub const fn new() -> Self {
        Ballot { votes: 0 }
    }

    /// Record one participant's YES vote, consuming the linear [`Vote`] token.
    pub fn record(mut self, _vote: Vote) -> Self {
        self.votes += 1;
        self
    }

    /// The number of YES votes collected so far.
    pub const fn yes_votes(&self) -> u32 {
        self.votes
    }

    /// **Resolve the ballot into a decision.** In this rung every [`Vote`] that
    /// reaches the ballot is a YES (a NO participant aborted unilaterally and emitted
    /// no vote), so a ballot that collected at least one vote commits; an empty ballot
    /// aborts (a conventional safe default — no member is willing).
    ///
    /// This mints the [`Decision`] the transaction's participants should apply, its
    /// outcome type fixed here. It trusts two things the types do not check: that the
    /// votes reflect the *full membership* (vote-completeness), and that this single
    /// decision is the one broadcast to every participant (single-decision) — see the
    /// module's runtime seam.
    pub fn resolve(self) -> Outcome {
        if self.votes > 0 {
            Outcome::Commit(Decision { _o: PhantomData })
        } else {
            Outcome::Abort(Decision { _o: PhantomData })
        }
    }
}

impl Default for Ballot {
    fn default() -> Self {
        Self::new()
    }
}

/// The result of [`Ballot::resolve`]: the single decision, tagged with its outcome.
/// Matching it yields a [`Decision<Commit>`] or a [`Decision<Abort>`].
#[must_use = "the ballot's decision must be applied to the participants"]
pub enum Outcome {
    /// Every vote was YES — the transaction commits.
    Commit(Decision<Commit>),
    /// The transaction aborts.
    Abort(Decision<Abort>),
}

/// A coordinator decision of outcome `O`. `Copy`: it is *meant* to be broadcast so
/// every participant applies the same one. The types do **not** force that —
/// [`decide`](Participant::decide) accepts any `Decision<O>`, so a single decision per
/// transaction (and hence uniform agreement) is a coordinator obligation, not a
/// consequence of this token; see the module's atomicity note.
///
/// Minted **only** by [`Ballot::resolve`] (private field, no public constructor), so a
/// participant cannot conjure the outcome it wants. What it certifies is narrow: that
/// *a ballot resolved to `O`* — not that the ballot saw every member, nor that it is the
/// one decision for this transaction (both are coordinator obligations; see the module's
/// runtime seam).
pub struct Decision<O: OutcomeKind> {
    _o: Phantom<O>,
}

impl<O: OutcomeKind> Clone for Decision<O> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<O: OutcomeKind> Copy for Decision<O> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_yes_commits_and_every_participant_applies_the_same_decision() {
        let (p0, v0) = Participant::<Working>::join(1).prepare(true).ok().unwrap();
        let (p1, v1) = Participant::<Working>::join(2).prepare(true).ok().unwrap();

        let ballot = Ballot::new().record(v0).record(v1);
        assert_eq!(ballot.yes_votes(), 2);

        let decision = match ballot.resolve() {
            Outcome::Commit(d) => d,
            Outcome::Abort(_) => panic!("all YES must commit"),
        };

        let f0 = p0.decide(decision);
        let f1 = p1.decide(decision); // decision is Copy — broadcast to both
        assert!(f0.committed());
        assert!(f1.committed());
    }

    #[test]
    fn a_yes_vote_carries_its_voters_id() {
        let (_p, vote) = Participant::<Working>::join(42).prepare(true).ok().unwrap();
        assert_eq!(vote.voter(), 42, "a vote records which participant cast it");
    }

    #[test]
    fn a_no_voter_aborts_unilaterally_and_emits_no_vote() {
        // Voting NO returns an already-aborted participant — no coordinator needed.
        let aborted = match Participant::<Working>::join(7).prepare(false) {
            Ok(_) => panic!("a NO vote must abort, not prepare"),
            Err(aborted) => aborted,
        };
        assert!(!aborted.committed());
        assert_eq!(aborted.id(), 7);
    }

    #[test]
    fn a_prepared_participant_applies_a_coordinator_abort() {
        // The apply-the-decision transition, not uniform agreement: a prepared
        // (in-doubt) participant cannot self-decide, and when handed an ABORT decision
        // (e.g. because a peer voted NO), it applies it. The types do NOT guarantee its
        // peers applied the *same* decision — see the module's single-decision seam.
        let (prepared, _vote) = Participant::<Working>::join(3).prepare(true).ok().unwrap();
        let decision = match Ballot::new().resolve() {
            Outcome::Abort(d) => d,
            Outcome::Commit(_) => panic!("an empty ballot must abort"),
        };
        let f = prepared.decide(decision);
        assert!(!f.committed(), "a prepared participant applies the coordinator's ABORT");
    }

    #[test]
    fn outcome_markers_report_their_kind() {
        // Read the marker const through a generic boundary so it is a real runtime
        // check (a direct `assert!(<Commit>::COMMITS)` const-folds to `assert!(true)`).
        fn commits<O: OutcomeKind>() -> bool {
            O::COMMITS
        }
        assert!(commits::<Commit>());
        assert!(!commits::<Abort>());
    }
}

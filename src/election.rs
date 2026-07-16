//! Winning leadership — the runtime-**witness** rung of the leadership-acquisition axis.
//!
//! Its structural dual is `term`. A `term::Reign<T>` types what a leader *may do* once
//! it holds authority, but mints that authority by assertion (`install`) — it presupposes the leader,
//! exactly as [`failover`](crate::failover)/[`staleness`](crate::staleness) do. `election` types the
//! act nobody else does: **becoming** leader by winning a majority of votes at a term. That cannot be
//! decided locally — it needs evidence from a quorum of the electorate — so it lands on the
//! coordinated, non-CALM side of the crate's recurring cut, the fourth axis to do so after order,
//! count, and occupancy.
//!
//! ## The witness: an election certificate is a vote-quorum at a term
//!
//! A candidate opens a [`Ballot<T>`] for term `T`, records the votes it receives, and
//! [`close`](Ballot::close)s it against the electorate [`Config<E>`](crate::membership::Config). The
//! ballot certifies its voters as a [`Quorum<E>`](crate::membership::Quorum) — reusing
//! [`membership`](crate::membership)'s `certify` boundary, not re-deriving majorities — and, only if
//! they reach a majority, mints an [`Elected<T, E>`](Elected) stamped with the term won and the
//! electorate that certified it. `Elected<T, E>` is unforgeable (private field, minted only here) and
//! is the evidence a real system's `term::Reign<T>` install *should* be gated on (the provenance the
//! lease modules take on faith). The coupling is **conventional, not type-enforced** — `term`'s
//! `install` root mints authority by assertion; this certificate is what a disciplined caller feeds
//! it, exactly as `term`'s own seam docs say.
//!
//! ## At most one leader per term — a runtime property from quorum intersection
//!
//! This property rests on the two seams below (it is not a type guarantee — see them first). Given
//! them: two candidates that both claimed term `T` would each hold a
//! [`Quorum<E>`](crate::membership::Quorum) of the *same* electorate, and any two majorities of one
//! configuration **intersect** ([`Quorum::intersect`](crate::membership::Quorum::intersect)) — the
//! shared voter would have voted twice in term `T`. Under the one-vote-per-term rule that is
//! forbidden, so a term has at most one leader. The crate already proves the intersection
//! (`membership`); `election` supplies the vote/term reading of it. The types enforce that every
//! certificate is minted only from a certified majority; the uniqueness *conclusion* is earned by the
//! seams, not by the types.
//!
//! ## Where the types stop (the runtime seam)
//!
//! * **One vote per voter per term is a declared discipline.** A [`Ballot`] dedups its *own* voters
//!   (a [`BTreeSet`]), but nothing stops a voter from also voting in a
//!   *rival's* term-`T` ballot — that cross-ballot double-vote is the axiom the uniqueness argument
//!   rests on, the same shape as [`byzantine`](crate::byzantine)'s fault budget `f`. The types make
//!   the certificate unforgeable; they do not police the electorate's honesty.
//! * **One electorate.** Uniqueness is *within* a configuration `E`; across configuration
//!   generations, quorums have different `E` and cannot be intersected —
//!   [`reconfig_safety`](crate::reconfig_safety) governs that boundary, not this rung.
//! * **`T` and `E` are type-level classes.** `Elected<T, E>` names a term and an electorate, not a
//!   particular election run (the crate's recurring class-not-instance limit).
//!
//! ## A certificate cannot be forged — unforgeability
//!
//! ```compile_fail
//! use quorum_types::election::Elected;
//! let _ = Elected::<7, 0> { _priv: () }; // private field — only `Ballot::close` mints an Elected
//! ```
//!
//! ## A certificate is tied to its electorate — the epoch is in the type
//!
//! ```compile_fail
//! use quorum_types::election::{Ballot, Elected};
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//! let cfg = Config::<0>::new(BTreeSet::from([1, 2, 3]));
//! let mut ballot = Ballot::<7>::open();
//! ballot.vote(1); ballot.vote(2);
//! let elected = ballot.close(&cfg).unwrap();      // Elected<7, 0>
//! let _wrong: Elected<7, 1> = elected;             // electorate 0 vs 1 do not unify — E0308
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::election::Ballot;
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//!
//! let electorate = Config::<0>::new(BTreeSet::from([1, 2, 3, 4, 5])); // majority = 3
//!
//! // Candidate A gathers three votes for term 7 and wins.
//! let mut a = Ballot::<7>::open();
//! a.vote(1);
//! a.vote(2);
//! a.vote(3);
//! let elected = a.close(&electorate).expect("a majority elects a leader");
//! assert_eq!(elected.term(), 7);
//!
//! // A candidate that gathers only a minority cannot be certified.
//! let mut b = Ballot::<7>::open();
//! b.vote(4);
//! b.vote(5);
//! assert!(b.close(&electorate).is_none(), "a minority wins no certificate");
//! ```

use crate::membership::{Config, NodeId, Quorum};
use std::collections::BTreeSet;

/// A candidate's vote collector for term `T`. Accumulates the distinct voters that have granted it
/// their term-`T` vote; [`close`](Self::close) certifies them against an electorate.
#[derive(Debug)]
#[must_use = "a Ballot collects votes; close it against the electorate to seek a certificate"]
pub struct Ballot<const T: u64> {
    voters: BTreeSet<NodeId>,
}

/// An **election certificate**: unforgeable evidence that a majority of electorate `E` voted for
/// this candidate at term `T`. Minted only by [`Ballot::close`] (private field). Like
/// [`membership::Quorum`](crate::membership::Quorum) it is a *witness* (a fact, hence `Clone`), not a
/// consumed resource — it carries both the term won and the electorate `E` that certified it, so the
/// "one electorate" seam is legible on the certificate itself. In a real system, holding one is the
/// provenance an install *should* be gated on — the lease [`failover`](crate::failover) takes on
/// faith; the coupling is conventional, not type-enforced (see the seam docs).
#[derive(Debug, Clone)]
#[must_use = "an Elected certificate is won leadership; use it to install a reign or it is wasted"]
pub struct Elected<const T: u64, const E: u64> {
    _priv: (),
}

impl<const T: u64> Ballot<T> {
    /// Open a fresh ballot for term `T` with no votes yet.
    pub fn open() -> Self {
        Ballot { voters: BTreeSet::new() }
    }

    /// Record a vote from `voter` for this candidate at term `T`. Idempotent within this ballot: a
    /// voter counted twice here still counts once (a [`BTreeSet`]). That a
    /// voter does not *also* vote in a rival's term-`T` ballot is the one-vote-per-term seam.
    pub fn vote(&mut self, voter: NodeId) {
        self.voters.insert(voter);
    }

    /// How many distinct votes have been recorded.
    pub fn tally(&self) -> usize {
        self.voters.len()
    }

    /// **Seek the certificate.** Certify the recorded voters as a
    /// [`Quorum<E>`](crate::membership::Quorum) of the electorate `config` (reusing
    /// [`Config::certify`](crate::membership::Config::certify)) and, only if they form a majority,
    /// mint an [`Elected<T, E>`](Elected) stamped with both the term and the electorate. Returns
    /// `None` when the votes are a minority or include non-members — no majority, no leader.
    pub fn close<const E: u64>(self, config: &Config<E>) -> Option<Elected<T, E>> {
        config.certify(self.voters).map(|_quorum: Quorum<E>| Elected { _priv: () })
    }
}

impl<const T: u64, const E: u64> Elected<T, E> {
    /// The term this certificate was won at.
    pub const fn term(&self) -> u64 {
        T
    }

    /// The electorate (configuration epoch) that certified this election. Two certificates of
    /// *different* `E` are different types and cannot be conflated — the "one electorate" boundary
    /// [`reconfig_safety`](crate::reconfig_safety) governs, made legible here.
    pub const fn electorate(&self) -> u64 {
        E
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn electorate() -> Config<0> {
        Config::new(BTreeSet::from([1, 2, 3, 4, 5]))
    }

    #[test]
    fn a_majority_elects_and_a_minority_does_not() {
        let cfg = electorate();
        let mut a = Ballot::<7>::open();
        a.vote(1);
        a.vote(2);
        a.vote(3);
        assert_eq!(a.tally(), 3);
        let elected = a.close(&cfg).expect("majority");
        assert_eq!(elected.term(), 7);

        let mut b = Ballot::<7>::open();
        b.vote(4);
        b.vote(5);
        assert!(b.close(&cfg).is_none(), "minority wins nothing");
    }

    #[test]
    fn double_vote_in_one_ballot_counts_once() {
        let cfg = electorate();
        let mut a = Ballot::<3>::open();
        a.vote(1);
        a.vote(1); // same voter again — deduped
        a.vote(2);
        assert_eq!(a.tally(), 2, "a voter counts once within a ballot");
        assert!(a.close(&cfg).is_none(), "two distinct votes are a minority of five");
    }

    /// The uniqueness argument, spelled out over the electorate: two candidates that both reach a
    /// majority at the same term must share a voter (quorum intersection). This test *demonstrates*
    /// the intersection the certificate relies on; the type-level guarantee is that both certificates
    /// are `Elected<7>` minted only from certified majorities.
    #[test]
    fn two_term_winners_would_share_a_voter() {
        let cfg = electorate();
        // A wins with {1,2,3}; a rival B claiming the same term would need another majority...
        let a_voters = BTreeSet::from([1, 2, 3]);
        let b_voters = BTreeSet::from([3, 4, 5]);
        let qa = cfg.certify(a_voters).expect("majority");
        let qb = cfg.certify(b_voters).expect("majority");
        assert!(
            qa.intersect(&qb).is_some(),
            "two same-term majorities intersect — the shared voter voted twice (forbidden)"
        );
    }
}

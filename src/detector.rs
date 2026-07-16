//! Confirmed failure — the runtime-**witness** rung of the failure-detection (liveness) axis.
//!
//! Its structural dual is `suspicion`. A `suspicion::Suspected<NODE>` is *one* observer's private,
//! local judgement that node `NODE` has gone silent — and from a single vantage a slow node is
//! indistinguishable from a dead one. Concluding a node is actually **dead** therefore cannot be done
//! locally; it needs a majority of the cluster to corroborate the silence. That is coordination — a
//! quorum round-trip — so this rung lands on the non-CALM/witness side of the crate's recurring cut,
//! the fifth axis to do so after order, count, occupancy, and leadership.
//!
//! ## The witness: a death certificate is a corroboration quorum
//!
//! A [`Report<NODE>`] collects the distinct reporters that each [`corroborate`](Report::corroborate)
//! the silence of node `NODE`. A corroboration crosses the network, so — exactly like
//! [`election`](crate::election)'s [`Ballot::vote`](crate::election::Ballot::vote) — it is a **claim**,
//! a bare [`NodeId`], not a moved local token: a peer's private `suspicion::Suspected<NODE>` cannot
//! travel over the wire, and its honesty is a seam (below), not something the type can carry across the
//! boundary. [`close`](Report::close) certifies the reporters against the electorate
//! [`Config<E>`](crate::membership::Config), reusing [`membership`](crate::membership)'s `certify`
//! boundary rather than re-deriving majorities, and mints a [`Confirmed<NODE, E>`](Confirmed) **only
//! if** they reach a majority. `Confirmed<NODE, E>` is unforgeable (private field, minted only here)
//! and, like [`membership::Quorum`](crate::membership::Quorum) and
//! [`election::Elected`](crate::election::Elected), is a *witness* — a fact, hence `Clone` — carrying
//! the node declared dead and the electorate that certified it.
//!
//! ## No contradictory verdict — a runtime property from quorum intersection
//!
//! This rests on the two seams below (it is not a type guarantee — read them first). Given them:
//! suppose one majority of electorate `E` reported `NODE` **dead** while another vouched it **alive**.
//! Both are [`Quorum<E>`](crate::membership::Quorum)s of the *same* electorate, and any two majorities
//! of one configuration **intersect** ([`Quorum::intersect`](crate::membership::Quorum::intersect)) —
//! the shared node would have to both suspect `NODE` and vouch for it in the same epoch. Under the
//! one-opinion-per-node rule that is a contradiction, so a node cannot be simultaneously confirmed dead
//! and confirmed alive. The crate already proves the intersection ([`membership`](crate::membership));
//! `detector` supplies the liveness reading of it. The types enforce that every certificate is minted
//! only from a certified majority; the no-contradiction *conclusion* is earned by the seams.
//!
//! A [`Confirmed<NODE, E>`](Confirmed) is exactly the death-provenance the rest of the crate takes on
//! faith: [`failover`](crate::failover) and [`reconfig`](crate::reconfig) evict a node and install a
//! new epoch on a *trusted* liveness judgement (a lease timing out); holding a `Confirmed` is what such
//! an eviction *should* be gated on — the same way `election::Elected` is the provenance a leader
//! install should require.
//!
//! ## Where the types stop (the runtime seam)
//!
//! * **A corroboration is a claim, not carried evidence.** [`corroborate`](Report::corroborate) records
//!   a reporter `NodeId` — the same modeling choice [`election`](crate::election) makes for a vote and
//!   [`byzantine`](crate::byzantine) makes for a fault claim. That a reporter *genuinely* held a local
//!   `suspicion::Suspected<NODE>` before reporting is trusted; a peer's linear suspicion token lives on
//!   *its* machine and cannot be moved here. The type makes the certificate unforgeable; it does not
//!   make the reporters honest.
//! * **One opinion per node per epoch is a declared discipline.** A [`Report`] dedups its *own*
//!   reporters (a [`BTreeSet`]), but nothing stops a node from reporting `NODE` silent here while
//!   vouching for it in a rival report — that cross-report contradiction is the axiom the
//!   no-contradiction argument rests on, the same shape as [`byzantine`](crate::byzantine)'s fault
//!   budget `f`.
//! * **A slow node is indistinguishable from a dead one.** Corroboration certifies "a majority reported
//!   `NODE` silent," **not** that `NODE` has crashed — a partitioned or paused node is reported exactly
//!   like a dead one. This is the fundamental asynchronous-failure-detection limit (FLP) that
//!   `suspicion` names; the quorum makes the judgement *agreed*, not *true*.
//! * **One electorate.** No-contradiction is *within* a configuration `E`; across configuration
//!   generations quorums have different `E` and cannot be intersected —
//!   [`reconfig_safety`](crate::reconfig_safety) governs that boundary, not this rung.
//! * **`NODE` and `E` are type-level classes.** `Confirmed<NODE, E>` names a node and an electorate,
//!   not a particular detection run (the crate's recurring class-not-instance limit).
//!
//! ## A death certificate cannot be forged — unforgeability
//!
//! ```compile_fail
//! use quorum_types::detector::Confirmed;
//! let _ = Confirmed::<3, 0> { _priv: () }; // private field — only `Report::close` mints a Confirmed
//! ```
//!
//! ## A certificate for one node is not one for another — node identity
//!
//! ```compile_fail
//! use quorum_types::detector::{Report, Confirmed};
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//! let cfg = Config::<0>::new(BTreeSet::from([1, 2, 3]));
//! let mut report = Report::<9>::open();
//! report.corroborate(1); report.corroborate(2);
//! let confirmed = report.close(&cfg).unwrap();   // Confirmed<9, 0>
//! let _wrong: Confirmed<8, 0> = confirmed;        // node 9 vs 8 do not unify — E0308
//! ```
//!
//! ## A certificate is tied to its electorate — the epoch is in the type
//!
//! ```compile_fail
//! use quorum_types::detector::{Report, Confirmed};
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//! let cfg = Config::<0>::new(BTreeSet::from([1, 2, 3]));
//! let mut report = Report::<9>::open();
//! report.corroborate(1); report.corroborate(2);
//! let confirmed = report.close(&cfg).unwrap();   // Confirmed<9, 0>
//! let _wrong: Confirmed<9, 1> = confirmed;        // electorate 0 vs 1 do not unify — E0308
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::detector::Report;
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//!
//! let electorate = Config::<0>::new(BTreeSet::from([1, 2, 3, 4, 5])); // majority = 3
//!
//! // Three peers each independently report node 9 gone silent.
//! let mut report = Report::<9>::open();
//! report.corroborate(1);
//! report.corroborate(2);
//! report.corroborate(3);
//! let confirmed = report.close(&electorate).expect("a majority confirms the death");
//! assert_eq!(confirmed.node(), 9);
//! assert_eq!(confirmed.electorate(), 0);
//!
//! // A lone report is not a confirmation.
//! let mut lonely = Report::<9>::open();
//! lonely.corroborate(4);
//! assert!(lonely.close(&electorate).is_none(), "one reporter is a minority — no certificate");
//! ```

use crate::membership::{Config, NodeId, Quorum};
use std::collections::BTreeSet;

/// A corroboration collector for the silence of node `NODE`. Accumulates the distinct reporters that
/// have claimed node `NODE` is silent; [`close`](Self::close) certifies them against an electorate.
#[derive(Debug)]
#[must_use = "a Report collects corroborations; close it against the electorate to seek a certificate"]
pub struct Report<const NODE: u64> {
    reporters: BTreeSet<NodeId>,
}

/// A **death certificate**: unforgeable evidence that a majority of electorate `E` reported node
/// `NODE` silent. Minted only by [`Report::close`] (private field). Like
/// [`membership::Quorum`](crate::membership::Quorum) it is a *witness* (a fact, hence `Clone`), not a
/// consumed resource — it carries both the node declared dead and the electorate `E` that certified it,
/// so the "one electorate" seam is legible on the certificate itself. In a real system, holding one is
/// the provenance an eviction *should* be gated on — the liveness judgement
/// [`failover`](crate::failover) takes on faith; the coupling is conventional, not type-enforced (see
/// the seam docs).
#[derive(Debug, Clone)]
#[must_use = "a Confirmed certificate is an agreed death; use it to gate an eviction or it is wasted"]
pub struct Confirmed<const NODE: u64, const E: u64> {
    _priv: (),
}

impl<const NODE: u64> Report<NODE> {
    /// Open a fresh report about node `NODE` with no corroborations yet.
    pub fn open() -> Self {
        Report { reporters: BTreeSet::new() }
    }

    /// Record that `reporter` claims node `NODE` has gone silent. A **claim**, not carried evidence: a
    /// reporter's local `suspicion::Suspected<NODE>` cannot be moved across the wire, so — as with
    /// [`election`](crate::election)'s vote — this records the reporter's `NodeId` and trusts that it
    /// genuinely suspected `NODE` (the corroboration seam). Idempotent in the *reporter*: the same
    /// reporter corroborating twice still counts once (a [`BTreeSet`]). That a reporter does not *also*
    /// vouch for `NODE` elsewhere is the one-opinion-per-node seam.
    pub fn corroborate(&mut self, reporter: NodeId) {
        self.reporters.insert(reporter);
    }

    /// How many distinct reporters have corroborated.
    pub fn tally(&self) -> usize {
        self.reporters.len()
    }

    /// **Seek the certificate.** Certify the reporters as a [`Quorum<E>`](crate::membership::Quorum) of
    /// the electorate `config` (reusing [`Config::certify`](crate::membership::Config::certify)) and,
    /// only if they form a majority, mint a [`Confirmed<NODE, E>`](Confirmed) stamped with both the
    /// node and the electorate. Returns `None` when the reporters are a minority or include
    /// non-members — no majority, no confirmed death.
    pub fn close<const E: u64>(self, config: &Config<E>) -> Option<Confirmed<NODE, E>> {
        config.certify(self.reporters).map(|_quorum: Quorum<E>| Confirmed { _priv: () })
    }
}

impl<const NODE: u64, const E: u64> Confirmed<NODE, E> {
    /// The node this certificate declares dead (a majority *reported* it silent — see the seam; the
    /// verdict is agreed, not proven).
    pub const fn node(&self) -> u64 {
        NODE
    }

    /// The electorate (configuration epoch) that certified this death. Two certificates of *different*
    /// `E` are different types and cannot be conflated — the "one electorate" boundary
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
    fn a_majority_confirms_and_a_minority_does_not() {
        let cfg = electorate();
        let mut report = Report::<9>::open();
        report.corroborate(1);
        report.corroborate(2);
        report.corroborate(3);
        assert_eq!(report.tally(), 3);
        let confirmed = report.close(&cfg).expect("majority");
        assert_eq!(confirmed.node(), 9);
        assert_eq!(confirmed.electorate(), 0);

        let mut lonely = Report::<9>::open();
        lonely.corroborate(4);
        lonely.corroborate(5);
        assert!(lonely.close(&cfg).is_none(), "two reporters are a minority of five");
    }

    #[test]
    fn same_reporter_corroborating_twice_counts_once() {
        let cfg = electorate();
        let mut report = Report::<7>::open();
        report.corroborate(1);
        report.corroborate(1); // same reporter again — deduped
        report.corroborate(2);
        assert_eq!(report.tally(), 2, "a reporter counts once within a report");
        assert!(report.close(&cfg).is_none(), "two distinct reporters are a minority of five");
    }

    /// The no-contradiction argument, spelled out over the electorate: a "dead" majority and an "alive"
    /// majority at the same epoch must share a node (quorum intersection). This test *demonstrates* the
    /// intersection the certificate relies on; the type-level guarantee is only that every certificate
    /// is minted from a certified majority.
    #[test]
    fn a_dead_majority_and_an_alive_majority_would_share_a_node() {
        let cfg = electorate();
        let dead_reporters = BTreeSet::from([1, 2, 3]);
        let alive_reporters = BTreeSet::from([3, 4, 5]);
        let q_dead = cfg.certify(dead_reporters).expect("majority");
        let q_alive = cfg.certify(alive_reporters).expect("majority");
        assert!(
            q_dead.intersect(&q_alive).is_some(),
            "the shared node would both suspect and vouch for the node (a contradiction)"
        );
    }
}

//! Byzantine quorum evidence — rung 4 of the evidence-discipline ladder.
//!
//! [`membership`](crate::membership) counts a **crash-fault** majority: two
//! quorums of `⌊n/2⌋+1` intersect in at least one node, and one node suffices
//! *because nodes do not lie*. Under Byzantine faults that reasoning collapses
//! at exactly one point: the shared node may be the liar. Rung 4 asks whether
//! the type system can tell the two kinds of evidence apart — so that crash
//! evidence supplied where Byzantine evidence is required is a compile error.
//!
//! ## The masking regime (why `4f+1`, not the famous `3f+1`)
//!
//! Byzantine quorum systems come in two regimes (Malkhi & Reiter, STOC '97):
//!
//! * **Dissemination** quorums — `n ≥ 3f+1`, quorums of `⌈(n+f+1)/2⌉`, any two
//!   intersect in `≥ f+1` nodes. Licensed **only for self-verifying data**
//!   (digitally signed): a faulty server can suppress a signed value but
//!   cannot undetectably alter it, so *one* correct node in the intersection
//!   suffices to propagate it. PBFT and HotStuff live here — their messages
//!   are authenticated.
//! * **Masking** quorums — `n ≥ 4f+1`, quorums of `⌈(n+2f+1)/2⌉`, any two
//!   intersect in `≥ 2f+1` nodes. For **unsigned** data, where the reader must
//!   identify the correct value *by vote*: the intersection holds `≥ f+1`
//!   correct up-to-date nodes, which outnumber all `f` Byzantine nodes —
//!   including liars *outside* the intersection vouching for a fabrication.
//!
//! This toy has no signatures — its values are plain `T`s — so the honest
//! regime is **masking**. Choosing `3f+1` here would silently assume a
//! signature scheme the toy does not model.
//!
//! ## What the types guarantee — and the trust model
//!
//! [`BftConfig::new`] rejects configurations that cannot tolerate their
//! declared fault budget (`n ≥ 4f+1`). [`BftConfig::certify`] is the runtime
//! boundary that mints a [`ByzQuorum`] — a certificate **distinct in type**
//! from the crash [`Quorum`](crate::membership::Quorum), so an API demanding
//! Byzantine evidence cannot be handed a counted majority. Two same-epoch
//! `ByzQuorum`s always overlap in `≥ 2f+1` nodes; [`ByzQuorum::intersect`] is
//! therefore *infallible* and returns an [`Overlap`] witness rather than an
//! `Option`.
//!
//! The evidence character, one rung further down the ladder: rung 2's witness
//! is a counted majority (exact); rung 3's is sampled laws (probabilistic);
//! rung 4's is a **counted supermajority conditional on an unverifiable trust
//! declaration**. The fault budget `f` is an *axiom the operator declares*,
//! not a fact the types check — nothing here can verify that at most `f`
//! members actually lie. Types verify chains; operators choose roots (the
//! third instance: rung 2's `Config::new` membership, rung 3's caller-chosen
//! samples, rung 4's declared `f`).
//!
//! **Scope fence (equivocation).** This module types the *counting* layer
//! only: who may be believed, never what they said. Byzantine nodes also lie
//! about **values** — real safety additionally needs `f+1` *matching*
//! responses drawn from an overlap, an attested-value layer this toy
//! deliberately does not model. Possessing an [`Overlap`] proves the
//! arithmetic ran against the declared budget; it does not launder any
//! particular value into truth.
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::byzantine::BftConfig;
//! use std::collections::BTreeSet;
//!
//! // n = 5, f = 1 (minimum masking cluster): quorums of ⌈(5+3)/2⌉ = 4.
//! let cfg = BftConfig::<0>::new(BTreeSet::from([1, 2, 3, 4, 5]), 1).unwrap();
//! let q1 = cfg.certify(BTreeSet::from([1, 2, 3, 4])).unwrap();
//! let q2 = cfg.certify(BTreeSet::from([2, 3, 4, 5])).unwrap();
//!
//! let overlap = q1.intersect(&q2); // infallible — no Option
//! assert_eq!(overlap.members(), &BTreeSet::from([2, 3, 4])); // ≥ 2f+1 = 3
//! assert_eq!(overlap.min_correct(), 2); // ≥ f+1 correct, IF at most f lie
//! ```
//!
//! ## Crash evidence is not Byzantine evidence
//!
//! The rung's namesake compile error: a counted majority cannot stand in for
//! a masking quorum, however large it is.
//!
//! ```compile_fail,E0308
//! use quorum_types::byzantine::{BftConfig, ByzQuorum};
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//!
//! fn read_repair(_evidence: &ByzQuorum<0>) { /* needs masking overlap */ }
//!
//! let crash_q = Config::<0>::new(BTreeSet::from([1, 2, 3, 4, 5]))
//!     .certify(BTreeSet::from([1, 2, 3])).unwrap();
//! read_repair(&crash_q); // Quorum<0> is not ByzQuorum<0> — wrong fault model
//! ```
//!
//! The epoch guard carries over unchanged — quorums of different generations
//! cannot even be asked to overlap:
//!
//! ```compile_fail,E0308
//! use quorum_types::byzantine::BftConfig;
//! use std::collections::BTreeSet;
//!
//! let members = BTreeSet::from([1, 2, 3, 4, 5]);
//! let q0 = BftConfig::<0>::new(members.clone(), 1).unwrap()
//!     .certify(BTreeSet::from([1, 2, 3, 4])).unwrap();
//! let q1 = BftConfig::<1>::new(members, 1).unwrap()
//!     .certify(BTreeSet::from([2, 3, 4, 5])).unwrap();
//! let _ = q0.intersect(&q1); // ByzQuorum<0> vs ByzQuorum<1> — type error
//! ```
//!
//! And the witness cannot be forged — its fields are private, so the only
//! constructor is the checked intersection:
//!
//! ```compile_fail,E0451
//! use quorum_types::byzantine::Overlap;
//! use std::collections::BTreeSet;
//! let _fake = Overlap::<0> { members: BTreeSet::from([9]), f: 1 };
//! ```

use std::collections::BTreeSet;

use crate::membership::NodeId;

/// A cluster configuration at epoch `E` with a **declared** Byzantine fault
/// budget `f`.
///
/// `f` is the trust root of this rung: the types propagate its consequences
/// but cannot check it against reality. Constructed only through
/// [`BftConfig::new`], which enforces the masking existence bound `n ≥ 4f+1`.
#[derive(Debug, Clone)]
pub struct BftConfig<const E: u64> {
    members: BTreeSet<NodeId>,
    f: usize,
}

/// A **certified masking quorum** of configuration `E`: a subset of size
/// `≥ ⌈(n+2f+1)/2⌉`, minted only by [`BftConfig::certify`].
///
/// Deliberately a different type from the crash
/// [`Quorum`](crate::membership::Quorum): the two certificates attest
/// different intersection guarantees, and substituting the weaker for the
/// stronger is a compile error, not a runtime surprise.
#[derive(Debug, Clone)]
#[must_use = "a ByzQuorum is a certified masking quorum; use it or the certification was pointless"]
pub struct ByzQuorum<const E: u64> {
    members: BTreeSet<NodeId>,
    f: usize,
}

/// The witness minted by [`ByzQuorum::intersect`]: the overlap of two
/// same-epoch masking quorums, guaranteed `≥ 2f+1` members by arithmetic.
///
/// What it buys — *conditional on the declared `f`* — is that its correct
/// up-to-date members (`≥ f+1`, see [`Overlap::min_correct`]) outnumber every
/// possible set of liars. It gates **standing**, not computation: members
/// stay readable, and nothing stops out-of-band arithmetic over them; only
/// the badge itself is unforgeable.
#[derive(Debug, Clone)]
pub struct Overlap<const E: u64> {
    members: BTreeSet<NodeId>,
    f: usize,
}

impl<const E: u64> BftConfig<E> {
    /// A configuration declaring fault budget `f` over the given members.
    ///
    /// Returns `None` unless `n ≥ 4f+1` — the existence bound for masking
    /// quorum systems. (At the bound, a quorum is exactly the `n − f` nodes
    /// that survive `f` crashes: availability and consistency meet.)
    pub fn new(members: BTreeSet<NodeId>, f: usize) -> Option<Self> {
        if members.len() > 4 * f {
            Some(BftConfig { members, f })
        } else {
            None
        }
    }

    /// The number of members in this configuration.
    pub fn size(&self) -> usize {
        self.members.len()
    }

    /// The declared fault budget — an operator-chosen axiom, not a checked fact.
    pub const fn fault_budget(&self) -> usize {
        self.f
    }

    /// The masking quorum threshold `⌈(n+2f+1)/2⌉`.
    ///
    /// Two subsets of this size intersect in `≥ 2q − n ≥ 2f+1` members. At
    /// `f = 0` this degenerates to `⌊n/2⌋+1` — the crash majority: the
    /// [`membership`](crate::membership) module is the `f = 0` fiber of this one.
    pub fn threshold(&self) -> usize {
        (self.members.len() + 2 * self.f + 1).div_ceil(2)
    }

    /// The epoch (generation) of this configuration.
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// The certify boundary: mint a [`ByzQuorum`] from a runtime subset.
    /// Returns `None` (no certificate) unless `subset` is drawn from this
    /// config's members and reaches the masking threshold.
    pub fn certify(&self, subset: BTreeSet<NodeId>) -> Option<ByzQuorum<E>> {
        let is_subset = subset.is_subset(&self.members);
        if is_subset && subset.len() >= self.threshold() {
            Some(ByzQuorum { members: subset, f: self.f })
        } else {
            None
        }
    }
}

impl<const E: u64> ByzQuorum<E> {
    /// The members that make up this quorum.
    pub fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }

    /// The fault budget this quorum was certified against.
    pub const fn fault_budget(&self) -> usize {
        self.f
    }

    /// The epoch this quorum was certified against.
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// The rung-4 safety lemma: the overlap with another quorum of the
    /// **same** configuration epoch.
    ///
    /// Infallible where rung 2's `intersect` returned `Option<NodeId>`: the
    /// masking threshold makes `|overlap| ≥ 2f+1` an arithmetic fact, and the
    /// shared `E` is enforced by the signature, so there is no failure case
    /// left to represent. What changed down the ladder is not fallibility but
    /// *what the witness means* — see [`Overlap`].
    pub fn intersect(&self, other: &ByzQuorum<E>) -> Overlap<E> {
        Overlap {
            members: self.members.intersection(&other.members).copied().collect(),
            f: self.f,
        }
    }
}

impl<const E: u64> Overlap<E> {
    /// The overlapping members: `≥ 2f+1` of them, by the masking arithmetic.
    pub fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }

    /// The minimum number of **correct** members in this overlap — at least
    /// `f+1`, and strictly more than any set of liars can muster —
    /// **conditional on at most `f` members actually being Byzantine**. If
    /// the operator's declared budget is wrong, this number is wrong; no type
    /// here can tell.
    pub fn min_correct(&self) -> usize {
        self.members.len() - self.f
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::Config;

    fn all_subsets(members: &BTreeSet<NodeId>) -> impl Iterator<Item = BTreeSet<NodeId>> + '_ {
        let all: Vec<NodeId> = members.iter().copied().collect();
        (0..(1u32 << all.len())).map(move |mask| {
            all.iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, &id)| id)
                .collect()
        })
    }

    #[test]
    fn masking_bound_rejects_undersized_configs() {
        let members = |n: u32| -> BTreeSet<NodeId> { (1..=n).collect() };
        assert!(BftConfig::<0>::new(members(4), 1).is_none(), "n=4 < 4f+1=5");
        assert!(BftConfig::<0>::new(members(5), 1).is_some(), "n=5 = 4f+1");
        assert!(BftConfig::<0>::new(members(8), 2).is_none(), "n=8 < 4f+1=9");
        assert!(BftConfig::<0>::new(members(9), 2).is_some(), "n=9 = 4f+1");
        // The famous dissemination bound is NOT enough here — no signatures:
        assert!(BftConfig::<0>::new(members(7), 2).is_none(), "n=3f+1 rejected");
    }

    #[test]
    fn threshold_matches_the_paper() {
        let cfg = |n: u32, f| BftConfig::<0>::new((1..=n).collect(), f).unwrap();
        assert_eq!(cfg(5, 1).threshold(), 4); // ⌈(5+3)/2⌉
        assert_eq!(cfg(9, 2).threshold(), 7); // ⌈(9+5)/2⌉
        assert_eq!(cfg(13, 3).threshold(), 10); // ⌈(13+7)/2⌉
    }

    #[test]
    fn f_zero_degenerates_to_crash_majority() {
        // At f = 0 the masking threshold ⌈(n+1)/2⌉ IS ⌊n/2⌋+1: the crash
        // membership module is the f = 0 fiber of the Byzantine one.
        for n in 1..=8u32 {
            let members: BTreeSet<NodeId> = (1..=n).collect();
            let byz = BftConfig::<0>::new(members.clone(), 0).unwrap();
            let crash = Config::<0>::new(members);
            assert_eq!(byz.threshold(), crash.threshold(), "n={n}");
        }
    }

    #[test]
    fn certify_requires_threshold_and_membership() {
        let cfg = BftConfig::<0>::new((1..=5).collect(), 1).unwrap();
        assert!(cfg.certify(BTreeSet::from([1, 2, 3, 4])).is_some(), "threshold certifies");
        assert!(cfg.certify(BTreeSet::from([1, 2, 3])).is_none(), "crash majority rejected");
        assert!(cfg.certify(BTreeSet::from([1, 2, 3, 9])).is_none(), "non-member rejected");
        assert!(cfg.certify(BTreeSet::new()).is_none(), "empty rejected");
    }

    /// Exhaustive proof over the toy domain: for every f=1 configuration up
    /// to size 7, every pair of certifiable quorums overlaps in ≥ 2f+1
    /// members, and the overlap's correct members outnumber ALL f liars no
    /// matter where the liars sit (any f-subset of the whole config).
    #[test]
    fn every_overlap_masks_every_possible_liar_set() {
        let f = 1usize;
        for n in 5..=7u32 {
            let members: BTreeSet<NodeId> = (1..=n).collect();
            let cfg = BftConfig::<0>::new(members.clone(), f).unwrap();
            let quorums: Vec<ByzQuorum<0>> =
                all_subsets(&members).filter_map(|s| cfg.certify(s)).collect();
            assert!(!quorums.is_empty(), "n={n}: some quorum must be certifiable");

            for q1 in &quorums {
                for q2 in &quorums {
                    let overlap = q1.intersect(q2);
                    assert!(
                        overlap.members().len() > 2 * f,
                        "n={n}: overlap {:?} smaller than 2f+1",
                        overlap.members()
                    );
                    assert_eq!(overlap.min_correct(), overlap.members().len() - f);
                    // Adversary placement is unconstrained: any f-subset of
                    // the CONFIG may lie, yet correct overlap members must
                    // still outnumber them.
                    for liars in all_subsets(&members).filter(|s| s.len() == f) {
                        let correct_in_overlap =
                            overlap.members().difference(&liars).count();
                        assert!(
                            correct_in_overlap > f,
                            "n={n}: liars {liars:?} outvote overlap {:?}",
                            overlap.members()
                        );
                    }
                }
            }
        }
    }

    /// Negative control: the crash-majority threshold does NOT survive the
    /// same adversary. This is the arithmetic fact that motivates the
    /// distinct certificate type.
    #[test]
    fn crash_majorities_fail_the_masking_property() {
        let members: BTreeSet<NodeId> = (1..=5).collect();
        let crash = Config::<0>::new(members);
        let q1 = crash.certify(BTreeSet::from([1, 2, 3])).unwrap();
        let q2 = crash.certify(BTreeSet::from([3, 4, 5])).unwrap();
        // The intersection is exactly {3}; if node 3 is the one liar, no
        // correct node is shared — crash evidence says nothing here.
        let shared: BTreeSet<NodeId> =
            q1.members().intersection(q2.members()).copied().collect();
        assert_eq!(shared, BTreeSet::from([3]));
        // And the Byzantine boundary refuses to certify that quorum size:
        let byz = BftConfig::<0>::new((1..=5).collect(), 1).unwrap();
        assert!(byz.certify(BTreeSet::from([1, 2, 3])).is_none());
    }
}

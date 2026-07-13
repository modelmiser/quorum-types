//! Dynamic, unbounded membership — research gap #2.
//!
//! The base module ([`crate`]) encodes membership as *type-level* sets
//! (`All`/`Lo`/`Hi`): static, at most two parts, and **disjoint** — a partition,
//! the way a GPU warp divides its lanes. That only suffices because a warp never
//! fails. Real clusters are dynamic, unbounded, and *do* fail, and their safety
//! rests on the **opposite** set relation:
//!
//! * warp-types complements are **disjoint** — `Lo ∩ Hi = ∅` (a partition).
//! * distributed quorums must **intersect** — any two majorities share a member.
//!   Overlap is precisely what prevents two groups from both committing.
//!
//! So the dynamic generalization is not "make the lane set bigger" — it is to
//! replace *disjoint complement* with *intersecting quorum*. Membership becomes a
//! **runtime** value ([`Config`] holds a `BTreeSet<NodeId>` of any size); the
//! type carries only the **relational** guarantees — the configuration epoch `E`
//! and the fact that a value *is* a quorum of it. This is the `gradual` pattern:
//! [`Config::certify`] is the runtime-checked boundary that mints a typed
//! [`Quorum`]; inside, the majority property is trusted structurally.
//!
//! ```
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//!
//! let cfg = Config::<0>::new(BTreeSet::from([1, 2, 3, 4, 5])); // majority = 3
//! let q1 = cfg.certify(BTreeSet::from([1, 2, 3])).unwrap();
//! let q2 = cfg.certify(BTreeSet::from([3, 4, 5])).unwrap();
//! // Two majorities of the same config always share a member:
//! assert!(q1.intersect(&q2).is_some());
//! // A sub-majority cannot be certified:
//! assert!(cfg.certify(BTreeSet::from([1, 2])).is_none());
//! ```
//!
//! The epoch guard from the base module carries over: quorums of *different*
//! configurations have different `E`, so intersecting them is a compile error —
//! you cannot even ask whether two different generations' quorums overlap.
//!
//! ```compile_fail
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//! let q0 = Config::<0>::new(BTreeSet::from([1, 2, 3])).certify(BTreeSet::from([1, 2, 3])).unwrap();
//! let q1 = Config::<1>::new(BTreeSet::from([1, 2, 3])).certify(BTreeSet::from([1, 2, 3])).unwrap();
//! let _ = q0.intersect(&q1); // Quorum<0> vs Quorum<1> — type error
//! ```

use core::marker::PhantomData;
use std::collections::BTreeSet;

/// A cluster member's identity.
pub type NodeId = u32;

/// A cluster configuration at type-level epoch `E`: a runtime set of members of
/// any size. The epoch tags the *generation*; the membership itself is dynamic.
#[derive(Debug, Clone)]
pub struct Config<const E: u64> {
    members: BTreeSet<NodeId>,
}

/// A **certified** majority subset of a [`Config`] at epoch `E`.
///
/// Constructed only through [`Config::certify`], so possessing a `Quorum<E>` is
/// evidence that its members form a majority of configuration `E`. The type
/// carries the *relation* (majority-of-E), never *which* members — that stays a
/// runtime value, which is what lets membership be unbounded.
#[derive(Debug, Clone)]
#[must_use = "a Quorum is a certified majority; use it or the certification was pointless"]
pub struct Quorum<const E: u64> {
    members: BTreeSet<NodeId>,
    _epoch: PhantomData<[(); 0]>,
}

impl<const E: u64> Config<E> {
    /// A configuration over the given members.
    pub fn new(members: BTreeSet<NodeId>) -> Self {
        Config { members }
    }

    /// The number of members in this configuration.
    pub fn size(&self) -> usize {
        self.members.len()
    }

    /// The majority threshold: strictly more than half. `⌊n/2⌋ + 1`.
    ///
    /// This is the value that makes quorums intersect: two subsets each of size
    /// `≥ threshold` have sizes summing to `≥ n + 1 > n`, so by inclusion–
    /// exclusion they must share a member.
    pub fn threshold(&self) -> usize {
        self.members.len() / 2 + 1
    }

    /// The epoch (generation) of this configuration.
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// The `gradual` boundary: certify a runtime subset as a [`Quorum`] of this
    /// configuration. Returns `None` (no certificate) unless `subset` is drawn
    /// from this config's members and reaches the majority threshold.
    pub fn certify(&self, subset: BTreeSet<NodeId>) -> Option<Quorum<E>> {
        let is_subset = subset.is_subset(&self.members);
        if is_subset && subset.len() >= self.threshold() {
            Some(Quorum { members: subset, _epoch: PhantomData })
        } else {
            None
        }
    }
}

impl<const E: u64> Quorum<E> {
    /// The members that make up this quorum.
    pub fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }

    /// The epoch this quorum was certified against.
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// The relational safety lemma: a member shared with `other`.
    ///
    /// For two quorums of the **same** configuration this is always `Some`
    /// (majority intersection). The shared epoch `E` is enforced by the type —
    /// quorums of different configurations have different `E` and cannot be
    /// passed here, so this signature *only* admits the case where intersection
    /// is guaranteed.
    pub fn intersect(&self, other: &Quorum<E>) -> Option<NodeId> {
        self.members.intersection(&other.members).next().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config5() -> Config<0> {
        Config::new(BTreeSet::from([1, 2, 3, 4, 5]))
    }

    #[test]
    fn threshold_is_strict_majority() {
        assert_eq!(config5().threshold(), 3); // ⌊5/2⌋+1
        assert_eq!(Config::<0>::new(BTreeSet::from([1, 2, 3, 4])).threshold(), 3); // ⌊4/2⌋+1
        assert_eq!(Config::<0>::new(BTreeSet::from([1])).threshold(), 1);
    }

    #[test]
    fn certify_requires_majority_and_membership() {
        let cfg = config5();
        assert!(cfg.certify(BTreeSet::from([1, 2, 3])).is_some(), "majority certifies");
        assert!(cfg.certify(BTreeSet::from([1, 2])).is_none(), "sub-majority rejected");
        assert!(cfg.certify(BTreeSet::from([1, 2, 9])).is_none(), "non-member rejected");
        assert!(cfg.certify(BTreeSet::new()).is_none(), "empty rejected");
    }

    /// Exhaustive proof over the toy domain: for every configuration up to size 6
    /// and every pair of certifiable quorums, `intersect` finds a shared member.
    /// This is the majority-intersection property that generalizes complementarity.
    #[test]
    fn any_two_same_config_quorums_intersect() {
        for n in 1..=6u32 {
            let members: BTreeSet<NodeId> = (1..=n).collect();
            let cfg = Config::<0>::new(members.clone());
            let all: Vec<NodeId> = members.iter().copied().collect();

            // Enumerate every subset via bitmask, keep the certifiable ones.
            let quorums: Vec<Quorum<0>> = (0..(1u32 << n))
                .filter_map(|mask| {
                    let subset: BTreeSet<NodeId> = all
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| mask & (1 << i) != 0)
                        .map(|(_, &id)| id)
                        .collect();
                    cfg.certify(subset)
                })
                .collect();

            for q1 in &quorums {
                for q2 in &quorums {
                    assert!(
                        q1.intersect(q2).is_some(),
                        "n={n}: quorums {:?} and {:?} must intersect",
                        q1.members(),
                        q2.members()
                    );
                }
            }
        }
    }
}

//! Compile-time reconfiguration safety — the *intersection-preserving reconfig*
//! rung. Operationalizes a model-checked finding: an aggressive membership change
//! can make an old-config majority and a new-config majority **disjoint**, so two
//! leaders elect across the reconfig (split-brain).
//!
//! [`reconfig`](crate::reconfig) already documents this hazard and covers it
//! *temporally* — its `across_epoch_quorums_can_be_disjoint` test shows the member
//! sets can be disjoint, and it leans on the **lease** to sequence the old leader
//! out. But that lease guard ("form only when no prior leader still serves") is
//! only *implementable* when the forming quorum shares a member with the old one —
//! the shared member is what carries "someone is still serving." An aggressive
//! shrink whose majorities can be disjoint severs that channel, and the guard
//! cannot be evaluated. The safe condition is that quorums keep intersecting
//! across the change:
//!
//! ```text
//!   maj(OLD) + maj(NEW) > |OLD ∪ NEW|
//! ```
//!
//! For a **nested** reconfig (members added xor removed) `|OLD ∪ NEW| = max(OLD,
//! NEW)`, giving `maj(OLD) + maj(NEW) > max(OLD, NEW)` — the **quorum-intersection
//! condition** that every Raft single-server change satisfies (it is broader,
//! admitting some multi-member steps such as `6 → 8`). This rung makes violating
//! it a **compile error**:
//! a `5 → 3` shrink (an old majority `{3,4,5}` and a new majority `{1,2}` are
//! disjoint) does not typecheck, while a single-member `5 → 4` change does.
//!
//! ```text
//!   maj: 1→1  2→2  3→2  4→3  5→3  6→4  7→4      (⌊m/2⌋+1)
//!   5→4:  maj5+maj4 = 3+3 = 6 > max(5,4)=5   ✓ intersection preserved
//!   5→3:  maj5+maj3 = 3+2 = 5 > max(5,3)=5   ✗ majorities can be disjoint
//! ```
//!
//! ## Breaking quorum intersection is a compile error
//!
//! ```compile_fail
//! use quorum_types::reconfig_safety::SizedConfig;
//! use std::collections::BTreeSet;
//! let c5 = SizedConfig::<5>::new(BTreeSet::from([1, 2, 3, 4, 5])).unwrap();
//! // 5 → 3 : maj(5)+maj(3) = 5, NOT > 5 — an old majority and a new majority can
//! // be disjoint, so this does not typecheck.
//! let _c3 = c5.reconfigure::<3>(BTreeSet::from([1, 2, 3]));
//! ```
//!
//! ## A single-member change compiles
//!
//! ```
//! use quorum_types::reconfig_safety::SizedConfig;
//! use std::collections::BTreeSet;
//! let c5 = SizedConfig::<5>::new(BTreeSet::from([1, 2, 3, 4, 5])).unwrap();
//! // 5 → 4 : maj(5)+maj(4) = 6 > 5 — every old majority meets every new majority.
//! let c4 = c5.reconfigure::<4>(BTreeSet::from([1, 2, 3, 4])).unwrap();
//! assert_eq!(c4.size(), 4);
//! ```
//!
//! ## Where the types stop (the runtime seam) — invariant-confluence boundary
//!
//! The size condition [`preserves_intersection`] is a pure arithmetic fact — the
//! **I-confluent, compile-time core**: it needs no runtime coordination and is
//! checked at monomorphization. But it is sound only for a *nested* change (union
//! = max of the sizes). Whether the two member *sets* are actually nested (an
//! add-only or remove-only step, not a same-size swap into a disjoint set) is
//! **runtime data**, checked at the `certify`-style gradual boundary inside
//! [`SizedConfig::reconfigure`]. So the split is the crate's usual one: the type owns the
//! *arithmetic* precondition; the runtime owns the *actual membership*. Types
//! verify the size step; the operator supplies the sets.

use crate::membership::NodeId;
use std::collections::BTreeSet;

const fn majority(m: usize) -> usize {
    m / 2 + 1
}
const fn max2(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// The **quorum-intersection condition** (which Raft's single-server-change rule
/// conservatively guarantees, and which is strictly broader): for a **nested**
/// reconfiguration between an `old`-member and a `new`-member config, every old
/// majority is guaranteed to meet every new majority iff
/// `maj(old) + maj(new) > max(old, new)`. A `const fn` so it can gate
/// [`SizedConfig::reconfigure`] at compile time and be asserted in tests.
pub const fn preserves_intersection(old: usize, new: usize) -> bool {
    majority(old) + majority(new) > max2(old, new)
}

/// A configuration whose member **count** `M` is lifted into the type, so that a
/// reconfiguration's intersection-safety is decided at compile time. Move-only: a
/// reconfiguration consumes the old config, which it supersedes.
#[must_use = "a SizedConfig is a live configuration; reconfigure it or hold it"]
pub struct SizedConfig<const M: usize> {
    members: BTreeSet<NodeId>,
}

impl<const M: usize> SizedConfig<M> {
    /// The `gradual` boundary: mint an `M`-member config from a runtime set —
    /// `Some` iff the set actually has `M` members, pinning the count into the type.
    pub fn new(members: BTreeSet<NodeId>) -> Option<Self> {
        (members.len() == M).then_some(SizedConfig { members })
    }

    /// The configuration's members.
    pub fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }

    /// The member count (mirrors `M`).
    pub const fn size(&self) -> usize {
        M
    }

    /// Reconfigure to an `N`-member configuration.
    ///
    /// **Compile time:** if the size step `M → N` would break quorum intersection
    /// ([`preserves_intersection`] is false), this fails to compile — the `const`
    /// block below is evaluated at monomorphization. **Runtime (`gradual`
    /// boundary):** returns `Some` only when `new_members` has exactly `N` members
    /// *and* is nested with the old set (add-only or remove-only) — the assumption
    /// the size condition relies on.
    pub fn reconfigure<const N: usize>(
        self,
        new_members: BTreeSet<NodeId>,
    ) -> Option<SizedConfig<N>> {
        const {
            assert!(
                preserves_intersection(M, N),
                "reconfig breaks quorum intersection: an old-config majority and a \
                 new-config majority can be disjoint, so two leaders can elect across \
                 the change (split-brain). Changing membership one member at a time \
                 always satisfies the condition (Raft's single-server-change rule)."
            );
        }
        if new_members.len() != N {
            return None;
        }
        let nested =
            new_members.is_subset(&self.members) || self.members.is_subset(&new_members);
        nested.then_some(SizedConfig { members: new_members })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_condition_matches_hand_computation() {
        assert!(preserves_intersection(5, 5)); // identity always safe
        assert!(preserves_intersection(5, 4)); // 3+3 > 5
        assert!(preserves_intersection(4, 5)); // symmetric
        assert!(preserves_intersection(3, 4)); // 2+3 > 4
        assert!(!preserves_intersection(5, 3)); // 3+2 = 5, not > 5
        assert!(!preserves_intersection(3, 5)); // symmetric
        assert!(!preserves_intersection(7, 4)); // 4+3 = 7, not > 7 (remove 3 at once)
    }

    #[test]
    fn single_member_shrink_succeeds_when_nested() {
        let c5 = SizedConfig::<5>::new(BTreeSet::from([1, 2, 3, 4, 5])).unwrap();
        let c4 = c5.reconfigure::<4>(BTreeSet::from([1, 2, 3, 4])).unwrap();
        assert_eq!(c4.size(), 4);
        assert_eq!(c4.members(), &BTreeSet::from([1, 2, 3, 4]));
    }

    #[test]
    fn single_member_grow_succeeds_when_nested() {
        let c4 = SizedConfig::<4>::new(BTreeSet::from([1, 2, 3, 4])).unwrap();
        let c5 = c4.reconfigure::<5>(BTreeSet::from([1, 2, 3, 4, 5])).unwrap();
        assert_eq!(c5.size(), 5);
    }

    #[test]
    fn non_nested_change_is_rejected_at_the_runtime_seam() {
        // 5 → 4 passes the compile-time size gate, but swapping in a member not
        // present in the old set (6) is not a nested step — the size condition's
        // assumption fails, and the runtime boundary rejects it.
        let c5 = SizedConfig::<5>::new(BTreeSet::from([1, 2, 3, 4, 5])).unwrap();
        assert!(c5.reconfigure::<4>(BTreeSet::from([1, 2, 3, 6])).is_none());
    }

    #[test]
    fn new_rejects_wrong_count() {
        assert!(SizedConfig::<5>::new(BTreeSet::from([1, 2, 3])).is_none());
        assert!(SizedConfig::<3>::new(BTreeSet::from([1, 2, 3])).is_some());
    }
}

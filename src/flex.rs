//! Flexible read/write quorums — typing the `R + W > N` frontier (Flexible Paxos).
//!
//! [`membership`](crate::membership) types a *single* quorum kind: any two
//! majorities intersect because `2·maj(N) > N`. But strict majority is only one
//! point on a frontier. **Flexible Paxos** (Howard, Malkhi & Spiegelman) observes
//! that read quorums need only intersect *write* quorums — not each other — which
//! relaxes the requirement to `R + W > N`, admitting asymmetric configurations
//! (fast writes with `W` small and `R` large, or the reverse). Loop-3-A of the
//! research loops proved parametrically (z3) that `R + W > N` is *exactly* the
//! condition under which every read quorum meets every write quorum. This module
//! turns that proof into **typed prevention**: *observing* that a read sees a write
//! — the safety-bearing operation — does not compile unless `R + W > N`, so a
//! miss-prone sizing can never yield an intersection witness at runtime.
//!
//! ## The load-bearing gate: `R + W > N` is checked at compile time
//!
//! [`ReadQuorum<N, R>`] and [`WriteQuorum<N, W>`] are *distinct* types over an
//! `N`-node cluster. The only way to observe that a read sees a write is
//! [`read_sees_write`], whose body contains an inline `const {}` assertion that
//! `R + W > N`. If the sizing violates the frontier, the crate does not compile —
//! the same `const {}` threshold-lift the reconfiguration rung uses, now for the
//! flexible frontier instead of quorum-intersection-under-reconfig.
//!
//! ```compile_fail
//! use quorum_types::flex::{ReadQuorum, WriteQuorum, read_sees_write};
//! use std::collections::BTreeSet;
//! let r = ReadQuorum::<5, 2>::certify(BTreeSet::from([0, 1])).unwrap();
//! let w = WriteQuorum::<5, 2>::certify(BTreeSet::from([2, 3])).unwrap();
//! let _ = read_sees_write(&r, &w); // R+W = 4, N = 5 → 4 > 5 is false → compile error
//! ```
//!
//! ## Quorums from different-sized clusters do not mix
//!
//! `N` is a type parameter, so a read quorum over a 5-node cluster and a write
//! quorum over a 7-node cluster fail to unify `N` — mixing them is a compile error
//! (the same epoch-unification trick the rest of the crate uses):
//!
//! ```compile_fail
//! use quorum_types::flex::{ReadQuorum, WriteQuorum, read_sees_write};
//! use std::collections::BTreeSet;
//! let r = ReadQuorum::<5, 3>::certify(BTreeSet::from([0, 1, 2])).unwrap();
//! let w = WriteQuorum::<7, 5>::certify(BTreeSet::from([0, 1, 2, 3, 4])).unwrap();
//! let _ = read_sees_write(&r, &w); // N: 5 vs 7 do not unify — different clusters
//! ```
//!
//! ## The happy path — an asymmetric (fast-write) configuration
//!
//! ```
//! use quorum_types::flex::{ReadQuorum, WriteQuorum, read_sees_write};
//! use std::collections::BTreeSet;
//!
//! // 5 nodes, W = 1 (write to any one node), R = 5 (read all). R+W = 6 > 5, so
//! // a read is guaranteed to observe the single written node.
//! let w = WriteQuorum::<5, 1>::certify(BTreeSet::from([3])).unwrap();
//! let r = ReadQuorum::<5, 5>::certify(BTreeSet::from([0, 1, 2, 3, 4])).unwrap();
//!
//! let overlap = read_sees_write(&r, &w);
//! assert!(overlap.shared().contains(&3), "the read observes the written node");
//! assert!(!overlap.shared().is_empty(), "R+W>N guarantees a nonempty intersection");
//! ```

use crate::membership::NodeId;
use std::collections::BTreeSet;

/// A **read quorum** of size `R` over an `N`-node cluster. Distinct from
/// [`WriteQuorum`]: the type system will not let one stand in for the other.
#[must_use]
pub struct ReadQuorum<const N: usize, const R: usize> {
    members: BTreeSet<NodeId>,
}

/// A **write quorum** of size `W` over an `N`-node cluster.
#[must_use]
pub struct WriteQuorum<const N: usize, const W: usize> {
    members: BTreeSet<NodeId>,
}

/// Evidence that a specific [`ReadQuorum<N, R>`] and [`WriteQuorum<N, W>`]
/// **intersect** — carrying the shared members. Obtainable only from
/// [`read_sees_write`], which does not compile unless `R + W > N`; by
/// inclusion-exclusion the shared set then has at least `R + W − N ≥ 1` members,
/// so this witness is genuinely nonempty (a real arithmetic guarantee, not a
/// trusted flag).
#[must_use]
pub struct Intersects<const N: usize, const R: usize, const W: usize> {
    shared: BTreeSet<NodeId>,
}

impl<const N: usize, const R: usize> ReadQuorum<N, R> {
    /// Certify a concrete member set as a read quorum of size `R`. The `gradual`
    /// boundary: returns `Some` only when `members` has exactly `R` distinct nodes,
    /// all in range `0..N`. `R` out of range (`0` or `> N`) is a compile error.
    pub fn certify(members: BTreeSet<NodeId>) -> Option<Self> {
        const {
            assert!(R >= 1 && R <= N, "ReadQuorum: R must satisfy 1 <= R <= N");
        }
        (members.len() == R && members.iter().all(|&m| (m as usize) < N))
            .then_some(ReadQuorum { members })
    }

    /// The certified member set.
    #[must_use]
    pub const fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }
}

impl<const N: usize, const W: usize> WriteQuorum<N, W> {
    /// Certify a concrete member set as a write quorum of size `W`. `W` out of
    /// range is a compile error.
    pub fn certify(members: BTreeSet<NodeId>) -> Option<Self> {
        const {
            assert!(W >= 1 && W <= N, "WriteQuorum: W must satisfy 1 <= W <= N");
        }
        (members.len() == W && members.iter().all(|&m| (m as usize) < N))
            .then_some(WriteQuorum { members })
    }

    /// The certified member set.
    #[must_use]
    pub const fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }
}

impl<const N: usize, const R: usize, const W: usize> Intersects<N, R, W> {
    /// The members shared by the read and write quorums — nonempty, because the
    /// only constructor ([`read_sees_write`]) does not compile unless `R + W > N`.
    #[must_use]
    pub const fn shared(&self) -> &BTreeSet<NodeId> {
        &self.shared
    }
}

/// **The flexible-quorum gate.** Witness that a read quorum observes a write
/// quorum, returning the shared members.
///
/// The body asserts `R + W > N` in an inline `const {}` block, so a read/write
/// sizing that could let a read miss a write **fails to compile**. When it does
/// compile, the returned [`Intersects`] is guaranteed nonempty: over `N` nodes,
/// `|r ∩ w| ≥ R + W − N ≥ 1` by inclusion-exclusion. This enforces the *sufficient*
/// direction of the parametric result (`R + W > N` ⇒ every read meets every write);
/// that the frontier is *exact* (necessity — `R + W ≤ N` admits disjoint quorums)
/// was established separately by the z3 proof, not by these types.
pub fn read_sees_write<const N: usize, const R: usize, const W: usize>(
    read: &ReadQuorum<N, R>,
    write: &WriteQuorum<N, W>,
) -> Intersects<N, R, W> {
    const {
        assert!(
            R + W > N,
            "flexible-quorum violation: R + W must exceed N, or a read can miss a write"
        );
    }
    let shared: BTreeSet<NodeId> = read.members.intersection(&write.members).copied().collect();
    Intersects { shared }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certify_accepts_right_size_and_rejects_wrong() {
        assert!(ReadQuorum::<5, 3>::certify(BTreeSet::from([0, 1, 2])).is_some());
        // wrong size
        assert!(ReadQuorum::<5, 3>::certify(BTreeSet::from([0, 1])).is_none());
        // node out of range (5 is not in 0..5)
        assert!(WriteQuorum::<5, 2>::certify(BTreeSet::from([4, 5])).is_none());
        assert!(WriteQuorum::<5, 2>::certify(BTreeSet::from([3, 4])).is_some());
    }

    #[test]
    fn majority_instance_intersects() {
        // R = W = maj(5) = 3 — the symmetric point on the frontier. 3+3 > 5.
        let r = ReadQuorum::<5, 3>::certify(BTreeSet::from([0, 1, 2])).unwrap();
        let w = WriteQuorum::<5, 3>::certify(BTreeSet::from([2, 3, 4])).unwrap();
        let overlap = read_sees_write(&r, &w);
        assert_eq!(overlap.shared(), &BTreeSet::from([2]), "the majorities share node 2");
    }

    #[test]
    fn intersection_is_at_least_r_plus_w_minus_n() {
        // R = 4, W = 4, N = 5 → |r ∩ w| ≥ 3.
        let r = ReadQuorum::<5, 4>::certify(BTreeSet::from([0, 1, 2, 3])).unwrap();
        let w = WriteQuorum::<5, 4>::certify(BTreeSet::from([1, 2, 3, 4])).unwrap();
        let overlap = read_sees_write(&r, &w);
        assert!(
            overlap.shared().len() >= 4 + 4 - 5,
            "inclusion-exclusion floor R+W-N must hold"
        );
        assert_eq!(overlap.shared(), &BTreeSet::from([1, 2, 3]));
    }

    #[test]
    fn asymmetric_fast_write_still_observed() {
        // W = 1 (write one node), R = 5 (read all): R+W = 6 > 5.
        let w = WriteQuorum::<5, 1>::certify(BTreeSet::from([3])).unwrap();
        let r = ReadQuorum::<5, 5>::certify(BTreeSet::from([0, 1, 2, 3, 4])).unwrap();
        let overlap = read_sees_write(&r, &w);
        assert!(overlap.shared().contains(&3), "a full read always sees the written node");
    }

    #[test]
    fn read_and_write_quorum_are_distinct_types() {
        // A ReadQuorum and a WriteQuorum with identical members are not the same
        // type — this is a compile-time distinction, exercised here only to show
        // both certify from the same set.
        let members = BTreeSet::from([0, 1, 2]);
        let r = ReadQuorum::<5, 3>::certify(members.clone()).unwrap();
        let w = WriteQuorum::<5, 3>::certify(members).unwrap();
        assert_eq!(r.members(), w.members());
    }
}

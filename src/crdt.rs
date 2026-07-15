//! Join-semilattice replicated state — the **coordination-free floor** (CALM).
//!
//! Every other module in this crate types a *boundary*: a place where an unproven
//! runtime fact earns a type-level guarantee ([`consistency::Local::commit`](crate::consistency::Local::commit)
//! needs a [`Quorum`](crate::membership::Quorum); [`causal`](crate::causal) needs a
//! predecessor's delivery witness). This module types the one place where there is
//! **no boundary at all**.
//!
//! A *state-based CRDT* (Shapiro et al.) is a value in a join-semilattice: a set
//! with a binary [`join`](JoinSemilattice::join) that is **commutative,
//! associative, and idempotent**. Those three laws are exactly what make merging
//! coordination-free — replicas that apply the same updates in *any order*, with
//! *arbitrary duplication and re-delivery*, converge to the identical state. This
//! is Hellerstein's CALM theorem made concrete: **monotone computation needs no
//! coordination**, and a join-semilattice is the canonical monotone structure.
//!
//! ## The asymmetry, one last time — but inverted
//!
//! Elsewhere the crate's load-bearing asymmetry is *"up is gated, down is free."*
//! Here it collapses: `join` is the up-move **and** it is free. It takes no
//! [`Quorum`](crate::membership::Quorum), cannot fail, and needs no epoch to
//! unify — because a join can never *disagree*. Where [`reconcile`](crate::reconcile)
//! treats a merge as a *proposal* (it re-enters the lattice at the bottom and only
//! a quorum lifts it back), a CRDT join is a *decision that was never in doubt*.
//! The difference is monotonicity: reconcile's merge can lose information, so it
//! must be justified; a semilattice join is monotone in both arguments, so it
//! cannot.
//!
//! ## Relationship to [`causal`](crate::causal)
//!
//! [`causal`](crate::causal) makes out-of-order delivery a **compile error** — it *enforces* an
//! order. This module makes order **irrelevant** — the laws mean there is nothing
//! to enforce. They are the two coordination-free strategies distributed systems
//! actually use: *impose a causal order*, or *pick operations whose order does not
//! matter*. A system that can express its state as a join-semilattice never needs
//! the causal typestate at all. (This is the *state-based* discipline — whole
//! states merged by join. **Operation-based** CRDTs ship deltas instead and
//! generally still assume causal delivery, which is exactly what [`causal`](crate::causal)
//! types — so the escape is specific to the state-based form modelled here.)
//!
//! ## What the type enforces vs. what the tests discharge
//!
//! Rust's type system cannot *prove* commutativity — that is an algebraic fact
//! about a function, not a shape. So this module splits the obligation the way the
//! crate always does:
//!
//! * **The type enforces the interface**: the only public way to combine two
//!   replica states is [`join`](JoinSemilattice::join), which is total, infallible,
//!   and witness-free — so the *interface* demands no coordinating evidence, which
//!   is structural. Whether a given `join` is *actually* coordination-free then
//!   reduces entirely to the semilattice laws below; the witness-free interface is
//!   necessary but not sufficient. The negative control `SumCounter` has this same
//!   structural interface yet is **not** coordination-free.
//! * **The tests discharge the laws**: the test harness checks commutativity,
//!   associativity, and idempotence on each instance, plus the headline
//!   *convergence* property (all delivery permutations, with duplicates, reach one
//!   fixpoint), and keeps a **negative control** — a deliberately non-idempotent
//!   "counter" whose law-check fails, proving the harness has teeth (here, on
//!   idempotence: addition is already commutative and associative).
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::crdt::{GCounter, JoinSemilattice};
//!
//! // Two replicas of a 3-node grow-only counter.
//! let mut a = GCounter::<3>::new();
//! a.increment::<0>(); // node 0 counts once
//! let mut b = GCounter::<3>::new();
//! b.increment::<1>(); // node 1 counts once, concurrently
//!
//! // Merge in either direction — same result, no coordination.
//! let ab = a.clone().join(b.clone());
//! let ba = b.join(a);
//! assert_eq!(ab, ba);
//! assert_eq!(ab.value(), 2);
//! ```

use std::collections::BTreeSet;

/// A value in a **join-semilattice**: it merges with another value of the same
/// type via [`join`](Self::join), a binary operation that MUST be
///
/// * **commutative** — `a.join(b) == b.join(a)`,
/// * **associative** — `a.join(b).join(c) == a.join(b.join(c))`, and
/// * **idempotent** — `a.join(a) == a`.
///
/// These are not checked by the compiler — they are a contract every impl owes.
/// The crate's own instances ([`GCounter`], [`GSet`]) are law-checked in
/// the test harness; a downstream impl that violates them forfeits the convergence
/// guarantee. The payoff of honoring them: replicas that see the same updates in
/// any order, with arbitrary duplication, converge to one state — with no quorum,
/// no lease, and no causal typestate.
pub trait JoinSemilattice: Sized {
    /// Merge two states. Total and infallible — a *lawful* join can never
    /// disagree, which is precisely why it needs no coordinating evidence.
    #[must_use]
    fn join(self, other: Self) -> Self;
}

/// Fold a stream of states into their join, left to right. Because [`join`] is
/// commutative, associative, and idempotent, the *result is independent of the
/// order and multiplicity* of `states` — the defining property of a
/// convergent replicated data type. Returns `None` only for an empty stream.
///
/// [`join`]: JoinSemilattice::join
#[must_use]
pub fn converge<L: JoinSemilattice>(states: impl IntoIterator<Item = L>) -> Option<L> {
    states.into_iter().reduce(L::join)
}

/// A **grow-only counter** over `N` nodes (Shapiro's G-Counter). Each node owns
/// one slot it alone increments; the counter's value is the sum of slots, and the
/// join is the pairwise maximum — so a slot never regresses and concurrent
/// increments at different nodes both survive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GCounter<const N: usize> {
    counts: [u64; N],
}

impl<const N: usize> GCounter<N> {
    /// A fresh counter with every slot at zero.
    #[must_use]
    pub const fn new() -> Self {
        GCounter { counts: [0; N] }
    }

    /// Increment node `K`'s own slot. `K` is a const parameter so an
    /// out-of-range node is a **compile error** (post-monomorphization), not a
    /// runtime panic — the same `const {}` bound-check idiom the reconfiguration
    /// rung uses.
    pub fn increment<const K: usize>(&mut self) {
        const {
            assert!(K < N, "GCounter::increment: node index K is out of range for N");
        }
        self.counts[K] += 1;
    }

    /// The counter's value: the sum over all node slots.
    #[must_use]
    pub fn value(&self) -> u64 {
        self.counts.iter().sum()
    }
}

impl<const N: usize> Default for GCounter<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> JoinSemilattice for GCounter<N> {
    /// Pairwise maximum of the two nodes' slot vectors — the join under which a
    /// slot is monotone non-decreasing.
    fn join(self, other: Self) -> Self {
        let mut counts = self.counts;
        for (slot, o) in counts.iter_mut().zip(other.counts.iter()) {
            if *o > *slot {
                *slot = *o;
            }
        }
        GCounter { counts }
    }
}

/// A **grow-only set** (Shapiro's G-Set): elements are only ever added, and the
/// join is set union. The other canonical join shape — union, where `GCounter`
/// was pairwise max.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GSet<T: Ord> {
    elems: BTreeSet<T>,
}

impl<T: Ord> GSet<T> {
    /// An empty grow-only set.
    #[must_use]
    pub fn new() -> Self {
        GSet { elems: BTreeSet::new() }
    }

    /// Add an element. Monotone — a G-Set never removes.
    pub fn insert(&mut self, x: T) {
        self.elems.insert(x);
    }

    /// Whether `x` is present.
    #[must_use]
    pub fn contains(&self, x: &T) -> bool {
        self.elems.contains(x)
    }

    /// The number of distinct elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.elems.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.elems.is_empty()
    }
}

impl<T: Ord> Default for GSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Ord> JoinSemilattice for GSet<T> {
    /// Set union — the join under which membership is monotone.
    fn join(self, other: Self) -> Self {
        let mut elems = self.elems;
        elems.extend(other.elems);
        GSet { elems }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert the three semilattice laws on a concrete triple. Any instance that
    /// passes this for enough triples is trusted to converge.
    fn assert_laws<L>(a: L, b: L, c: L)
    where
        L: JoinSemilattice + Clone + PartialEq + core::fmt::Debug,
    {
        // Commutative: a ⊔ b == b ⊔ a
        assert_eq!(
            a.clone().join(b.clone()),
            b.clone().join(a.clone()),
            "join not commutative"
        );
        // Associative: (a ⊔ b) ⊔ c == a ⊔ (b ⊔ c)
        assert_eq!(
            a.clone().join(b.clone()).join(c.clone()),
            a.clone().join(b.clone().join(c.clone())),
            "join not associative"
        );
        // Idempotent: a ⊔ a == a
        assert_eq!(a.clone().join(a.clone()), a, "join not idempotent");
    }

    fn gcounter(slots: [u64; 3]) -> GCounter<3> {
        GCounter { counts: slots }
    }

    #[test]
    fn gcounter_obeys_the_semilattice_laws() {
        assert_laws(gcounter([1, 0, 3]), gcounter([0, 2, 3]), gcounter([4, 1, 0]));
    }

    #[test]
    fn gset_obeys_the_semilattice_laws() {
        let a = GSet { elems: BTreeSet::from([1, 2]) };
        let b = GSet { elems: BTreeSet::from([2, 3]) };
        let c = GSet { elems: BTreeSet::from([3, 4]) };
        assert_laws(a, b, c);
    }

    #[test]
    fn increment_is_per_node_and_sums() {
        let mut g = GCounter::<3>::new();
        g.increment::<0>();
        g.increment::<0>();
        g.increment::<2>();
        assert_eq!(g.value(), 3, "value is the sum of node slots");
        assert_eq!(g.counts, [2, 0, 1], "each node increments only its own slot");
    }

    /// **The headline: convergence.** A multiset of concurrent updates, delivered
    /// in *any permutation and with duplicates*, reaches one fixpoint. This is the
    /// property the causal rung enforces by *typing order*; here it is free,
    /// because the join laws make order and multiplicity irrelevant.
    #[test]
    fn all_delivery_orders_converge_to_one_state() {
        // Three replicas each observed a different concurrent increment.
        let r0 = {
            let mut g = GCounter::<3>::new();
            g.increment::<0>();
            g
        };
        let r1 = {
            let mut g = GCounter::<3>::new();
            g.increment::<1>();
            g
        };
        let r2 = {
            let mut g = GCounter::<3>::new();
            g.increment::<2>();
            g
        };

        let expected = gcounter([1, 1, 1]);

        // Every permutation of delivery converges to the same state...
        let perms = [
            [r0.clone(), r1.clone(), r2.clone()],
            [r0.clone(), r2.clone(), r1.clone()],
            [r1.clone(), r0.clone(), r2.clone()],
            [r1.clone(), r2.clone(), r0.clone()],
            [r2.clone(), r0.clone(), r1.clone()],
            [r2.clone(), r1.clone(), r0.clone()],
        ];
        for perm in perms {
            assert_eq!(converge(perm).unwrap(), expected, "a delivery order diverged");
        }

        // ...and re-delivering a message it already merged changes nothing
        // (idempotence = at-least-once delivery is safe).
        let with_dupes = [r0.clone(), r1.clone(), r0.clone(), r2.clone(), r1.clone()];
        assert_eq!(converge(with_dupes).unwrap(), expected, "duplicates perturbed the state");
    }

    #[test]
    fn gset_converges_regardless_of_order() {
        let a = GSet { elems: BTreeSet::from(["x"]) };
        let b = GSet { elems: BTreeSet::from(["y"]) };
        let ab = converge([a.clone(), b.clone()]).unwrap();
        let ba = converge([b, a]).unwrap();
        assert_eq!(ab, ba);
        assert_eq!(ab.len(), 2);
    }

    /// **Negative control.** A "counter" whose join *adds* instead of taking the
    /// max is not a semilattice — it is not idempotent (`a ⊔ a = 2a ≠ a`). The law
    /// harness must reject it, or it would rubber-stamp non-convergent merges.
    #[derive(Clone, PartialEq, Debug)]
    struct SumCounter(u64);
    impl JoinSemilattice for SumCounter {
        fn join(self, other: Self) -> Self {
            SumCounter(self.0 + other.0) // WRONG: not idempotent
        }
    }

    #[test]
    #[should_panic(expected = "not idempotent")]
    fn negative_control_non_idempotent_join_is_caught() {
        // a ⊔ a = 2a ≠ a for a ≠ 0 — assert_laws must fail on the idempotence check.
        assert_laws(SumCounter(1), SumCounter(2), SumCounter(3));
    }
}

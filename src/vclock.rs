//! Vector-clock **happens-before** witness — the concurrency-detection rung.
//!
//! The crate already types two neighbours of this. [`causal`](crate::causal) *enforces*
//! an order (deliver `B` before its cause `A` = compile error). [`reconcile`](crate::reconcile)
//! merges two committed *values* that disagree. Neither answers the question that sits
//! *between* them: given two updates, **do they actually conflict, or does one supersede
//! the other?** That is exactly what a vector clock decides (Fidge 1988; Mattern 1988),
//! and it is the precondition for reconciliation — you should only reach for a merge when
//! the updates are genuinely *concurrent*.
//!
//! ## The mechanism
//!
//! * [`VClock<N>`] is `N` per-process event counts. [`tick`](VClock::tick) records a
//!   local event; [`merge`](VClock::merge) is the pairwise **max** — the join that a
//!   node applies on receiving another's clock. (That join is commutative, associative,
//!   idempotent: a vector clock *is* a [`crdt`](crate::crdt) join-semilattice.)
//! * [`compare`](VClock::compare) returns a [`Relation`] carrying a **witness**:
//!   [`Before`](Relation::Before)/[`After`](Relation::After) carry an [`Ordered`] (one
//!   clock dominates — one update happened-before the other); [`Concurrent`](Relation::Concurrent)
//!   carries a [`Concurrent`] (neither dominates — the updates are causally independent).
//! * The lossy **last-writer-wins** shortcut [`take_dominant`] *requires* an [`Ordered`].
//!   Because [`compare`](VClock::compare) hands one out **only** when the clocks are
//!   actually ordered, applying LWW to concurrent updates — silently discarding one
//!   branch's writes, the classic **lost-update** bug — cannot be written *by honest
//!   use of a freshly-minted witness* (the residual escape hatch is witness reuse/
//!   mispairing; see the runtime-seam note). Concurrent updates must instead go through
//!   [`merge_concurrent`](Versioned::merge_concurrent), which *requires* a [`Concurrent`]
//!   and keeps both.
//!
//! ## Dropping a concurrent update by LWW is a compile error
//!
//! `take_dominant` consumes an [`Ordered`]; a [`Concurrent`] witness (the only thing
//! `compare` yields for concurrent clocks) is a **different type** and will not unify:
//!
//! ```compile_fail
//! use quorum_types::vclock::{VClock, Versioned, Relation, take_dominant};
//! // a = [1,0], b = [0,1] — concurrent (each ticked its own process).
//! let a = Versioned::new(VClock::<2>::new().tick(0), "a");
//! let b = Versioned::new(VClock::<2>::new().tick(1), "b");
//! let Relation::Concurrent(w) = a.clock().compare(b.clock()) else { unreachable!() };
//! let _ = take_dominant(a, b, w); // ERROR: expected `Ordered`, found `Concurrent`
//! ```
//!
//! ## The happy path — dominance when ordered, merge when concurrent
//!
//! ```
//! use quorum_types::vclock::{VClock, Versioned, Relation, take_dominant};
//!
//! // Ordered: b saw a's event, then ticked — a happens-before b.
//! let a = Versioned::new(VClock::<2>::new().tick(0), 1);
//! let b = Versioned::new(a.clock().merge(&VClock::new()).tick(1), 2);
//! match a.clock().compare(b.clock()) {
//!     Relation::Before(ord) => {
//!         let latest = take_dominant(a, b, ord); // safe LWW — b truly supersedes a
//!         assert_eq!(*latest.value(), 2);
//!     }
//!     _ => unreachable!("b dominates a"),
//! }
//!
//! // Concurrent: two independent updates — merge, never drop.
//! let p = Versioned::new(VClock::<2>::new().tick(0), 10);
//! let q = Versioned::new(VClock::<2>::new().tick(1), 20);
//! match p.clock().compare(q.clock()) {
//!     Relation::Concurrent(w) => {
//!         let merged = p.merge_concurrent(q, w, |x, y| x + y); // keeps both
//!         assert_eq!(*merged.value(), 30);
//!         assert_eq!(merged.clock().at(0), 1);
//!         assert_eq!(merged.clock().at(1), 1);
//!     }
//!     _ => unreachable!("independent updates are concurrent"),
//! }
//! ```
//!
//! ## Relationship to [`reconcile`](crate::reconcile) — the evidence it warrants
//!
//! `reconcile` mints a [`Diverged`](crate::reconcile::Diverged) by comparing two
//! committed *values*. But two equal-looking values, or one that merely supersedes the
//! other, are not a real conflict. A [`Concurrent`] witness here is the honest evidence
//! that reconciliation is *warranted*: the updates are causally independent, so no
//! last-writer-wins choice is safe and a lawful merge is the only lossless resolution.
//! The two modules are **not wired together** (`Diverged::detect` does not consume a
//! `Concurrent`) — this is a motivating convention, not an enforced type-level link.
//!
//! ## Where the types stop (the runtime seam)
//!
//! Unlike [`session::Fresh`](crate::session::Fresh) and [`causal::Delivered`](crate::causal::Delivered),
//! which trust a caller-supplied `bool`, [`Ordered`]/[`Concurrent`] are minted by
//! [`compare`](VClock::compare) — a **pure, total decision procedure over the two clocks
//! themselves**, so the witness faithfully reflects the clocks it was computed from. Two
//! trust gaps remain, and they are the crate's usual ones. First, the witness is not
//! type-bound to the specific values, and (being `Copy`) it is reusable:
//! [`take_dominant`]/[`merge_concurrent`](Versioned::merge_concurrent) trust that you
//! pass the *same* pair whose clocks you compared, **in the same argument order** — the
//! [`Ordered`] records a `Left`/`Right` relative to [`compare`](VClock::compare)'s
//! receiver/argument, so swapping the pair, or reusing an `Ordered` minted from a
//! *different* (ordered) pair on a concurrent one, re-enables the very lost update
//! `take_dominant` guards against. This is self-attestation, like
//! [`reconcile`](crate::reconcile)'s caller-chosen samples: the gate is sound for honest
//! freshly-minted use, not against a caller mispairing its own witnesses. Second, a
//! vector clock is
//! only as truthful as the layer that *ticks* it: a node that fails to record a real
//! causal dependency produces a clock that *reports* concurrency where there was order
//! (the same declared-vs-true boundary as [`causal`](crate::causal), one level down —
//! there the graph is declared, here the counts are). The decision procedure is exact;
//! its inputs are the operator's responsibility.

use core::marker::PhantomData;

/// A vector clock over `N` processes: `entries[i]` counts events observed from
/// process `i`. `Copy` — a clock is a plain value, freely duplicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VClock<const N: usize> {
    entries: [u64; N],
}

impl<const N: usize> VClock<N> {
    /// The bottom clock — no events observed anywhere.
    pub const fn new() -> Self {
        VClock { entries: [0; N] }
    }

    /// Record a local event at process `i` (increment its count). Consumes and returns
    /// the clock so a tick reads as a transition.
    ///
    /// # Panics
    /// If `i >= N`, and (in debug builds) on `u64` counter overflow.
    pub const fn tick(mut self, i: usize) -> Self {
        self.entries[i] += 1;
        self
    }

    /// The count observed from process `i`.
    pub const fn at(&self, i: usize) -> u64 {
        self.entries[i]
    }

    /// The **join**: pairwise maximum — what a node applies on receiving `other`'s
    /// clock. Commutative, associative, idempotent (a join-semilattice, like
    /// [`crdt`](crate::crdt)).
    pub fn merge(&self, other: &Self) -> Self {
        let mut out = self.entries;
        for (o, &b) in out.iter_mut().zip(other.entries.iter()) {
            *o = (*o).max(b);
        }
        VClock { entries: out }
    }

    /// Classify the causal relationship between `self` and `other`, minting the
    /// corresponding witness. `self ≤ other` on every entry (strictly less somewhere)
    /// ⇒ [`Before`](Relation::Before) (`other` dominates); the mirror ⇒
    /// [`After`](Relation::After); equal everywhere ⇒ [`Equal`](Relation::Equal);
    /// otherwise the clocks are incomparable ⇒ [`Concurrent`](Relation::Concurrent).
    pub fn compare(&self, other: &Self) -> Relation {
        let mut le = true; // self <= other everywhere
        let mut ge = true; // self >= other everywhere
        for (&a, &b) in self.entries.iter().zip(other.entries.iter()) {
            if a > b {
                le = false;
            }
            if a < b {
                ge = false;
            }
        }
        match (le, ge) {
            (true, true) => Relation::Equal,
            (true, false) => Relation::Before(Ordered { dominant: Side::Right }),
            (false, true) => Relation::After(Ordered { dominant: Side::Left }),
            (false, false) => Relation::Concurrent(Concurrent { _priv: PhantomData }),
        }
    }
}

impl<const N: usize> Default for VClock<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Which argument of [`compare`](VClock::compare) dominated: `Left` = the receiver
/// (`self`/`a`), `Right` = the argument (`b`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// Witness that two clocks are **causally ordered** — one happened-before the other.
/// Minted only by [`VClock::compare`]. Carries which side dominates so that
/// dominance resolution is total and correct; there is no public constructor.
#[derive(Debug, Clone, Copy)]
pub struct Ordered {
    dominant: Side,
}

/// Witness that two clocks are **concurrent** — causally independent, neither
/// dominating. Minted only by [`VClock::compare`]. Its presence is the honest
/// precondition for a lossless merge (and for [`reconcile`](crate::reconcile)).
#[derive(Debug, Clone, Copy)]
pub struct Concurrent {
    // Zero-size, but private: obtainable only through `compare`.
    _priv: PhantomData<()>,
}

/// The verdict of [`VClock::compare`], each variant carrying its witness.
#[must_use = "a comparison verdict decides whether LWW is safe or a merge is required"]
pub enum Relation {
    /// `self` happened-before the argument — the argument dominates.
    Before(Ordered),
    /// The argument happened-before `self` — `self` dominates.
    After(Ordered),
    /// The clocks are identical — the same causal history.
    Equal,
    /// The clocks are incomparable — the updates are concurrent and both must be kept.
    Concurrent(Concurrent),
}

/// A value tagged with the vector clock at which it was produced.
#[derive(Debug, Clone, Copy)]
pub struct Versioned<T, const N: usize> {
    clock: VClock<N>,
    value: T,
}

impl<T, const N: usize> Versioned<T, N> {
    /// Pair a value with its clock.
    pub const fn new(clock: VClock<N>, value: T) -> Self {
        Versioned { clock, value }
    }

    /// The value's clock.
    pub const fn clock(&self) -> &VClock<N> {
        &self.clock
    }

    /// The value.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// **Lossless merge of two concurrent versions.** Requires a [`Concurrent`]
    /// witness — so this is reachable only when [`compare`](VClock::compare) actually
    /// found the versions independent. Combines the values with `f` and advances the
    /// clock to the [`join`](VClock::merge) of both (the merge is a new event that
    /// causally follows both inputs). Both values are *seen*, so nothing is silently
    /// dropped — but `f` is applied in a fixed positional order (`f(self, other)`), so
    /// for the merge to be a true order-insensitive CRDT join, value-level
    /// commutativity/associativity of `f` is the **caller's** responsibility; the type
    /// only guarantees the clock join and that both values reach `f`.
    pub fn merge_concurrent<F>(self, other: Self, _w: Concurrent, f: F) -> Self
    where
        F: FnOnce(T, T) -> T,
    {
        Versioned {
            clock: self.clock.merge(&other.clock),
            value: f(self.value, other.value),
        }
    }
}

/// **Last-writer-wins, safely.** Keep the version whose clock dominates, discarding the
/// other. Requires an [`Ordered`] witness — so it is *impossible* to call on concurrent
/// versions (their `compare` yields a [`Concurrent`], not an [`Ordered`]), which is how
/// the type system rules out the lost-update bug. Correct because the witness records
/// *which* side dominates.
pub fn take_dominant<T, const N: usize>(a: Versioned<T, N>, b: Versioned<T, N>, ord: Ordered) -> Versioned<T, N> {
    match ord.dominant {
        Side::Left => a,
        Side::Right => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_clocks_yield_an_ordered_witness_and_lww_is_safe() {
        // a = [1,0]; b saw a then ticked its own => [1,1]. a happens-before b.
        let a = Versioned::new(VClock::<2>::new().tick(0), "a");
        let b_clock = a.clock().merge(&VClock::new()).tick(1);
        let b = Versioned::new(b_clock, "b");

        match a.clock().compare(b.clock()) {
            Relation::Before(ord) => {
                let latest = take_dominant(a, b, ord);
                assert_eq!(*latest.value(), "b", "the dominant (later) version wins");
            }
            _ => panic!("b must dominate a"),
        }
    }

    #[test]
    fn concurrent_clocks_yield_a_concurrent_witness_and_force_a_merge() {
        // a = [1,0], b = [0,1] — each ticked only its own process => concurrent.
        let a = Versioned::new(VClock::<2>::new().tick(0), 10);
        let b = Versioned::new(VClock::<2>::new().tick(1), 20);

        match a.clock().compare(b.clock()) {
            Relation::Concurrent(w) => {
                let merged = a.merge_concurrent(b, w, |x, y| x + y);
                assert_eq!(*merged.value(), 30, "both concurrent updates are kept");
                assert_eq!(merged.clock().at(0), 1);
                assert_eq!(merged.clock().at(1), 1, "merged clock is the join");
            }
            _ => panic!("independent updates must be concurrent"),
        }
    }

    #[test]
    fn equal_clocks_compare_equal() {
        let a = VClock::<3>::new().tick(0).tick(1);
        let b = VClock::<3>::new().tick(1).tick(0);
        assert!(matches!(a.compare(&b), Relation::Equal));
    }

    #[test]
    fn after_is_the_mirror_of_before() {
        let earlier = VClock::<2>::new().tick(0);
        let later = earlier.tick(1); // dominates earlier
        assert!(matches!(later.compare(&earlier), Relation::After(_)));
        assert!(matches!(earlier.compare(&later), Relation::Before(_)));
    }

    #[test]
    fn merge_is_a_semilattice_join() {
        // The three join laws that make a vector clock a CRDT semilattice.
        let a = VClock::<3>::new().tick(0).tick(2);
        let b = VClock::<3>::new().tick(1).tick(2);
        let c = VClock::<3>::new().tick(0).tick(1).tick(1);
        assert_eq!(a.merge(&a), a, "idempotent");
        assert_eq!(a.merge(&b), b.merge(&a), "commutative");
        assert_eq!(a.merge(&b).merge(&c), a.merge(&b.merge(&c)), "associative");
    }

    #[test]
    fn dominance_direction_is_recorded_correctly_both_ways() {
        // After(Ordered{Left}) must keep the left (self) argument.
        let later = Versioned::new(VClock::<2>::new().tick(0).tick(1), "later");
        let earlier = Versioned::new(VClock::<2>::new().tick(0), "earlier");
        match later.clock().compare(earlier.clock()) {
            Relation::After(ord) => assert_eq!(*take_dominant(later, earlier, ord).value(), "later"),
            _ => panic!("later must dominate"),
        }
    }
}

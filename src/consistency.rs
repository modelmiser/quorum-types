//! Consistency-lattice value types — the last unbuilt row of the
//! warp-types→distributed mapping.
//!
//! The other modules type *membership* (who is in a quorum). This one types the
//! **data**: how much consensus a value carries. A value is not just a `T`; it
//! is a `T` at a known height in a small consistency lattice:
//!
//! ```text
//!            At<T, N>      strongest — committed, and pinned to the exact
//!               │          configuration epoch N that agreed to it
//!               │  forget_epoch  (free: drop *which* generation)
//!               ▼
//!            Agreed<T>     committed by *some* quorum — safe to act on,
//!               │          but the generation is erased
//!               ┆
//!               ┆  commit  (quorum-gated: needs a &Quorum<E> as evidence)
//!               ┆
//!            Local<T>      weakest — this node's uncommitted view; a proposal
//! ```
//!
//! **The load-bearing asymmetry: up is quorum-gated, down is free.**
//!
//! * Going *up* the lattice ([`Local::commit`]) requires a
//!   [`membership::Quorum<E>`](crate::membership::Quorum) — evidence that a
//!   majority of configuration `E` witnessed the value. There is deliberately
//!   no other constructor for a committed value, so an [`Agreed`]/[`At`] is
//!   **unforgeable**: possessing one is proof it cleared a real quorum.
//! * Going *down* ([`At::forget_epoch`]) is a free weakening — discarding
//!   information is always sound.
//!
//! This is the value-level analogue of the whole crate's discipline: the
//! `gradual` boundary ([`Config::certify`](crate::membership::Config::certify))
//! is where an unproven runtime value earns a type-level guarantee, and here it
//! is exactly where a `Local` proposal earns the right to be `At`/`Agreed`.
//!
//! ## Acting on an uncommitted value is a compile error
//!
//! A function that requires consensus bounds its argument on [`Committed`] (the
//! upper set `{Agreed, At}` of the lattice). `Local` does not implement it, so
//! passing a proposal where a decision is required does not typecheck:
//!
//! ```compile_fail
//! use quorum_types::consistency::{Local, Committed};
//! fn apply<C: Committed>(_decided: C) { /* mutate authoritative state */ }
//! apply(Local::new("x = 42")); // Local is not Committed — reject the proposal
//! ```
//!
//! ## A value agreed at one epoch is not agreed at another
//!
//! `At<T, N>` pins the configuration generation into the type, so a value agreed
//! by config 7 cannot stand in for one that must be agreed by config 3 — the
//! same epoch guard the rest of the crate enforces, now on the *data*:
//!
//! ```compile_fail
//! use quorum_types::consistency::{Local, At};
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//!
//! let q7 = Config::<7>::new(BTreeSet::from([1, 2, 3]))
//!     .certify(BTreeSet::from([1, 2, 3])).unwrap();
//! let agreed_at_7: At<i32, 7> = Local::new(5).commit(&q7);
//!
//! fn needs_epoch_3(_v: At<i32, 3>) {}
//! needs_epoch_3(agreed_at_7); // At<_, 7> vs At<_, 3> — epochs do not unify
//! ```
//!
//! ## Committing consumes the proposal
//!
//! `Local<T>` is move-only: `commit` takes it by value, so a stale proposal
//! cannot be committed twice (nor read after it has been decided):
//!
//! ```compile_fail
//! use quorum_types::consistency::Local;
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//!
//! let q = Config::<0>::new(BTreeSet::from([1, 2, 3]))
//!     .certify(BTreeSet::from([1, 2, 3])).unwrap();
//! let proposal = Local::new(5);
//! let _decided = proposal.commit(&q);
//! let _again = proposal.commit(&q); // `proposal` already moved — consumed once
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::consistency::Local;
//! use quorum_types::membership::Config;
//! use std::collections::BTreeSet;
//!
//! let cfg = Config::<7>::new(BTreeSet::from([1, 2, 3])); // majority = 2
//! let quorum = cfg.certify(BTreeSet::from([1, 2, 3])).unwrap();
//!
//! let proposal = Local::new("x = 42");   // Local<&str>
//! let decided = proposal.commit(&quorum); // At<&str, 7>
//! assert_eq!(decided.epoch(), 7);
//! assert_eq!(*decided.value(), "x = 42");
//!
//! let forgotten = decided.forget_epoch(); // Agreed<&str> — down the lattice
//! assert_eq!(*forgotten.value(), "x = 42");
//! ```

use crate::membership::Quorum;

mod sealed {
    /// Closes the [`Consistency`](super::Consistency) lattice: the only
    /// inhabitants — and therefore the only lattice heights — are the three
    /// defined in this module.
    pub trait Sealed {}
}

/// A value at a known height in the consistency lattice. Sealed: the set of
/// consistency levels is closed to the three types below.
pub trait Consistency: sealed::Sealed {
    /// Height in the lattice — strictly increasing with consensus strength.
    /// [`Local`] < [`Agreed`] < [`At`].
    const LEVEL: u8;
}

/// The upper set `{Agreed, At}` of the lattice: values that have cleared a
/// quorum. [`Local`] is deliberately excluded, so an API that bounds an argument
/// on `Committed` will not accept an uncommitted proposal — the rejection is a
/// compile error, not a runtime check.
pub trait Committed: Consistency {}

/// **Bottom of the lattice.** This node's uncommitted view — a *proposal*.
///
/// Move-only: the only thing you can do with a `Local` besides inspect it is
/// [`commit`](Local::commit) it through a quorum, which consumes it. There is no
/// method to read a `Local` as if it were authoritative.
#[must_use = "a Local value is an uncommitted proposal; commit it or it is lost"]
pub struct Local<T> {
    value: T,
}

/// **Committed, epoch-pinned.** A value that cleared the quorum of configuration
/// `N`. The type records *which* generation agreed, so it cannot be substituted
/// for a value that must be agreed by a different configuration.
#[must_use = "an At value is a committed decision; acting on it is the point"]
pub struct At<T, const N: u64> {
    value: T,
}

/// **Committed, epoch-erased.** A value agreed by *some* quorum — safe to act
/// on — with the generation forgotten. Reachable only by weakening an [`At`]
/// ([`At::forget_epoch`]), so every `Agreed` provably passed a real quorum.
#[must_use = "an Agreed value is a committed decision; acting on it is the point"]
pub struct Agreed<T> {
    value: T,
}

impl<T> sealed::Sealed for Local<T> {}
impl<T> Consistency for Local<T> {
    const LEVEL: u8 = 0;
}

impl<T> sealed::Sealed for Agreed<T> {}
impl<T> Consistency for Agreed<T> {
    const LEVEL: u8 = 1;
}
impl<T> Committed for Agreed<T> {}

impl<T, const N: u64> sealed::Sealed for At<T, N> {}
impl<T, const N: u64> Consistency for At<T, N> {
    const LEVEL: u8 = 2;
}
impl<T, const N: u64> Committed for At<T, N> {}

/// The lattice ordering is a *compile-time* invariant, not a runtime assumption:
/// `Local` < `Agreed` < `At`. If a future edit reshuffles the levels, the crate
/// fails to build here rather than silently inverting the consistency ordering.
const _LEVELS_STRICTLY_INCREASE: () = {
    assert!(<Local<()> as Consistency>::LEVEL < <Agreed<()> as Consistency>::LEVEL);
    assert!(<Agreed<()> as Consistency>::LEVEL < <At<(), 0> as Consistency>::LEVEL);
};

impl<T> Local<T> {
    /// Propose a value — the bottom of the lattice. Anyone can do this; it
    /// asserts nothing beyond "this node currently holds `value`".
    pub const fn new(value: T) -> Self {
        Local { value }
    }

    /// Inspect the proposed value without claiming it is authoritative. This is
    /// the *only* read offered on a `Local`, and it is by-reference: a proposal
    /// never yields an owned, committed value except through [`commit`](Self::commit).
    pub const fn peek(&self) -> &T {
        &self.value
    }

    /// **The quorum-gated up-move.** Consume this proposal and, witnessed by a
    /// [`Quorum<E>`](crate::membership::Quorum), promote it to [`At<T, E>`] — a
    /// value committed at exactly the quorum's configuration epoch `E`.
    ///
    /// The quorum is *evidence*: holding a `&Quorum<E>` means a majority of
    /// configuration `E` exists to witness the value. `E` is inferred from that
    /// witness and pinned into the result type. There is no way up the lattice
    /// that does not pass through this boundary.
    pub fn commit<const E: u64>(self, _witness: &Quorum<E>) -> At<T, E> {
        At { value: self.value }
    }
}

impl<T, const N: u64> At<T, N> {
    /// The committed value.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// The configuration epoch that agreed to this value (mirrors `N`).
    pub const fn epoch(&self) -> u64 {
        N
    }

    /// Take ownership of the committed value, discarding the consistency wrapper.
    pub fn into_value(self) -> T {
        self.value
    }

    /// **The free down-move.** Weaken to [`Agreed<T>`], forgetting *which*
    /// generation agreed. Sound without any check — discarding information never
    /// violates safety — which is why, unlike [`commit`](Local::commit), it needs
    /// no quorum.
    pub fn forget_epoch(self) -> Agreed<T> {
        Agreed { value: self.value }
    }
}

impl<T> Agreed<T> {
    /// The committed value.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Take ownership of the committed value, discarding the consistency wrapper.
    pub fn into_value(self) -> T {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::Config;
    use std::collections::BTreeSet;

    fn quorum_at<const E: u64>() -> Quorum<E> {
        Config::<E>::new(BTreeSet::from([1, 2, 3]))
            .certify(BTreeSet::from([1, 2, 3]))
            .expect("full set is a majority")
    }

    #[test]
    fn commit_pins_the_quorums_epoch() {
        let q = quorum_at::<7>();
        let decided = Local::new(42).commit(&q);
        assert_eq!(decided.epoch(), 7, "commit pins the witnessing config's epoch");
        assert_eq!(*decided.value(), 42, "commit preserves the value");
    }

    #[test]
    fn forget_epoch_weakens_but_preserves_value() {
        let q = quorum_at::<3>();
        let decided = Local::new("payload").commit(&q); // At<_, 3>
        let agreed = decided.forget_epoch(); // Agreed<_>
        assert_eq!(*agreed.value(), "payload", "weakening keeps the value");
    }

    #[test]
    fn committed_bound_accepts_both_agreed_and_at() {
        // The positive counterpart to the `Local` compile_fail doctest: anything
        // that has cleared a quorum satisfies the `Committed` bound.
        fn decide<C: Committed>(c: C) -> C {
            c
        }
        let q = quorum_at::<1>();
        let at = Local::new(1).commit(&q);
        let agreed = decide(at).forget_epoch();
        let _ = decide(agreed); // both compile — At and Agreed are Committed
    }

    #[test]
    fn peek_reads_a_proposal_without_committing() {
        let proposal = Local::new(vec![1, 2, 3]);
        assert_eq!(proposal.peek(), &vec![1, 2, 3]);
        // still uncommitted — can go on to commit it
        let q = quorum_at::<0>();
        assert_eq!(*proposal.commit(&q).value(), vec![1, 2, 3]);
    }
}

//! # quorum-types (feasibility toy)
//!
//! **Question this crate answers:** does the *complement-proof* mechanism from
//! `warp-types` survive being indexed by a type-level **epoch**, such that two
//! stale halves from *different* epochs cannot typecheck a `merge`?
//!
//! This is the 30-minute feasibility gate for a distributed-verification
//! direction ("quorum-types"). It is **not** a distributed system. It models
//! the friendliest possible one — fixed membership, no failure — exactly as a
//! GPU warp does, and asks only whether epoch-indexing composes cleanly with
//! compile-time complement proofs.
//!
//! ## What is (deliberately) modelled
//!
//! * [`Quorum<E, S>`] — a membership token at **type-level epoch `E`** over an
//!   active member-set `S`. Move-only (no `Copy`/`Clone`): a token is a *linear*
//!   resource, consumed by exactly one `merge`.
//! * [`split`](Quorum::split) — a reconfiguration that partitions an `All`
//!   quorum into complementary `Lo`/`Hi` halves at the **same** epoch.
//! * [`merge`] — recombines two halves, but *only* when the type system can
//!   prove (a) they carry the **same epoch `E`** and (b) their sets are
//!   **complementary** ([`ComplementOf`]).
//!
//! ## The load-bearing property: split-brain is unrepresentable
//!
//! Because the epoch is a *type* parameter, a merge of two halves from
//! different epochs fails to unify `E` — it is a **compile error**, not a
//! runtime check:
//!
//! ```compile_fail
//! use quorum_types::{Quorum, All, merge};
//! let (lo3, _) = Quorum::<3, All>::genesis(0xFFFF).split();
//! let (_, hi4) = Quorum::<4, All>::genesis(0xFFFF).split();
//! let _ = merge(lo3, hi4); // E: 3 vs 4 do not unify — split-brain unrepresentable
//! ```
//!
//! Two *non-complementary* halves are likewise rejected. `Lo`'s only registered
//! complement is `Hi`, so `merge`'s `A: ComplementOf<B>` bound forces the second
//! argument to be `Quorum<E, Hi>` — a second `Lo` fails to unify:
//!
//! ```compile_fail
//! use quorum_types::{Quorum, All, merge};
//! let (lo_a, _) = Quorum::<3, All>::genesis(0xFFFF).split();
//! let (lo_b, _) = Quorum::<3, All>::genesis(0xFFFF).split();
//! let _ = merge(lo_a, lo_b); // expected `Quorum<3, Hi>`, found `Quorum<3, Lo>`
//! ```
//!
//! And a token cannot be merged twice — linearity is enforced by move
//! semantics, so the second use is a use-after-move:
//!
//! ```compile_fail
//! use quorum_types::{Quorum, All, merge};
//! let (lo, hi) = Quorum::<3, All>::genesis(0).split();
//! let _ = merge(lo, hi);
//! let _ = merge(lo, hi); // `lo`/`hi` already moved — a token is consumed once
//! ```
//!
//! The happy path — same epoch, complementary sets — compiles and recombines
//! the runtime membership mask:
//!
//! ```
//! use quorum_types::{Quorum, All, merge};
//! let (lo, hi) = Quorum::<7, All>::genesis(0xDEAD_BEEF).split();
//! let whole = merge(lo, hi);
//! assert_eq!(whole.members(), 0xDEAD_BEEF);
//! assert_eq!(whole.epoch(), 7);
//! ```
//!
//! ## The failure layer: [`mod@failover`]
//!
//! This module's `merge` **cannot fail** — the lockstep assumption a real
//! distributed system violates. The [`failover`] module adds the lease-degraded
//! complement (validated first in `tla/quorum.tla`): a runtime lease guard for
//! reconfiguration, because the TLA+ model proved the type-level epoch is
//! *necessary but not sufficient* — split-brain is temporal and cannot be
//! discharged structurally.
//!
//! ## Still out of scope (parking lot → later versions)
//!
//! Splits into more than two parts, the consistency-lattice value types
//! (`Agreed`/`Local`/`At`), deterministic partition/heal simulation
//! (`turmoil`), benchmarks, CI.
//!
//! ## Relationship to `warp-types`
//!
//! The [`ActiveSet`] / [`ComplementOf`] traits here are a **minimal
//! reimplementation** of the concept in the published `warp-types` crate — a
//! model, not the real GPU trait surface — kept self-contained so this
//! experiment varies only the *epoch* dimension. `warp-types` is treated as a
//! read-only reference and is not a dependency.

#![forbid(unsafe_code)]

pub mod failover;

use core::marker::PhantomData;

mod sealed {
    /// Prevents downstream code from asserting bogus member-sets or false
    /// complement proofs — the guarantees are only as trustworthy as the set
    /// of impls, so the set of impls is closed.
    pub trait Sealed {}
}

/// A type-level active member-set. Sealed: the only inhabitants are the ones
/// defined in this crate.
pub trait ActiveSet: sealed::Sealed {}

/// The full membership (a quorum of everyone).
#[derive(Debug)]
pub struct All;
/// The low complementary half produced by [`Quorum::split`].
#[derive(Debug)]
pub struct Lo;
/// The high complementary half produced by [`Quorum::split`].
#[derive(Debug)]
pub struct Hi;

impl sealed::Sealed for All {}
impl sealed::Sealed for Lo {}
impl sealed::Sealed for Hi {}
impl ActiveSet for All {}
impl ActiveSet for Lo {}
impl ActiveSet for Hi {}

/// A compile-time proof that `Self` and `Other` are the two complementary
/// halves of a common parent — i.e. their union is the parent and their
/// intersection is empty. Sealed: no downstream code can fabricate a
/// complement relation that does not hold.
///
/// The only proofs that exist are `Lo ⟂ Hi` and `Hi ⟂ Lo`. There is
/// deliberately no `All: ComplementOf<_>` and no `Lo: ComplementOf<Lo>`.
pub trait ComplementOf<Other: ActiveSet>: ActiveSet + sealed::Sealed {}

impl ComplementOf<Hi> for Lo {}
impl ComplementOf<Lo> for Hi {}

/// A membership token at type-level epoch `E` over member-set `S`.
///
/// Move-only by construction (no `Copy`/`Clone`): the token is a *linear*
/// resource. `members` is the runtime membership bitmask; the type-level `E`
/// and `S` are what the compiler reasons about.
#[must_use = "a Quorum token is a linear resource; dropping it silently loses membership"]
pub struct Quorum<const E: u64, S: ActiveSet> {
    members: u32,
    _set: PhantomData<S>,
}

impl<const E: u64, S: ActiveSet> Quorum<E, S> {
    /// The runtime membership bitmask carried by this token.
    pub const fn members(&self) -> u32 {
        self.members
    }

    /// The epoch this token belongs to (mirrors the type-level `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }
}

impl<const E: u64> Quorum<E, All> {
    /// Mint a full quorum at epoch `E` with the given membership mask.
    ///
    /// The epoch is supplied via turbofish, e.g. `Quorum::<3, All>::genesis(m)`.
    pub const fn genesis(members: u32) -> Self {
        Quorum { members, _set: PhantomData }
    }

    /// Partition a full quorum into complementary `Lo`/`Hi` halves at the
    /// **same** epoch. Consumes `self` (the whole can't coexist with its parts).
    ///
    /// The bit-16 split point is a toy stand-in for a real membership partition.
    pub fn split(self) -> (Quorum<E, Lo>, Quorum<E, Hi>) {
        let lo = self.members & 0x0000_FFFF;
        let hi = self.members & 0xFFFF_0000;
        (
            Quorum { members: lo, _set: PhantomData },
            Quorum { members: hi, _set: PhantomData },
        )
    }
}

/// Recombine two halves into a full quorum.
///
/// Compiles **only** when the type system can prove both preconditions:
/// * `a` and `b` share the same type-level epoch `E` (unifying the two `E`s), and
/// * `A: ComplementOf<B>` (the sets are complementary halves).
///
/// Both tokens are consumed. In this toy `merge` is total (it cannot fail) —
/// the lockstep assumption a real distributed `merge` does not get to make.
pub fn merge<const E: u64, A, B>(a: Quorum<E, A>, b: Quorum<E, B>) -> Quorum<E, All>
where
    A: ComplementOf<B>,
    B: ActiveSet,
{
    Quorum { members: a.members | b.members, _set: PhantomData }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_epoch_complements_split_and_merge() {
        let all = Quorum::<3, All>::genesis(0xDEAD_BEEF);
        assert_eq!(all.epoch(), 3);

        let (lo, hi) = all.split();
        assert_eq!(lo.members(), 0x0000_BEEF, "lo half keeps low 16 bits");
        assert_eq!(hi.members(), 0xDEAD_0000, "hi half keeps high 16 bits");
        assert_eq!(lo.epoch(), 3, "split preserves epoch");
        assert_eq!(hi.epoch(), 3, "split preserves epoch");

        let merged = merge(lo, hi);
        assert_eq!(merged.members(), 0xDEAD_BEEF, "merge is the union of halves");
        assert_eq!(merged.epoch(), 3, "merge preserves epoch");
    }

    #[test]
    fn merge_is_order_independent() {
        // Hi: ComplementOf<Lo> also holds, so halves may be merged either way.
        let (lo, hi) = Quorum::<9, All>::genesis(0xABCD_1234).split();
        let merged = merge(hi, lo);
        assert_eq!(merged.members(), 0xABCD_1234);
        assert_eq!(merged.epoch(), 9);
    }

    #[test]
    fn empty_membership_round_trips() {
        let (lo, hi) = Quorum::<0, All>::genesis(0).split();
        assert_eq!(lo.members(), 0);
        assert_eq!(hi.members(), 0);
        assert_eq!(merge(lo, hi).members(), 0);
    }
}

//! Two-phase **locking** (2PL) — the per-transaction **phase** discipline behind
//! conflict-serializability (Eswaran, Gray, Lorie & Traiger, 1976).
//!
//! **Not to be confused with [`twophase`](crate::twophase)** — that rung is
//! two-phase *commit*, an **atomicity** protocol (all participants decide the same
//! way). This is two-phase *locking*, an **isolation** protocol (concurrent
//! transactions do not interfere). They share the words "two phase" and nothing
//! else: 2PC's phases are *vote* then *decide*; 2PL's phases are *grow* then
//! *shrink*. Together they type two different letters of ACID — [`twophase`](crate::twophase) and
//! [`saga`](crate::saga) buy **A**tomicity, this rung and `occ` (this loop's
//! optimistic sibling) buy
//! **I**solation.
//!
//! The rule is one sentence: a transaction may acquire locks (the **growing**
//! phase) and it may release them (the **shrinking** phase), but **once it has
//! released any lock it may never acquire another**. That single monotone
//! boundary is the per-transaction rule 2PL's theorem needs: when *every*
//! transaction obeys it, the interleaved schedule is conflict-serializable — each
//! behaves as if it executed atomically at the instant it held its last lock (that
//! all peers are two-phase is a whole-schedule premise; this rung types one
//! transaction's own boundary — see the seam). Acquiring after a release is the one
//! move that breaks the guarantee, and it is the one move this module makes
//! *unrepresentable*.
//!
//! ## The mechanism — the phase is a type, the boundary is a method's absence
//!
//! [`Growing`] and [`Shrinking`] are the two phases (sealed — there is no third).
//! A [`Txn`]`<PH>` is a transaction currently in phase `PH`.
//!
//! * [`Txn::begin`] starts a transaction in the [`Growing`] phase.
//! * [`acquire`](Txn::acquire) — defined **only** on `Txn<`[`Growing`]`>` — takes a
//!   lock, hands back a linear [`Lock`] guard, and *stays* in `Growing`.
//! * [`release`](Txn::release) — consumes a [`Lock`] guard. The **first** release
//!   moves the transaction `Growing` → [`Shrinking`]; further releases keep it in
//!   `Shrinking`.
//! * **`Txn<Shrinking>` has no [`acquire`](Txn::acquire).** Once you have crossed
//!   into the shrinking phase, re-acquiring a lock is not a runtime check that
//!   fails — the method does not exist.
//!
//! ## A well-formed transaction grows, then shrinks
//!
//! ```
//! use quorum_types::two_phase_lock::Txn;
//! let txn = Txn::begin();                 // Growing, 0 locks
//! let (txn, a) = txn.acquire();           // Growing, holds A
//! let (txn, b) = txn.acquire();           // Growing, holds A, B  (still growing)
//! let txn = txn.release(a);               // Shrinking — first release ends growth
//! let txn = txn.release(b);               // Shrinking — keep releasing
//! txn.commit();                           // done
//! ```
//!
//! ## Acquiring after a release is a compile error
//!
//! This is the 2PL violation — a lock taken *after* the transaction began letting
//! go, which lets another transaction interleave and destroys serializability.
//! `Txn<Shrinking>` simply has no `acquire`:
//!
//! ```compile_fail
//! use quorum_types::two_phase_lock::Txn;
//! let (txn, a) = Txn::begin().acquire();
//! let txn = txn.release(a);   // now Shrinking
//! let _ = txn.acquire();      // no `acquire` on Txn<Shrinking>: acquire-after-release is a 2PL violation
//! ```
//!
//! ## A lock guard is linear — you cannot release the same lock twice
//!
//! [`Lock`] is move-only (no `Copy`/`Clone`); [`release`](Txn::release) consumes it,
//! so a double-release is a use-after-move:
//!
//! ```compile_fail
//! use quorum_types::two_phase_lock::Txn;
//! let (txn, a) = Txn::begin().acquire();
//! let txn = txn.release(a);
//! let _ = txn.release(a);   // `a` already moved into the first release
//! ```
//!
//! ## You cannot fabricate a lock — releasing needs one you acquired
//!
//! [`Lock`] has a private field and no public constructor, so the only way to get
//! one is [`acquire`](Txn::acquire) on a growing transaction:
//!
//! ```compile_fail
//! use quorum_types::two_phase_lock::{Txn, Lock};
//! let txn = Txn::begin();
//! let _ = txn.release(Lock {}); // Lock has a private field: no public constructor
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! The types own **one transaction's phase discipline**: it grows, then shrinks,
//! and never re-grows. They do **not** own:
//!
//! * **That every transaction obeys 2PL.** Serializability is a property of the
//!   *whole schedule* — it holds only if *all* concurrent transactions are
//!   two-phase. This rung constrains one transaction's own moves; that its peers
//!   are equally disciplined is the same *global* obligation as
//!   [`lockorder`](crate::lockorder)'s "every thread agrees on the ranks" — a
//!   property no single handle can see.
//! * **The actual lock table.** [`acquire`](Txn::acquire) models "I took the lock
//!   on some resource"; it does not check that the lock was free, that a real
//!   mutual-exclusion table granted it, or *which* resource it names. Two
//!   transactions' [`Lock`] guards are indistinguishable at the type level.
//! * **Deadlock.** 2PL buys serializability at the price of deadlock: two
//!   transactions, each holding what the other wants, both still in the growing
//!   phase, wait forever. Nothing here prevents it — that hazard is exactly what
//!   [`lockorder`](crate::lockorder) types (acquire in rank order), and a
//!   *serializable **and** deadlock-free* system needs both disciplines at once.
//! * **Strict 2PL.** Basic 2PL (this rung) permits releasing a lock before the
//!   transaction commits, which risks *cascading aborts* (a peer read your
//!   uncommitted write). **Strict** 2PL forbids that by holding every write lock
//!   until commit — i.e. collapsing the shrinking phase onto the commit point.
//!   The variant is a policy on *when* [`release`](Txn::release) may be called, not
//!   a different type; this rung types the phase boundary that both share.

use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A phase of a two-phase-locking transaction. Sealed: the only phases are
/// [`Growing`] and [`Shrinking`], so no downstream code can invent a third that
/// escapes the monotone grow-then-shrink boundary.
pub trait Phase: sealed::Sealed {}

/// The **growing** phase: the transaction may still acquire locks. A transaction
/// begins here.
#[derive(Debug)]
pub struct Growing;
/// The **shrinking** phase: the transaction has released at least one lock and may
/// only release more — it can never acquire again.
#[derive(Debug)]
pub struct Shrinking;

impl sealed::Sealed for Growing {}
impl sealed::Sealed for Shrinking {}
impl Phase for Growing {}
impl Phase for Shrinking {}

/// A held lock, as a move-only linear guard.
///
/// Minted only by [`acquire`](Txn::acquire) (private field, no public
/// constructor) and consumed only by [`release`](Txn::release) — so a release
/// *costs* an acquisition and a lock cannot be fabricated. It deliberately carries
/// **no transaction identity**: at the type level one transaction's lock is
/// indistinguishable from another's (see the seam — the real lock table is not
/// modelled). Nothing here counts locks, precisely because a count would be a
/// *per-transaction* fact that a globally-linear, identity-less guard cannot keep
/// honest; the guarantee this rung makes is the *phase boundary*, which is
/// type-level and needs no tally.
#[must_use = "a Lock is a held lock; release it (or the transaction never lets it go)"]
pub struct Lock {
    _priv: (),
}

/// A transaction currently in phase `PH`.
///
/// Move-only and `#[must_use]`: a transaction is a linear resource that must be
/// driven to [`commit`](Txn::commit) (or explicitly dropped — an abandoned
/// transaction). It carries no runtime state at all — the type-level `PH` is the
/// entire model, and it is what forbids acquire-after-release.
#[must_use = "a Txn is a live transaction; commit it (or explicitly drop to abort)"]
pub struct Txn<PH: Phase> {
    _ph: PhantomData<PH>,
}

impl Txn<Growing> {
    /// Begin a new transaction in the growing phase, holding no locks.
    pub const fn begin() -> Self {
        Txn { _ph: PhantomData }
    }

    /// Acquire a lock. Available **only** in the growing phase; the transaction
    /// stays growing and gains a linear [`Lock`] guard.
    ///
    /// There is deliberately no `acquire` on `Txn<`[`Shrinking`]`>` — that absence
    /// is what makes acquire-after-release a compile error.
    pub fn acquire(self) -> (Txn<Growing>, Lock) {
        (Txn { _ph: PhantomData }, Lock { _priv: () })
    }

    /// Release a lock for the **first** time, ending the growing phase and moving
    /// the transaction into [`Shrinking`]. Consumes a [`Lock`] guard.
    ///
    /// This is the one-way boundary of 2PL: after it, no further `acquire` exists.
    pub fn release(self, _lock: Lock) -> Txn<Shrinking> {
        Txn { _ph: PhantomData }
    }
}

impl Txn<Shrinking> {
    /// Release another lock, staying in the shrinking phase. Consumes a [`Lock`]
    /// guard. There is no `acquire` counterpart here — that is the whole point.
    pub fn release(self, _lock: Lock) -> Txn<Shrinking> {
        Txn { _ph: PhantomData }
    }
}

impl<PH: Phase> Txn<PH> {
    /// Commit the transaction, consuming it. Available in either phase: a
    /// transaction that acquired and released within the discipline may commit, and
    /// a transaction that (under strict 2PL) still holds its locks commits here too.
    ///
    /// This toy does not model durability or the write set — `commit` simply ends
    /// the transaction's life. Any still-held [`Lock`] guards are separate linear
    /// resources; dropping them models release-at-commit.
    pub fn commit(self) -> Committed {
        Committed { _priv: () }
    }
}

/// A committed transaction receipt. Constructed only by [`Txn::commit`] (private
/// field, no public constructor) — its existence certifies that a transaction ran
/// its phases and committed. It carries no payload: this rung types the *lock
/// discipline*, not a written value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Committed {
    _priv: (),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grow_then_shrink_then_commit() {
        // The canonical 2PL shape: acquire in the growing phase, release in the
        // shrinking phase, commit. The whole sequence type-checks; that is the point.
        let txn = Txn::begin();
        let (txn, a) = txn.acquire();
        let (txn, b) = txn.acquire(); // still Growing after multiple acquires
        let txn = txn.release(a); // first release -> Shrinking
        let txn = txn.release(b);
        let committed = txn.commit();
        let also = committed; // Committed is Copy: a receipt is a shareable fact
        assert_eq!(committed, also);
    }

    #[test]
    fn strict_2pl_holds_locks_to_commit() {
        // Strict 2PL: acquire, then commit while still in the growing phase, holding
        // the lock — the shrinking phase collapses onto the commit point. The guard
        // is dropped (released) at commit.
        let (txn, lock) = Txn::begin().acquire();
        let _receipt = txn.commit(); // still Growing, still holding
        drop(lock); // release-at-commit
    }

    #[test]
    fn a_transaction_can_stay_in_the_growing_phase() {
        // Acquiring repeatedly never forces a phase change — only a release does.
        let (txn, a) = Txn::begin().acquire();
        let (txn, b) = txn.acquire();
        let (txn, c) = txn.acquire();
        let (txn, d) = txn.acquire(); // still Growing: can acquire a fourth
        // now unwind (first release crosses into Shrinking)
        let txn = txn.release(a);
        let txn = txn.release(b);
        let txn = txn.release(c);
        let txn = txn.release(d);
        let _ = txn.commit();
    }
}

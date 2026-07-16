//! Optimistic concurrency control (OCC) — serializability from a **validation**
//! phase instead of locks (Kung & Robinson, 1981). Like `two_phase_lock` (this
//! loop's pessimistic sibling), this rung types one transaction's phase machine (no
//! commit without a passed validation), not the whole-schedule guarantee — see the
//! runtime seam.
//!
//! `two_phase_lock` is the *pessimistic* road to
//! isolation: take a lock before you touch anything, and never re-grow. OCC is its
//! **optimistic dual** — take *no* locks, read a private snapshot, do your work,
//! and only at the very end **validate** that nothing you read has changed since;
//! commit if so, abort and retry if not. It bets that conflicts are rare, so the
//! common path pays nothing; a lock-based transaction pays on every access whether
//! or not a conflict was ever possible. Same guarantee (conflict-serializability),
//! opposite bet on contention — the same dual as
//! [`twophase`](crate::twophase)↔[`saga`](crate::saga) and
//! [`crdt`](crate::crdt)↔[`causal`](crate::causal).
//!
//! The one move that must be impossible is **committing without validating** — a
//! transaction that writes back over a snapshot some other transaction has already
//! overwritten is the lost-update / stale-write anomaly. This module makes it
//! unrepresentable: `commit` exists **only** on a validated transaction, and the
//! only door into the validated phase is a validation that actually passed.
//!
//! ## The mechanism — validation is a phase transition, not a token
//!
//! [`Reading`] and [`Valid`] are the two phases. A [`Txn`]`<PH>` carries the
//! version it snapshotted at [`begin`](Txn::begin) and the value it intends to
//! write.
//!
//! * [`Txn::begin`] starts a transaction in the [`Reading`] phase over a snapshot
//!   version.
//! * [`validate`](Txn::validate) — defined **only** on `Txn<`[`Reading`]`>` —
//!   **consumes** the transaction and compares the snapshot against the current
//!   version. On a match it re-emits the *same* transaction in the [`Valid`] phase;
//!   on a mismatch it returns a [`Conflict`] and the transaction is gone (an OCC
//!   abort — you re-[`begin`](Txn::begin) from a fresh snapshot).
//! * [`commit`](Txn::commit) — defined **only** on `Txn<`[`Valid`]`>` — consumes the
//!   validated transaction and produces its [`Committed`] write.
//!
//! Because validation *transforms the transaction itself* rather than handing back
//! a free-floating "validated" token, a validation of transaction A can never be
//! used to wave transaction B through: there is no separable witness to move. This
//! is the deliberate fix for the crate's recurring
//! witness-that-is-not-unforgeable hazard — the proof of validation *is* the typed
//! transaction, so it cannot be detached and replayed.
//!
//! ## Read, validate, commit
//!
//! ```
//! use quorum_types::occ::Txn;
//! // snapshot the world at version 7, intend to write 0xABCD
//! let txn = Txn::begin(7, 0xABCD);
//! // at commit time the current version is still 7 — nobody wrote under us
//! let txn = txn.validate(7).expect("no conflict");
//! let write = txn.commit();
//! assert_eq!(write.value(), 0xABCD);
//! ```
//!
//! ## A conflict aborts — the transaction is consumed, not committable
//!
//! ```
//! use quorum_types::occ::Txn;
//! let txn = Txn::begin(7, 0xABCD);
//! // someone advanced the version to 9 since our snapshot: our read set is stale
//! match txn.validate(9) {
//!     Ok(_) => unreachable!("a stale snapshot must not validate"),
//!     Err(conflict) => {
//!         assert_eq!(conflict.snapshot(), 7);
//!         assert_eq!(conflict.observed(), 9);
//!     }
//! }
//! // `txn` was moved into `validate` — there is no committing it now.
//! ```
//!
//! ## Committing without validating is a compile error
//!
//! [`commit`](Txn::commit) exists only on `Txn<`[`Valid`]`>`. A transaction still
//! in the [`Reading`] phase has no `commit` method — you cannot skip validation:
//!
//! ```compile_fail
//! use quorum_types::occ::Txn;
//! let txn = Txn::begin(7, 0xABCD);
//! let _ = txn.commit(); // no `commit` on Txn<Reading>: validate first
//! ```
//!
//! ## You cannot fabricate a validated transaction
//!
//! [`Txn`]'s fields are private, so a `Txn<Valid>` cannot be built by hand — the
//! only route into the [`Valid`] phase is [`validate`](Txn::validate) returning
//! `Ok`:
//!
//! ```compile_fail
//! use quorum_types::occ::{Txn, Valid};
//! let forged: Txn<Valid> = Txn { snapshot: 7, value: 0, _ph: core::marker::PhantomData };
//! let _ = forged.commit(); // Txn has private fields: no hand-built Valid transaction
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! OCC's phase machine is structural, but the door from [`Reading`] to [`Valid`] is
//! opened by a **runtime** comparison — so this rung's guarantee rests on a trusted
//! runtime witness (the [`vclock`](crate::vclock) / [`fencing`](crate::fencing)
//! species), not on structure alone. The types own the *shape* (no commit without a
//! passed validation, no detachable witness); they do **not** own:
//!
//! * **That `current_version` is the truth.** [`validate`](Txn::validate) trusts the
//!   version it is handed to faithfully reflect *every* committed write to the read
//!   set. A version counter that misses a write validates a doomed transaction. The
//!   type cannot check that the number is honest — the same trust
//!   [`fencing`](crate::fencing) places in its monotone token authority.
//! * **Validate-then-commit atomicity.** The fatal OCC race: a writer commits in the
//!   window *between* [`validate`](Txn::validate) returning [`Valid`] and
//!   [`commit`](Txn::commit) running. Real OCC closes that window with a critical
//!   section (validation and write-back under one latch, or a version-CAS at
//!   write-back). Here the two are distinct method calls with a gap the type cannot
//!   see — so "validated" means "was valid at the instant of the check", not
//!   "still valid at write-back". Closing the window is a runtime obligation.
//! * **The read set.** A real validator compares *every item the transaction read*.
//!   This toy collapses the whole read set to one scalar `snapshot` version; it types
//!   the *decision* (matched ⇒ commit, else abort), not the bookkeeping of which
//!   items were read. The collapse errs only in the **safe** direction: a global
//!   version bumped by a write this transaction did not actually read causes a
//!   *false abort* (a spurious retry), never a false *commit* — equality admits only
//!   the provably-unchanged case, so it can over-abort but never under-abort.

use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A phase of an optimistic transaction. Sealed: the only phases are [`Reading`]
/// and [`Valid`], so a `Txn<Valid>` can arise only through
/// [`validate`](Txn::validate), never by naming a downstream phase type.
pub trait Phase: sealed::Sealed {}

/// The **reading** phase: the transaction has a snapshot and has not yet been
/// validated. It cannot commit.
#[derive(Debug)]
pub struct Reading;
/// The **valid** phase: [`validate`](Txn::validate) confirmed the snapshot was
/// still current. Only a transaction in this phase can [`commit`](Txn::commit).
#[derive(Debug)]
pub struct Valid;

impl sealed::Sealed for Reading {}
impl sealed::Sealed for Valid {}
impl Phase for Reading {}
impl Phase for Valid {}

/// An optimistic transaction in phase `PH`.
///
/// Move-only and `#[must_use]`: a live transaction is a linear resource, resolved
/// by [`commit`](Txn::commit) (via [`validate`](Txn::validate)) or dropped (abort).
/// `snapshot` is the version observed at [`begin`](Txn::begin); `value` is the
/// intended write. Both fields are private — a `Txn<Valid>` cannot be forged.
#[must_use = "an OCC Txn is a live transaction; validate and commit it (or drop to abort)"]
pub struct Txn<PH: Phase> {
    snapshot: u64,
    value: u64,
    _ph: PhantomData<PH>,
}

impl Txn<Reading> {
    /// Begin an optimistic transaction: snapshot the current `version` and record
    /// the `value` this transaction intends to write. Takes no locks.
    pub const fn begin(version: u64, value: u64) -> Self {
        Txn { snapshot: version, value, _ph: PhantomData }
    }

    /// Validate the read snapshot against the `current_version` and **consume** the
    /// transaction. If nothing has changed (`current_version == snapshot`), re-emit
    /// the transaction in the [`Valid`] phase, ready to commit; otherwise return a
    /// [`Conflict`] and abort (the transaction is gone — re-[`begin`](Txn::begin)).
    ///
    /// Consuming `self` is what fuses the validation to *this* transaction: there is
    /// no separate token to hand to a different, un-validated transaction.
    pub fn validate(self, current_version: u64) -> Result<Txn<Valid>, Conflict> {
        if current_version == self.snapshot {
            Ok(Txn { snapshot: self.snapshot, value: self.value, _ph: PhantomData })
        } else {
            Err(Conflict { snapshot: self.snapshot, observed: current_version })
        }
    }
}

impl Txn<Valid> {
    /// Commit the validated transaction, producing its write. Reachable **only**
    /// from the [`Valid`] phase — i.e. only after [`validate`](Txn::validate)
    /// confirmed the snapshot was current.
    pub fn commit(self) -> Committed {
        Committed { value: self.value }
    }
}

impl<PH: Phase> Txn<PH> {
    /// The version this transaction snapshotted at [`begin`](Txn::begin).
    pub const fn snapshot(&self) -> u64 {
        self.snapshot
    }
}

/// A validation failure: the current version differs from the one this transaction
/// snapshotted, so its read set is stale and it must abort. Returned by
/// [`validate`](Txn::validate); the transaction it came from has been consumed
/// (aborted) — OCC's response is to re-[`begin`](Txn::begin) from a fresh snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conflict {
    snapshot: u64,
    observed: u64,
}

impl Conflict {
    /// The version the aborted transaction had snapshotted.
    pub const fn snapshot(&self) -> u64 {
        self.snapshot
    }
    /// The version observed at validation time. Any value other than the
    /// [`snapshot`](Conflict::snapshot) aborts; with a faithful monotone version
    /// counter it is the newer version a conflicting write advanced to.
    pub const fn observed(&self) -> u64 {
        self.observed
    }
}

/// A committed OCC write. Constructed only by [`Txn::commit`] on a `Txn<Valid>`
/// (private field, no public constructor) — so its existence certifies that a
/// validation passed for *this* write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Committed {
    value: u64,
}

impl Committed {
    /// The value written by the committed transaction.
    pub const fn value(&self) -> u64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_conflict_commits() {
        let txn = Txn::begin(7, 0xABCD);
        assert_eq!(txn.snapshot(), 7);
        let txn = txn.validate(7).expect("version unchanged -> valid");
        assert_eq!(txn.commit().value(), 0xABCD);
    }

    #[test]
    fn a_stale_snapshot_conflicts_and_aborts() {
        let txn = Txn::begin(7, 0xABCD);
        match txn.validate(9) {
            Ok(_) => panic!("version advanced -> must conflict"),
            Err(conflict) => {
                assert_eq!(conflict.snapshot(), 7);
                assert_eq!(conflict.observed(), 9);
            }
        }
        // `txn` was consumed by `validate`; nothing left to commit.
    }

    #[test]
    fn validation_is_fused_to_the_transaction() {
        // The value that commits is the value the *validated* transaction carried —
        // there is no way to validate one transaction and commit another's write.
        let a = Txn::begin(3, 111);
        let b = Txn::begin(3, 222);
        let a = a.validate(3).unwrap();
        // `b` is still Reading; it has no commit. Only `a` (validated) can commit,
        // and it commits *its own* value.
        assert_eq!(a.commit().value(), 111);
        // b would have to validate on its own to commit; drop it (abort).
        drop(b);
    }
}

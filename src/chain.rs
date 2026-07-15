//! Chain replication — linearizability from a linear **topology**, with no quorum
//! anywhere (van Renesse & Schneider, OSDI 2004).
//!
//! Every consistency rung so far earns its guarantee from a *set*: a quorum that
//! intersects ([`membership`](crate::membership), [`flex`](crate::flex),
//! [`byzantine`](crate::byzantine)), a lattice of values
//! ([`consistency`](crate::consistency), [`crdt`](crate::crdt)), or a clock
//! ([`vclock`](crate::vclock), [`commit_wait`](crate::commit_wait)). Chain
//! replication takes a different road: the replicas are arranged in a **line**
//! Head → … → Tail. An update is applied at the Head, forwarded one hop at a time,
//! and is **committed exactly when it reaches the Tail** — at which point, *in a
//! correct chain*, every upstream replica has already applied it (a durability
//! premise the types propagate but do not verify — see the seam below). Reads are
//! served at the Tail, so they observe only committed state. The strength comes
//! from the *order of traversal* rather than from any two sets overlapping — though
//! that order is only one of the premises the full guarantee rests on.
//!
//! This module types that traversal discipline **structurally**: an update's
//! position in the chain is a type parameter, forwarding advances it exactly one
//! hop, and a commit is reachable *only* from the Tail. Committing an update that
//! has not traversed the whole chain is not a runtime check — it is
//! *unrepresentable*. What the types own is precisely this *single update's*
//! traversal order; the cross-update per-link FIFO ordering that linearizability
//! *across* updates also requires, and the single-head premise, stay runtime seams
//! (spelled out below).
//!
//! ## The mechanism — position as a type, forwarding as a type-level successor
//!
//! [`Head`], [`Mid`], and [`Tail`] are the chain positions (sealed — no other
//! position exists). The [`Forward`] trait carries the successor as an associated
//! type: `Head`'s next is `Mid`, `Mid`'s next is `Tail`. **`Tail` has no
//! [`Forward`] impl**, so there is no "next" past the tail — forwarding off the end
//! of the chain fails to resolve a method.
//!
//! * [`Update::issue`] mints an [`Update`]`<`[`Head`]`>` — only the Head accepts new
//!   updates.
//! * [`forward`](Update::forward) — defined for any `P: Forward` — advances
//!   `Update<P>` to `Update<P::Next>`, one hop.
//! * [`commit`](Update::commit) — defined **only** on [`Update`]`<`[`Tail`]`>` —
//!   consumes the update and mints a [`Committed`]. No other position has it.
//!
//! ## An update traverses the whole chain, then commits at the tail
//!
//! ```
//! use quorum_types::chain::Update;
//! let at_head = Update::issue(0xC0FFEE);
//! let at_mid = at_head.forward();   // Head -> Mid
//! let at_tail = at_mid.forward();   // Mid  -> Tail
//! let committed = at_tail.commit(); // only the tail can commit
//! assert_eq!(committed.value(), 0xC0FFEE);
//! ```
//!
//! ## Committing before the tail is a compile error
//!
//! [`commit`](Update::commit) exists only on `Update<Tail>`. An update still at the
//! head (or middle) has no `commit` method — a half-replicated update cannot be
//! acknowledged:
//!
//! ```compile_fail
//! use quorum_types::chain::Update;
//! let at_head = Update::issue(1);
//! let _ = at_head.commit(); // no `commit` on Update<Head>: it has not reached the tail
//! ```
//!
//! ## Forwarding off the end of the chain is a compile error
//!
//! `Tail` implements no [`Forward`], so `Update<Tail>` has no `forward` — you cannot
//! advance past the last replica:
//!
//! ```compile_fail
//! use quorum_types::chain::Update;
//! let at_tail = Update::issue(1).forward().forward(); // now at Tail
//! let _ = at_tail.forward(); // no `forward` on Update<Tail>: nothing past the tail
//! ```
//!
//! ## Only the head issues — a compile error to inject at the tail
//!
//! [`issue`](Update::issue) is defined only on `Update<Head>`, so an update cannot
//! be born at the tail (which would bypass replication entirely):
//!
//! ```compile_fail
//! use quorum_types::chain::{Update, Tail};
//! let _ = Update::<Tail>::issue(1); // no `issue` for Update<Tail>: updates enter at the head
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! The types own **one update's traversal order**: it is issued at the head,
//! advances one hop at a time, and is acknowledgeable only from the tail. They do
//! **not** own:
//!
//! * **Per-node durability.** [`forward`](Update::forward) models "this replica
//!   applied the update and passed it on," but the type cannot check that a replica
//!   actually persisted before forwarding — the apply is trusted to precede the
//!   hop. A [`Committed`] witnesses *traversal to the tail role*, not that N
//!   physical replicas each hold the value durably.
//! * **Cross-update FIFO order.** This rung types a single update walking the
//!   chain. Chain replication's linearizability across *many* updates rests on each
//!   link delivering in FIFO order (the tail applies updates in head-issue order).
//!   That per-link ordering is a runtime property of the transport, not something
//!   these per-update tokens track. (An out-of-tree TLC model, in the research
//!   harness and not shipped in this crate, is the discriminant: in-order
//!   forwarding keeps the tail's applied sequence an order-preserving prefix of the
//!   head's; drop the in-order guard and the tail reorders — a violation.)
//! * **Chain membership and failure recovery.** The hard part of a real chain —
//!   detecting a dead replica, splicing it out, and re-linking head/tail (the CRAQ
//!   / master's job) — is entirely a runtime protocol. The chain's *shape* is
//!   operator state; the types assume a fixed, correct Head → Mid → Tail line, the
//!   same way [`fencing`](crate::fencing) assumes a correct token authority.
//! * **A single, real head.** That `Head` is the one true entry point (no second
//!   writer injecting off-chain) is a configuration axiom the types propagate but
//!   cannot verify — the same declared-axiom seam as [`byzantine`](crate::byzantine)'s
//!   fault budget `f`.

use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A position in the replication chain. Sealed: the only positions are the three
/// defined here, so no downstream code can invent a fourth that skips the order.
pub trait Position: sealed::Sealed {}

/// The chain **head**: the single entry point where updates are issued.
#[derive(Debug)]
pub struct Head;
/// A chain **interior** replica: receives from its predecessor, forwards to its
/// successor.
#[derive(Debug)]
pub struct Mid;
/// The chain **tail**: where an update is committed and linearizable reads are
/// served. It has no successor.
#[derive(Debug)]
pub struct Tail;

impl sealed::Sealed for Head {}
impl sealed::Sealed for Mid {}
impl sealed::Sealed for Tail {}
impl Position for Head {}
impl Position for Mid {}
impl Position for Tail {}

/// A position that has a **successor** in the chain: forwarding one hop moves an
/// update from `Self` to [`Next`](Forward::Next).
///
/// Implemented for [`Head`] (→ [`Mid`]) and [`Mid`] (→ [`Tail`]). Deliberately
/// **not** implemented for [`Tail`]: there is nothing past the tail, so
/// `Update<Tail>` has no `forward` method.
pub trait Forward: Position {
    /// The next position down the chain.
    type Next: Position;
}

impl Forward for Head {
    type Next = Mid;
}
impl Forward for Mid {
    type Next = Tail;
}

/// An update in flight at chain position `P`.
///
/// Move-only and `#[must_use]`: an update that has entered the chain has taken
/// effect at the head and must be driven to the tail (committed) or explicitly
/// dropped — a silently discarded in-flight update is a lost write. The runtime
/// `value` is the command payload; the type-level `P` is the replica it currently
/// sits at.
#[must_use = "an in-flight Update has been applied at the head; forward it to the tail and commit it"]
pub struct Update<P: Position> {
    value: u64,
    _pos: PhantomData<P>,
}

impl Update<Head> {
    /// Issue a new update at the **head** of the chain. Updates enter only here;
    /// there is no way to mint an `Update` at an interior or tail position.
    pub const fn issue(value: u64) -> Self {
        Update { value, _pos: PhantomData }
    }
}

impl<P: Position> Update<P> {
    /// The command payload carried by this update (unchanged as it traverses).
    pub const fn value(&self) -> u64 {
        self.value
    }
}

impl<P: Forward> Update<P> {
    /// Advance the update **one hop** down the chain, from `P` to its successor
    /// `P::Next`. Consumes the update at `P` (it now lives at the next replica).
    ///
    /// This is only defined where a successor exists, so the chain can be walked
    /// `Head` → `Mid` → `Tail` but not past the tail.
    pub fn forward(self) -> Update<P::Next> {
        Update { value: self.value, _pos: PhantomData }
    }
}

impl Update<Tail> {
    /// Commit the update — reachable **only** from the tail, i.e. only after the
    /// update has traversed the entire chain and every upstream replica has applied
    /// it. Consumes the update and mints a [`Committed`] receipt.
    pub fn commit(self) -> Committed {
        Committed { value: self.value }
    }
}

/// A committed update: it reached the tail of the chain, so — in a correct chain —
/// every upstream replica has applied it and a tail read will observe it.
///
/// Constructed **only** by [`Update::commit`] on an `Update<Tail>` (private field,
/// no public constructor). It certifies *traversal to the tail role* — not that any
/// particular number of physical replicas hold the value durably, and not a total
/// order relative to other updates (that is the per-link FIFO seam).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Committed {
    value: u64,
}

impl Committed {
    /// The committed command payload.
    pub const fn value(&self) -> u64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_traverses_head_to_tail_and_commits() {
        let committed = Update::issue(0xDEAD_BEEF)
            .forward() // Head -> Mid
            .forward() // Mid  -> Tail
            .commit();
        assert_eq!(committed.value(), 0xDEAD_BEEF, "commit preserves the payload");
    }

    #[test]
    fn the_payload_is_unchanged_at_every_hop() {
        let at_head = Update::issue(42);
        assert_eq!(at_head.value(), 42);
        let at_mid = at_head.forward();
        assert_eq!(at_mid.value(), 42, "forwarding does not alter the payload");
        let at_tail = at_mid.forward();
        assert_eq!(at_tail.value(), 42);
        assert_eq!(at_tail.commit().value(), 42);
    }

    #[test]
    fn committed_is_a_plain_value_receipt() {
        let a = Update::issue(7).forward().forward().commit();
        let b = a; // Copy: a committed receipt is a fact, freely shareable
        assert_eq!(a, b);
        assert_eq!(a.value(), 7);
    }
}

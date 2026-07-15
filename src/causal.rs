//! Causal-delivery typestate — the *order* rung of the mapping.
//!
//! The [`consistency`](crate::consistency) lattice types *how much consensus* a
//! value carries. This module types something orthogonal: the **causal order** in
//! which values may be applied. In a causally-consistent system a message may be
//! delivered only after every message it causally depends on — deliver `B` before
//! its predecessor `A` and a reader sees an effect before its cause. Here that
//! rule is a **compile error**, not a runtime buffer check.
//!
//! ## The mechanism
//!
//! * [`Delivered<Ev>`] is a **witness** — *proof* that event `Ev`, and inductively
//!   all of its causal ancestors, has been delivered at this node. It is `Copy`: a
//!   witness is a *fact*, not a resource, so one delivered predecessor may enable
//!   **many** successors (causal fan-out — a DAG, not just a chain). Its only
//!   constructors are [`origin`] (the empty history) and [`Msg::deliver`]/
//!   [`Join::deliver`] (which demand the predecessor's witness first), so a
//!   witness always traces a delivered chain back to the origin — though *which*
//!   predecessor a message declares is the broadcast layer's call (see the seam).
//! * [`Msg<T, Id, Pred>`] is a message in transit: payload `T`, a type-level
//!   identity `Id`, and a type-level immediate predecessor `Pred`. It is
//!   **move-only** — delivered exactly once.
//! * [`Msg::deliver`] consumes the message *given `Delivered<Pred>`* and returns
//!   the payload plus a fresh `Delivered<Id>`. Supplying the wrong (or a missing)
//!   predecessor witness does not unify — the causal gap is caught at compile time.
//!
//! ## Delivering out of causal order is a compile error
//!
//! You hold only the origin witness; a message whose predecessor you have not
//! delivered cannot be delivered — its `deliver` wants `Delivered<A>` and you have
//! only `Delivered<Genesis>`:
//!
//! ```compile_fail
//! use quorum_types::causal::{Msg, origin};
//! enum A {}
//! enum B {}
//! let g = origin();                      // Delivered<Genesis>
//! let msg_b = Msg::<i32, B, A>::new(2);  // B causally depends on A
//! let _ = msg_b.deliver(g);              // wants Delivered<A>, has Delivered<Genesis> — causal gap
//! ```
//!
//! ## The happy path — deliver in causal order
//!
//! ```
//! use quorum_types::causal::{Msg, origin, Genesis};
//! enum A {}
//! enum B {}
//! let g = origin();
//! let (va, da) = Msg::<i32, A, Genesis>::new(1).deliver(g);  // A after the origin
//! let (vb, _db) = Msg::<i32, B, A>::new(2).deliver(da);      // then B after A
//! assert_eq!((va, vb), (1, 2));
//! ```
//!
//! ## Concurrent joins are checkable too — *when the shape is static*
//!
//! A merge point with two immediate predecessors ([`Join`]) requires **both**
//! witnesses. Concurrency is not a hole in the discipline; the discipline is
//! purely about *order*, and a static join is just two required facts:
//!
//! ```
//! use quorum_types::causal::{Msg, Join, origin, Genesis};
//! enum A {}
//! enum B {}
//! enum M {}
//! let g = origin();
//! let (_, da) = Msg::<i32, A, Genesis>::new(1).deliver(g);   // da, db are concurrent
//! let (_, db) = Msg::<i32, B, Genesis>::new(2).deliver(g);   // (both after origin, Copy witness)
//! let (vm, _dm) = Join::<i32, M, A, B>::new(3).deliver(da, db); // M joins A and B
//! assert_eq!(vm, 3);
//! ```
//!
//! ## Where the types stop (the runtime seam) — the invariant-confluence boundary
//!
//! What is compile-time enforceable here is the **causal-order rule itself**:
//! *no event before its predecessors*. That rule is **coordination-free**
//! (I-confluent) — each node, holding witnesses, refuses an out-of-order delivery
//! locally, with no agreement required — which is exactly why the type system can
//! own it. What the types **cannot** own is the causal DAG's *shape*: in a real
//! system a message's predecessors arrive as a **vector clock in the wire bytes**,
//! discovered at receive time, not fixed as the `Pred`/`Id` markers of a type.
//! Lifting a runtime vector clock into a type parameter is the same wall the crate
//! hits at the network boundary (rung 5: a const-generic epoch cannot be minted
//! from wire bytes) and across processes (rung 7). So the split is clean and
//! familiar: **the discipline is a compile-time type (I-confluent, local); the
//! dependency graph is runtime data the deserialization boundary must supply.**
//! Types verify the ordering of a chain; the operator (here, the causal-broadcast
//! layer) chooses the graph.

use core::marker::PhantomData;

/// Phantom marker over type-level event id(s). The `fn() -> T` form keeps the
/// wrappers (`Delivered`/`Msg`/`Join`) covariant in `T` and unconditionally
/// `Send`/`Sync`/`Copy` — the markers carry no data, only identity.
type Phantom<T> = PhantomData<fn() -> T>;

/// The causal origin — the empty history. `Delivered<Genesis>` (via [`origin`]) is
/// the one witness a node holds before delivering anything. Uninhabited: it is a
/// pure type-level marker, never a value.
pub enum Genesis {}

/// **A causal-delivery witness** — evidence that event `Ev` was reached by
/// delivering a chain, *in order*, from [`origin`] to `Ev`. `Copy` because it is a
/// *fact*, not a linear resource: one delivered predecessor may enable arbitrarily
/// many successors (causal fan-out).
///
/// **What it does and does not prove.** You cannot fabricate a `Delivered<Ev>` from
/// nothing — the only constructors ([`origin`], [`Msg::deliver`], [`Join::deliver`])
/// each demand the predecessor's witness, so holding one proves a *declared* chain
/// to `Ev` was delivered in order. It does **not** prove the declared predecessors
/// are `Ev`'s *true* causal ones: [`Msg::new`] trusts its `Id`/`Pred` (a message may
/// name `Genesis` as predecessor and skip real ancestors). Assigning those markers
/// faithfully is the broadcast layer's job — the type enforces the chain you
/// *declare*, not the dependency graph reality (see the module's runtime-seam note).
/// This is weaker than [`consistency::Committed`](crate::consistency::Committed),
/// whose evidence is a real [`Quorum`](crate::membership::Quorum).
pub struct Delivered<Ev> {
    _ev: Phantom<Ev>,
}

// Copy/Clone for ALL `Ev`, not just `Ev: Copy`. A witness is a fact about the
// *history*, independent of any data in `Ev` (which is only ever a phantom
// marker) — so `#[derive]`, which would add a spurious `Ev: Copy` bound and make
// `Delivered<Genesis>` non-`Copy`, is deliberately not used here.
impl<Ev> Clone for Delivered<Ev> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<Ev> Copy for Delivered<Ev> {}

/// **A message in transit.** Payload `T`, type-level identity `Id`, and immediate
/// causal predecessor `Pred`. Move-only: delivered exactly once.
#[must_use = "a Msg is undelivered; deliver it in causal order or its effect is lost"]
pub struct Msg<T, Id, Pred> {
    value: T,
    _id: Phantom<(Id, Pred)>,
}

impl<T, Id, Pred> Msg<T, Id, Pred> {
    /// Form a message claiming identity `Id` and immediate predecessor `Pred`.
    /// (A real causal-broadcast layer assigns `Id`/`Pred` from the vector clock on
    /// the wire; see the module's runtime-seam note.)
    pub const fn new(value: T) -> Self {
        Msg { value, _id: PhantomData }
    }

    /// **Causal delivery.** Consume the message, *given the witness that its
    /// predecessor was delivered*, yielding the payload and a fresh witness that
    /// `Id` is now delivered. Out-of-order delivery does not typecheck: `_pred`
    /// must be exactly `Delivered<Pred>`.
    pub fn deliver(self, _pred: Delivered<Pred>) -> (T, Delivered<Id>) {
        (self.value, Delivered { _ev: PhantomData })
    }
}

/// **A causal merge point** with two immediate predecessors `P1` and `P2`.
/// Delivery requires *both* witnesses — a static join is just two required facts.
#[must_use = "a Join is undelivered; deliver it in causal order or its effect is lost"]
pub struct Join<T, Id, P1, P2> {
    value: T,
    _id: Phantom<(Id, P1, P2)>,
}

impl<T, Id, P1, P2> Join<T, Id, P1, P2> {
    /// Form a merge event depending on both `P1` and `P2`.
    pub const fn new(value: T) -> Self {
        Join { value, _id: PhantomData }
    }

    /// Deliver the merge, given witnesses for *both* predecessors.
    pub fn deliver(self, _p1: Delivered<P1>, _p2: Delivered<P2>) -> (T, Delivered<Id>) {
        (self.value, Delivered { _ev: PhantomData })
    }
}

/// The witness a node holds before delivering anything: the causal origin.
pub const fn origin() -> Delivered<Genesis> {
    Delivered { _ev: PhantomData }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum A {}
    enum B {}
    enum C {}
    enum M {}

    #[test]
    fn chain_delivers_in_order() {
        let g = origin();
        let (va, da) = Msg::<i32, A, Genesis>::new(1).deliver(g);
        let (vb, _db) = Msg::<i32, B, A>::new(2).deliver(da);
        assert_eq!((va, vb), (1, 2));
    }

    #[test]
    fn witness_is_copy_so_one_predecessor_fans_out() {
        // Delivered<A> is a fact, not a resource: deliver B and C, both after A,
        // reusing the same witness — a DAG, not just a chain.
        let g = origin();
        let (_, da) = Msg::<i32, A, Genesis>::new(1).deliver(g);
        let (vb, _db) = Msg::<i32, B, A>::new(2).deliver(da);
        let (vc, _dc) = Msg::<i32, C, A>::new(3).deliver(da); // da reused — Copy
        assert_eq!((vb, vc), (2, 3));
    }

    #[test]
    fn join_requires_both_predecessors() {
        let g = origin();
        let (_, da) = Msg::<i32, A, Genesis>::new(1).deliver(g);
        let (_, db) = Msg::<i32, B, Genesis>::new(2).deliver(g);
        let (vm, _dm) = Join::<i32, M, A, B>::new(9).deliver(da, db);
        assert_eq!(vm, 9);
    }
}

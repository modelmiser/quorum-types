//! FIFO (per-sender) broadcast — the **floor** of the delivery-ordering hierarchy.
//!
//! [`causal`](crate::causal) types the *causal* order (a message after every message it
//! causally depends on). This module types the weakest non-trivial delivery order beneath it:
//! **FIFO** — messages from *one* sender are delivered in the order that sender sent them, and
//! nothing is promised *across* senders. FIFO is the **source axis** at its floor; causal is its
//! refinement. The classical hierarchy (Hadzilacos–Toueg; Défago, Schiper & Urban 2004) is two
//! axes, not one chain: this rung and `causal` are the coordination-free (source) axis;
//! `total_order` is the orthogonal agreement axis.
//!
//! Two existing rungs *assume* FIFO channels as an untyped seam — [`chain`](crate::chain) (the
//! per-link FIFO order that carries linearizability down the chain) and
//! [`snapshot`](crate::snapshot) (Chandy–Lamport is orphan-safe only over FIFO channels). This
//! module is the type that *discharges* that assumption for a single sender's stream.
//!
//! ## The mechanism — a per-sender sequence, checked by const arithmetic
//!
//! * [`Delivered<S, N>`] is a **witness** — proof that sender `S`'s stream was delivered *in
//!   order* up through sequence number `N`. It is `Copy` because it proves **reachability** ("seq
//!   `N` was reached in order"), a fact — not a linear resource. It does **not** prove *at-most-once*
//!   delivery: holding one `Delivered<S, 1>` you may deliver two different `Msg<_, S, 2>` values,
//!   so the type owns *order*, not *no-duplication* (a separate reliability axis — see the seam).
//!   [`origin`] mints `Delivered<S, 0>` — nothing delivered yet.
//! * [`Msg<T, S, N>`] is a message in transit from sender `S` at sequence position `N`
//!   (positions start at 1). **Move-only** — each `Msg` *value* is delivered at most once (though,
//!   per the previous point, the *position* it carries is not thereby unique).
//! * [`Msg::deliver`] consumes the message *given `Delivered<S, PREV>`* and compiles **only** if
//!   `N == PREV + 1`, via an inline `const { assert! }`. A gap or reorder within a sender's stream
//!   fails **const-evaluation** (E0080) — the same arithmetic wall [`lockorder`](crate::lockorder)
//!   and [`staleness`](crate::staleness) use, and a *different* mechanism from `causal`'s
//!   type-identity unification (E0308). FIFO is arithmetic; causal is identity.
//!
//! ## A gap within a sender's stream is a compile error
//!
//! You have delivered `S` up through seq 1; seq 3 cannot be delivered next (2 is missing):
//!
//! ```compile_fail
//! use quorum_types::fifo::{Msg, origin};
//! enum S {}
//! let (_, d1) = Msg::<i32, S, 1>::new(10).deliver(origin::<S>()); // Delivered<S, 1>
//! let msg3 = Msg::<i32, S, 3>::new(30);
//! let _ = msg3.deliver(d1); // wants N == PREV + 1, i.e. 3 == 1 + 1 — const-eval fails (gap)
//! ```
//!
//! You also cannot **forge** a delivered-prefix witness to skip ahead — `Delivered`'s field is
//! private, so a fabricated `Delivered<S, 5>` (which would let you deliver seq 6 without 1–5)
//! does not construct outside this module:
//!
//! ```compile_fail
//! use quorum_types::fifo::Delivered;
//! use core::marker::PhantomData;
//! enum S {}
//! let forged: Delivered<S, 5> = Delivered { _s: PhantomData }; // private field — cannot construct
//! ```
//!
//! ## The happy path — deliver one sender's stream in order
//!
//! ```
//! use quorum_types::fifo::{Msg, origin};
//! enum S {}
//! let d0 = origin::<S>();                                   // Delivered<S, 0>
//! let (v1, d1) = Msg::<i32, S, 1>::new(10).deliver(d0);     // seq 1 after the origin
//! let (v2, d2) = Msg::<i32, S, 2>::new(20).deliver(d1);     // then seq 2
//! let (v3, _d3) = Msg::<i32, S, 3>::new(30).deliver(d2);    // then seq 3
//! assert_eq!((v1, v2, v3), (10, 20, 30));
//! ```
//!
//! ## Two senders are independent — that *is* FIFO (weaker than causal)
//!
//! FIFO constrains each sender's stream and says nothing across senders. Two senders' streams
//! interleave freely; no witness relates them. This is the rung's deliberate ceiling — the extra
//! cross-sender constraint is exactly what [`causal`](crate::causal) adds on top.
//!
//! ```
//! use quorum_types::fifo::{Msg, origin};
//! enum A {}
//! enum B {}
//! let (va1, _da1) = Msg::<i32, A, 1>::new(1).deliver(origin::<A>()); // A's stream and
//! let (vb1, _db1) = Msg::<i32, B, 1>::new(2).deliver(origin::<B>()); // B's stream are unrelated
//! assert_eq!((va1, vb1), (1, 2));
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! What is compile-time enforceable here is the **per-sender in-order rule** — and it is
//! enforceable precisely because FIFO is **coordination-free** (I-confluent): a receiver, holding
//! `Delivered<S, PREV>`, refuses seq `PREV + 2` locally, with no agreement. That is the same
//! reason `causal` and the CALM floor ([`crdt`](crate::crdt), [`calm`](crate::calm)) are
//! compile-time-ownable. What the types **cannot** own: that the sequence number stamped on a
//! message is its *true* send position, and that a sender does not skip or rewind its own counter.
//! Those numbers arrive in the wire bytes, assigned by the broadcast layer — the same
//! deserialization boundary `causal` names for its predecessor markers. Types verify the ordering
//! of a *declared* per-sender sequence; the transport supplies the numbers. Nor do the types own
//! *no-duplication* or a single delivery timeline: because the witness proves reachability (it is
//! `Copy`), the same position may be delivered by more than one `Msg`. Exactly-once delivery is an
//! orthogonal reliability property, not part of the FIFO *ordering* this rung types.

use core::marker::PhantomData;

/// Phantom marker over the sender identity. The `fn() -> S` form keeps [`Delivered`]/[`Msg`]
/// covariant in `S` and unconditionally `Send`/`Sync`/`Copy` — the marker carries no data.
type Phantom<S> = PhantomData<fn() -> S>;

/// **A FIFO-delivery witness** — evidence that sender `S`'s stream has been delivered *in order*
/// up through sequence number `N`. `Copy` because a delivered prefix is a *fact* about `S`'s
/// stream, independent of any data.
///
/// **What it does and does not prove.** Its only constructors are [`origin`] (`N == 0`, the empty
/// prefix) and [`Msg::deliver`] (which demands `Delivered<S, N-1>`), so holding `Delivered<S, N>`
/// proves `S`'s stream was delivered *contiguously* from 1 to `N`. It does **not** prove the
/// numbers reflect `S`'s true send order — that is the transport's job (see the runtime-seam note).
pub struct Delivered<S, const N: u64> {
    _s: Phantom<S>,
}

// Copy/Clone for ALL `S` and `N`, not gated on `S: Copy`. A witness is a fact about the stream,
// not about any data — `#[derive]` would add a spurious `S: Copy` bound, so it is written by hand.
impl<S, const N: u64> Clone for Delivered<S, N> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<S, const N: u64> Copy for Delivered<S, N> {}

/// **A message in transit** from sender `S` at sequence position `N` (positions start at 1).
/// Move-only: delivered exactly once.
#[must_use = "a Msg is undelivered; deliver it in per-sender sequence or its effect is lost"]
pub struct Msg<T, S, const N: u64> {
    value: T,
    _s: Phantom<S>,
}

impl<T, S, const N: u64> Msg<T, S, N> {
    /// Form a message from sender `S` claiming sequence position `N`. (A real broadcast layer
    /// assigns `N` from the sender's counter on the wire; see the module's runtime-seam note.)
    pub const fn new(value: T) -> Self {
        Msg { value, _s: PhantomData }
    }

    /// **FIFO delivery.** Consume the message *given the witness that `S`'s stream reached the
    /// immediately-preceding position `PREV`*, yielding the payload and a fresh
    /// `Delivered<S, N>`. Compiles **only** if `N == PREV + 1`: a gap or reorder within `S`'s
    /// stream fails the inline `const` assert after monomorphization (E0080).
    pub fn deliver<const PREV: u64>(self, _prev: Delivered<S, PREV>) -> (T, Delivered<S, N>) {
        const { assert!(N == PREV + 1, "FIFO violation: this sender's stream skipped or reordered a sequence number") }
        (self.value, Delivered { _s: PhantomData })
    }
}

impl<T, S> Msg<T, S, 1> {
    /// Convenience for the first message of a stream: deliver seq 1 given only the
    /// `Delivered<S, 0>` origin. Equivalent to [`deliver`](Msg::deliver) with `PREV == 0`.
    pub fn deliver_first(self, _origin: Delivered<S, 0>) -> (T, Delivered<S, 1>) {
        (self.value, Delivered { _s: PhantomData })
    }
}

/// The witness a receiver holds before delivering anything from sender `S`: the empty prefix,
/// `Delivered<S, 0>`.
pub const fn origin<S>() -> Delivered<S, 0> {
    Delivered { _s: PhantomData }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum A {}
    enum B {}

    #[test]
    fn stream_delivers_in_order() {
        let d0 = origin::<A>();
        let (v1, d1) = Msg::<i32, A, 1>::new(10).deliver(d0);
        let (v2, d2) = Msg::<i32, A, 2>::new(20).deliver(d1);
        let (v3, _d3) = Msg::<i32, A, 3>::new(30).deliver(d2);
        assert_eq!((v1, v2, v3), (10, 20, 30));
    }

    #[test]
    fn deliver_first_is_deliver_with_prev_zero() {
        let (v1, d1) = Msg::<i32, A, 1>::new(7).deliver_first(origin::<A>());
        let (v2, _d2) = Msg::<i32, A, 2>::new(8).deliver(d1);
        assert_eq!((v1, v2), (7, 8));
    }

    #[test]
    fn two_senders_are_independent() {
        // FIFO relates a sender to *itself*; A's stream and B's stream have no witness in common.
        let (va, _da) = Msg::<i32, A, 1>::new(1).deliver(origin::<A>());
        let (vb, _db) = Msg::<i32, B, 1>::new(2).deliver(origin::<B>());
        assert_eq!((va, vb), (1, 2));
    }

    #[test]
    fn witness_is_copy_within_a_stream_is_still_linear_by_position() {
        // The witness is Copy (a fact), but each position advances the type-level N, so a stream
        // is still a strict contiguous chain — Copy buys convenience, not the ability to skip.
        let d0 = origin::<A>();
        let (_v1, d1) = Msg::<i32, A, 1>::new(1).deliver(d0);
        let _also_d1 = d1; // Copy: the fact "A delivered through 1" can be named twice
        let (v2, _d2) = Msg::<i32, A, 2>::new(2).deliver(d1);
        assert_eq!(v2, 2);
    }
}

//! At-least-once delivery — **retransmit until acknowledged**, the runtime-witness dual of
//! `at_most_once`.
//!
//! `at_most_once` (and, for idempotent effects, [`crdt`](crate::crdt)) ensure a message's effect
//! fires *no more than once*. This module ensures it fires *no fewer than once*: the sender keeps a
//! message `Pending` and retransmits it until the receiver returns an [`Ack`], the only key that
//! lets the sender retire it. Compose the two — at-least-once delivery of an at-most-once effect —
//! and the effect fires **exactly once**: exactly-once *processing*, the achievable cousin of the
//! impossible exactly-once *delivery* (which is the seam — you cannot type "arrived exactly once",
//! only "applied exactly once").
//!
//! This is why the rung is a runtime **witness** while `at_most_once` is structural: at-most-once
//! is local (move semantics, no agreement), but at-least-once needs a **round trip** — evidence
//! from the *other* side that a copy landed. That round trip is the coordination the type system
//! cannot manufacture, so it enters as a witness — the same CALM boundary the structural/witness
//! split tracks throughout the crate.
//!
//! ## The mechanism — a pending message, retired only by its ack
//!
//! * [`Pending<T, Id>`] is a sent-but-unacked message with a type-level identity `Id`. Move-only
//!   and `#[must_use]`: it must be resent or retired, not silently dropped.
//! * [`resend`](Pending::resend) emits a [`WireCopy<T, Id>`] (borrowing the pending, so you may
//!   resend as many times as needed) — modelling retransmission.
//! * [`WireCopy::deliver`] hands the payload to the receiver and mints an [`Ack<Id>`] — the witness
//!   that *this* message was delivered. A `WireCopy` that is dropped instead models a lost packet.
//! * [`retire`](Pending::retire) consumes the pending **only given `Ack<Id>` for the same `Id`**.
//!   The ack is unforgeable (private field) and identity-tied, so an ack for a message of one `Id`
//!   cannot retire a message of a *different* `Id` (that is a type error, not a runtime mix-up).
//!   What `Id` distinguishes is a *class* of message, not a value instance: giving each logical
//!   message its own `Id` is a caller obligation (see the seam), the same shape as `byzantine`'s
//!   fault budget and `calm`'s labels.
//!
//! ## You cannot retire without a genuine ack
//!
//! ```compile_fail
//! use quorum_types::at_least_once::Ack;
//! use core::marker::PhantomData;
//! enum M {}
//! let forged: Ack<M> = Ack { _id: PhantomData, _priv: () }; // private fields — cannot construct
//! ```
//!
//! ## An ack for one `Id` cannot retire a message of another `Id`
//!
//! ```compile_fail
//! use quorum_types::at_least_once::send;
//! enum X {}
//! enum Y {}
//! let px = send::<X, _>(1_i32);
//! let py = send::<Y, _>(2_i32);
//! let (_v, ack_y) = py.resend().deliver();   // Ack<Y>
//! let _ = px.retire(ack_y);                   // px wants Ack<X> — Ack<Y> does not unify
//! ```
//!
//! ## The happy path — send, (maybe lose,) retransmit, deliver, retire
//!
//! ```
//! use quorum_types::at_least_once::send;
//! enum M {}
//! let pending = send::<M, _>(100_i32);
//! let _lost = pending.resend();                // first copy is dropped — a lost packet
//! let (delivered, ack) = pending.resend().deliver(); // retransmit; this copy lands
//! assert_eq!(delivered, 100);
//! let recovered = pending.retire(ack);         // the ack is the only key that retires the pending
//! assert_eq!(recovered, 100);
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! The types own "**cannot retire without a matching ack**" — so a sender that respects them keeps
//! retransmitting until it has delivery evidence, which is exactly at-least-once. What they cannot
//! own is **liveness**: if *every* copy is lost, no ack is ever minted and the pending is never
//! retired — the type system can require the witness but cannot force it to arrive. Nor can it make
//! the receiver actually apply the payload. Nor does it enforce **`Id` discipline**: `Id` names a
//! *class* of message, not a value instance, and `send` accepts any marker, so giving each logical
//! message a fresh `Id` — and not reusing one across two live pendings — is a caller obligation (as
//! `byzantine`'s `f` and `calm`'s labels are). And note the honest limit of the whole axis: this
//! delivers a message *at least once*, so downstream must be idempotent ([`crdt`](crate::crdt)) or
//! single-use (`at_most_once`) for the *effect* to be once — exactly-once *delivery* itself is not
//! representable, only exactly-once *processing* by composition.

use core::marker::PhantomData;

/// Phantom marker over the message identity. The `fn() -> Id` form keeps the wrappers covariant in
/// `Id` and unconditionally `Send`/`Sync` — the marker carries no data, only identity. `Id` is meant
/// to be a distinct **nominal** marker (a zero-variant `enum`), not a lifetime-parametric type:
/// covariance is harmless for nominal markers but could let subtyping blur two lifetime-bearing
/// `Id`s, which is outside this rung's intended use.
type Phantom<Id> = PhantomData<fn() -> Id>;

/// A sent-but-unacknowledged message with type-level identity `Id`. Move-only and `#[must_use]`:
/// resend it or retire it, never silently drop it (dropping it is giving up on delivery).
#[must_use = "a Pending message is unacknowledged; resend it until delivered, then retire it with the ack"]
pub struct Pending<T, Id> {
    payload: T,
    _id: Phantom<Id>,
}

impl<T: Clone, Id> Pending<T, Id> {
    /// Emit a copy of the message onto the wire (a retransmission). Borrows `self`, so the sender
    /// keeps the pending and may resend again if this copy is lost.
    pub fn resend(&self) -> WireCopy<T, Id> {
        WireCopy { payload: self.payload.clone(), _id: PhantomData }
    }
}

impl<T, Id> Pending<T, Id> {
    /// Retire the pending, consuming it — permitted **only** with an [`Ack<Id>`] for this message's
    /// own `Id`. Returns the buffered payload (now safe to discard). There is no other way to
    /// finish: no ack, no retire.
    pub fn retire(self, _ack: Ack<Id>) -> T {
        self.payload
    }
}

/// A copy of a [`Pending`] message in flight. Delivering it mints the [`Ack`]; dropping it models a
/// lost packet (the sender must [`resend`](Pending::resend)).
#[must_use = "a WireCopy is in flight; deliver it to ack, or drop it to model a lost packet"]
pub struct WireCopy<T, Id> {
    payload: T,
    _id: Phantom<Id>,
}

impl<T, Id> WireCopy<T, Id> {
    /// Receiver-side delivery: hand the payload to the application and mint the [`Ack<Id>`] that
    /// lets the sender retire the corresponding pending.
    pub fn deliver(self) -> (T, Ack<Id>) {
        (self.payload, Ack { _id: PhantomData, _priv: () })
    }
}

/// **A delivery acknowledgement** for messages of identity `Id` — evidence that a copy was
/// delivered at the receiver. Unforgeable (private fields) and identity-tied: it is minted only by
/// [`WireCopy::deliver`], and it retires **only** a pending of the same `Id` (an ack of one `Id`
/// will not retire a pending of another — E0308). Move-only: one delivery yields one ack, which
/// retires one pending. Note `Id` distinguishes a *class*, not a value instance — the caller is
/// responsible for a fresh `Id` per logical message (see the module's seam note).
pub struct Ack<Id> {
    // `_id` ties the ack to its message; `_priv` (a real private field, not a comment) blocks
    // construction outside this module, so an ack is obtainable only through `deliver`.
    _id: Phantom<Id>,
    _priv: (),
}

/// Begin sending `payload` under the fresh message identity `Id`, yielding the [`Pending`] the
/// sender retransmits until acked. `Id` is a type-level marker chosen per logical message.
pub fn send<Id, T>(payload: T) -> Pending<T, Id> {
    Pending { payload, _id: PhantomData }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum M {}

    #[test]
    fn deliver_then_retire() {
        let pending = send::<M, _>(42_i32);
        let (delivered, ack) = pending.resend().deliver();
        assert_eq!(delivered, 42);
        assert_eq!(pending.retire(ack), 42);
    }

    #[test]
    fn retransmit_after_loss_still_delivers() {
        // Two copies are lost before one lands — the sender keeps the pending and resends.
        let pending = send::<M, _>(7_i32);
        let _lost1 = pending.resend();
        let _lost2 = pending.resend();
        let (v, ack) = pending.resend().deliver();
        assert_eq!(v, 7);
        assert_eq!(pending.retire(ack), 7, "at-least-once: retransmit outlasts the losses");
    }

    #[test]
    fn many_copies_deliver_but_one_ack_retires_once() {
        // The transport may deliver several copies (each mints its own ack); the sender retires
        // exactly once with one of them. (The at-most-once *effect* is `at_most_once`'s job.)
        let pending = send::<M, _>(9_i32);
        let (_v1, _ack1) = pending.resend().deliver();
        let (_v2, ack2) = pending.resend().deliver();
        assert_eq!(pending.retire(ack2), 9);
    }
}

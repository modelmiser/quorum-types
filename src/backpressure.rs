//! Receiver backpressure — bounding the **receiver's** buffer, by receiver-minted credit.
//!
//! This is the runtime-**witness** rung of the flow-control (occupancy) axis; its structural dual
//! is `send_window`. A `send_window` bounds the
//! *sender's own* in-flight work — a self-imposed limit needing no coordination. But it says
//! nothing about the **receiver**: a sender obeying its own window can still overrun a slow
//! receiver's buffer. Not overflowing the *peer* is not a property the sender can know locally —
//! it needs evidence from the receiver — so it lands on the coordinated, non-CALM side of the
//! crate's recurring cut.
//!
//! The textbook mechanism is **credit-based flow control** (equivalently TCP's advertised receive
//! window, `rwnd`): the receiver tells the sender how much buffer is free, and the sender may
//! send only that much. Here that advertisement is a linear, unforgeable [`Credit`] — the sender
//! can transmit only by **spending** a credit the receiver minted, so the number of messages
//! outstanding toward the receiver can never exceed the buffer the receiver actually advertised.
//!
//! ## The witness: a credit is receiver-minted evidence of free buffer
//!
//! A [`Receiver<CAP>`] admits at most `CAP` outstanding (granted-but-not-yet-drained) messages.
//! [`grant`](Receiver::grant) reserves one buffer slot (`outstanding += 1`) and mints a
//! [`Credit<CAP>`]; [`accept`](Receiver::accept) drains a delivered message and frees its slot
//! (`outstanding -= 1`). Because a [`Credit`] is unforgeable (private field, minted only inside
//! [`grant`](Receiver::grant)) and single-use (move-only, consumed by
//! [`spend`](Credit::spend)), the sender holds at most as many send-rights as the receiver had
//! free buffer. So **outstanding ≤ CAP → the receiver never overflows** — within one receiver's
//! own bookkeeping, and under the two seam axioms below that scope it — a bound carried
//! by evidence that crossed the network boundary, exactly the kind of witness
//! [`at_least_once`](crate::at_least_once)'s `Ack` is.
//!
//! Note the **direction** that distinguishes this credit from that ack. An
//! [`Ack`](crate::at_least_once::Ack) flows back *after* delivery and says "you may **stop**
//! retransmitting" (a liveness/dedup key). A [`Credit`] flows *before* the send and says "you may
//! **proceed**" (a receiver-buffer safety key). Opposite ends of the message, opposite guarantees.
//!
//! ## Where the types stop (the runtime seam)
//!
//! * **`grant` honestly reflects real buffer state.** The type enforces `outstanding ≤ CAP`
//!   *within* the [`Receiver`]'s bookkeeping, but that a granted credit corresponds to genuinely
//!   free memory on the receiving host is a **declared axiom** — the same shape as
//!   [`byzantine`](crate::byzantine)'s fault budget `f` or [`at_least_once`](crate::at_least_once)'s
//!   trusted ack. The types make the credit unforgeable; they do not audit the receiver's honesty.
//! * **One receiver per buffer — `CAP` is a class, not an instance.** A [`Credit<CAP>`] unifies
//!   with any [`Receiver<CAP>`], so two receivers sharing `CAP` could cross-accept. `CAP` is a
//!   type-level **class**, not a value identity (the limit [`at_least_once`](crate::at_least_once)'s
//!   `Ack<Id>` also documents). One [`Receiver`] per logical buffer is a **caller obligation**.
//! * **Liveness.** A [`Receiver`] that never [`grant`](Receiver::grant)s — because its buffer
//!   stays full — starves the sender. Backpressure *is* that refusal; whether the receiver ever
//!   drains and grants again is a liveness property outside these types (the honest limit, as
//!   `at_least_once`'s is "every copy may be lost").
//!
//! ## Composing with `send_window`
//!
//! Real flow control runs **both** rungs and sends the minimum: `in_flight ≤ min(local send
//! window, outstanding receiver credits)` — TCP's `min(cwnd-style self-limit, rwnd)`. The
//! structural window caps what the sender will *hold*; the witness credit caps what the receiver
//! will *accept*. Neither subsumes the other: drop the window and the sender can exhaust itself;
//! drop the credit and the sender can overrun the receiver.
//!
//! ## A credit cannot be forged — unforgeability
//!
//! ```compile_fail
//! use quorum_types::backpressure::Credit;
//! let _ = Credit::<4> { _priv: () }; // private field — only Receiver::grant mints a Credit
//! ```
//!
//! ## A credit is spent once — linearity
//!
//! ```compile_fail
//! use quorum_types::backpressure::Receiver;
//! let mut rx = Receiver::<4>::open();
//! let credit = rx.grant().unwrap();
//! let _w1 = credit.spend("a");
//! let _w2 = credit.spend("b"); // `credit` already moved — one credit authorizes one send
//! ```
//!
//! ## A credit is tied to its buffer size — the size is in the type
//!
//! ```compile_fail
//! use quorum_types::backpressure::Receiver;
//! let mut small = Receiver::<2>::open();
//! let mut big = Receiver::<4>::open();
//! let credit = small.grant().unwrap(); // Credit<2>
//! let wire = credit.spend("x");        // Wire<_, 2>
//! let _ = big.accept(wire); // CAP: 4 vs 2 do not unify — E0308
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::backpressure::Receiver;
//!
//! // A receiver with room for 2 outstanding messages.
//! let mut rx = Receiver::<2>::open();
//! assert_eq!(rx.free(), 2);
//!
//! // The receiver advertises two credits; its free buffer drops to zero.
//! let c1 = rx.grant().unwrap();
//! let c2 = rx.grant().unwrap();
//! assert_eq!(rx.free(), 0);
//!
//! // Buffer full: the receiver withholds credit — this is the backpressure.
//! assert!(rx.grant().is_none());
//!
//! // The sender may transmit only by spending a receiver-minted credit.
//! let w1 = c1.spend("hello");
//! let w2 = c2.spend("world");
//!
//! // Delivery drains the buffer, freeing capacity to grant again.
//! assert_eq!(rx.accept(w1), "hello");
//! assert_eq!(rx.free(), 1);
//! let c3 = rx.grant().unwrap(); // room re-opened
//! assert_eq!(rx.accept(w2), "world");
//! let _ = c3.spend("again");
//! ```

/// A **receiving endpoint** that admits at most `CAP` outstanding (granted-but-not-yet-drained)
/// messages at once. Its `outstanding` counter is the number of live send-rights it has issued;
/// `free == CAP - outstanding` is the buffer room it may still advertise.
///
/// Open one with [`open`](Self::open). Credits it mints are the only way to send to it, so it is
/// the sole authority over its own occupancy — the witness at the heart of this rung.
#[derive(Debug)]
pub struct Receiver<const CAP: u64> {
    outstanding: u64,
}

/// A single-send **authorization** minted by a [`Receiver`]. Unforgeable (private field, minted
/// only by [`Receiver::grant`]) and move-only (no `Copy`/`Clone`), so it authorizes **exactly
/// one** send: a duplicate transmission needs a fresh credit, which only the receiver can issue.
/// Holding a `Credit<CAP>` is evidence the receiver had a free buffer slot.
#[derive(Debug)]
#[must_use = "a Credit is send permission the receiver reserved buffer for; spend it or drop it to release"]
pub struct Credit<const CAP: u64> {
    _priv: (),
}

/// An in-flight message carrying its admission — produced by [`Credit::spend`], consumed by
/// [`Receiver::accept`]. Move-only: it is one occupant of the receiver's buffer.
#[derive(Debug)]
#[must_use = "a Wire is an outstanding message; accept it at the receiver to free its buffer slot"]
pub struct Wire<T, const CAP: u64> {
    payload: T,
}

impl<const CAP: u64> Receiver<CAP> {
    /// Open a receiver with an empty buffer — all `CAP` slots free.
    pub const fn open() -> Self {
        Receiver { outstanding: 0 }
    }

    /// Buffer room the receiver may still advertise (`CAP - outstanding`).
    pub const fn free(&self) -> u64 {
        CAP - self.outstanding
    }

    /// Outstanding messages: granted credits not yet [`accept`](Self::accept)ed.
    pub const fn outstanding(&self) -> u64 {
        self.outstanding
    }

    /// Advertise one buffer slot to the sender, minting a [`Credit`] and reserving the slot
    /// (`outstanding += 1`). Returns `None` when the buffer is full (`outstanding == CAP`) — the
    /// receiver **withholds** credit, and that withholding *is* the backpressure. Because only
    /// this method constructs a [`Credit`], the sender can never manufacture send-rights the
    /// receiver did not reserve buffer for.
    pub fn grant(&mut self) -> Option<Credit<CAP>> {
        if self.outstanding < CAP {
            self.outstanding += 1;
            Some(Credit { _priv: () })
        } else {
            None
        }
    }

    /// Drain a delivered message, freeing its buffer slot (`outstanding -= 1`) so a fresh credit
    /// can be granted, and returning the payload. Consuming the [`Wire`] linearly is what pairs
    /// each drain with exactly one prior [`grant`](Self::grant)/[`spend`](Credit::spend).
    ///
    /// (The `saturating_sub` does **not** make cross-accept safe — a [`Wire`] misrouted to a
    /// *different* [`Receiver`] of the same `CAP` still under-counts that receiver and can drive
    /// it to over-grant and overflow. The real protection is the one-receiver-per-buffer obligation
    /// in the seam docs; saturating merely avoids an underflow panic if that obligation is broken.
    /// With one receiver per buffer, a [`Wire<_, CAP>`] exists only because *this* receiver granted
    /// its credit, so `outstanding > 0` whenever a wire is in hand.)
    pub fn accept<T>(&mut self, wire: Wire<T, CAP>) -> T {
        self.outstanding = self.outstanding.saturating_sub(1);
        wire.payload
    }
}

impl<const CAP: u64> Credit<CAP> {
    /// Spend this authorization to put one message on the wire. Consumes the credit (single-use),
    /// so the resulting [`Wire`] is the unique occupant that credit paid for.
    pub fn spend<T>(self, payload: T) -> Wire<T, CAP> {
        // `self` (the authorization) is consumed by move here — a duplicate send needs a fresh
        // credit, which only the receiver can mint.
        Wire { payload }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_bounds_outstanding_and_withholds_when_full() {
        let mut rx = Receiver::<2>::open();
        assert_eq!(rx.free(), 2);
        assert_eq!(rx.outstanding(), 0);

        let c1 = rx.grant().expect("room");
        let c2 = rx.grant().expect("room");
        assert_eq!(rx.free(), 0);
        assert_eq!(rx.outstanding(), 2);

        // Full: the receiver withholds credit — backpressure.
        assert!(rx.grant().is_none(), "a full buffer grants no credit");

        // Spend and deliver to free capacity.
        let w1 = c1.spend(10);
        let w2 = c2.spend(20);
        assert_eq!(rx.accept(w1), 10);
        assert_eq!(rx.free(), 1, "accept frees a buffer slot");
        assert_eq!(rx.accept(w2), 20);
        assert_eq!(rx.free(), 2);
    }

    #[test]
    fn credit_regenerates_after_delivery() {
        let mut rx = Receiver::<1>::open();
        let c1 = rx.grant().expect("room");
        assert!(rx.grant().is_none(), "single slot already reserved");
        let w1 = c1.spend("first");
        assert_eq!(rx.accept(w1), "first");
        // The one slot re-opened.
        let c2 = rx.grant().expect("slot freed by accept");
        let _ = c2.spend("second");
    }

    /// Spot-check `outstanding ≤ CAP` (the receiver never overflows) along one adversarial
    /// schedule of grants, withheld grants, spends, and accepts. The guarantee is that a `Wire`
    /// exists only by spending a receiver-minted `Credit`; this fixes one schedule and asserts
    /// the bound after each step.
    #[test]
    fn receiver_never_overflows_along_one_schedule() {
        const CAP: u64 = 3;
        let mut rx = Receiver::<CAP>::open();
        let mut inflight: Vec<Wire<u64, CAP>> = Vec::new();
        let mut next: u64 = 0;

        // grant/withheld-grant/deliver interleaving.
        for grant_step in [true, true, true, true, false, false, true, false] {
            if grant_step {
                if let Some(credit) = rx.grant() {
                    inflight.push(credit.spend(next));
                    next += 1;
                }
                // else: withheld — the receiver is full, no send happens.
            } else if !inflight.is_empty() {
                let wire = inflight.remove(0);
                rx.accept(wire);
            }
            assert!(rx.outstanding() <= CAP, "receiver overflow: {} > {CAP}", rx.outstanding());
            assert_eq!(rx.outstanding(), inflight.len() as u64, "a credit was forged or lost");
        }

        // Drain the rest so no #[must_use] wire is dropped.
        for wire in inflight {
            rx.accept(wire);
        }
        assert_eq!(rx.free(), CAP);
    }
}

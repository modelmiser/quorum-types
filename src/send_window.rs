//! Send-window flow control — bounding your **own** in-flight work, coordination-free.
//!
//! This is the **structural** rung of the flow-control (occupancy) axis; its runtime-witness
//! dual is `backpressure`. The ordering rungs ([`fifo`](crate::fifo),
//! [`total_order`](crate::total_order)) type *when* a message is delivered and the count rungs
//! ([`at_most_once`](crate::at_most_once), [`at_least_once`](crate::at_least_once)) type *how
//! many times* its effect fires; this axis types *how many messages may be in flight at once*.
//!
//! A sender that transmits without bound can exhaust its **own** resources — its retransmit
//! buffer, its file descriptors, its memory for un-acked messages. The classic fix is a
//! **send window**: allow at most `N` outstanding (sent-but-not-yet-completed) messages, and
//! refuse to send a further one until an outstanding one completes. That bound is *self-imposed*
//! and needs no coordination — the sender enforces it with purely local bookkeeping — which is
//! exactly why it lands on the structural, CALM-free side of the crate's recurring cut.
//!
//! ## Occupancy, not volume — how this differs from [`escrow`](crate::escrow)
//!
//! [`escrow`](crate::escrow) also hands out a bounded quantity as linear tokens, but the
//! quantity it bounds is *cumulative*: a [`Reservation`](crate::escrow::Reservation) is drawn
//! **down** monotonically (split/spend conserve a total that only ever decreases within a tree).
//! A send window bounds an *instantaneous* quantity: a [`Slot`] is consumed by
//! [`send`](Window::send) and **regenerated** by [`complete`](Window::complete). It is a
//! **semaphore**, not a budget — capacity returns when work finishes. Escrow answers "how much
//! may I ever spend?"; the window answers "how many may I have outstanding right now?".
//!
//! ## The load-bearing invariant: at most `N` slots exist
//!
//! A [`Slot<S, N>`] is a **linear** (move-only, no `Copy`/`Clone`) token, unforgeable outside
//! this module (private fields). It is minted only by [`send`](Window::send) — which decrements
//! the window's `free` — and destroyed only by [`complete`](Window::complete) — which increments
//! it. So across a window and its live slots the sum `free + (live slots) == N` is conserved,
//! and therefore — **within one window value** — **at most `N` messages are outstanding at any
//! instant**, by construction. A `Slot` *is* the right to have one message in flight; for a given
//! window there are never more than `N` of them. (Two windows sharing a brand `S` are the disclosed
//! exception — see the seam below; that is why each logical stream needs its own `S`.)
//!
//! ## Where the types stop (the runtime seam)
//!
//! Two boundaries the types do **not** cross:
//!
//! * **One window per brand `S`.** The brand `S` ties a [`Slot`] to a window *type*, not a
//!   window *value*: [`complete`](Window::complete) accepts any `Slot<S, N>`, so two windows
//!   sharing the same `S` could cross-complete and drive `free` above `N`. `S` is a type-level
//!   **class**, not an instance identity (the same limit [`at_least_once`](crate::at_least_once)'s
//!   `Ack<Id>` documents). Giving each logical stream a fresh `S` — so its slots unify only with
//!   its own window — is a **caller obligation**. Within one window threaded linearly, the bound
//!   holds by construction.
//! * **This bounds *your* in-flight, not the receiver's buffer.** A send window protects the
//!   *sender*. It says nothing about whether the *receiver* has room — that guarantee needs
//!   evidence from the receiver and is `backpressure`'s job. Real flow
//!   control runs both and sends the minimum of the two windows.
//!
//! ## A slot is spent once — linearity
//!
//! ```compile_fail
//! use quorum_types::send_window::{open, Window};
//! enum S {}
//! let w: Window<S, 4> = open();
//! let (slot, w) = w.send().unwrap();
//! let w = w.complete(slot);
//! let _ = w.complete(slot); // `slot` already moved — a slot completes once
//! ```
//!
//! ## A slot cannot be forged — unforgeability
//!
//! ```compile_fail
//! use quorum_types::send_window::Slot;
//! enum S {}
//! let _ = Slot::<S, 4> { _brand: core::marker::PhantomData, _priv: () }; // private fields
//! ```
//!
//! ## Windows of different sizes do not mix — the size is in the type
//!
//! ```compile_fail
//! use quorum_types::send_window::{open, Window};
//! enum S {}
//! let small: Window<S, 2> = open();
//! let big: Window<S, 4> = open();
//! let (slot, _small) = small.send().unwrap(); // slot: Slot<S, 2>
//! let _ = big.complete(slot); // N: 4 vs 2 do not unify — E0308
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::send_window::{open, Window};
//! enum Link {} // a fresh brand for this one stream
//!
//! // A send window of 2 outstanding messages.
//! let w: Window<Link, 2> = open();
//! assert_eq!(w.free(), 2);
//!
//! // Send two — the window fills.
//! let (s1, w) = w.send().unwrap();
//! let (s2, w) = w.send().unwrap();
//! assert_eq!(w.free(), 0);
//! assert_eq!(w.in_flight(), 2);
//!
//! // A third send is refused locally — this *is* backpressure, no coordination needed.
//! let w = w.send().unwrap_err();
//! assert_eq!(w.free(), 0);
//!
//! // Complete one; capacity regenerates and a send succeeds again.
//! let w = w.complete(s1);
//! assert_eq!(w.free(), 1);
//! let (_s3, w) = w.send().unwrap();
//! assert_eq!(w.in_flight(), 2);
//! let _ = (s2, w);
//! ```

use core::marker::PhantomData;

/// Invariant marker: a window and its slots are branded by a caller-chosen type `S` so that
/// slots unify only with a window of the same brand. Covariant, zero-sized, sends nothing.
type Brand<S> = PhantomData<fn() -> S>;

/// A sender's **send window**: at most `N` messages may be outstanding on stream `S` at once.
///
/// Move-only (it is threaded through [`send`](Self::send)/[`complete`](Self::complete)); `free`
/// is how many further messages may be sent before one must complete. Open one with
/// [`open`]. The bound is self-imposed and coordination-free — see the module docs.
#[must_use = "a Window is the right to send; dropping it forfeits the stream's outstanding budget"]
pub struct Window<S, const N: u64> {
    free: u64,
    _brand: Brand<S>,
}

// Debug for all `S` — a window is bookkeeping about a stream, not about `S`. `#[derive(Debug)]`
// would add a spurious `S: Debug` bound (empty brand enums are not `Debug`), so it is by hand.
impl<S, const N: u64> core::fmt::Debug for Window<S, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Window").field("free", &self.free).field("cap", &N).finish()
    }
}

/// One **in-flight slot** — a linear token proving the sender reserved capacity for exactly one
/// outstanding message on stream `S`. Unforgeable (private fields, minted only by
/// [`send`](Window::send)) and move-only (no `Copy`/`Clone`), so it cannot be duplicated to
/// exceed the window: there are never more than `N` slots.
#[must_use = "a Slot is one outstanding message; complete it (or the window never regenerates)"]
pub struct Slot<S, const N: u64> {
    _brand: Brand<S>,
    _priv: (),
}

// Debug for all `S` (see the note on `Window` above — avoids a spurious `S: Debug` bound).
impl<S, const N: u64> core::fmt::Debug for Slot<S, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Slot").field("cap", &N).finish()
    }
}

/// Open a fresh send window of `N` free slots for a stream branded `S`.
///
/// Like [`escrow::Reservation::grant`](crate::escrow::Reservation::grant) this is the root where
/// the capacity `N` is *asserted*; it can be called more than once, and giving each logical
/// stream its own brand `S` is what keeps their slots from mixing (see the module seam docs).
pub fn open<S, const N: u64>() -> Window<S, N> {
    Window { free: N, _brand: PhantomData }
}

impl<S, const N: u64> Window<S, N> {
    /// How many further messages may be sent before one must [`complete`](Self::complete).
    pub const fn free(&self) -> u64 {
        self.free
    }

    /// How many messages are currently outstanding (`N - free`).
    pub const fn in_flight(&self) -> u64 {
        N - self.free
    }

    /// Reserve a slot for one outstanding message. Consumes the window and returns a [`Slot`]
    /// plus the reduced window.
    ///
    /// # Errors
    /// Returns `Err(self)` — the window handed back unchanged, no slot minted — when it is already
    /// full (`free == 0`). That refusal is the **local backpressure signal**, computed from the
    /// sender's own state with no coordination: getting room means completing an outstanding
    /// message, not asking anyone.
    pub fn send(self) -> Result<(Slot<S, N>, Self), Self> {
        if self.free > 0 {
            let slot = Slot { _brand: PhantomData, _priv: () };
            let next = Window { free: self.free - 1, _brand: PhantomData };
            Ok((slot, next))
        } else {
            Err(self)
        }
    }

    /// Complete an outstanding message: consume its [`Slot`] and return the freed capacity to the
    /// window (`free + 1`). This is the **regeneration** that makes a window a semaphore rather
    /// than a drained budget. Consuming the slot linearly is what pairs each completion with
    /// exactly one prior [`send`](Self::send).
    ///
    /// # Panics
    /// Debug builds assert `free < N` before regenerating. That holds for any *single* window used
    /// linearly (a `Slot` exists only because a matching `send` decremented `free`). It can fail
    /// only if the one-window-per-brand-`S` obligation (see the seam docs) is broken by completing
    /// a slot minted by a *different* window of the same brand — in which case debug catches it
    /// loudly here, while release would instead wrap `free` past `N` and make
    /// [`in_flight`](Self::in_flight) return garbage. So: one window per brand.
    pub fn complete(self, slot: Slot<S, N>) -> Self {
        let Slot { .. } = slot; // consume the slot linearly; its existence proved one send
        debug_assert!(
            self.free < N,
            "send_window: completed more slots than were sent — the one-window-per-brand-S \
             obligation (see module seam docs) was violated"
        );
        Window { free: self.free + 1, _brand: PhantomData }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum S {}

    #[test]
    fn window_bounds_outstanding_and_refuses_when_full() {
        let w: Window<S, 2> = open();
        assert_eq!(w.free(), 2);
        assert_eq!(w.in_flight(), 0);

        let (s1, w) = w.send().expect("free");
        let (s2, w) = w.send().expect("free");
        assert_eq!(w.free(), 0);
        assert_eq!(w.in_flight(), 2);

        // Full: a further send is refused and hands the window back intact.
        let w = w.send().expect_err("window full");
        assert_eq!(w.free(), 0, "a refused send leaves the window untouched");

        // Keep the slots alive to the end (they are #[must_use]).
        let w = w.complete(s1);
        let w = w.complete(s2);
        assert_eq!(w.free(), 2);
    }

    #[test]
    fn complete_regenerates_capacity() {
        let w: Window<S, 1> = open();
        let (s1, w) = w.send().expect("free");
        assert_eq!(w.free(), 0);
        // Regenerate, then reuse the capacity.
        let w = w.complete(s1);
        assert_eq!(w.free(), 1, "complete returns capacity — a semaphore, not a drained budget");
        let (s2, w) = w.send().expect("regenerated");
        assert_eq!(w.in_flight(), 1);
        let _ = w.complete(s2);
    }

    /// Spot-check the conservation `free + live_slots == N` along one adversarial schedule of
    /// interleaved sends, refused sends, and completions. The guarantee is structural (a slot is
    /// linear and unforgeable); this fixes one schedule and asserts the bound after each step.
    #[test]
    fn occupancy_never_exceeds_n_along_one_schedule() {
        const N: u64 = 3;
        let mut w: Window<S, N> = open();
        let mut live: Vec<Slot<S, N>> = Vec::new();

        // Deterministic "adversarial" mix: send until full, over-send (refused), drain, refill.
        for step in [true, true, true, true, false, false, true] {
            if step {
                match w.send() {
                    Ok((slot, next)) => {
                        live.push(slot);
                        w = next;
                    }
                    Err(unchanged) => w = unchanged, // full — refused locally
                }
            } else if let Some(slot) = live.pop() {
                w = w.complete(slot);
            }
            // Invariant after every step: nothing exceeded the window.
            assert!(w.in_flight() <= N, "occupancy {} exceeded window {N}", w.in_flight());
            assert_eq!(w.free() + live.len() as u64, N, "a slot was created or destroyed");
        }

        // Drain the rest so no #[must_use] slot is dropped.
        while let Some(slot) = live.pop() {
            w = w.complete(slot);
        }
        assert_eq!(w.free(), N);
    }
}

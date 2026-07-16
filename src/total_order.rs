//! Total-order (atomic) broadcast — the **agreement axis** of delivery ordering.
//!
//! `fifo` and [`causal`](crate::causal) are the *source axis*: they order a
//! sender's messages relative to that sender, and they are **coordination-free** — a receiver
//! decides delivery locally, holding only witnesses, with no agreement. This module types the
//! orthogonal *agreement axis*: **total order** — *all* correct processes deliver *all* messages
//! in **one** common order. That is a fundamentally different kind of guarantee. It is not
//! locally decidable: total-order broadcast is **equivalent to consensus** (Chandra & Toueg 1996;
//! Défago, Schiper & Urban, ACM CSUR 2004), so the global order can only come from a trusted
//! ordering authority — a sequencer, a leader, a consensus instance. That is exactly why this
//! rung is a **runtime witness** (species-2) and `fifo`/`causal` are structural (species-1): the
//! structural/witness split *coincides with* the coordination-free (CALM) boundary the
//! [`calm`](crate::calm) and [`crdt`](crate::crdt) modules track. Total order is a witness
//! **because** it is not I-confluent.
//!
//! ## The mechanism — a sequencer mints the order; a linear cursor consumes it
//!
//! * [`Sequencer`] is the single ordering authority (model of the leader / consensus instance).
//!   It is the **only** minter of [`Ordered`] messages: [`assign`](Sequencer::assign) stamps the
//!   next global position onto a payload. One sequencer ⇒ one global order.
//! * [`Ordered<T>`] is a message that has been assigned a global position. Its fields are
//!   **private**, so it cannot be forged: every `Ordered<T>` originates at a sequencer (the only
//!   other way to hold one is to have it handed back intact by [`OutOfOrder::into_parts`] after a
//!   refused delivery — still sequencer-minted). **Move-only** — delivered at most once.
//! * [`Cursor`] is a receiver's delivery watermark. It is **linear** (move-only): every delivery
//!   consumes the cursor and re-emits the next one, so a receiver's deliveries form a single
//!   serial chain — you cannot fork the stream or deliver twice from one watermark.
//!   [`deliver`](Cursor::deliver) accepts an [`Ordered<T>`] only when its position equals the
//!   watermark; a gap or reorder is returned as [`OutOfOrder`] (a **runtime** outcome — positions
//!   are runtime data; see the seam), with the message handed back so the receiver can buffer it.
//!
//! ## The happy path — one order, delivered in sequence
//!
//! ```
//! use quorum_types::total_order::{Sequencer, Cursor};
//! let mut seq = Sequencer::new();
//! let m0 = seq.assign("a");        // position 0
//! let m1 = seq.assign("b");        // position 1
//! // A receiver delivers strictly in the sequenced order.
//! let (v0, cur) = Cursor::new().deliver(m0).expect("0 is next");
//! let (v1, _cur) = cur.deliver(m1).expect("1 is next");
//! assert_eq!((v0, v1), ("a", "b"));
//! ```
//!
//! ## Delivering out of the total order is refused (at runtime)
//!
//! ```
//! use quorum_types::total_order::{Sequencer, Cursor};
//! let mut seq = Sequencer::new();
//! let _m0 = seq.assign(10);
//! let m1 = seq.assign(20);          // position 1
//! // The receiver is at watermark 0; position 1 cannot be delivered yet.
//! match Cursor::new().deliver(m1) {
//!     Ok(_) => unreachable!("out-of-order delivery must be refused"),
//!     Err(o) => {
//!         assert_eq!((o.expected(), o.got()), (0, 1));
//!         let (_buffered, _cursor) = o.into_parts();   // handed back to buffer until 0 arrives
//!     }
//! }
//! ```
//!
//! ## You cannot forge an ordered message
//!
//! ```compile_fail
//! use quorum_types::total_order::Ordered;
//! // No public constructor: Ordered's fields are private, so it can only come from a Sequencer.
//! let forged: Ordered<i32> = Ordered { payload: 99, pos: 0 };
//! ```
//!
//! ## The cursor is linear — you cannot deliver twice from one watermark
//!
//! ```compile_fail
//! use quorum_types::total_order::{Sequencer, Cursor};
//! let mut seq = Sequencer::new();
//! let m0 = seq.assign(1);
//! let m1 = seq.assign(2);
//! let cur = Cursor::new();
//! let _ = cur.deliver(m0);   // consumes `cur`
//! let _ = cur.deliver(m1);   // error: `cur` moved — the delivery stream cannot fork
//! ```
//!
//! ## Where the types stop (the runtime seam) — the consensus the types cannot own
//!
//! What the types own here is real but *local*: an `Ordered<T>` is **unforgeable** (only a
//! sequencer mints one), delivery is **once** (move-only message) and **serial** (linear cursor).
//! What the types **cannot** own is the crux of total order: that *every* receiver shares the
//! *same* sequencer — that the positions two receivers see agree on one global order. That
//! agreement is precisely **consensus**, and no amount of local typestate can manufacture it; it
//! is a trusted runtime input, the same wall the crate meets whenever a guarantee stops being
//! coordination-free (the const-generic epoch that cannot be minted from wire bytes in rung 5).
//! The position itself is runtime data — which is why an out-of-order delivery is an
//! [`OutOfOrder`] *value*, not a compile error: the type system can force you to *check* the
//! order, but the order is decided off-machine by the authority. The price of the strongest
//! delivery order is that its whole global shape escapes the type system into the sequencer.

/// The single ordering authority — a model of the leader / consensus instance that decides the
/// one global order. The **only** minter of [`Ordered`] messages.
#[derive(Debug, Default)]
pub struct Sequencer {
    next: u64,
}

impl Sequencer {
    /// A fresh sequencer, positioned to assign 0 first.
    #[must_use]
    pub const fn new() -> Self {
        Sequencer { next: 0 }
    }

    /// Stamp the next global position onto `payload`, yielding an [`Ordered<T>`] and advancing the
    /// sequencer. This is the trust boundary: the returned order is only *globally* meaningful if
    /// every receiver draws from this same sequencer (the consensus seam).
    ///
    /// The `u64` counter would overflow only after 2⁶⁴ assignments; the `+= 1` is left to
    /// **fail loud** (debug panic) rather than saturate on purpose — a saturating counter would
    /// silently re-issue the same position and break the very total order this type exists to
    /// guarantee, a strictly worse failure than a panic.
    pub fn assign<T>(&mut self, payload: T) -> Ordered<T> {
        let pos = self.next;
        self.next += 1;
        Ordered { payload, pos }
    }

    /// The position the sequencer will assign next.
    #[must_use]
    pub const fn peek(&self) -> u64 {
        self.next
    }
}

/// **A message assigned a global position** by a [`Sequencer`]. Its fields are private, so it is
/// unforgeable — outside this module the only way to hold one is to receive it from a sequencer.
/// Move-only: delivered at most once.
#[derive(Debug)]
#[must_use = "an Ordered message is undelivered; deliver it in total order via a Cursor"]
pub struct Ordered<T> {
    payload: T,
    pos: u64,
}

impl<T> Ordered<T> {
    /// The global position this message was assigned. (Readable, but not constructible — reading
    /// the order is fine; minting it is the sequencer's sole right.)
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.pos
    }
}

/// A receiver's **linear** delivery watermark: the next global position it will accept. Move-only,
/// so a receiver's deliveries form one serial chain — the stream cannot fork or re-deliver.
#[derive(Debug)]
#[must_use = "a Cursor is the receiver's place in the total order; thread it through each delivery"]
pub struct Cursor {
    next: u64,
}

impl Cursor {
    /// A fresh receiver, waiting for global position 0.
    pub const fn new() -> Self {
        Cursor { next: 0 }
    }

    /// The global position this receiver will deliver next.
    #[must_use]
    pub const fn awaiting(&self) -> u64 {
        self.next
    }

    /// **Total-order delivery.** Consume the cursor; if `msg`'s position is exactly the watermark,
    /// deliver its payload and re-emit the advanced cursor. Otherwise return [`OutOfOrder`] — the
    /// message handed back for buffering, and the *unchanged* cursor — because the position is
    /// runtime data the sequencer decided, an out-of-order arrival is a runtime outcome, not a
    /// type error (see the module's seam note).
    pub fn deliver<T>(self, msg: Ordered<T>) -> Result<(T, Cursor), OutOfOrder<T>> {
        if msg.pos == self.next {
            Ok((msg.payload, Cursor { next: self.next + 1 }))
        } else {
            Err(OutOfOrder {
                expected: self.next,
                got: msg.pos,
                message: msg,
                cursor: self,
            })
        }
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor::new()
    }
}

/// The outcome of a delivery that did not match the receiver's watermark. Carries the rejected
/// message back (so the receiver can buffer it until its turn) via [`into_parts`](OutOfOrder::into_parts),
/// the **unchanged** cursor, and the `expected`/`got` positions.
///
/// Its fields are **private** and set *only* by [`Cursor::deliver`], so — like [`Ordered`] itself —
/// this diagnostic cannot be forged, and the labels are invariants by construction:
/// [`got`](OutOfOrder::got)` == message.`[`position()`](Ordered::position) and
/// [`expected`](OutOfOrder::expected)` == cursor.`[`awaiting()`](Cursor::awaiting). A downstream
/// reader may trust them.
#[derive(Debug)]
#[must_use = "an OutOfOrder holds your message and cursor — buffer the message and keep the cursor"]
pub struct OutOfOrder<T> {
    expected: u64,
    got: u64,
    message: Ordered<T>,
    cursor: Cursor,
}

impl<T> OutOfOrder<T> {
    /// The position the receiver was waiting for. Equals the returned cursor's
    /// [`awaiting()`](Cursor::awaiting) by construction.
    #[must_use]
    pub const fn expected(&self) -> u64 {
        self.expected
    }

    /// The position the arriving message actually carried. Equals the wrapped message's
    /// [`position()`](Ordered::position) by construction.
    #[must_use]
    pub const fn got(&self) -> u64 {
        self.got
    }

    /// Borrow the rejected message (e.g. to inspect its [`position`](Ordered::position)) before
    /// deciding to buffer it.
    pub const fn message(&self) -> &Ordered<T> {
        &self.message
    }

    /// Recover the rejected message and the **unchanged** cursor — buffer the message and retry
    /// the delivery once the gap ahead of the cursor fills.
    pub fn into_parts(self) -> (Ordered<T>, Cursor) {
        (self.message, self.cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequencer_assigns_increasing_positions() {
        let mut seq = Sequencer::new();
        let a = seq.assign('a');
        let b = seq.assign('b');
        let c = seq.assign('c');
        assert_eq!((a.position(), b.position(), c.position()), (0, 1, 2));
    }

    #[test]
    fn delivers_in_sequenced_order() {
        let mut seq = Sequencer::new();
        let m0 = seq.assign(10);
        let m1 = seq.assign(20);
        let (v0, cur) = Cursor::new().deliver(m0).expect("0 is next");
        let (v1, cur) = cur.deliver(m1).expect("1 is next");
        assert_eq!((v0, v1), (10, 20));
        assert_eq!(cur.awaiting(), 2);
    }

    #[test]
    fn out_of_order_is_refused_and_hands_back_the_message() {
        let mut seq = Sequencer::new();
        let _m0 = seq.assign(10);
        let m1 = seq.assign(20);
        let err = Cursor::new().deliver(m1).unwrap_err();
        assert_eq!((err.expected(), err.got()), (0, 1));
        assert_eq!(err.message().position(), 1, "got() equals the wrapped message's position by construction");
        // The cursor is unchanged and the message came back — buffer it, deliver 0, then retry.
        let (_msg, cur) = err.into_parts();
        assert_eq!(cur.awaiting(), 0);
    }

    #[test]
    fn buffer_then_retry_after_the_gap_fills() {
        // Messages arrive 1 then 0; the receiver buffers 1, delivers 0, then delivers 1.
        let mut seq = Sequencer::new();
        let m0 = seq.assign(100);
        let m1 = seq.assign(200);
        let err = Cursor::new().deliver(m1).unwrap_err(); // 1 arrives first — refused
        let (buffered, cursor) = err.into_parts();
        let (v0, cur) = cursor.deliver(m0).expect("0 fills the gap");
        let (v1, _cur) = cur.deliver(buffered).expect("now 1 is next");
        assert_eq!((v0, v1), (100, 200));
    }

    #[test]
    fn two_receivers_from_one_sequencer_agree_on_the_order() {
        // The single-sequencer premise: two receivers drawing the same positions deliver the same
        // order. (That they *do* share one sequencer is the consensus seam the types can't own.)
        let mut seq = Sequencer::new();
        let a0 = seq.assign(1);
        let a1 = seq.assign(2);
        // Receiver R1
        let (r1v0, c1) = Cursor::new().deliver(a0).unwrap();
        // Receiver R2 gets messages with the same positions (re-minted here only for the test).
        let mut seq2 = Sequencer::new();
        let b0 = seq2.assign(1);
        let (r2v0, _c2) = Cursor::new().deliver(b0).unwrap();
        let (r1v1, _c1) = c1.deliver(a1).unwrap();
        assert_eq!(r1v0, r2v0, "same first position ⇒ same first delivery");
        assert_eq!((r1v0, r1v1), (1, 2));
    }
}

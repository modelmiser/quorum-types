//! **Consistent cut** — the predicate a distributed snapshot must satisfy (Lamport,
//! 1978; Mattern, 1988). This is the *denotational* half of a dual: `snapshot` is the
//! Chandy–Lamport protocol that **produces** a cut; this rung is the property that
//! **certifies** one, independent of how it was produced.
//!
//! A *cut* of a distributed execution is a frontier — for each process, how many of
//! its events are "before the line". A cut is **consistent** iff it is left-closed
//! under causality: whenever it includes the *receive* of a message, it also includes
//! that message's *send*. The forbidden shape is an **orphan**: a message received
//! inside the cut but sent outside it — a global state that says "this was delivered"
//! about something that, according to the same state, was never sent. No real
//! execution ever passes through such a state, so admitting one as a recovery line or
//! a snapshot target is the bug.
//!
//! ## The mechanism — verification is a phase transition, not a token
//!
//! The frontier is a [`vclock`](crate::vclock)-shaped object: `N` per-process event
//! counts. A cut moves through two phases:
//!
//! * [`Cut`]`<`[`Unverified`]`, N>` — a proposed frontier, built by
//!   [`proposed`](Cut::proposed). It carries no promise of consistency.
//! * [`verify`](Cut::verify) — defined **only** on `Cut<`[`Unverified`]`, N>` —
//!   **consumes** the cut and checks every message in a supplied log for the orphan
//!   shape. If none is found it re-emits the *same* frontier as
//!   `Cut<`[`Consistent`]`, N>`; otherwise it returns the offending [`Orphan`] and the
//!   cut is gone.
//! * A [`Consistent`] cut is what a recovery/rollback layer may target. Functions that
//!   require global consistency take `Cut<`[`Consistent`]`, N>`, so an unverified
//!   frontier cannot be substituted.
//!
//! Because verification *transforms the cut itself* rather than handing back a
//! free-floating "verified" token, a verification of one frontier can never be used to
//! wave a different, unchecked one through — there is no separable witness to move.
//! This is the same fusion `occ` uses for its validated transaction, applied
//! deliberately to head off the crate's recurring detachable-witness hazard: the proof
//! of consistency *is* the typed cut.
//!
//! Unlike `snapshot`'s purely structural phase wall, the [`Unverified`]→[`Consistent`]
//! door is a **runtime** check over the message log — so this rung's guarantee rests on
//! a trusted runtime witness (the [`vclock`](crate::vclock) species), not on structure
//! alone. See the runtime seam.
//!
//! ## A causally-closed cut verifies
//!
//! ```
//! use quorum_types::consistent_cut::{Cut, Message};
//! // two processes; the cut includes 2 events of P0 and 2 of P1.
//! let cut = Cut::proposed([2, 2]);
//! // P0's event 1 sent a message delivered as P1's event 2: send(1) <= 2, recv(2) <= 2.
//! // both endpoints are inside the cut, so it is causally closed.
//! let log = [Message { from: 0, sent: 1, to: 1, received: 2 }];
//! let cut = cut.verify(&log).expect("send is inside the cut -> consistent");
//! assert_eq!(cut.at(0), 2);
//! assert_eq!(cut.at(1), 2);
//! ```
//!
//! ## An orphan message rejects the cut
//!
//! ```
//! use quorum_types::consistent_cut::{Cut, Message};
//! // the cut includes only 1 event of P0 but 2 of P1.
//! let cut = Cut::proposed([1, 2]);
//! // P0's event 2 sent a message received as P1's event 2: recv(2) <= 2 is inside,
//! // but send(2) > 1 is outside — an orphan. The receive is in the cut, the send is not.
//! let log = [Message { from: 0, sent: 2, to: 1, received: 2 }];
//! match cut.verify(&log) {
//!     Ok(_) => unreachable!("a received-but-not-sent message is not a consistent cut"),
//!     Err(orphan) => {
//!         assert_eq!(orphan.from(), 0);
//!         assert_eq!(orphan.sent(), 2);
//!         assert_eq!(orphan.to(), 1);
//!         assert_eq!(orphan.received(), 2);
//!     }
//! }
//! ```
//!
//! ## A message sent inside but received outside is fine — that is channel state
//!
//! ```
//! use quorum_types::consistent_cut::{Cut, Message};
//! let cut = Cut::proposed([2, 1]);
//! // sent as P0 event 2 (inside), received as P1 event 2 (outside): in flight across
//! // the cut. That is exactly the channel state a snapshot records — not an orphan.
//! let log = [Message { from: 0, sent: 2, to: 1, received: 2 }];
//! let cut = cut.verify(&log).expect("in-transit message is not an orphan");
//! assert_eq!(cut.at(0), 2);
//! ```
//!
//! ## Using an unverified cut where a consistent one is required is a compile error
//!
//! A recovery layer takes `Cut<`[`Consistent`]`, N>`. A proposed frontier that has not
//! been through [`verify`](Cut::verify) is a different type and will not unify:
//!
//! ```compile_fail
//! use quorum_types::consistent_cut::{Cut, Consistent};
//! fn rollback_to<const N: usize>(_line: Cut<Consistent, N>) {}
//! let cut = Cut::proposed([1, 1]);
//! rollback_to(cut); // ERROR: expected Cut<Consistent, _>, found Cut<Unverified, _>
//! ```
//!
//! ## You cannot fabricate a consistent cut by hand
//!
//! [`Cut`]'s fields are private, so a `Cut<Consistent, N>` cannot be built by hand —
//! the only route into the [`Consistent`] phase is [`verify`](Cut::verify) returning
//! `Ok` (which certifies "no orphan *in the supplied log*" — log completeness is the
//! caller's, per the runtime seam):
//!
//! ```compile_fail
//! use quorum_types::consistent_cut::{Cut, Consistent};
//! let forged: Cut<Consistent, 2> = Cut { frontier: [9, 9], _ph: core::marker::PhantomData };
//! let _ = forged.at(0); // Cut has private fields: no hand-built Consistent cut
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! [`verify`](Cut::verify) is an **exact, total** decision over the frontier and the
//! message log it is handed — given those inputs, it admits a cut iff no orphan exists,
//! with no slack. What it cannot check is that the *inputs are honest*:
//!
//! * **The log is complete.** A message that was actually received inside the cut but
//!   is *missing* from the log cannot be checked — an omitted orphan verifies. This is
//!   the [`vclock`](crate::vclock) seam one level down: a cut is only as truthful as
//!   the layer that records its sends and receives, exactly as a vector clock is only
//!   as truthful as the layer that ticks it.
//! * **The indices are real.** `sent`/`received` are trusted to be that message's true
//!   event positions and `frontier[i]` the true count of process `i`'s included events.
//!   The type owns the *decision* (received-inside ∧ sent-outside ⇒ reject), not the
//!   bookkeeping that produced the numbers.
//!
//! The two rungs compose across this seam: `snapshot` records a frontier by an
//! orphan-free *protocol*, and this rung certifies a frontier by the orphan-free
//! *predicate*. Chandy–Lamport's theorem is precisely that the former always satisfies
//! the latter — the operational and denotational views of one consistent cut.

use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A phase of a cut. Sealed: the only phases are [`Unverified`] and [`Consistent`],
/// so a `Cut<Consistent, N>` can arise only through [`verify`](Cut::verify), never by
/// naming a phase type directly.
pub trait Phase: sealed::Sealed {}

/// The **unverified** phase: a proposed frontier with no consistency promise.
#[derive(Debug)]
pub struct Unverified;
/// The **consistent** phase: [`verify`](Cut::verify) confirmed the frontier is
/// causally left-closed (orphan-free). Only such a cut may serve as a recovery line.
#[derive(Debug)]
pub struct Consistent;

impl sealed::Sealed for Unverified {}
impl sealed::Sealed for Consistent {}
impl Phase for Unverified {}
impl Phase for Consistent {}

/// A message in the execution: sent as event [`sent`](Message::sent) of process
/// [`from`](Message::from), delivered as event [`received`](Message::received) of
/// process [`to`](Message::to). Event indices are 1-based; a `Cut` frontier of `k`
/// for a process means its events `1..=k` are inside the cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    /// The sending process index.
    pub from: usize,
    /// The sender's event index at which the message was sent.
    pub sent: u64,
    /// The receiving process index.
    pub to: usize,
    /// The receiver's event index at which the message was delivered.
    pub received: u64,
}

/// A cut of a distributed execution over `N` processes, in phase `PH`. `frontier[i]`
/// is how many of process `i`'s events are inside the cut.
///
/// Move-only and `#[must_use]`: a proposed cut is resolved by
/// [`verify`](Cut::verify). The frontier is private — a `Cut<Consistent, N>` cannot be
/// forged; the only consistent cut is one `verify` re-emitted.
#[must_use = "a proposed Cut must be verified before it can serve as a consistent global state"]
pub struct Cut<PH: Phase, const N: usize> {
    frontier: [u64; N],
    _ph: PhantomData<PH>,
}

impl<const N: usize> Cut<Unverified, N> {
    /// Propose a cut from a frontier of per-process event counts. Carries no promise of
    /// consistency until [`verify`](Cut::verify) passes.
    pub const fn proposed(frontier: [u64; N]) -> Self {
        Cut { frontier, _ph: PhantomData }
    }

    /// Check the cut against the message `log` and **consume** it. If no message is an
    /// orphan — received inside the cut but sent outside it — re-emit the *same*
    /// frontier as a [`Consistent`] cut. Otherwise return the offending [`Orphan`]; the
    /// cut is gone.
    ///
    /// Consuming `self` is what fuses the proof to *this* frontier: there is no separate
    /// token that could certify a different, unverified cut.
    ///
    /// # Panics
    /// If any message names a process index `>= N` (`m.from` or `m.to`), like the
    /// bounds panic [`vclock::VClock::tick`](crate::vclock::VClock::tick) documents. The
    /// process count is a type parameter; a message referring to a non-existent process
    /// is a caller error, not an [`Orphan`].
    pub fn verify(self, log: &[Message]) -> Result<Cut<Consistent, N>, Orphan> {
        for m in log {
            let received_inside = m.received <= self.frontier[m.to];
            let sent_outside = m.sent > self.frontier[m.from];
            if received_inside && sent_outside {
                return Err(Orphan { from: m.from, sent: m.sent, to: m.to, received: m.received });
            }
        }
        Ok(Cut { frontier: self.frontier, _ph: PhantomData })
    }
}

impl<PH: Phase, const N: usize> Cut<PH, N> {
    /// How many of process `i`'s events are inside the cut.
    ///
    /// # Panics
    /// If `i >= N` (bounds), as [`vclock::VClock::tick`](crate::vclock::VClock::tick).
    pub const fn at(&self, i: usize) -> u64 {
        self.frontier[i]
    }
}

/// A consistency violation: a message received inside the cut but sent outside it —
/// the receive is included, its cause (the send) is not. Returned by
/// [`verify`](Cut::verify); the cut it came from has been consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Orphan {
    from: usize,
    sent: u64,
    to: usize,
    received: u64,
}

impl Orphan {
    /// The sending process whose send fell outside the cut.
    pub const fn from(&self) -> usize {
        self.from
    }
    /// The send event index that was outside the cut.
    pub const fn sent(&self) -> u64 {
        self.sent
    }
    /// The receiving process whose receive fell inside the cut.
    pub const fn to(&self) -> usize {
        self.to
    }
    /// The receive event index that was inside the cut.
    pub const fn received(&self) -> u64 {
        self.received
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_causally_closed_cut_verifies() {
        // send at P0 event 1 (inside [2,2]), received at P1 event 2 (inside): closed.
        let cut = Cut::proposed([2, 2]);
        let log = [Message { from: 0, sent: 1, to: 1, received: 2 }];
        let cut = cut.verify(&log).expect("both endpoints inside -> consistent");
        assert_eq!(cut.at(0), 2);
        assert_eq!(cut.at(1), 2);
    }

    #[test]
    fn an_orphan_rejects_the_cut() {
        // send at P0 event 2 (outside [1,2]), received at P1 event 2 (inside): orphan.
        let cut = Cut::proposed([1, 2]);
        let log = [Message { from: 0, sent: 2, to: 1, received: 2 }];
        match cut.verify(&log) {
            Ok(_) => panic!("received-but-not-sent is not a consistent cut"),
            Err(orphan) => {
                assert_eq!(orphan.from(), 0);
                assert_eq!(orphan.sent(), 2);
                assert_eq!(orphan.to(), 1);
                assert_eq!(orphan.received(), 2);
            }
        }
    }

    #[test]
    fn an_in_transit_message_is_not_an_orphan() {
        // send at P0 event 2 (inside [2,1]), received at P1 event 2 (outside): in flight.
        let cut = Cut::proposed([2, 1]);
        let log = [Message { from: 0, sent: 2, to: 1, received: 2 }];
        let cut = cut.verify(&log).expect("sent-inside received-outside is channel state");
        assert_eq!(cut.at(0), 2);
        assert_eq!(cut.at(1), 1);
    }

    #[test]
    fn an_empty_log_always_verifies() {
        // With no messages there is nothing to violate closure.
        let cut = Cut::proposed([5, 3, 7]);
        let cut = cut.verify(&[]).expect("no messages -> trivially consistent");
        assert_eq!(cut.at(2), 7);
    }

    #[test]
    fn verification_is_fused_to_the_frontier() {
        // The verified cut carries the frontier that was checked — there is no
        // separable witness to attach to a different, unchecked frontier.
        let good = Cut::proposed([2, 2]);
        let log = [Message { from: 0, sent: 1, to: 1, received: 2 }];
        let good = good.verify(&log).expect("consistent");
        assert_eq!(good.at(0), 2);
        // A different frontier would have to run its own verify to become Consistent.
    }
}

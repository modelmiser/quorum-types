//! Pessimistic **message logging** — roll-forward recovery, the dual of
//! `recovery_line`.
//!
//! `recovery_line` recovers a crashed process by rolling
//! *everyone* back to a consistent line: cheap checkpoints, but recovery can lose
//! work and cascade (Randell's domino effect). Message logging takes the opposite
//! road — log enough about each delivered message that a crashed process can be
//! **replayed forward** from its last checkpoint by re-delivering the logged
//! messages in the logged order, recovering it *without rolling any surviving
//! process back* (Strom & Yemini 1985; Elnozahy, Alvisi, Wang & Johnson,
//! ACM CSUR 2002). Backward recovery vs forward recovery — the two families of
//! rollback-recovery, and this crate's fourth structural/witness dual.
//!
//! The correctness condition of *pessimistic* logging is the **always-no-orphans**
//! rule (Alvisi & Marzullo 1998): a process must not let its state depend on — in
//! particular, must not *send a message caused by* — a delivered message whose
//! determinant is not yet stably logged. If it did and then crashed, the logged
//! world could not reproduce that delivery, so every surviving recipient of the
//! messages the process sent because of it becomes an **orphan** whose state
//! reflects an event that no longer happened — and they too must roll back,
//! reintroducing exactly the cascade logging was meant to avoid. Pessimistic
//! logging obeys the rule by logging each message *before* delivering it; this
//! module makes that order **structural** — delivering an unlogged message is
//! unrepresentable, so no state can come to depend on one.
//!
//! ## The mechanism — a phase wall between receipt and delivery
//!
//! A message off the wire is [`Msg`]`<T, `[`Received`]`>`. It cannot be handed to
//! the application yet: [`deliver`](Msg::deliver) does not exist on
//! [`Received`]. The only route onward is [`log`](Msg::log), which persists the
//! determinant (the stable-storage seam) and yields [`Msg`]`<T, `[`Logged`]`>`;
//! and *only* [`Logged`] has [`deliver`](Msg::deliver). Delivery consumes the
//! message, hands the payload to the application, and mints a [`Stable`] witness —
//! evidence that *this* delivery is backed by a logged determinant and can be
//! reproduced on replay.
//!
//! A process only creates an orphan by *sending*. [`caused_by`](Msg::caused_by)
//! forms an outgoing message that cites the [`Stable`] witnesses of the deliveries
//! it depends on. Because a [`Stable`] is minted *only* by delivering a logged
//! message, an outgoing message **cannot cite an unlogged delivery as a cause** —
//! every cited cause is one whose determinant was logged before it was delivered
//! (hence replayable *if the log survives* — the durability seam), which is the
//! enforceable half of the always-no-orphans rule. (That the citation is *complete* —
//! names every true
//! cause, omits none — is the operator's honest declaration, exactly as in
//! [`causal`](crate::causal); see the seam.)
//!
//! ## The happy path — log, deliver, then send a caused message
//!
//! ```
//! use quorum_types::message_log::Msg;
//! // A message arrives; log its determinant before the application sees it.
//! let incoming = Msg::received(0xBEEF_u64);
//! let (payload, stable) = incoming.log().deliver();
//! assert_eq!(payload, 0xBEEF);
//! // A reply *caused by* that delivery may cite its Stable witness — the reply
//! // depends only on a delivery that will survive replay.
//! let reply = Msg::caused_by(0xF00D_u64, &[stable]);
//! let (rp, _) = reply.log().deliver();
//! assert_eq!(rp, 0xF00D);
//! ```
//!
//! ## Delivering before logging is a compile error
//!
//! [`deliver`](Msg::deliver) exists only on [`Msg`]`<T, `[`Logged`]`>`. A message
//! straight off the wire has no `deliver` — the application cannot observe it
//! before its determinant is logged:
//!
//! ```compile_fail
//! use quorum_types::message_log::Msg;
//! let incoming = Msg::received(1_u64);
//! let _ = incoming.deliver(); // no `deliver` on Msg<T, Received>: log it first
//! ```
//!
//! ## You cannot forge a logged message to skip the log
//!
//! [`Msg`]'s fields are private, so a `Msg<T, Logged>` cannot be built by hand —
//! the only way into [`Logged`] is [`log`](Msg::log):
//!
//! ```compile_fail
//! use quorum_types::message_log::{Msg, Logged};
//! use core::marker::PhantomData;
//! let forged: Msg<u64, Logged> = Msg { payload: 1, _st: PhantomData }; // private fields
//! let _ = forged.deliver();
//! ```
//!
//! ## You cannot forge a stability witness
//!
//! [`Stable`] has no public constructor; it is minted only by
//! [`deliver`](Msg::deliver). A send cannot cite a stability it never earned:
//!
//! ```compile_fail
//! use quorum_types::message_log::Stable;
//! let forged = Stable {}; // no public constructor: only `deliver` mints one
//! let _ = quorum_types::message_log::Msg::caused_by(1_u64, &[forged]);
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! The types own the *order* — no delivery without a preceding [`log`](Msg::log),
//! no caused send without the delivery's [`Stable`] — which is precisely the
//! pessimistic-logging discipline. They do **not** own:
//!
//! * **Log stability.** [`log`](Msg::log) *models* persisting the determinant, but
//!   the type cannot check the write actually reached stable storage before the
//!   crash. A [`Stable`] witnesses "this determinant was logged," not "the log
//!   survived" — the same trusted-durability seam as
//!   [`chain`](crate::chain)'s per-node apply and [`fencing`](crate::fencing)'s
//!   token authority.
//! * **The real dependency graph.** [`caused_by`](Msg::caused_by) trusts the caller
//!   to cite the deliveries a send *actually* depends on. It guarantees every cited
//!   dependency is a logged delivery; it cannot verify the citation is complete —
//!   an omitted true dependency is invisible, exactly the declared-vs-true boundary
//!   [`causal`](crate::causal) documents (there the predecessor graph, here the
//!   causing deliveries). The type enforces the rule over what you *declare*.
//! * **Deterministic replay.** Roll-forward recovery assumes re-delivering the
//!   logged messages in the logged order reproduces the process's state
//!   (piecewise determinism). Non-determinism the log does not capture is out of
//!   scope. An out-of-tree TLC model is the discriminant: log-before-deliver keeps
//!   every surviving process orphan-free across a crash; deliver-before-log admits
//!   an orphan.

use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A logging state of a message. Sealed: the only states are [`Received`] and
/// [`Logged`], so a `Msg<T, Logged>` can arise only through [`Msg::log`], never by
/// naming a state directly.
pub trait State: sealed::Sealed {}

/// The **received** state: a message off the wire whose determinant is not yet
/// logged. It cannot be delivered — [`deliver`](Msg::deliver) is absent here.
#[derive(Debug)]
pub struct Received;
/// The **logged** state: [`log`](Msg::log) has persisted the determinant, so the
/// message may now be delivered and later replayed.
#[derive(Debug)]
pub struct Logged;

impl sealed::Sealed for Received {}
impl sealed::Sealed for Logged {}
impl State for Received {}
impl State for Logged {}

/// A message carrying payload `T`, in logging state `ST`.
///
/// Move-only and `#[must_use]`: a received message must be logged and delivered (or
/// explicitly dropped), never silently observed — an unlogged delivery is the orphan
/// hazard this rung rules out. The `payload` is the application datum; the type-level
/// `ST` is whether its determinant has been logged.
#[must_use = "a received Msg is undelivered; log it before delivering or its effect is unrecoverable"]
pub struct Msg<T, ST: State> {
    payload: T,
    _st: PhantomData<ST>,
}

impl<T> Msg<T, Received> {
    /// A message straight off the wire — determinant not yet logged, so not yet
    /// deliverable.
    pub const fn received(payload: T) -> Self {
        Msg { payload, _st: PhantomData }
    }

    /// Form an **outgoing** message caused by the deliveries whose [`Stable`]
    /// witnesses are supplied. Because a [`Stable`] is minted only by delivering a
    /// *logged* message, a send cannot be caused by an unlogged delivery — the
    /// always-no-orphans rule holds by construction. The new message is itself
    /// [`Received`]: its recipient must log it in turn.
    ///
    /// The type guarantees every cited dependency is a logged delivery; that the
    /// citation is *complete* (names every true cause) is the operator's, per the
    /// module's runtime-seam note.
    pub const fn caused_by(payload: T, _deps: &[Stable]) -> Self {
        Msg { payload, _st: PhantomData }
    }

    /// **Log the determinant**, transitioning the message into [`Logged`]. Models
    /// persisting to stable storage; the type owns the *order* (this must precede
    /// delivery), not that the write survives a crash (the seam).
    pub fn log(self) -> Msg<T, Logged> {
        Msg { payload: self.payload, _st: PhantomData }
    }
}

impl<T> Msg<T, Logged> {
    /// **Deliver** the logged message to the application. Reachable only after
    /// [`log`](Msg::log), so no application state can depend on an unlogged message.
    /// Consumes the message, returns the payload, and mints a [`Stable`] witness that
    /// this delivery is backed by a logged determinant (hence reproducible on
    /// replay).
    pub fn deliver(self) -> (T, Stable) {
        (self.payload, Stable { _priv: () })
    }
}

/// **A stability witness** — evidence that a delivered message's determinant was
/// logged, so the delivery survives a crash by replay. `Copy`, because it is a
/// *fact* about a delivery, not a resource: one stable delivery may be a declared
/// cause of arbitrarily many sends (fan-out).
///
/// Minted only by [`Msg::deliver`] (no public constructor). Its presence is what a
/// [`caused_by`](Msg::caused_by) send cites to prove it does not depend on an
/// unlogged delivery. It does **not** prove the log physically survived (the
/// durability seam), only that the determinant was logged before delivery.
#[derive(Debug, Clone, Copy)]
pub struct Stable {
    // Zero-size but *private*: a real field (not just a comment) so `Stable {}`
    // cannot be written outside this module — obtainable only through `deliver`.
    _priv: (),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_then_deliver_returns_payload_and_stability() {
        let (payload, _stable) = Msg::received(42_u64).log().deliver();
        assert_eq!(payload, 42, "delivery preserves the payload");
    }

    #[test]
    fn a_caused_send_cites_the_stability_of_its_cause() {
        // Deliver an incoming message (logged first), then send a reply caused by it.
        let (_, stable) = Msg::received(1_u64).log().deliver();
        let reply = Msg::caused_by(2_u64, &[stable]);
        let (rp, _) = reply.log().deliver();
        assert_eq!(rp, 2, "the reply is deliverable and depends only on a logged cause");
    }

    #[test]
    fn stability_is_a_fact_and_fans_out() {
        // One stable delivery may be the declared cause of several sends: Stable is Copy.
        let (_, s) = Msg::received("cause").log().deliver();
        let a = Msg::caused_by(10_u64, &[s]);
        let b = Msg::caused_by(20_u64, &[s]); // s reused — Copy
        assert_eq!(a.log().deliver().0, 10);
        assert_eq!(b.log().deliver().0, 20);
    }

    #[test]
    fn a_send_may_cite_several_stable_causes() {
        let (_, s1) = Msg::received(1_u64).log().deliver();
        let (_, s2) = Msg::received(2_u64).log().deliver();
        let joined = Msg::caused_by(3_u64, &[s1, s2]);
        assert_eq!(joined.log().deliver().0, 3);
    }

    #[test]
    fn an_independent_send_cites_no_causes() {
        // A message not caused by any delivery is still well-formed (empty deps).
        let independent = Msg::caused_by(7_u64, &[]);
        assert_eq!(independent.log().deliver().0, 7);
    }
}

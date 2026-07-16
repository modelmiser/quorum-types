//! Local failure suspicion — the **structural** rung of the failure-detection (liveness) axis.
//!
//! Its runtime-witness dual is `detector`. Liveness is out of scope in every other module of this
//! crate ([`lockorder`](crate::lockorder): "Safety only; liveness is out of scope, as everywhere in
//! this crate"); the death of a node is named only as an *untyped seam*
//! ([`chain`](crate::chain) mentions "detecting a dead replica" but does not model it). This axis
//! types the two halves nobody else does: locally **suspecting** a node has gone silent (here,
//! structural) and the cluster **confirming** it dead (`detector`, a witness).
//!
//! The split is the crate's recurring cut. Deciding *"I have not heard from node `NODE` for too long"*
//! needs only your own clock — no coordination, the CALM/structural side. Deciding *"node `NODE` is
//! actually dead"* cannot be done from one vantage (a slow node is indistinguishable from a dead one),
//! so it needs corroboration from a quorum — `detector`, the non-CALM/witness side. Liveness is the
//! fifth axis to land on this boundary, after order, count, occupancy, and leadership.
//!
//! ## What the type owns
//!
//! A [`Monitor<NODE>`] watches one peer, node `NODE`, recording the last tick it was `heard_at` and a
//! runtime silence budget. [`suspect`](Monitor::suspect) is the local boundary: it returns
//! `Some(`[`Suspected<NODE>`]`)` **iff** the measured silence `now − last_heard` **exceeds** the
//! budget, and `None` while the peer is still within it (like [`staleness`](crate::staleness)'s
//! `read_within` returning an `Option`). A [`Suspected<NODE>`] is a linear, unforgeable token meaning
//! "*I* locally suspect node `NODE` has gone silent."
//!
//! * **Node identity is the primary mechanism (E0308).** The suspected node rides in the type as a
//!   const brand, so a `Suspected<3>` and a `Suspected<4>` are **different types**: a suspicion about
//!   one node can never be passed where a suspicion about another is required. This is the same
//!   type-identity unification [`term`](crate::term) and [`membership`](crate::membership) use,
//!   deliberately chosen over a timeout-threshold `const` gate (which would merely re-skin
//!   [`staleness`](crate::staleness)'s `Δ ≤ TOL` check): here the **timeout is a runtime budget**, and
//!   *identity* is what the type enforces.
//! * **Linear (E0382).** A `Suspected<NODE>` is move-only (no `Copy`/`Clone`): a local, one-shot alarm —
//!   you act on the observation once and then re-observe, so a stale suspicion cannot be acted on twice.
//! * **Unforgeable (E0451).** Private field; minted only by [`suspect`](Monitor::suspect) crossing the
//!   silence boundary.
//!
//! ## Where the types stop (the runtime seam)
//!
//! * **A slow node is indistinguishable from a dead one.** The silence measurement is trusted:
//!   `now − last_heard` assumes comparable clocks, and a partition or a GC pause looks identical to a
//!   crash. `Suspected<NODE>` certifies "I observed silence past my budget," **not** that `NODE` has
//!   failed — that is the fundamental limit of asynchronous failure detection (FLP), and the whole
//!   reason death is only *confirmed* by the witness rung, never proven.
//! * **The budget is a policy, not a truth.** A short budget suspects live-but-slow peers; a long one
//!   is slow to react. The type propagates whatever budget you set; it does not choose a correct one.
//! * **The silence measurement saturates, and the skew is one-sided.** `now − last_heard` is a
//!   *saturating* subtraction: a `now` that runs behind `last_heard` (a backward clock, or a
//!   `last_heard` stamped in the future) floors the measured silence to `0` and so **suppresses**
//!   suspicion — a genuinely dead peer looks freshly heard. This is the mirror of
//!   [`staleness`](crate::staleness)'s hazard, where the skew that most flatters the guarantee is
//!   exactly the one the measurement hides; the protective direction (a bogus *early* `now` cannot
//!   *spuriously* suspect a live peer) is only half the story.
//! * **A suspicion is local; reporting it sends a claim.** `Suspected<NODE>` is an in-process token;
//!   telling peers "I suspect `NODE`" puts a message on the wire — a bare [`NodeId`](crate::membership)
//!   claim, **not** this unforgeable token, which cannot cross the network. The witness rung `detector`
//!   therefore aggregates *claims* (exactly as [`election`](crate::election) aggregates votes), and
//!   that honesty gap — a reporter that lies about suspecting `NODE` — is `detector`'s seam, not one a
//!   linear token could close.
//! * **`NODE` is a type-level class, not a value instance.** Two `Suspected<NODE>` of the same node are
//!   indistinguishable to the type (the crate's recurring class-not-instance limit).
//!
//! ## A suspicion cannot be forged — unforgeability
//!
//! ```compile_fail
//! use quorum_types::suspicion::Suspected;
//! let _ = Suspected::<3> { _priv: () }; // private field — only `Monitor::suspect` mints a Suspected
//! ```
//!
//! ## A suspicion about one node is not a suspicion about another — node identity
//!
//! ```compile_fail
//! use quorum_types::suspicion::{Monitor, Suspected};
//! fn evict(_: Suspected<3>) {}
//! let mon = Monitor::<4>::new(100, 5); // watching node 4
//! let s = mon.suspect(200).unwrap();    // Suspected<4>
//! evict(s); // ERROR: expected `Suspected<3>`, found `Suspected<4>` — E0308
//! ```
//!
//! ## A suspicion is acted on once — linearity
//!
//! ```compile_fail
//! use quorum_types::suspicion::{Monitor, Suspected};
//! fn raise_alarm(_: Suspected<7>) {}
//! let mon = Monitor::<7>::new(100, 5);
//! let s = mon.suspect(200).unwrap();
//! raise_alarm(s);       // moved
//! let _ = s.node();     // ERROR: `s` used after move — a suspicion is a one-shot alarm (E0382)
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::suspicion::Monitor;
//!
//! // Watch node 4 with a silence budget of 5 ticks; last heard at tick 100.
//! let mut mon = Monitor::<4>::new(100, 5);
//!
//! // Still within budget → no suspicion.
//! assert!(mon.suspect(103).is_none(), "3 ≤ 5: still alive");
//!
//! // A fresh message resets the clock.
//! mon.heard_at(110);
//! assert!(mon.suspect(114).is_none(), "4 ≤ 5 since last heard");
//!
//! // Silence past the budget → a Suspected<4>.
//! let s = mon.suspect(120).expect("10 > 5: gone silent");
//! assert_eq!(s.node(), 4);
//! ```

/// Watches a single peer, node `NODE`, for silence past a runtime budget. The watched node rides in
/// the type as a const brand, so monitors (and the [`Suspected`] tokens they mint) for different nodes
/// are different types.
#[derive(Debug, Clone, Copy)]
pub struct Monitor<const NODE: u64> {
    last_heard: u64,
    timeout: u64,
}

/// Local, unforgeable evidence that **this** observer suspects node `NODE` has gone silent past its
/// budget. Linear (move-only, no `Copy`/`Clone`): a local, one-shot alarm, acted on once and then
/// re-observed. It is *suspicion*, not proof of death, and it is **local** — reporting it to peers
/// sends a `NodeId` claim, not this token (that is `detector`'s seam). See the module's runtime seam.
#[derive(Debug)]
#[must_use = "a Suspected token is a local one-shot alarm that this observer suspects node NODE has gone silent; act on it or drop it"]
pub struct Suspected<const NODE: u64> {
    _priv: (),
}

impl<const NODE: u64> Monitor<NODE> {
    /// Watch node `NODE`, last heard from at `last_heard`, suspecting it after `timeout` ticks of
    /// silence.
    pub const fn new(last_heard: u64, timeout: u64) -> Self {
        Monitor { last_heard, timeout }
    }

    /// The node this monitor watches (its type-level `NODE`).
    pub const fn node(&self) -> u64 {
        NODE
    }

    /// The silence budget: how many ticks of quiet before the node is suspected.
    pub const fn timeout(&self) -> u64 {
        self.timeout
    }

    /// Record a fresh message from the peer at tick `now`, resetting the silence clock. A live peer
    /// keeps refreshing this and is never suspected.
    pub fn heard_at(&mut self, now: u64) {
        self.last_heard = now;
    }

    /// **The local boundary.** Suspect the peer iff its silence `now − last_heard` **exceeds** the
    /// budget, minting a [`Suspected<NODE>`](Suspected). Returns `None` while the peer is still within
    /// budget. No coordination — but this is *suspicion*, not death (see the module seam). Silence at
    /// exactly the budget is still within it (`>`, not `≥`).
    pub fn suspect(&self, now: u64) -> Option<Suspected<NODE>> {
        let silence = now.saturating_sub(self.last_heard);
        (silence > self.timeout).then_some(Suspected { _priv: () })
    }
}

impl<const NODE: u64> Suspected<NODE> {
    /// The node this suspicion is about (its type-level `NODE`).
    pub const fn node(&self) -> u64 {
        NODE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_budget_is_not_suspected_and_beyond_it_is() {
        let mon = Monitor::<4>::new(100, 5);
        assert!(mon.suspect(103).is_none(), "3 < 5");
        assert!(mon.suspect(105).is_none(), "5 == 5 (boundary, still within)");
        assert!(mon.suspect(106).is_some(), "6 > 5");
    }

    #[test]
    fn a_fresh_message_resets_suspicion() {
        let mut mon = Monitor::<4>::new(100, 5);
        assert!(mon.suspect(120).is_some(), "20 > 5: silent");
        mon.heard_at(118);
        assert!(mon.suspect(120).is_none(), "2 ≤ 5 since last heard: alive again");
        assert_eq!(mon.node(), 4);
        assert_eq!(mon.timeout(), 5);
    }

    #[test]
    fn a_suspicion_names_its_node() {
        let mon = Monitor::<9>::new(0, 3);
        let s = mon.suspect(10).expect("10 > 3");
        assert_eq!(s.node(), 9, "the suspected node rides in the type and the value");
    }

    #[test]
    fn saturating_silence_never_underflows() {
        // A clock that runs backward (now < last_heard) reports zero silence, not a wrapped huge one,
        // so a bogus early `now` cannot *spuriously* suspect a live peer. Note the dual, documented in
        // the seam: this same flooring *suppresses* suspicion of a genuinely dead peer under a backward
        // skew — the saturation is one-sided, not a free safety win.
        let mon = Monitor::<1>::new(200, 5);
        assert!(mon.suspect(100).is_none(), "now < last_heard → 0 silence, within budget");
    }
}

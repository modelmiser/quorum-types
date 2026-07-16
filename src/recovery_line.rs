//! **Recovery line** — uncoordinated checkpoint rollback recovery, and the
//! **domino effect** it can trigger (Randell 1975; Koo & Toueg 1987). The backward
//! dual of `message_log`'s roll-forward recovery.
//!
//! When processes checkpoint *independently* (no coordination), their latest
//! checkpoints need not form a consistent global state. To recover after a crash you
//! must find a **recovery line**: a set of one checkpoint per process that is a
//! consistent cut — the greatest one at or below the checkpoints you would like to
//! keep. Finding it is not a yes/no check but a **search**: whenever the tentative
//! line retains a message's *receive* but drops its *send* (an orphan), you must roll
//! the receiver back below that receive — which may drop a send *it* made, orphaning
//! a further process, and so on. That cascade is Randell's **domino effect**; in the
//! worst case it rolls every process back to its initial state.
//!
//! ## Relationship to [`consistent_cut`](crate::consistent_cut)
//!
//! This rung *reuses* [`consistent_cut`](crate::consistent_cut)'s orphan predicate —
//! received-inside ∧ sent-outside — but does a different job with it.
//! [`consistent_cut`](crate::consistent_cut) **checks** a fixed frontier and accepts
//! or rejects it. A recovery line must instead **compute** the largest consistent
//! frontier ≤ the failure frontier: not "is this cut consistent?" but "what is the
//! greatest consistent cut below it?" — a fixpoint, whose distance from the starting
//! frontier is the domino depth. Checking membership versus computing the meet.
//!
//! ## The mechanism — resolution is a phase transition, not a token
//!
//! The frontier is a [`vclock`](crate::vclock)-shaped array: `line[i]` is how many of
//! process `i`'s intervals are retained (its restored checkpoint). A line moves
//! through two phases:
//!
//! * [`Line`]`<`[`Tentative`]`, N>` — the checkpoints you start from (a failure
//!   frontier), built by [`from_checkpoints`](Line::from_checkpoints). No promise of
//!   consistency.
//! * [`resolve`](Line::resolve) — defined **only** on `Line<`[`Tentative`]`, N>` —
//!   **consumes** the line and rolls processes back until no orphan remains,
//!   re-emitting the greatest consistent cut ≤ the start as
//!   `Line<`[`Resolved`]`, N>`. Because resolution *transforms the line itself*
//!   rather than handing back a free-floating "resolved" token, a resolution of one
//!   frontier can never certify a different, unresolved one — the same fusion
//!   [`consistent_cut`](crate::consistent_cut) and [`occ`](crate::occ) use to head
//!   off the crate's recurring detachable-witness hazard. All the mutable search
//!   state lives *inside* [`resolve`](Line::resolve); the resolved line is immutable.
//! * A [`Resolved`] line is what a rollback layer may restore to
//!   ([`restore`](Line::restore) is defined only there), and it reports how far the
//!   domino reached ([`rolled_back`](Line::rolled_back), [`dominoed`](Line::dominoed)).
//!
//! Unlike `message_log`'s purely structural phase wall, the
//! [`Tentative`]→[`Resolved`] door is a **runtime** search over the dependency log —
//! so this rung's guarantee rests on a trusted runtime witness (the
//! [`consistent_cut`](crate::consistent_cut)/[`vclock`](crate::vclock) species): the
//! log must be complete and its interval indices real. See the seam.
//!
//! ## A consistent failure frontier resolves with no rollback
//!
//! ```
//! use quorum_types::recovery_line::{Line, Dependency};
//! // P0 sent in interval 2, P1 received it in interval 2. Keeping both is consistent.
//! let deps = [Dependency { sender: 0, send_ivl: 2, receiver: 1, recv_ivl: 2 }];
//! let line = Line::from_checkpoints([2, 2]).resolve(&deps);
//! assert_eq!(line.at(0), 2);
//! assert_eq!(line.at(1), 2);
//! assert!(!line.dominoed(), "the send is retained, so nothing rolls back");
//! ```
//!
//! ## The domino effect — one dropped interval cascades down a chain
//!
//! ```
//! use quorum_types::recovery_line::{Line, Dependency};
//! // A chain of causal messages: P0 -> P1 -> P2, each sent in interval 2.
//! let deps = [
//!     Dependency { sender: 0, send_ivl: 2, receiver: 1, recv_ivl: 2 },
//!     Dependency { sender: 1, send_ivl: 2, receiver: 2, recv_ivl: 2 },
//! ];
//! // P0 can only be restored to checkpoint 1 (its interval 2 is lost). That drops
//! // P0's send to P1, so P1 must roll back below the receive — which drops P1's send
//! // to P2, so P2 must roll back too. One lost interval dominoes across all three.
//! let line = Line::from_checkpoints([1, 2, 2]).resolve(&deps);
//! assert_eq!(line.at(0), 1);
//! assert_eq!(line.at(1), 1, "P1 rolled back by the domino");
//! assert_eq!(line.at(2), 1, "P2 rolled back by the domino");
//! assert!(line.dominoed());
//! assert_eq!(line.rolled_back(2), 1, "P2 lost one interval it never crashed in");
//! ```
//!
//! ## Restoring to an unresolved line is a compile error
//!
//! [`restore`](Line::restore) exists only on `Line<`[`Resolved`]`, N>`. A tentative
//! failure frontier — which may contain orphans — has no `restore`:
//!
//! ```compile_fail
//! use quorum_types::recovery_line::Line;
//! let tentative = Line::from_checkpoints([1, 2, 2]);
//! let _ = tentative.restore(); // no `restore` on Line<Tentative, N>: resolve it first
//! ```
//!
//! ## You cannot fabricate a resolved line by hand
//!
//! [`Line`]'s fields are private, so a `Line<Resolved, N>` cannot be built directly —
//! the only route into [`Resolved`] is [`resolve`](Line::resolve):
//!
//! ```compile_fail
//! use quorum_types::recovery_line::{Line, Resolved};
//! use core::marker::PhantomData;
//! let forged: Line<Resolved, 2> =
//!     Line { frontier: [9, 9], initial: [9, 9], _ph: PhantomData }; // private fields
//! let _ = forged.at(0);
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! [`resolve`](Line::resolve) is an **exact, total** search: over the frontier and
//! the dependency log it is handed, it returns the greatest orphan-free cut below the
//! start, with no slack (and it always terminates — line entries only decrease and are
//! bounded at 0, the all-initial cut being trivially consistent). What it cannot check
//! is that the inputs are honest:
//!
//! * **The log is complete.** A dependency missing from the log cannot force a
//!   rollback it should — an omitted orphan leaves the line falsely high. This is the
//!   [`consistent_cut`](crate::consistent_cut) seam: a recovery line is only as
//!   truthful as the layer that records sends and receives.
//! * **The indices are real.** `send_ivl`/`recv_ivl` are trusted to be the true
//!   interval positions and `line[i]` the true retained-interval count. The type owns
//!   the *search* (roll back every orphan to its greatest consistent fixpoint), not
//!   the bookkeeping that produced the numbers.
//!
//! The two rungs bound the recovery design space: `recovery_line` pays at *recovery*
//! time (cheap uncoordinated checkpoints, but a possibly-unbounded rollback search),
//! while `message_log` pays at *logging* time (log before every
//! delivery, but recovery replays forward with no rollback). Coordinated
//! checkpointing — recording along a [`consistent_cut`](crate::consistent_cut) so the
//! latest checkpoints are *always* a recovery line — is the third corner, and reduces
//! this search to the identity.

use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A phase of a recovery line. Sealed: the only phases are [`Tentative`] and
/// [`Resolved`], so a `Line<Resolved, N>` can arise only through
/// [`resolve`](Line::resolve), never by naming a phase directly.
pub trait Phase: sealed::Sealed {}

/// The **tentative** phase: a failure frontier of checkpoints with no consistency
/// promise — it may retain orphans.
#[derive(Debug)]
pub struct Tentative;
/// The **resolved** phase: [`resolve`](Line::resolve) rolled back every orphan, so
/// the line is a consistent cut and may be restored to.
#[derive(Debug)]
pub struct Resolved;

impl sealed::Sealed for Tentative {}
impl sealed::Sealed for Resolved {}
impl Phase for Tentative {}
impl Phase for Resolved {}

/// A rollback dependency induced by a message: sent during interval
/// [`send_ivl`](Dependency::send_ivl) of process [`sender`](Dependency::sender), and
/// received during interval [`recv_ivl`](Dependency::recv_ivl) of process
/// [`receiver`](Dependency::receiver). Interval indices are 1-based; a `line` value of
/// `k` for a process means its intervals `1..=k` are retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dependency {
    /// The sending process index.
    pub sender: usize,
    /// The sender's interval in which the message was sent.
    pub send_ivl: u64,
    /// The receiving process index.
    pub receiver: usize,
    /// The receiver's interval in which the message was delivered.
    pub recv_ivl: u64,
}

/// A recovery line over `N` processes, in phase `PH`. `line[i]` is how many of process
/// `i`'s intervals are retained (its restored checkpoint).
///
/// Move-only and `#[must_use]`: a tentative line is resolved by
/// [`resolve`](Line::resolve). The fields are private — a `Line<Resolved, N>` cannot
/// be forged; the only resolved line is one [`resolve`](Line::resolve) re-emitted.
#[must_use = "a tentative recovery line must be resolved before a process can be restored to it"]
pub struct Line<PH: Phase, const N: usize> {
    frontier: [u64; N],
    initial: [u64; N],
    _ph: PhantomData<PH>,
}

impl<const N: usize> Line<Tentative, N> {
    /// Start from a set of per-process checkpoints (a failure frontier). Carries no
    /// promise of consistency until [`resolve`](Line::resolve) rolls back its orphans.
    pub const fn from_checkpoints(frontier: [u64; N]) -> Self {
        Line { frontier, initial: frontier, _ph: PhantomData }
    }

    /// **Roll back to the greatest consistent cut** at or below the start, consuming
    /// the line. Repeatedly, for every dependency whose *receive* is retained but
    /// whose *send* has been dropped (an orphan), roll the receiver back below that
    /// receive — the domino step — until a full pass makes no change. Re-emits the
    /// resolved frontier as a [`Resolved`] line; the tentative one is gone.
    ///
    /// Consuming `self` is what fuses the result to *this* search: there is no separate
    /// token that could certify a different, unresolved frontier.
    ///
    /// # Panics
    /// If any dependency names a process index `>= N` (`d.sender` or `d.receiver`),
    /// like the bounds panic [`vclock::VClock::tick`](crate::vclock::VClock::tick)
    /// documents. The process count is a type parameter; a dependency naming a
    /// non-existent process is a caller error.
    pub fn resolve(self, deps: &[Dependency]) -> Line<Resolved, N> {
        let mut frontier = self.frontier;
        loop {
            let mut changed = false;
            for d in deps {
                let received_retained = d.recv_ivl <= frontier[d.receiver];
                let sent_dropped = d.send_ivl > frontier[d.sender];
                // Orphan: the receive survives but its cause does not. Drop the receive
                // too — roll the receiver back below it. This may drop a send it made,
                // orphaning a further process on the next pass. `changed` must track an
                // *actual* decrease, not merely that the orphan predicate fired: the
                // termination argument (entries strictly decrease, bounded at 0) rests
                // on that, so a no-op write (e.g. an out-of-contract `recv_ivl == 0`,
                // already floored) must not spin the fixpoint forever.
                let target = d.recv_ivl.saturating_sub(1);
                if received_retained && sent_dropped && target < frontier[d.receiver] {
                    frontier[d.receiver] = target;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Line { frontier, initial: self.initial, _ph: PhantomData }
    }
}

impl<PH: Phase, const N: usize> Line<PH, N> {
    /// How many of process `i`'s intervals are retained by the line.
    ///
    /// # Panics
    /// If `i >= N` (bounds), as [`vclock::VClock::tick`](crate::vclock::VClock::tick).
    pub const fn at(&self, i: usize) -> u64 {
        self.frontier[i]
    }
}

impl<const N: usize> Line<Resolved, N> {
    /// How many intervals process `i` was rolled back by resolution — the domino's
    /// reach at that process (its starting checkpoint minus its resolved one).
    ///
    /// # Panics
    /// If `i >= N` (bounds).
    pub const fn rolled_back(&self, i: usize) -> u64 {
        self.initial[i] - self.frontier[i]
    }

    /// Whether resolution had to roll *any* process back — i.e. the starting frontier
    /// was not already a consistent cut, so the domino fired.
    pub fn dominoed(&self) -> bool {
        let mut i = 0;
        while i < N {
            if self.initial[i] != self.frontier[i] {
                return true;
            }
            i += 1;
        }
        false
    }

    /// The restore target: the retained-interval count per process. Reachable only on
    /// a [`Resolved`] line, so a process can never be restored to a frontier that still
    /// contains an orphan.
    pub const fn restore(&self) -> [u64; N] {
        self.frontier
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_consistent_frontier_resolves_without_rollback() {
        // Send in interval 2 retained by both endpoints: no orphan, no rollback.
        let deps = [Dependency { sender: 0, send_ivl: 2, receiver: 1, recv_ivl: 2 }];
        let line = Line::from_checkpoints([2, 2]).resolve(&deps);
        assert_eq!(line.at(0), 2);
        assert_eq!(line.at(1), 2);
        assert!(!line.dominoed());
        assert_eq!(line.rolled_back(1), 0);
    }

    #[test]
    fn one_dropped_interval_dominoes_down_a_chain() {
        let deps = [
            Dependency { sender: 0, send_ivl: 2, receiver: 1, recv_ivl: 2 },
            Dependency { sender: 1, send_ivl: 2, receiver: 2, recv_ivl: 2 },
        ];
        // P0 restored only to checkpoint 1 -> its send drops -> P1 rolls back -> P1's
        // send drops -> P2 rolls back. The cascade.
        let line = Line::from_checkpoints([1, 2, 2]).resolve(&deps);
        assert_eq!([line.at(0), line.at(1), line.at(2)], [1, 1, 1]);
        assert!(line.dominoed());
        assert_eq!(line.rolled_back(0), 0, "P0 was the crash, not rolled back further");
        assert_eq!(line.rolled_back(1), 1);
        assert_eq!(line.rolled_back(2), 1);
        assert_eq!(line.restore(), [1, 1, 1]);
    }

    #[test]
    fn an_empty_log_resolves_to_the_start() {
        // No dependencies -> nothing can orphan -> the frontier is already consistent.
        let line = Line::from_checkpoints([5, 3, 7]).resolve(&[]);
        assert_eq!(line.restore(), [5, 3, 7]);
        assert!(!line.dominoed());
    }

    #[test]
    fn an_in_transit_message_forces_no_rollback() {
        // Sent in interval 2 (retained), received in interval 3 (dropped): in flight
        // across the line, not an orphan — so no process rolls back.
        let deps = [Dependency { sender: 0, send_ivl: 2, receiver: 1, recv_ivl: 3 }];
        let line = Line::from_checkpoints([2, 2]).resolve(&deps);
        assert_eq!(line.restore(), [2, 2]);
        assert!(!line.dominoed());
    }

    #[test]
    fn an_out_of_contract_zero_interval_still_terminates() {
        // recv_ivl == 0 is out of the 1-based contract, but must not spin the fixpoint:
        // the write is a floored no-op, so `changed` never fires on it and resolve halts.
        let deps = [Dependency { sender: 0, send_ivl: 1, receiver: 1, recv_ivl: 0 }];
        let line = Line::from_checkpoints([0, 0]).resolve(&deps);
        assert_eq!(line.restore(), [0, 0], "already at the initial state, nothing to roll back");
        assert!(!line.dominoed());
    }

    #[test]
    fn the_domino_can_reach_the_initial_state() {
        // A two-hop chain where every send is in the first retained interval: dropping
        // P0's interval 1 rolls the whole chain to nothing (checkpoint 0).
        let deps = [
            Dependency { sender: 0, send_ivl: 1, receiver: 1, recv_ivl: 1 },
            Dependency { sender: 1, send_ivl: 1, receiver: 2, recv_ivl: 1 },
        ];
        let line = Line::from_checkpoints([0, 1, 1]).resolve(&deps);
        assert_eq!(line.restore(), [0, 0, 0], "rolled all the way to the initial state");
        assert!(line.dominoed());
    }
}

//! Commit-wait / external consistency (Spanner's TrueTime) — the *physical-clock* rung.
//!
//! Every clock the crate has typed so far is *logical*: a [`session`](crate::session)
//! watermark, a [`causal`](crate::causal) chain. Logical time is honest by construction —
//! it counts real events. **Physical** time is the opposite: a wall clock can simply be
//! *wrong*, off by an unknown skew. Spanner's TrueTime confronts that by exposing the
//! uncertainty explicitly — `TT.now()` returns an *interval* `[earliest, latest]`
//! guaranteed to bracket true time — and buys **external consistency** (if T1 commits
//! before T2 starts in real time, `ts(T1) < ts(T2)`) with **commit-wait**: after stamping
//! a commit at the interval's upper bound, a transaction *waits out the uncertainty* before
//! letting anyone observe it.
//!
//! This rung types that wait. You cannot type *what time it is*; you can type *that you
//! waited for the uncertainty to close* before externalizing a write.
//!
//! ## The mechanism
//!
//! * [`TrueTime`] has an uncertainty width `ε`; [`now`](TrueTime::now) turns a raw clock
//!   reading `r` into the interval `[r, r + ε]` that brackets true time.
//! * A commit stamps itself at the interval's **latest** bound and enters
//!   [`Pending<T>`] — a move-only state that exposes the assigned timestamp (needed to
//!   test the wait) but **not** the value: a pending commit is not yet observable.
//! * [`try_release`](Pending::try_release) consumes a *later* interval and succeeds
//!   **only if** its `earliest` has passed the commit timestamp (the ε window closed),
//!   yielding an [`Externalized<T>`]. Only [`Externalized`] exposes
//!   [`value`](Externalized::value) — the externally observable read.
//!
//! ## Reading a value *back through the API* before its window closes is a compile error
//!
//! [`Pending`] has no `value` accessor; a value handed to a commit is reachable again only
//! through [`Externalized`], and the only route there is a successful `try_release`. (This
//! gates the value *routed through* `Pending → Externalized`; a caller that kept its own
//! copy of the payload can still read that — "externalize" here means "reveal via this
//! handle," not "no copy exists anywhere". See the seam.)
//!
//! ```compile_fail
//! use quorum_types::commit_wait::{TrueTime, Pending};
//! let tt = TrueTime::new(10);
//! let pending = Pending::stamp("write", tt.now(100)); // commit_ts = 110
//! let _ = pending.value(); // ERROR: no method `value` on `Pending` — not yet externalizable
//! ```
//!
//! ## The happy path — stamp, wait out ε, then externalize
//!
//! ```
//! use quorum_types::commit_wait::{TrueTime, Pending};
//! let tt = TrueTime::new(10); // ε = 10
//!
//! // Commit at reading 100 → interval [100, 110] → commit_ts = 110 (the upper bound).
//! let pending = Pending::stamp("v", tt.now(100));
//! assert_eq!(pending.commit_ts(), 110);
//!
//! // Too early: a reading of 105 → [105,115], earliest 105 has NOT passed 110.
//! let pending = match pending.try_release(tt.now(105)) {
//!     Ok(_) => unreachable!("the window is still open"),
//!     Err(still_pending) => still_pending,
//! };
//!
//! // Waited enough: a reading of 111 → [111,121], earliest 111 > 110 → externalize.
//! let externalized = pending.try_release(tt.now(111)).ok().unwrap();
//! assert_eq!(*externalized.value(), "v");
//! assert_eq!(externalized.commit_ts(), 110);
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! The type owns exactly one thing: the value **routed through this handle** was not
//! revealed until *a clock reading* said the uncertainty window had closed. Everything the
//! *guarantee* rests on is runtime trust, not compile-time fact:
//!
//! * **Only the value through the handle is gated.** [`Pending::stamp`] takes the value by
//!   move, but nothing stops a caller keeping its own copy of a `Copy`/`Clone` payload and
//!   revealing *that* with no wait. The types gate the `Pending → Externalized` path, not
//!   the existence of a copy — the same limit [`reconcile`](crate::reconcile) states (types
//!   gate *standing*, not out-of-band computation over readable values).
//! * **The reading is trusted.** [`TrueTime::now`] takes the raw reading as an argument;
//!   the witness certifies "a reading past the window was observed," not that the reading
//!   was truthful — the same declared-vs-true boundary as [`session::Fresh`](crate::session::Fresh).
//! * **ε must be a real bound.** External consistency holds only if `[r, r+ε]` *actually*
//!   brackets true time — TrueTime's GPS/atomic-clock hardware assumption. A too-small ε
//!   silently breaks the guarantee; the types cannot check the physics. (The interval is
//!   modelled one-sidedly as `[r, r+ε]` — the raw reading is the lower bound — a
//!   simplification of TrueTime's ~symmetric `[c−ε, c+ε]`, since only the `earliest` lower
//!   bound is load-bearing for commit-wait.)
//! * **Every transaction must commit-wait.** This types *one* transaction's local wait.
//!   External consistency is a *global* property — it needs *all* writers to wait and a
//!   consistent ε — exactly the shape of [`byzantine`](crate::byzantine)'s fault budget: a
//!   local, checkable discipline standing in for a global, trusted one.

/// A TrueTime clock with uncertainty width `ε`. [`now`](TrueTime::now) brackets a raw
/// reading into the interval guaranteed to contain true time.
#[derive(Debug, Clone, Copy)]
pub struct TrueTime {
    epsilon: u64,
}

impl TrueTime {
    /// A clock whose readings are uncertain by up to `epsilon`.
    #[must_use]
    pub const fn new(epsilon: u64) -> Self {
        TrueTime { epsilon }
    }

    /// Bracket a raw clock `reading` into `[reading, reading + ε]` — the interval that
    /// (under the ε assumption) contains true time.
    #[must_use]
    pub const fn now(&self, reading: u64) -> Interval {
        Interval { earliest: reading, latest: reading + self.epsilon }
    }

    /// The uncertainty width.
    #[must_use]
    pub const fn epsilon(&self) -> u64 {
        self.epsilon
    }
}

/// A TrueTime interval `[earliest, latest]` guaranteed to bracket true time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    earliest: u64,
    latest: u64,
}

impl Interval {
    /// The earliest instant true time could be — a *lower* bound (once this has passed a
    /// timestamp, that timestamp is definitely in the past).
    #[must_use]
    pub const fn earliest(&self) -> u64 {
        self.earliest
    }

    /// The latest instant true time could be — an *upper* bound (the conservative choice
    /// for a commit timestamp).
    #[must_use]
    pub const fn latest(&self) -> u64 {
        self.latest
    }
}

/// A committed value that has been stamped but **not yet externalized**. Move-only; its
/// value is deliberately unreadable — a commit becomes observable only after its
/// uncertainty window closes, via [`try_release`](Pending::try_release).
#[must_use = "a Pending commit is invisible until released; try_release it once the window closes, or its write is never externalized"]
pub struct Pending<T> {
    value: T,
    commit_ts: u64,
}

impl<T> Pending<T> {
    /// Stamp a commit at the **latest** bound of `now` — the conservative timestamp no
    /// concurrent reading can already have surpassed. Enters the pending (unobservable)
    /// state.
    pub fn stamp(value: T, now: Interval) -> Self {
        Pending { value, commit_ts: now.latest }
    }

    /// The assigned commit timestamp. Knowable so the wait can be tested — but the *value*
    /// is not, because the commit is not yet externally consistent.
    #[must_use]
    pub const fn commit_ts(&self) -> u64 {
        self.commit_ts
    }

    /// **Commit-wait.** Attempt to externalize, given a *later* clock interval. Succeeds
    /// only when `now.earliest > commit_ts` — i.e. true time has provably passed the
    /// timestamp, so it is safe to reveal. Otherwise returns the still-`Pending` commit to
    /// wait more.
    pub fn try_release(self, now: Interval) -> Result<Externalized<T>, Pending<T>> {
        if now.earliest > self.commit_ts {
            Ok(Externalized { value: self.value, commit_ts: self.commit_ts })
        } else {
            Err(self)
        }
    }
}

/// A commit whose uncertainty window has provably closed — the **only** state from which
/// the committed value may be read. Reaching it required a clock reading past the commit
/// timestamp (commit-wait), so an externalized timestamp is guaranteed to be in the past.
#[must_use = "an externalized commit carries the value you waited to reveal; use it"]
pub struct Externalized<T> {
    value: T,
    commit_ts: u64,
}

impl<T> Externalized<T> {
    /// The externally observable value — safe to reveal because commit-wait completed.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// The commit timestamp, now guaranteed to lie in the past.
    #[must_use]
    pub const fn commit_ts(&self) -> u64 {
        self.commit_ts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_uses_the_latest_bound() {
        let tt = TrueTime::new(7);
        let now = tt.now(50);
        assert_eq!((now.earliest(), now.latest()), (50, 57));
        let pending = Pending::stamp("x", now);
        assert_eq!(pending.commit_ts(), 57, "commit stamps at the interval's upper bound");
    }

    #[test]
    fn release_before_the_window_closes_is_refused() {
        let tt = TrueTime::new(10);
        let pending = Pending::stamp(1, tt.now(100)); // commit_ts = 110
        // earliest 100..=110 have not passed 110 — still pending.
        let pending = pending.try_release(tt.now(100)).err().expect("earliest 100 !> 110");
        let pending = pending.try_release(tt.now(110)).err().expect("earliest 110 !> 110");
        // 111 passes: earliest 111 > 110.
        let ext = pending.try_release(tt.now(111)).ok().expect("earliest 111 > 110");
        assert_eq!(*ext.value(), 1);
        assert_eq!(ext.commit_ts(), 110);
    }

    #[test]
    fn wait_duration_is_about_epsilon() {
        // A commit stamped from reading r must wait until a reading > r + ε — roughly ε of
        // real time (this is the throughput cost commit-wait pays for external consistency).
        let tt = TrueTime::new(20);
        let pending = Pending::stamp((), tt.now(1000)); // commit_ts = 1020
        let pending = pending.try_release(tt.now(1019)).err().expect("must still wait at r+ε-1");
        // Just past r+ε (a wait of ~ε real time), a real release succeeds.
        assert!(pending.try_release(tt.now(1021)).is_ok(), "at r+ε+1 the window has closed");
    }

    #[test]
    fn zero_uncertainty_needs_only_a_strictly_later_reading() {
        // With ε = 0 the interval is a point; you still cannot externalize at the same
        // instant (external consistency needs strict pastness), only strictly after.
        let tt = TrueTime::new(0);
        let pending = Pending::stamp("v", tt.now(5)); // commit_ts = 5
        assert!(pending.try_release(tt.now(5)).is_err(), "same instant is not strictly past");
        let tt2 = TrueTime::new(0);
        let pending2 = Pending::stamp("v", tt2.now(5));
        assert!(pending2.try_release(tt2.now(6)).is_ok());
    }
}

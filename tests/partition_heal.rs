//! Deterministic in-process partition/heal simulation.
//!
//! This is the executable end of the arc: the base module proved cross-epoch
//! merge is a compile error, `tla/quorum.tla` proved the epoch is *necessary but
//! not sufficient*, and `src/failover.rs` encoded the runtime lease guard. Here
//! we drive that real API through a crash → partition → heal cycle and assert
//! the safety property the TLA+ model checks — `NoSplitBrain` — holds throughout.
//!
//! Crucially the safety DECISION (may we fail over yet?) is delegated to the
//! library's [`reconfigure`]: the harness only *respects* its `Result`. The
//! `serving` list is test bookkeeping standing in for the model's `serving` set.
//!
//! A note on the const-generic seam: `Leased<const E, All>` fixes the epoch in
//! the type, so a runtime loop cannot bump epochs into a variable — the sim uses
//! concrete epochs `0 → 1 → 2` with explicit turbofish. That friction is itself
//! on-thesis: the type-level epoch is a *within-configuration* property; a
//! runtime simulator lives on the dynamic side of the `gradual` boundary.

use quorum_types::All;
use quorum_types::failover::{reconfigure, FailoverError, Lease, Leased, Tick};

const TTL: Tick = 5;

/// Harness state. `serving` holds the epochs whose leader currently believes it
/// is authoritative — the quantity `NoSplitBrain` bounds.
struct Cluster {
    now: Tick,
    serving: Vec<u64>,
    hi_reachable: bool,
}

impl Cluster {
    fn new() -> Self {
        Cluster { now: 0, serving: Vec::new(), hi_reachable: true }
    }

    /// The invariant the TLA+ model checks, asserted at every step.
    fn assert_no_split_brain(&self) {
        assert!(
            self.serving.len() <= 1,
            "split-brain: {:?} serving at t={}",
            self.serving,
            self.now
        );
    }
}

/// Replays the TLA+ counterexample trace as an executable scenario, then heals,
/// and asserts `NoSplitBrain` at every step. The refusal at t=3 is exactly the
/// `quorum_noguard.cfg` State-3→4 transition, prevented here by the real guard.
#[test]
fn replays_trace_and_heals_without_split_brain() {
    let mut c = Cluster::new();

    // t=0: leader forms a full quorum at epoch 0, lease valid through t=5.
    let lease0 = Lease::new(c.now, TTL);
    let leader0: Leased<0, All> = Leased::genesis(0xDEAD_BEEF, lease0);
    c.serving.push(0);
    c.assert_no_split_brain();

    // t=1: partition — the `hi` holder becomes unreachable.
    c.now = 1;
    c.hi_reachable = false;
    c.assert_no_split_brain();

    // t=3: premature failover attempt. The lease is still valid, so the library
    // REFUSES — the harness must not crown epoch 1. This is the guard working.
    c.now = 3;
    let premature = reconfigure::<1>(lease0, c.now, 0xFACE, Lease::new(c.now, TTL));
    assert!(
        matches!(premature, Err(FailoverError::LeaseStillValid { until: 5 })),
        "guard must refuse failover while the old lease is valid"
    );
    // Because it was Err, epoch 1 is NOT added to `serving`.
    c.assert_no_split_brain();
    assert_eq!(c.serving, vec![0]);

    // t=6: the epoch-0 lease has lapsed. Retire the old leader first (affine
    // surrender succeeds now that its lease is expired), then fail over.
    c.now = 6;
    assert!(leader0.surrender(c.now).is_ok(), "expired leader can step down");
    c.serving.retain(|&e| e != 0);

    let leader1: Leased<1, All> = reconfigure::<1>(lease0, c.now, 0xF00D_1234, Lease::new(c.now, TTL))
        .expect("failover must succeed once the prior lease has lapsed");
    c.serving.push(1);
    assert_eq!(leader1.epoch(), 1);
    c.assert_no_split_brain();

    // t=7: heal. The old `hi` holder returns believing it still leads epoch 0 —
    // but its lease (until t=5) is invalid now, so it cannot re-serve; it can
    // only step down. No split-brain results.
    c.now = 7;
    c.hi_reachable = true;
    assert!(!lease0.is_valid(c.now), "the revived stale leader's lease is dead");
    c.assert_no_split_brain();
    assert_eq!(c.serving, vec![1]);
}

/// Exhaustively drives the guard across the whole time domain: `reconfigure`
/// must return `Ok` iff the attempt is strictly after the prior lease expires.
/// This pins the boundary the whole design rests on, exercising real code.
#[test]
fn guard_boundary_is_exact_over_time() {
    let lease0 = Lease::new(0, TTL); // valid through t=5

    for now in 0..=12 {
        let r = reconfigure::<1>(lease0, now, 0xAB, Lease::new(now, TTL));
        let expected_ok = now > lease0.expires_after();
        assert_eq!(
            r.is_ok(),
            expected_ok,
            "at t={now}: failover Ok should be {expected_ok} (lease expires_after={})",
            lease0.expires_after()
        );
    }
}

/// Documents that the guard is load-bearing: there is a non-empty window
/// (`0..=5`) during which an old leader still holds authority AND a naive
/// failover would form a second one. The library's refusal over exactly this
/// window is what prevents the split-brain the negative-control model found.
#[test]
fn refusal_window_matches_old_lease_lifetime() {
    let lease0 = Lease::new(0, TTL);

    let refused: Vec<Tick> = (0..=12)
        .filter(|&now| reconfigure::<1>(lease0, now, 0, Lease::new(now, TTL)).is_err())
        .collect();

    // Refused for exactly the ticks the old lease is still alive.
    assert_eq!(refused, vec![0, 1, 2, 3, 4, 5]);
    assert!(refused.iter().all(|&t| lease0.is_valid(t)));
}

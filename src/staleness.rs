//! Bounded-staleness reads — the *read-path* physical-time rung.
//!
//! [`commit_wait`](crate::commit_wait) types the write path: a write is held back until its
//! uncertainty window closes. This module types the mirror choice on the **read** path.
//! A read has a dial between two extremes:
//!
//! * a **linearizable** read must reflect the latest committed write — it needs the leader
//!   (or a quorum): *coordination*;
//! * a **bounded-staleness** read may be served from a lagging follower **locally**, with
//!   no coordination, *provided the client accepts an age bound* `Δ`.
//!
//! The tradeoff is exactly the crate's recurring cut — pay coordination for freshness, or
//! go local and accept staleness — and here the staleness budget `Δ` rides **in the type**.
//!
//! Note this is *physical* recency, orthogonal to [`session`](crate::session)'s *logical*
//! freshness: a session read proves the replica reflects *your writes* (read-your-writes);
//! a bounded-staleness read promises only that the data is *at most `Δ` old*, regardless of
//! whose writes those are.
//!
//! ## The mechanism
//!
//! * A [`Replica<T>`] carries its value and the timestamp it was `last_applied`.
//! * [`read_within::<Δ>`](Replica::read_within) serves a local read, returning
//!   `Some(`[`Staleness<Δ, T>`]`)` **iff** the measured lag `now − last_applied ≤ Δ`. The
//!   age bound is a **const generic**, so it is part of the read's type.
//! * [`require::<TOL>`](Staleness::require) spends a `Staleness<Δ>` where a caller tolerates
//!   up to `TOL` staleness. It carries an inline `const { assert!(Δ ≤ TOL) }`: a read whose
//!   bound is looser than the tolerance is a **compile error** (the same const-gate
//!   [`flex`](crate::flex) uses for `R + W > N`).
//! * The strong path is a **distinct type**: [`Replica::read_linearizable`] needs a
//!   [`LeaderLease`] and yields a [`Linearizable<T>`], which no `Staleness<Δ>` can stand in
//!   for.
//!
//! ## Using a too-stale read where a tighter bound is required is a compile error
//!
//! ```compile_fail
//! use quorum_types::staleness::Replica;
//! let replica = Replica::new("v", 100);
//! // A read known only to be ≤ 50 stale...
//! let read = replica.read_within::<50>(120).unwrap();
//! // ...cannot satisfy a caller that tolerates at most 10: 50 ≤ 10 is false.
//! let _v = read.require::<10>(); // ERROR: const assertion Δ ≤ TOL fails at compile time
//! ```
//!
//! ## A bounded-staleness read is not a linearizable read
//!
//! ```compile_fail
//! use quorum_types::staleness::{Replica, Linearizable};
//! fn needs_fresh<T>(_: Linearizable<T>) {}
//! let replica = Replica::new(7, 100);
//! let stale = replica.read_within::<50>(120).unwrap();
//! needs_fresh(stale); // ERROR: expected `Linearizable<_>`, found `Staleness<50, _>`
//! ```
//!
//! ## The happy path — a tight read satisfies a looser tolerance
//!
//! ```
//! use quorum_types::staleness::Replica;
//! let replica = Replica::new("data", 100);
//!
//! // At now = 130 the follower is 30 old; a ≤ 50 read succeeds.
//! let read = replica.read_within::<50>(130).expect("30 ≤ 50");
//! // A ≤ 50 read satisfies a client tolerating ≤ 100: 50 ≤ 100 holds.
//! assert_eq!(read.require::<100>(), "data");
//!
//! // Too stale for the bound → no read (the runtime seam).
//! assert!(replica.read_within::<50>(200).is_none()); // 100 old > 50
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! * **The lag is trusted, and the skew is asymmetric.** `now − last_applied` assumes `now`
//!   and `last_applied` are on comparable, synchronized clocks; a skewed clock reports a
//!   false age. The witness certifies "a lag measurement came in under `Δ`," not that the
//!   data is truly that fresh — the same declared-vs-true boundary as
//!   [`commit_wait`](crate::commit_wait). Worse, the failure is *one-sided*: a replica whose
//!   `last_applied` runs *ahead* of `now` (a fast/bogus clock reporting a future timestamp)
//!   saturates the measured lag to `0` and passes *any* `Δ`, even `Δ = 0` — the skew that
//!   most flatters freshness is exactly the one the measurement hides.
//! * **`Δ ≤ TOL` is *sufficient*, not a freshness proof.** It guarantees the *declared*
//!   bound composes correctly; whether that bound reflects reality is the clock's problem.
//! * **The lease is trusted.** [`LeaderLease`] is minted from a runtime "am I the valid
//!   leader?" check — a crude time-based detector, like [`failover`](crate::failover)'s
//!   lease; the type propagates it, it does not verify leadership.

/// A follower replica: a value and the timestamp at which it was last brought up to date.
#[derive(Debug, Clone, Copy)]
pub struct Replica<T> {
    value: T,
    last_applied: u64,
}

impl<T: Clone> Replica<T> {
    /// A replica holding `value`, last updated at `last_applied`.
    pub const fn new(value: T, last_applied: u64) -> Self {
        Replica { value, last_applied }
    }

    /// The timestamp this replica was last brought up to date.
    pub const fn last_applied(&self) -> u64 {
        self.last_applied
    }

    /// **Bounded-staleness read.** Serve a local read at age bound `Δ`: returns the value,
    /// tagged [`Staleness<Δ, T>`], iff the measured lag `now − last_applied` is within `Δ`.
    /// No coordination — but the freshness promise is only `Δ`, and only as true as the
    /// clocks (see the module's runtime seam). `None` when the replica is staler than `Δ`.
    pub fn read_within<const DELTA: u64>(&self, now: u64) -> Option<Staleness<DELTA, T>> {
        let lag = now.saturating_sub(self.last_applied);
        (lag <= DELTA).then(|| Staleness { value: self.value.clone() })
    }

    /// **Linearizable read.** *Typed as* reflecting the latest committed write — the
    /// distinction a [`Staleness`] cannot satisfy — but needs a [`LeaderLease`]
    /// (coordination). Whether the value *is* the latest is the lease's problem (trusted),
    /// exactly as `Δ` is the clock's: this toy returns the local copy and does not perform a
    /// leader round-trip. Yields a [`Linearizable<T>`], a *distinct* type from any [`Staleness`].
    pub fn read_linearizable(&self, _lease: &LeaderLease) -> Linearizable<T> {
        Linearizable { value: self.value.clone() }
    }
}

/// A value read from a follower, known to be **at most `DELTA` stale**. The age bound is a
/// type-level const; [`require`](Staleness::require) checks it against a caller's tolerance
/// at compile time.
#[must_use = "a bounded-staleness read carries an age budget; spend it via require or into_value"]
pub struct Staleness<const DELTA: u64, T> {
    value: T,
}

impl<const DELTA: u64, T> Staleness<DELTA, T> {
    /// Spend the read where the caller tolerates up to `TOL` staleness. Compiles **only**
    /// if `DELTA ≤ TOL` — a read looser than the tolerance fails the inline `const` assert
    /// after monomorphization. Returns the value.
    pub fn require<const TOL: u64>(self) -> T {
        const { assert!(DELTA <= TOL, "bounded-staleness read is looser than the required tolerance") }
        self.value
    }

    /// Accept the read at its own bound (any staleness is fine to the caller), taking the
    /// value out.
    pub fn into_value(self) -> T {
        self.value
    }

    /// The read's age bound (its type-level `DELTA`).
    #[must_use]
    pub const fn bound(&self) -> u64 {
        DELTA
    }
}

/// A value from a **linearizable** read — *typed as* reflecting the latest committed write.
/// A distinct type from [`Staleness`]: code demanding a `Linearizable` cannot be handed a
/// stale read. The freshness itself rests on the [`LeaderLease`] (trusted), not on this type.
#[must_use = "a linearizable read is typed as the latest write; use it"]
pub struct Linearizable<T> {
    value: T,
}

impl<T> Linearizable<T> {
    /// The value (typed as the latest — see [`Linearizable`]'s note on lease trust).
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Take the value out.
    pub fn into_value(self) -> T {
        self.value
    }
}

/// A leader lease — evidence permitting a linearizable read. Minted from a runtime
/// leadership/validity check ([`acquire`](LeaderLease::acquire)); the type propagates it,
/// it does not verify leadership (see the module's runtime seam).
#[derive(Debug, Clone, Copy)]
pub struct LeaderLease {
    _private: (),
}

impl LeaderLease {
    /// The runtime boundary: mint a lease iff the caller's "I am the valid leader now"
    /// check holds. A crude eventually-accurate detector, like [`failover`](crate::failover)'s.
    pub fn acquire(is_valid_leader: bool) -> Option<Self> {
        is_valid_leader.then_some(LeaderLease { _private: () })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_within_bound_succeeds_and_beyond_it_fails() {
        let replica = Replica::new("v", 100);
        assert!(replica.read_within::<50>(130).is_some(), "30 ≤ 50");
        assert!(replica.read_within::<50>(150).is_some(), "50 ≤ 50 (boundary)");
        assert!(replica.read_within::<50>(151).is_none(), "51 > 50");
    }

    #[test]
    fn a_tight_read_satisfies_a_looser_tolerance() {
        let replica = Replica::new(42, 100);
        let read = replica.read_within::<10>(105).unwrap(); // 5 ≤ 10
        assert_eq!(read.bound(), 10);
        assert_eq!(read.require::<50>(), 42); // 10 ≤ 50 compiles
    }

    #[test]
    fn require_at_the_exact_tolerance_compiles() {
        // The const gate is `Δ ≤ TOL`, not `<`: a read bounded by exactly the tolerance is
        // acceptable. (A read staler than TOL is a compile error — see the compile_fail doctest.)
        let replica = Replica::new(7, 100);
        let read = replica.read_within::<10>(108).unwrap(); // 8 ≤ 10
        assert_eq!(read.require::<10>(), 7); // Δ == TOL == 10 must compile
    }

    #[test]
    fn into_value_accepts_any_bound() {
        let replica = Replica::new("x", 0);
        let read = replica.read_within::<1000>(500).unwrap();
        assert_eq!(read.into_value(), "x");
    }

    #[test]
    fn linearizable_read_needs_a_lease() {
        let replica = Replica::new(9, 100);
        assert!(LeaderLease::acquire(false).is_none(), "not the leader → no lease");
        let lease = LeaderLease::acquire(true).expect("valid leader");
        let fresh = replica.read_linearizable(&lease);
        assert_eq!(*fresh.value(), 9);
        assert_eq!(fresh.into_value(), 9);
    }

    #[test]
    fn staleness_at_the_exact_boundary_is_within_bound() {
        // now - last_applied == DELTA is "within" (≤, not <).
        let replica = Replica::new((), 200);
        assert!(replica.read_within::<0>(200).is_some(), "0 age, Δ=0 → within");
        assert!(replica.read_within::<0>(201).is_none(), "1 age, Δ=0 → too stale");
    }
}

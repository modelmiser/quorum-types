//! The lease-degraded complement — the failure layer the epoch toy omits.
//!
//! The base module makes cross-epoch [`merge`](crate::merge) a *compile* error:
//! split-brain is unrepresentable at the type level. But that is **necessary,
//! not sufficient** — a fact established by the TLA+ model in `tla/quorum.tla`.
//! Split-brain is *temporal*: an old leader
//! keeps serving until its lease lapses, and no type can express "wait for the
//! old lease to expire." So failover needs a **runtime** lease check.
//!
//! This module encodes exactly that boundary — the `gradual` edge where static
//! epoch tracking hands off to a dynamic lease guard:
//!
//! * [`Lease`] — authority granted through an expiry tick.
//! * [`Leased<E, S>`] — a membership token at type-level epoch `E` that carries
//!   a lease. Affine: an *expired* token is released via
//!   [`surrender`](Leased::surrender), which *refuses* a still-valid one.
//!   (Caveat: Rust affinity permits dropping an unused value, so this is a
//!   discipline the API steers toward, not one the compiler forces — see the
//!   type's docs.)
//! * [`reconfigure`] — mint a full quorum at a new epoch, but only once the prior
//!   lease has expired. This is the TLA+ guard as a runtime check: the exact
//!   precondition whose removal produced split-brain in `quorum_noguard.cfg`.

use core::marker::PhantomData;

use crate::{ActiveSet, All};

/// Logical time — a monotonically advancing tick, the same domain as epochs.
pub type Tick = u64;

/// Authority granted at `granted_at`, valid through `expires_after` (inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    granted_at: Tick,
    expires_after: Tick,
}

impl Lease {
    /// A lease granted at `granted_at` for `ttl` ticks.
    pub const fn new(granted_at: Tick, ttl: Tick) -> Self {
        Self { granted_at, expires_after: granted_at + ttl }
    }

    /// Whether this lease still confers authority at logical time `now`.
    pub const fn is_valid(&self, now: Tick) -> bool {
        now <= self.expires_after
    }

    /// The tick through which this lease is valid.
    pub const fn expires_after(&self) -> Tick {
        self.expires_after
    }
}

/// A membership token at type-level epoch `E` carrying a [`Lease`].
///
/// Move-only (affine). The type-level `E` is checked *within* a configuration;
/// crossing to a new configuration goes through [`reconfigure`], where a runtime
/// lease check takes over — the boundary the TLA+ model proved unavoidable.
///
/// **Linearity caveat.** Rust affinity means an *unused* token can still be
/// dropped: `#[must_use]` flags an ignored *return value*, but nothing forces a
/// bound token to be surrendered before it leaves scope. Enforcing must-consume
/// (true linear typing) would require a panicking `Drop`; that is deliberately
/// left out of this feasibility layer. [`surrender`](Leased::surrender) is the
/// intended release path and refuses a live lease.
#[derive(Debug)]
#[must_use = "a leased quorum is authority; release it only via `surrender` once expired"]
pub struct Leased<const E: u64, S: ActiveSet> {
    members: u32,
    lease: Lease,
    _set: PhantomData<S>,
}

/// Why a [`reconfigure`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverError {
    /// The prior lease still confers authority — failing over now would allow
    /// two leaders at once (the split-brain trace from `quorum_noguard.cfg`).
    LeaseStillValid {
        /// The tick through which the prior lease remains valid.
        until: Tick,
    },
}

impl<const E: u64, S: ActiveSet> Leased<E, S> {
    /// The runtime membership bitmask.
    pub const fn members(&self) -> u32 {
        self.members
    }

    /// This token's lease.
    pub const fn lease(&self) -> Lease {
        self.lease
    }

    /// The epoch this token belongs to (mirrors the type-level `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// Affine surrender: consume an **expired** token. If the lease is still
    /// valid the token is handed back (`Err`) — you may not drop live authority,
    /// because a surviving stale leader is precisely what causes split-brain.
    pub fn surrender(self, now: Tick) -> Result<(), Self> {
        if self.lease.is_valid(now) {
            Err(self)
        } else {
            Ok(())
        }
    }
}

impl<const E: u64> Leased<E, All> {
    /// Mint a full quorum at epoch `E` with the given membership and lease.
    pub const fn genesis(members: u32, lease: Lease) -> Self {
        Self { members, lease, _set: PhantomData }
    }
}

/// Reconfigure after a suspected failure: mint a full quorum at a **new** epoch
/// `N`, but only once the prior lease has expired at logical time `now`.
///
/// This is the TLA+ guard as a runtime check. Passing the prior *lease* (not a
/// token) models failing over away from an unreachable leader whose token died
/// with it but whose expiry is known. On refusal the caller learns when the old
/// lease lapses so it can retry.
///
/// `N` is chosen at the boundary and must exceed the superseded epoch. That
/// ordering is a *logical* invariant, not statically enforced — stable Rust
/// cannot express `N > E` on const generics, and this unenforceable seam is
/// exactly the "necessary but not sufficient" the model identified.
///
/// ```
/// use quorum_types::failover::{reconfigure, Lease, FailoverError, Leased};
/// use quorum_types::All;
///
/// let prior = Lease::new(0, 5); // valid through tick 5
///
/// // Premature failover (now=3, lease still valid) is refused — this is the
/// // split-brain the negative-control TLA+ model produced, rejected here.
/// let refused = reconfigure::<1>(prior, 3, 0xFF, Lease::new(3, 5));
/// assert!(matches!(refused, Err(FailoverError::LeaseStillValid { until: 5 })));
///
/// // After the lease lapses (now=6) the new configuration may form.
/// let quorum: Leased<1, All> = reconfigure::<1>(prior, 6, 0xAB, Lease::new(6, 5)).unwrap();
/// assert_eq!(quorum.epoch(), 1);
/// assert_eq!(quorum.members(), 0xAB);
/// ```
pub fn reconfigure<const N: u64>(
    prior_lease: Lease,
    now: Tick,
    new_members: u32,
    new_lease: Lease,
) -> Result<Leased<N, All>, FailoverError> {
    if prior_lease.is_valid(now) {
        return Err(FailoverError::LeaseStillValid { until: prior_lease.expires_after });
    }
    Ok(Leased::genesis(new_members, new_lease))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_validity_boundary_is_inclusive() {
        let l = Lease::new(0, 5);
        assert!(l.is_valid(5), "valid through the expiry tick");
        assert!(!l.is_valid(6), "expired one tick later");
    }

    #[test]
    fn premature_failover_is_refused() {
        // Mirrors quorum_noguard.cfg State 3 -> 4: forming epoch 1 while the
        // epoch-0 lease is still valid. The guard rejects it.
        let prior = Lease::new(0, 5);
        let r = reconfigure::<1>(prior, 3, 0xFF, Lease::new(3, 5));
        assert_eq!(r.unwrap_err(), FailoverError::LeaseStillValid { until: 5 });
    }

    #[test]
    fn failover_after_expiry_succeeds() {
        let prior = Lease::new(0, 5);
        let q: Leased<1, All> = reconfigure::<1>(prior, 6, 0xAB, Lease::new(6, 5))
            .expect("lease expired, failover should succeed");
        assert_eq!(q.epoch(), 1);
        assert_eq!(q.members(), 0xAB);
    }

    #[test]
    fn live_token_cannot_be_surrendered() {
        let q = Leased::<0, All>::genesis(0xFF, Lease::new(0, 5));
        // Lease valid at now=3 -> surrender refused, token handed back.
        let back = q.surrender(3);
        assert!(back.is_err(), "must not drop live authority");
    }

    #[test]
    fn expired_token_surrenders() {
        let q = Leased::<0, All>::genesis(0xFF, Lease::new(0, 5));
        assert!(q.surrender(6).is_ok(), "expired token may be released");
    }
}

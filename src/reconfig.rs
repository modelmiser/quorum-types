//! Unifying the two safety mechanisms: lease-degraded **dynamic** quorums.
//!
//! [`failover`](crate::failover) gave a *temporal* guard (a lease: don't crown a
//! new leader until the old one's lease lapses). [`membership`](crate::membership)
//! gave a *structural* guard (any two majorities of one config intersect). This
//! module composes them, and composing reveals why both are needed:
//!
//! * **Within an epoch, safety is structural.** Two quorums of the *same* config
//!   share a member ([`LeasedQuorum::intersect`]) — they cannot both act without
//!   agreeing.
//! * **Across an epoch, safety is temporal.** Two quorums of *different* configs
//!   need not intersect at all — the member sets can be **disjoint** (see the
//!   test `across_epoch_quorums_can_be_disjoint`). Intersection gives nothing
//!   during a membership change, so the *lease* is the sole thing preventing an
//!   old and a new leader from both acting.
//!
//! Neither mechanism covers both regimes; each covers exactly where the other
//! fails. [`reconfigure`] therefore checks **both** at the `gradual` boundary —
//! a temporal lease check *and* a structural majority certificate — and returns
//! a [`ReconfigError`] naming whichever failed.

use crate::failover::{Lease, Tick};
use crate::membership::{Config, NodeId};
use std::collections::BTreeSet;

/// A certified majority quorum of config epoch `E` that also holds a [`Lease`].
///
/// Combines the structural certificate (`membership::Quorum`) with the temporal
/// lease (`failover::Leased`). Possessing one is evidence of *both* — a majority
/// of configuration `E`, authoritative until its lease lapses.
#[derive(Debug, Clone)]
#[must_use = "a LeasedQuorum is authority; hold it or the certification was pointless"]
pub struct LeasedQuorum<const E: u64> {
    members: BTreeSet<NodeId>,
    lease: Lease,
}

/// Why a [`reconfigure`] was refused — the two guards, each nameable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconfigError {
    /// Temporal guard (gap #1): the prior lease still confers authority.
    LeaseStillValid {
        /// Tick through which the prior lease remains valid.
        until: Tick,
    },
    /// Structural guard (gap #2): the proposed set is not a majority of the new
    /// configuration.
    NotAQuorum,
}

impl<const E: u64> LeasedQuorum<E> {
    /// Certify a subset as a leased majority quorum of `cfg`. Reuses the
    /// structural certification from [`Config::certify`] and attaches a lease.
    /// Returns `None` unless `subset` is a majority of `cfg`.
    pub fn certify(cfg: &Config<E>, subset: BTreeSet<NodeId>, lease: Lease) -> Option<Self> {
        let certified = cfg.certify(subset)?;
        Some(LeasedQuorum { members: certified.members().clone(), lease })
    }

    /// The members constituting this quorum.
    pub fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }

    /// This quorum's lease.
    pub const fn lease(&self) -> Lease {
        self.lease
    }

    /// The epoch this quorum was certified against.
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// Within-epoch structural safety: a member shared with `other`. Always
    /// `Some` for two quorums of the same config. The shared `E` is enforced by
    /// the type — cross-epoch quorums cannot be passed here (they need not
    /// intersect, so the operation would be meaningless).
    pub fn intersect(&self, other: &LeasedQuorum<E>) -> Option<NodeId> {
        self.members.intersection(&other.members).next().copied()
    }
}

/// Reconfigure to a new epoch `N` and configuration, composing **both** guards:
///
/// 1. *temporal* — refuse while `prior_lease` is still valid at `now`;
/// 2. *structural* — the new subset must be a majority of `new_cfg`.
///
/// The order matters only for which error surfaces first; both must pass. This
/// is the across-epoch boundary where intersection safety does not apply, so the
/// lease is load-bearing.
pub fn reconfigure<const N: u64>(
    prior_lease: Lease,
    now: Tick,
    new_cfg: &Config<N>,
    new_subset: BTreeSet<NodeId>,
    new_lease: Lease,
) -> Result<LeasedQuorum<N>, ReconfigError> {
    if prior_lease.is_valid(now) {
        return Err(ReconfigError::LeaseStillValid { until: prior_lease.expires_after() });
    }
    LeasedQuorum::certify(new_cfg, new_subset, new_lease).ok_or(ReconfigError::NotAQuorum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg<const E: u64>(ids: impl IntoIterator<Item = NodeId>) -> Config<E> {
        Config::new(ids.into_iter().collect())
    }

    #[test]
    fn certify_requires_majority() {
        let c = cfg::<0>([1, 2, 3, 4, 5]);
        let l = Lease::new(0, 5);
        assert!(LeasedQuorum::certify(&c, BTreeSet::from([1, 2, 3]), l).is_some());
        assert!(LeasedQuorum::certify(&c, BTreeSet::from([1, 2]), l).is_none());
    }

    #[test]
    fn within_epoch_quorums_intersect() {
        let c = cfg::<0>([1, 2, 3, 4, 5]);
        let l = Lease::new(0, 5);
        let q1 = LeasedQuorum::certify(&c, BTreeSet::from([1, 2, 3]), l).unwrap();
        let q2 = LeasedQuorum::certify(&c, BTreeSet::from([3, 4, 5]), l).unwrap();
        assert!(q1.intersect(&q2).is_some(), "structural safety within an epoch");
    }

    #[test]
    fn reconfigure_refuses_while_lease_valid() {
        let prior = Lease::new(0, 5);
        let new_cfg = cfg::<1>([1, 2, 3]);
        // now=3, prior lease valid through 5 -> temporal guard fires.
        let r = reconfigure::<1>(prior, 3, &new_cfg, BTreeSet::from([1, 2, 3]), Lease::new(3, 5));
        assert_eq!(r.unwrap_err(), ReconfigError::LeaseStillValid { until: 5 });
    }

    #[test]
    fn reconfigure_refuses_non_majority_after_expiry() {
        let prior = Lease::new(0, 5);
        let new_cfg = cfg::<1>([1, 2, 3, 4, 5]);
        // now=6, lease expired -> temporal passes; but [1,2] is not a majority.
        let r = reconfigure::<1>(prior, 6, &new_cfg, BTreeSet::from([1, 2]), Lease::new(6, 5));
        assert_eq!(r.unwrap_err(), ReconfigError::NotAQuorum);
    }

    #[test]
    fn reconfigure_succeeds_when_expired_and_majority() {
        let prior = Lease::new(0, 5);
        let new_cfg = cfg::<1>([1, 2, 3, 4, 5]);
        let q = reconfigure::<1>(prior, 6, &new_cfg, BTreeSet::from([3, 4, 5]), Lease::new(6, 5))
            .expect("expired lease + majority should reconfigure");
        assert_eq!(q.epoch(), 1);
        assert_eq!(q.members(), &BTreeSet::from([3, 4, 5]));
    }

    /// The reason the lease is indispensable: a quorum of the new config can be
    /// entirely disjoint from a quorum of the old config, so intersection offers
    /// no across-epoch safety. Only the lease sequencing prevents split-brain.
    #[test]
    fn across_epoch_quorums_can_be_disjoint() {
        let old = cfg::<0>([1, 2, 3]);
        let new = cfg::<1>([4, 5, 6]);
        let l = Lease::new(0, 5);
        let q_old = LeasedQuorum::certify(&old, BTreeSet::from([1, 2, 3]), l).unwrap();
        let q_new = LeasedQuorum::certify(&new, BTreeSet::from([4, 5, 6]), Lease::new(6, 5)).unwrap();

        // Disjoint member sets — no shared node to enforce agreement.
        assert!(q_old.members().is_disjoint(q_new.members()));
        // (And the type system won't even let us call q_old.intersect(&q_new):
        //  Quorum<0> vs Quorum<1>. Safety here is purely the lease ordering.)
    }
}

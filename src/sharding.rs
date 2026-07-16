//! Key-space partitioning by disjoint shard brands — the *coordination-free* rung.
//!
//! The crate's founding move (`lib.rs`) splits a **member set** into two disjoint
//! halves, `Lo` and `Hi`, and proves them disjoint by type-level brand unification:
//! a `Quorum<E, Lo>` cannot be used where a `Quorum<E, Hi>` is expected. This module
//! is that same mechanism generalized from a **2-way split of the node set** to an
//! **N-way partition of the key space** (Karger et al. 1997, consistent hashing):
//! under a fixed partition of `shard_count` shards every key routes to exactly one
//! shard `S`, and a `Key<S>` cannot be used where a `Key<T>` (`T ≠ S`) is expected.
//!
//! The payoff is a CALM one (Bailis et al., *Coordination Avoidance*): an operation
//! confined to a **single shard** is coordination-free — one node owns that shard and
//! decides alone, no round-trip. Only a transaction that spans shards needs
//! coordination, and that is the witness rung (`cross_shard`).
//! This rung types the coordination-free half; it is the structural, CALM-side
//! partner of that dual.
//!
//! ## The mechanism
//!
//! * [`Key<S>`] — a key certified to live in shard `S`. It carries a private `hash`
//!   field and a private marker, so it has no external literal form and is
//!   **unforgeable**: the only way to obtain one is [`Shard::<S>::admit`](Shard::admit),
//!   the partition function. It is `Copy` — a key's *location* is a reusable fact, not
//!   a consumable resource.
//! * [`Shard<S>`] — the partition function for shard `S`. [`admit`](Shard::admit)
//!   mints a `Key<S>` **only if** the hash routes to `S` under the given shard count
//!   (`hash % shard_count == S`); a hash that routes elsewhere returns `None`. This is
//!   the routing **seam** (see below).
//! * [`Partition<S>`] — the state owned by shard `S`. Its mutator
//!   [`apply`](Partition::apply) takes a `Key<S>`; a `Key<T>` for any other shard fails
//!   to unify (a compile error). A single-shard [`apply`](Partition::apply) returns nothing to wait on —
//!   the coordination-free character made structural.
//!
//! ## Shards are disjoint — using a foreign key is a compile error
//!
//! ```compile_fail
//! use quorum_types::sharding::{Shard, Partition};
//! let mut p0 = Partition::<0>::new();
//! let k1 = Shard::<1>::admit(7, 2).unwrap();   // hash 7 % 2 == 1 → Key<1>
//! p0.apply(k1, 100); // expected `Key<0>`, found `Key<1>` — shards do not share keys
//! ```
//!
//! ## A key's shard membership cannot be forged
//!
//! ```compile_fail
//! use quorum_types::sharding::Key;
//! // `Key` has a private field, so it has no public literal form: a caller cannot
//! // assert "this key is in shard 0" without going through the partition function.
//! let _forged: Key<0> = Key { hash: 42, _priv: () };
//! ```
//!
//! ## The happy path — a single-shard write needs no coordination
//!
//! ```
//! use quorum_types::sharding::{Shard, Partition};
//!
//! // Two shards, count = 2. Key hashes route by `hash % 2`.
//! let mut p0 = Partition::<0>::new();
//! let mut p1 = Partition::<1>::new();
//!
//! let k0 = Shard::<0>::admit(4, 2).expect("4 % 2 == 0");
//! let k1 = Shard::<1>::admit(7, 2).expect("7 % 2 == 1");
//!
//! // Each write lands on its own shard, alone — no barrier, no vote, no round-trip.
//! p0.apply(k0, 100);
//! p1.apply(k1, 200);
//!
//! assert_eq!(p0.get(k0), Some(100));
//! assert_eq!(p1.get(k1), Some(200));
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! What the types own is *disjointness and locality*: a `Key<S>` is unforgeable and
//! carries exactly one type-level shard brand — a `Key<T>` cannot stand in for a
//! `Key<S>` — and a single-shard operation touches only its own shard's state. Whether
//! that brand matches the key's *true* placement is the routing seam below. Two things
//! are **not** owned:
//!
//! * **The partition function is trusted.** [`Shard::admit`](Shard::admit) is the sole
//!   minter of a `Key<S>`, and it is only as correct as its `hash % shard_count`
//!   routing and the `shard_count` it is handed. A caller that passes an inconsistent
//!   `shard_count`, or a routing that does not match how the cluster actually placed
//!   the key, gets a well-typed but wrongly-placed key. This is the same root-of-trust
//!   shape as [`Config::new`](crate::membership::Config::new): the type verifies the
//!   brand, the operator chooses the routing.
//! * **`Key<S>` is a per-shard brand, not a per-key identity.** The type-level `S` is a
//!   shard *class*, not a value instance: two different keys that both route to shard 0
//!   share the type `Key<0>`. Disjointness is between *shards*, never between individual
//!   keys within a shard (the same type-level-class-not-instance seam as
//!   [`at_least_once`](crate::at_least_once)'s `Id`). Unlike the base module's **sealed**
//!   `Lo`/`Hi`, the shard brand space `S: u64` is **open** — there is no closed set of
//!   shard inhabitants and no type-level proof that the shards partition the key space;
//!   an out-of-range shard simply never mints a key (`admit` returns `None`).

use std::collections::BTreeMap;

/// A key certified to live in shard `S`.
///
/// Carries a private `hash` field (read via [`hash`](Key::hash)). Unforgeable: the
/// private fields mean it has no external literal form, so the only source is
/// [`Shard::<S>::admit`](Shard::admit), the partition function. `Copy` because a key's
/// *location* is a reusable fact — reading it does not consume it (contrast the linear
/// tokens in `cross_shard`, where a vote *is* consumed).
#[derive(Debug, Clone, Copy)]
pub struct Key<const S: u64> {
    hash: u64,
    _priv: (),
}

impl<const S: u64> Key<S> {
    /// The runtime hash of this key.
    pub const fn hash(&self) -> u64 {
        self.hash
    }

    /// The shard this key belongs to (mirrors the type-level `S`).
    pub const fn shard(&self) -> u64 {
        S
    }
}

/// The partition function for shard `S`: the gradual boundary that turns an untyped
/// key hash into a branded [`Key<S>`].
///
/// A zero-sized handle; all its behaviour is in [`admit`](Shard::admit).
#[derive(Debug, Clone, Copy)]
pub struct Shard<const S: u64>;

impl<const S: u64> Shard<S> {
    /// Route `hash` under a partition of `shard_count` shards. Mints a [`Key<S>`]
    /// **only if** the hash belongs to shard `S` (`hash % shard_count == S`); a hash
    /// that routes to a different shard returns `None`.
    ///
    /// This is the crate's usual gradual boundary: an `Option`-returning check at the
    /// edge (like [`membership::Config::certify`](crate::membership::Config::certify)
    /// or [`staleness`](crate::staleness)'s read window). `shard_count == 0` is a
    /// degenerate partition and always returns `None`.
    pub const fn admit(hash: u64, shard_count: u64) -> Option<Key<S>> {
        if shard_count != 0 && hash % shard_count == S {
            Some(Key { hash, _priv: () })
        } else {
            None
        }
    }
}

/// The state owned by shard `S` — a toy key/value store keyed by key hash.
///
/// Its mutator [`apply`](Partition::apply) accepts only a [`Key<S>`]; a key from any
/// other shard fails to unify. A single-shard `apply` is coordination-free: it returns
/// immediately with nothing to wait on, because one node owns shard `S`.
#[derive(Debug, Default)]
pub struct Partition<const S: u64> {
    data: BTreeMap<u64, u64>,
}

impl<const S: u64> Partition<S> {
    /// Open an empty partition for shard `S`.
    pub fn new() -> Self {
        Partition { data: BTreeMap::new() }
    }

    /// The shard this partition owns (mirrors the type-level `S`).
    pub const fn shard(&self) -> u64 {
        S
    }

    /// Write `value` at `key`. Requires a [`Key<S>`]: a key from another shard is a
    /// compile error, so a partition can only be mutated through its own shard's keys.
    ///
    /// Coordination-free — this is a single-shard operation and returns nothing to
    /// wait on. A write spanning shards is not expressible here; it lives in
    /// `cross_shard`.
    pub fn apply(&mut self, key: Key<S>, value: u64) {
        self.data.insert(key.hash(), value);
    }

    /// Read the value at `key`, if present. Requires a [`Key<S>`].
    pub fn get(&self, key: Key<S>) -> Option<u64> {
        self.data.get(&key.hash()).copied()
    }

    /// The number of keys currently stored in this partition.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether this partition holds no keys.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_routes_by_hash_modulo_count() {
        // Under 3 shards, hash 6 → shard 0, hash 7 → shard 1, hash 8 → shard 2.
        assert!(Shard::<0>::admit(6, 3).is_some());
        assert!(Shard::<0>::admit(7, 3).is_none(), "7 % 3 == 1, not shard 0");
        assert!(Shard::<1>::admit(7, 3).is_some());
        assert!(Shard::<2>::admit(8, 3).is_some());
    }

    #[test]
    fn admitted_key_reports_its_shard_and_hash() {
        let k = Shard::<1>::admit(7, 3).unwrap();
        assert_eq!(k.shard(), 1);
        assert_eq!(k.hash(), 7);
    }

    #[test]
    fn zero_shard_count_admits_nothing() {
        assert!(Shard::<0>::admit(0, 0).is_none(), "a 0-shard partition is degenerate");
    }

    #[test]
    fn single_shard_write_and_read_round_trip() {
        let mut p = Partition::<0>::new();
        let k = Shard::<0>::admit(6, 3).unwrap();
        assert!(p.is_empty());
        p.apply(k, 42);
        assert_eq!(p.get(k), Some(42));
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn distinct_keys_in_one_shard_share_the_brand() {
        // hashes 6 and 9 both route to shard 0 under count 3; both are `Key<0>`.
        let mut p = Partition::<0>::new();
        let k6 = Shard::<0>::admit(6, 3).unwrap();
        let k9 = Shard::<0>::admit(9, 3).unwrap();
        p.apply(k6, 60);
        p.apply(k9, 90);
        assert_eq!(p.get(k6), Some(60));
        assert_eq!(p.get(k9), Some(90));
        assert_eq!(p.len(), 2, "two distinct keys, one shard");
    }

    #[test]
    fn keys_are_copy_a_location_is_a_reusable_fact() {
        let mut p = Partition::<0>::new();
        let k = Shard::<0>::admit(6, 3).unwrap();
        p.apply(k, 1);
        // `k` is still usable after being passed by value — Copy.
        assert_eq!(p.get(k), Some(1));
        assert_eq!(k.hash(), 6);
    }
}

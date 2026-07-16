//! Local log compaction — the **structural** rung of the safe-to-forget (garbage-collection) axis.
//!
//! Its runtime-witness dual is `stability`. Every recovery and replication rung in the crate keeps
//! history *around* — [`message_log`](crate::message_log) logs determinants before delivery,
//! [`chain`](crate::chain) forwards updates down a line, [`snapshot`](crate::snapshot) records state —
//! and none of them ever type the *reclaiming* of that storage. Real logs cannot grow forever; a node
//! must eventually **forget** an old prefix. This axis types the two halves of that: a node locally
//! surrendering its own prefix (here, structural) and the cluster certifying that doing so is *safe*
//! (`stability`, a witness).
//!
//! The split is the crate's recurring cut, but with a sharp twist worth stating up front. The *act* of
//! dropping your own on-disk prefix needs no coordination — it is your storage, the CALM/structural
//! side. Knowing the drop is *safe* (that no peer still needs what you dropped) is the opposite
//! extreme: not a majority but **every** replica must have moved past the cut, the strongest
//! coordination in the crate (`stability`, the witness — see its `min`-over-*all* barrier). So this
//! structural rung types only the local, irreversible act; it deliberately does **not** promise safety.
//!
//! ## What the type owns
//!
//! A [`Retained<STREAM>`] is a node's retained history for one log stream `STREAM`, above a runtime
//! **floor** — the highest position it has already forgotten. [`compact`](Retained::compact) is the
//! irreversible move: it **consumes** the retained log (the pre-compaction view, which could still read
//! the now-forgotten prefix, ceases to exist), raises the floor, and returns the compacted log together
//! with a [`Forgotten<STREAM>`] receipt.
//!
//! * **Linearity is the primary mechanism (E0382).** [`Retained`] is move-only (no `Copy`/`Clone`):
//!   forgetting is irreversible, so once you `compact`, the old handle is gone and you cannot read the
//!   dropped prefix through it. This is *linearity-as-destruction* — where [`escrow`](crate::escrow)
//!   conserves a budget and [`at_least_once`](crate::at_least_once) retires a delivered message, here
//!   the consumed resource is *retained state being permanently discarded*, a use the crate has not
//!   typed before.
//! * **Stream identity (E0308).** The log stream rides in the type as a const brand, so a
//!   `Forgotten<1>` and a `Forgotten<2>` are different types: a compaction receipt for one stream can
//!   never stand in for another (the same type-identity unification [`detector`](crate::detector) uses
//!   for a node, deliberately *not* a floor-threshold `const` gate, which would merely re-skin
//!   [`staleness`](crate::staleness)'s `Δ ≤ TOL` — the floor is a *runtime* value).
//! * **Unforgeable receipt (E0451), but only against a bare literal.** [`Forgotten<STREAM>`] has a
//!   private field and is minted only by [`compact`](Retained::compact), so you cannot fabricate the
//!   receipt by a struct literal. It is only a node's *self-asserted* record of its own floor, though:
//!   the mint path trusts [`since`](Retained::since)'s floor, so a receipt attests the floor a node
//!   *claims* to have reached — not that any prefix was genuinely, let alone safely, reclaimed (a
//!   below-floor `compact` is a no-op that still mints one). Corroborating that a reclaim was real and
//!   safe is `stability`'s job; this structural rung is a local self-assertion, and the private field
//!   buys only that the record cannot be conjured *without* a `compact` call.
//!
//! ## Where the types stop (the runtime seam)
//!
//! * **Nothing here checks the drop is safe.** [`compact`](Retained::compact) will happily forget a prefix a lagging peer
//!   still needs — the linear token models the *irreversibility* of the act, not its *safety*. Safety
//!   is precisely the witness rung's job: a real node compacts only up to a `stability::StableUpTo`
//!   frontier (the coupling is conventional, not type-enforced — kept decoupled so each rung is an
//!   independent module, the same way [`detector`](crate::detector) documents rather than imports the
//!   eviction it should gate).
//! * **The floor only rises along a single handle.** Threaded through
//!   [`compact`](Retained::compact) (which takes `max(floor, upto)`), the floor is monotone — no method
//!   lowers it, so you cannot un-forget *through a handle*. This is an API property (the absence of a
//!   lowering operation), not a `const` arithmetic wall. It is **per-handle, not global**:
//!   [`since`](Retained::since) is an unrestricted constructor (a node loads its persisted floor once at
//!   startup), so nothing stops minting a fresh `Retained` at a lower floor — honestly, a node lying to
//!   itself about its own storage. The type owns monotonicity of the handle you thread, not of the node.
//! * **`STREAM` is a type-level class, not a value instance.** Two `Retained<STREAM>` of the same
//!   stream are indistinguishable to the type (the crate's recurring class-not-instance limit); giving
//!   each real log stream a distinct `STREAM` is a caller obligation.
//! * **Positions and the readable/forgotten boundary are runtime.** [`readable`](Retained::readable) is
//!   an ordinary comparison against the floor, not a typed guarantee; the type owns the irreversibility
//!   of the drop, not which positions exist.
//!
//! ## A compaction receipt cannot be conjured without a compact call — unforgeability
//!
//! ```compile_fail
//! use quorum_types::compaction::Forgotten;
//! let _ = Forgotten::<1> { upto: 0, _priv: () }; // private field — only `Retained::compact` mints one
//! ```
//!
//! ## A receipt for one stream is not one for another — stream identity
//!
//! ```compile_fail
//! use quorum_types::compaction::{Retained, Forgotten};
//! let (_log, receipt) = Retained::<1>::since(0).compact(5); // Forgotten<1>
//! let _wrong: Forgotten<2> = receipt;                        // stream 1 vs 2 do not unify — E0308
//! ```
//!
//! ## The forgotten prefix cannot be read back — linearity (forgetting is irreversible)
//!
//! ```compile_fail
//! use quorum_types::compaction::Retained;
//! let log = Retained::<1>::since(0);
//! let (_compacted, _receipt) = log.compact(5); // consumes `log`
//! let _ = log.readable(3);                      // ERROR: `log` used after move — the old view is gone (E0382)
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::compaction::Retained;
//!
//! // A retained log for stream 1 at genesis (floor 0 — only the sentinel position 0 is forgotten).
//! let log = Retained::<1>::since(0);
//! assert!(log.readable(3), "position 3 is still retained");
//!
//! // Forget everything up to and including position 5. The old handle is consumed.
//! let (log, receipt) = log.compact(5);
//! assert_eq!(receipt.upto(), 5);
//! assert_eq!(log.floor(), 5);
//! assert!(!log.readable(5), "position 5 has been compacted away");
//! assert!(log.readable(6), "position 6 is still retained");
//!
//! // Compaction never moves backward: a lower cut is a no-op on the floor.
//! let (log, _) = log.compact(2);
//! assert_eq!(log.floor(), 5, "the floor only rises");
//! ```

/// A node's retained history for one log stream `STREAM`, above a runtime **floor** (the highest
/// position already forgotten). Move-only and `#[must_use]`: forgetting is irreversible, so the
/// pre-compaction view must not outlive a [`compact`](Self::compact) — hold it or compact it, never
/// silently duplicate it.
#[must_use = "a Retained log is the node's storage; compact it or keep it, but it is move-only because forgetting is irreversible"]
pub struct Retained<const STREAM: u64> {
    floor: u64,
}

/// A **receipt** recording that the node compacted stream `STREAM` to a floor of `upto` — its own
/// *self-asserted* record of its floor, not corroborated evidence that a prefix was safely reclaimed
/// (see the module seam). Minted only by [`Retained::compact`] (private field, so it cannot be forged
/// by a bare struct literal — though the floor it records is whatever `since`/`compact` were handed).
/// Unlike [`Retained`] it is a *fact* (hence `Clone`), not a resource: the storage is already gone, so
/// duplicating the receipt duplicates only the record.
#[derive(Debug, Clone)]
#[must_use = "a Forgotten receipt records the node's compaction floor; keep it as provenance or drop it"]
pub struct Forgotten<const STREAM: u64> {
    upto: u64,
    _priv: (),
}

impl<const STREAM: u64> Retained<STREAM> {
    /// A retained log for stream `STREAM` whose floor is `floor` — everything at or below `floor` is
    /// already forgotten, everything above it is retained. Positions are 1-based; floor `0` is the
    /// genesis sentinel (only the sentinel position 0 is "forgotten", i.e. nothing real). This is an
    /// unrestricted constructor: a node loads its persisted floor here once at startup (see the seam on
    /// per-handle monotonicity).
    pub const fn since(floor: u64) -> Self {
        Retained { floor }
    }

    /// The current floor: the highest position that has been forgotten.
    pub const fn floor(&self) -> u64 {
        self.floor
    }

    /// The log stream (its type-level `STREAM`) this retained log belongs to.
    pub const fn stream(&self) -> u64 {
        STREAM
    }

    /// Whether position `pos` is still retained (readable). A runtime comparison against the floor —
    /// the type owns the irreversibility of the drop, not which positions exist.
    pub const fn readable(&self, pos: u64) -> bool {
        pos > self.floor
    }

    /// **Forget the prefix up to and including `upto`.** Consumes the retained log (the pre-compaction
    /// view is gone — you cannot read the dropped prefix through it any longer) and returns the
    /// compacted log, with its floor raised to `max(floor, upto)`, plus a [`Forgotten<STREAM>`] receipt.
    /// The receipt's [`upto`](Forgotten::upto) is that *resulting* floor, so it exceeds the requested cut
    /// when the floor was already higher (a below-floor cut is a no-op on the floor). Coordination-free —
    /// a node reclaims its own storage — but *not* checked to be safe: see the seam.
    pub fn compact(self, upto: u64) -> (Retained<STREAM>, Forgotten<STREAM>) {
        let new_floor = if upto > self.floor { upto } else { self.floor };
        (Retained { floor: new_floor }, Forgotten { upto: new_floor, _priv: () })
    }
}

impl<const STREAM: u64> Forgotten<STREAM> {
    /// The resulting floor this receipt attests (`max(prior floor, requested cut)`) — the highest
    /// position forgotten, which may exceed the cut requested of [`compact`](Retained::compact).
    pub const fn upto(&self) -> u64 {
        self.upto
    }

    /// The log stream (its type-level `STREAM`) this receipt belongs to.
    pub const fn stream(&self) -> u64 {
        STREAM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_raises_the_floor_and_forgets_the_prefix() {
        let log = Retained::<1>::since(0);
        assert!(log.readable(3));
        let (log, receipt) = log.compact(5);
        assert_eq!(receipt.upto(), 5);
        assert_eq!(receipt.stream(), 1);
        assert_eq!(log.floor(), 5);
        assert!(!log.readable(5), "5 ≤ floor: forgotten");
        assert!(log.readable(6), "6 > floor: retained");
    }

    #[test]
    fn the_floor_only_rises_and_the_receipt_reflects_the_resulting_floor() {
        let (log, receipt) = Retained::<7>::since(10).compact(4); // cut below the floor
        assert_eq!(log.floor(), 10, "a lower cut does not lower the floor");
        assert_eq!(receipt.upto(), 10, "the receipt attests the resulting floor (10), not the cut (4)");
        assert_eq!(log.stream(), 7, "the stream rides on the retained log too");
        let (log, receipt) = log.compact(15);
        assert_eq!(log.floor(), 15);
        assert_eq!(receipt.upto(), 15);
    }

    #[test]
    fn a_receipt_names_its_stream() {
        let (_log, receipt) = Retained::<9>::since(0).compact(3);
        assert_eq!(receipt.stream(), 9, "the stream rides in the type and the receipt");
    }
}

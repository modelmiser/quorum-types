//! Global stability — the runtime-**witness** rung of the safe-to-forget (garbage-collection) axis.
//!
//! Its structural dual is `compaction`. A `compaction::Retained` can
//! forget its own prefix locally and irreversibly, but nothing in that rung checks the drop is *safe* —
//! a lagging replica may still need what was dropped. This rung mints the evidence that makes forgetting
//! safe: a certificate that **every** replica has already moved past a cut, so nothing at or below it is
//! needed by anyone.
//!
//! ## The witness is a *unanimous* `min` barrier — not a quorum
//!
//! Every other witness rung in this crate earns its guarantee from **intersection**: any two majorities
//! of a configuration share a member ([`membership`](crate::membership),
//! [`election`](crate::election), [`detector`](crate::detector)). Safe-to-forget is the first axis where
//! intersection is the *wrong* primitive. A majority reporting "I have applied everything up to `W`"
//! says nothing about the minority that has **not** — and that lagging minority is exactly who still
//! needs the prefix you are about to delete. So the safe frontier is not a quorum's; it is the
//! **minimum watermark over *all* members**. A [`Barrier<STREAM, E>`] collects one watermark
//! [`ack`](Barrier::ack) per member of stream `STREAM` at configuration epoch `E`, and
//! [`seal`](Barrier::seal) mints a [`StableUpTo<STREAM, E>`] **only if every member has acked**,
//! carrying the `min` of their watermarks as the frontier. This is the strongest coordination in the
//! crate — unanimity, a full round from *everyone*, strictly beyond a majority — which is precisely why
//! safe-to-forget sits at the far, most-coordinated end of the CALM boundary the structural/witness
//! split has tracked across order, count, occupancy, leadership, and liveness. It is the sixth axis to
//! land on that split, and the first whose witness is a barrier rather than a quorum.
//!
//! ## Why `min`-over-all is the safe frontier
//!
//! If the frontier is the minimum over *all* members, then every member's watermark is `≥` the
//! frontier — so every member has *reported* applying everything at or below it, and (taking those
//! reports at face value — the watermark seam) discarding that prefix strands no one. Weaken "all" to "a majority" and the argument breaks: the majority's minimum can sit
//! *above* a laggard outside it, and forgetting up to that frontier deletes a prefix the laggard still
//! needs (a lost read). The `min` and the `all` are both load-bearing; a z3 model and a TLC model
//! (out-of-tree) discharge exactly that — a majority-min admits a stranded member, an all-min never
//! does.
//!
//! ## Where the types stop (the runtime seam)
//!
//! * **Unanimity is a liveness cost, and this is the honest price of the axis.** [`seal`](Barrier::seal) returns `None`
//!   until *every* member has acked, so a single silent or dead node halts garbage collection forever.
//!   A real system escapes this by *excluding* a node it has confirmed dead — which is exactly a
//!   [`detector::Confirmed`](crate::detector::Confirmed) driving a reconfiguration that shrinks the
//!   roster (the composition with the liveness axis). This rung does not import that; it takes the
//!   roster as given and is honest that a live-but-slow member can stall it.
//! * **A watermark is a claim, not carried evidence.** [`ack`](Barrier::ack) records a member's reported
//!   watermark — a bare number, exactly as [`election`](crate::election) records a vote and
//!   [`detector`](crate::detector) a corroboration. That a member has *truly* applied everything up to
//!   what it reports is trusted; the type makes the certificate unforgeable, not the members honest.
//! * **The roster is trusted to be epoch `E`'s membership — and the brand does *not* check this.** The
//!   barrier is opened over an explicit member set, and `E` is a bare caller-chosen const with no
//!   runtime [`Config<E>`](crate::membership::Config) anywhere in this module. So the brand only tags
//!   the *certificate*'s electorate — a `StableUpTo<_, 5>` cannot be used where a `StableUpTo<_, 3>` is
//!   required (the E0308 doctest) — it does **not** verify that the roster you opened over is epoch
//!   `E`'s true membership; that binding is entirely trusted here. This is materially weaker than
//!   [`detector`](crate::detector), whose `close<E>(&Config<E>)` derives `E` from a real
//!   [`Config<E>`](crate::membership::Config) value and certifies against its members.
//!   [`reconfig_safety`](crate::reconfig_safety) governs generations, not this rung.
//! * **`STREAM` and `E` are type-level classes.** `StableUpTo<STREAM, E>` names a stream and an
//!   electorate, not a particular GC round (the crate's recurring class-not-instance limit).
//!
//! ## A stability certificate cannot be forged — unforgeability
//!
//! ```compile_fail
//! use quorum_types::stability::StableUpTo;
//! let _ = StableUpTo::<1, 0> { frontier: 0, _priv: () }; // private fields — only `Barrier::seal` mints one
//! ```
//!
//! ## A certificate for one stream is not one for another — stream identity
//!
//! ```compile_fail
//! use quorum_types::stability::{Barrier, StableUpTo};
//! use std::collections::BTreeSet;
//! let mut barrier = Barrier::<1, 0>::over(BTreeSet::from([1, 2, 3]));
//! barrier.ack(1, 5); barrier.ack(2, 8); barrier.ack(3, 6);
//! let stable = barrier.seal().unwrap();      // StableUpTo<1, 0>
//! let _wrong: StableUpTo<2, 0> = stable;      // stream 1 vs 2 do not unify — E0308
//! ```
//!
//! ## A certificate is tied to its electorate — the epoch is in the type
//!
//! ```compile_fail
//! use quorum_types::stability::{Barrier, StableUpTo};
//! use std::collections::BTreeSet;
//! let mut barrier = Barrier::<1, 0>::over(BTreeSet::from([1, 2, 3]));
//! barrier.ack(1, 5); barrier.ack(2, 8); barrier.ack(3, 6);
//! let stable = barrier.seal().unwrap();      // StableUpTo<1, 0>
//! let _wrong: StableUpTo<1, 1> = stable;      // electorate 0 vs 1 do not unify — E0308
//! ```
//!
//! ## The happy path — unanimous acks yield the `min` frontier
//!
//! ```
//! use quorum_types::stability::Barrier;
//! use std::collections::BTreeSet;
//!
//! // Stream 1, electorate 0, three replicas that must all report in.
//! let mut barrier = Barrier::<1, 0>::over(BTreeSet::from([1, 2, 3]));
//! barrier.ack(1, 5);
//! barrier.ack(2, 8);
//!
//! // Two of three acked — no certificate: unanimity, not a majority.
//! assert!(barrier.clone().seal().is_none(), "a majority is not enough to forget");
//!
//! // The laggard reports in.
//! barrier.ack(3, 3);
//! let stable = barrier.seal().expect("every replica has acked");
//! assert_eq!(stable.frontier(), 3, "the frontier is the minimum — the laggard's watermark");
//! assert_eq!(stable.stream(), 1);
//! assert_eq!(stable.electorate(), 0);
//! ```

use crate::membership::NodeId;
use std::collections::{BTreeMap, BTreeSet};

/// A stability barrier for one log stream `STREAM` at configuration epoch `E`: it collects each
/// member's reported watermark and, once **all** members have reported, certifies the safe-to-forget
/// frontier. Opened over an explicit roster (the members that must all ack).
#[derive(Debug, Clone)]
#[must_use = "a Barrier collects watermark acks; seal it against its roster to seek a stability certificate"]
pub struct Barrier<const STREAM: u64, const E: u64> {
    roster: BTreeSet<NodeId>,
    acks: BTreeMap<NodeId, u64>,
}

/// A **stability certificate**: unforgeable evidence that every member of stream `STREAM`'s electorate
/// `E` **reported** applying everything up to `frontier` (that they *truly* did is the watermark seam,
/// not something the certificate proves), so the prefix at or below it is safe to forget under that
/// trust. Minted only by [`Barrier::seal`] (private fields). Like [`membership::Quorum`](crate::membership::Quorum) it
/// is a *witness* — a fact, hence `Clone` — carrying the agreed frontier, the stream, and the electorate
/// that certified it. In a real system, holding one is what a `compaction` should compact up to; the
/// coupling is conventional, not type-enforced (see the seam docs).
#[derive(Debug, Clone)]
#[must_use = "a StableUpTo certificate attests a prefix is safe to forget; use it to gate compaction or it is wasted"]
pub struct StableUpTo<const STREAM: u64, const E: u64> {
    frontier: u64,
    _priv: (),
}

impl<const STREAM: u64, const E: u64> Barrier<STREAM, E> {
    /// Open a barrier over `roster` — the members that must **all** ack before the prefix is safe to
    /// forget — with no watermarks reported yet.
    pub fn over(roster: BTreeSet<NodeId>) -> Self {
        Barrier { roster, acks: BTreeMap::new() }
    }

    /// Record that `node` **reports** having applied everything up to and including `watermark` — a
    /// **claim**, not carried evidence (a peer's local progress cannot be moved here, and that it
    /// *truly* applied that far is the watermark seam). A node's watermark only advances, so a later,
    /// higher report supersedes an earlier one (the recorded value is the maximum reported).
    pub fn ack(&mut self, node: NodeId, watermark: u64) {
        self.acks
            .entry(node)
            .and_modify(|w| {
                if watermark > *w {
                    *w = watermark;
                }
            })
            .or_insert(watermark);
    }

    /// How many distinct nodes have acked — a progress counter, **not** a unanimity predicate. It
    /// counts every ack, including any from nodes outside the roster, so `reported() == roster.len()`
    /// does not imply [`seal`](Self::seal) will succeed (a non-roster ack inflates this while a roster
    /// member is still missing). Only `seal` decides unanimity.
    pub fn reported(&self) -> usize {
        self.acks.len()
    }

    /// **Seal the barrier.** Mint a [`StableUpTo<STREAM, E>`] **only if every member of the roster has
    /// acked** (unanimity, not a majority), carrying the `min` **over the roster** (not over the acks
    /// map — a non-roster ack can never inflate the frontier) as the safe-to-forget frontier. Returns
    /// `None` if any member has not reported — a single silent node blocks garbage collection (the
    /// liveness cost of unanimity; see the seam). An empty roster also yields `None` (there is no member
    /// to establish a frontier — a conservative refusal, not a claim that everything is stable; the `min`
    /// over an empty set is where the trailing `?` is live, the one case the unanimity check lets reach
    /// it).
    pub fn seal(self) -> Option<StableUpTo<STREAM, E>> {
        if !self.roster.iter().all(|m| self.acks.contains_key(m)) {
            return None;
        }
        let frontier = self.roster.iter().map(|m| self.acks[m]).min()?;
        Some(StableUpTo { frontier, _priv: () })
    }
}

impl<const STREAM: u64, const E: u64> StableUpTo<STREAM, E> {
    /// The safe-to-forget frontier: every member has *reported* applying everything up to and including
    /// this position (a claim — see the watermark seam; the frontier is agreed, not proven), so the
    /// prefix at or below it strands no one under that trust.
    pub const fn frontier(&self) -> u64 {
        self.frontier
    }

    /// The log stream (its type-level `STREAM`) this certificate belongs to.
    pub const fn stream(&self) -> u64 {
        STREAM
    }

    /// The electorate (configuration epoch) whose members unanimously certified this frontier.
    pub const fn electorate(&self) -> u64 {
        E
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> BTreeSet<NodeId> {
        BTreeSet::from([1, 2, 3])
    }

    #[test]
    fn unanimity_is_required_and_the_frontier_is_the_min() {
        let mut barrier = Barrier::<1, 0>::over(roster());
        barrier.ack(1, 5);
        barrier.ack(2, 8);
        assert_eq!(barrier.reported(), 2);
        assert!(barrier.clone().seal().is_none(), "two of three is a majority, not unanimity");

        barrier.ack(3, 3);
        let stable = barrier.seal().expect("all three acked");
        assert_eq!(stable.frontier(), 3, "the frontier is the laggard's watermark (the min)");
        assert_eq!(stable.stream(), 1);
        assert_eq!(stable.electorate(), 0);
    }

    #[test]
    fn a_later_higher_ack_supersedes_but_a_lower_one_is_ignored() {
        let mut barrier = Barrier::<1, 0>::over(roster());
        barrier.ack(1, 2);
        barrier.ack(2, 9);
        barrier.ack(3, 4);
        barrier.ack(1, 6); // node 1 advances 2 → 6 (a higher late ack supersedes)
        barrier.ack(1, 3); // a lower late ack must be ignored (watermarks only advance)
        let stable = barrier.seal().expect("all acked");
        assert_eq!(stable.frontier(), 4, "min is node 3's 4; node 1 stayed at 6, not the lower 3");
    }

    /// The safety-critical behavior at the API boundary: an ack from a node *outside* the roster must
    /// not affect the certified frontier — the `min` is taken over the roster, never over the acks map.
    /// A refactor to `self.acks.values().min()` would deflate the frontier and pass every *other* test;
    /// this one pins it.
    #[test]
    fn seal_ignores_non_roster_acks_and_takes_the_roster_min() {
        let mut barrier = Barrier::<1, 0>::over(roster()); // roster {1, 2, 3}
        barrier.ack(1, 5);
        barrier.ack(2, 8);
        barrier.ack(3, 3);
        barrier.ack(4, 0); // node 4 is NOT in the roster; its watermark 0 must be ignored
        assert_eq!(barrier.reported(), 4, "reported() counts the non-roster ack…");
        let stable = barrier.seal().expect("all roster members acked");
        assert_eq!(stable.frontier(), 3, "…but the frontier is the roster min (3), not node 4's 0");
    }

    #[test]
    fn a_missing_roster_member_yields_none_despite_extra_non_roster_acks() {
        let mut barrier = Barrier::<1, 0>::over(roster()); // roster {1, 2, 3}
        barrier.ack(1, 5);
        barrier.ack(2, 8);
        barrier.ack(4, 9); // a non-roster node cannot substitute for the missing member 3
        assert_eq!(barrier.reported(), 3, "three acks, but node 3 is missing");
        assert!(barrier.seal().is_none(), "unanimity is over the roster, not the ack count");
    }

    #[test]
    fn an_empty_roster_yields_none() {
        // Vacuous unanimity, but there is no member to establish a frontier: a conservative refusal.
        let barrier = Barrier::<1, 0>::over(BTreeSet::new());
        assert!(barrier.seal().is_none(), "no member ⇒ no frontier ⇒ no certificate");
    }

    /// The safety argument, made concrete: the all-min frontier is `≤` every member's watermark (so no
    /// member is stranded), whereas a *majority*-min can exceed a laggard's watermark (stranding it).
    /// This demonstrates why `min`-over-*all* is load-bearing, not `min`-over-a-quorum.
    #[test]
    fn all_min_strands_no_one_but_a_majority_min_would() {
        let watermarks: BTreeMap<NodeId, u64> = BTreeMap::from([(1, 5), (2, 8), (3, 3)]);

        let all_min = *watermarks.values().min().unwrap();
        assert_eq!(all_min, 3);
        assert!(
            watermarks.values().all(|&w| w >= all_min),
            "every member is at or above the all-min frontier — none stranded"
        );

        // A majority {1, 2} (excluding the laggard 3) would certify min(5, 8) = 5 …
        let majority_min = *[watermarks[&1], watermarks[&2]].iter().min().unwrap();
        assert_eq!(majority_min, 5);
        // … but node 3 is only at 3, so forgetting up to 5 deletes positions 4 and 5 it still needs.
        assert!(
            watermarks[&3] < majority_min,
            "the laggard sits below the majority-min frontier — a majority-min would strand it"
        );
    }
}

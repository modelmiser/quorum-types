//! Deadlock **detection** by a global wait-for graph — the witness dual to
//! [`lockorder`](crate::lockorder)'s structural avoidance.
//!
//! [`lockorder`](crate::lockorder) makes a circular wait *unrepresentable* by ranking
//! locks and acquiring in strictly increasing order — a purely local discipline, "no
//! detector, no rollback, no timeout", coordination-free. That is one of the two
//! textbook answers to the deadlock hazard. This module is the **other**: when locks
//! are *not* globally ranked, a cycle of "holds one, waits for another" can form, and
//! the only way to find it is to assemble the **wait-for graph** across nodes and search
//! it for a cycle (Chandy–Misra–Haas 1983; a stable-property detection over the same
//! consistent-cut substrate `snapshot`/`consistent_cut` type).
//!
//! No single node can see a global cycle — it needs edges reported by others — so
//! detection is irreducibly a **runtime witness**, exactly where avoidance was
//! structural. The two rungs split on the coordination-free (CALM) boundary the crate's
//! whole thesis tracks: avoid locally, or detect globally.
//!
//! ## The mechanism — a fused witness over a global-graph reachability predicate
//!
//! This witness sits in the crate's existing **global-predicate** family — a runtime
//! predicate over assembled global state, fused to the typed result, trusting that state
//! to reflect a consistent cut: [`consistent_cut`](crate::consistent_cut) checks causal
//! closure, [`recovery_line`](crate::recovery_line) computes a fixpoint meet. What is new
//! is the *predicate*, not the witness shape: a **cycle-reachability** search over a
//! directed graph, where the siblings check causal ordering or compute a meet.
//!
//! * [`WaitForGraph<E>`] collects wait-edges at configuration epoch `E`: each
//!   [`wait`](WaitForGraph::wait)`(waiter, holder)` records that `waiter` is blocked on
//!   the resource `holder` is holding (the single-outstanding-request model, so each
//!   waiter has one outgoing edge). Recording an edge is a *claim* — the witness seam,
//!   like [`consistent_cut`](crate::consistent_cut)'s log or [`detector`](crate::detector)'s
//!   `corroborate`.
//! * [`detect`](WaitForGraph::detect) **consumes** the graph and searches it. It returns
//!   an [`Acyclic<E>`] witness if the observed graph has no cycle, or a [`Cycle<E>`]
//!   certificate naming the nodes that form a cycle and a victim to abort. The verdict is
//!   *fused* to the search — there is no detachable "verified" token to replay (the
//!   [`occ`](crate::occ) / [`consistent_cut`](crate::consistent_cut) discipline).
//!
//! ## A deadlock certificate cannot be forged
//!
//! ```compile_fail
//! use quorum_types::deadlock::Cycle;
//! use std::collections::BTreeSet;
//! // Private fields: no public literal. You cannot assert a deadlock without detecting one.
//! let _forged: Cycle<1> = Cycle { members: BTreeSet::new(), victim: 0, _priv: () };
//! ```
//!
//! ## A certificate is move-only — `abort_victim` consumes it
//!
//! ```compile_fail
//! use quorum_types::deadlock::WaitForGraph;
//! let g = WaitForGraph::<1>::new().wait(1, 2).wait(2, 1);
//! if let Err(cycle) = g.detect() {
//!     let _v = cycle.abort_victim();
//!     let _again = cycle.abort_victim(); // `cycle` already moved — one abort per detection
//! }
//! ```
//!
//! ## A certificate from another epoch cannot resolve this one
//!
//! ```compile_fail
//! use quorum_types::deadlock::{WaitForGraph, Cycle};
//! fn resolve(_c: Cycle<2>) {}
//! let g = WaitForGraph::<1>::new().wait(1, 2).wait(2, 1);
//! if let Err(cycle) = g.detect() {
//!     resolve(cycle); // Cycle<1> vs Cycle<2> do not unify
//! }
//! ```
//!
//! ## The happy path — a cycle is detected; an acyclic graph certifies deadlock-freedom
//!
//! ```
//! use quorum_types::deadlock::WaitForGraph;
//!
//! // 1 waits on 2, 2 waits on 3, 3 waits on 1 — a three-node deadlock.
//! let g = WaitForGraph::<7>::new().wait(1, 2).wait(2, 3).wait(3, 1);
//! let cycle = g.detect().expect_err("this graph has a cycle");
//! assert!(cycle.members().contains(&1) && cycle.members().contains(&2) && cycle.members().contains(&3));
//! let victim = cycle.abort_victim(); // resolving the deadlock consumes the certificate
//! assert!([1, 2, 3].contains(&victim));
//!
//! // A chain with no back-edge is acyclic: 1→2→3, and 3 waits on no one.
//! let ok = WaitForGraph::<7>::new().wait(1, 2).wait(2, 3);
//! assert!(ok.detect().is_ok(), "a wait chain that terminates is not a deadlock");
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! What the types own is that a [`Cycle`] cannot be fabricated — only a real detected
//! cycle mints one. Four things are **not** owned:
//!
//! * **The wait-for graph is trusted global evidence.** [`detect`](WaitForGraph::detect)
//!   is only as truthful as the edges reported to it; a single node cannot observe the
//!   global graph, and a graph assembled from a torn (non-consistent) cut can report a
//!   *phantom* deadlock or miss a real one. Sound detection is over a consistent cut
//!   (the [`consistent_cut`](crate::consistent_cut) substrate) — that is the same
//!   root-of-trust as [`consistent_cut`](crate::consistent_cut)'s "the log is complete".
//! * **Cycle == deadlock only under single-outstanding-request waiting.** Each node has
//!   one out-edge here, so a cycle is exactly a deadlock. A process blocked on *several*
//!   resources at once (out-degree > 1, the AND-wait model) deadlocks on a *knot*, not a
//!   bare cycle — cycle-presence is then necessary but not sufficient, and this
//!   functional-graph walk would be unsound for that model. Not representable here.
//! * **Victim selection is a policy, and the certificate steers rather than enforces.**
//!   [`abort_victim`](Cycle::abort_victim) returns the highest node id in the cycle — a
//!   deterministic stand-in (real systems choose by cost/age/priority; the type
//!   guarantees only that the victim is a cycle member). And because [`victim`](Cycle::victim)
//!   and [`members`](Cycle::members) expose that id non-destructively, the move-only
//!   certificate *steers* toward one abort per detection rather than enforcing it (Rust
//!   affinity, not true linearity — cf. [`saga`](crate::saga)); not re-acting on a stale
//!   detection after the graph has changed is a caller obligation.
//! * **`E` is a config *class*, not an instance.** The brand distinguishes certificates
//!   of different epochs (a stale-epoch cycle will not unify) but there is no `Config<E>`
//!   checked here — the same type-level-class-not-instance seam `detector`/`stability`
//!   carry.

use crate::membership::NodeId;
use std::collections::{BTreeMap, BTreeSet};

/// A wait-for graph at configuration epoch `E`: who is blocked waiting on whom.
///
/// Each node has at most one outgoing edge (the single-outstanding-request model): a
/// blocked node waits on the one holder of the resource it wants. A cycle in this graph
/// is a deadlock. Consumed by [`detect`](WaitForGraph::detect).
#[must_use = "a WaitForGraph must be detected on, or the deadlock check never runs"]
pub struct WaitForGraph<const E: u64> {
    edges: BTreeMap<NodeId, NodeId>,
}

impl<const E: u64> WaitForGraph<E> {
    /// Open an empty wait-for graph for epoch `E`.
    pub fn new() -> Self {
        WaitForGraph { edges: BTreeMap::new() }
    }

    /// Record that `waiter` is blocked waiting on the resource `holder` holds.
    ///
    /// A *claim* assembled into the global graph (the witness seam). Re-recording a
    /// `waiter` replaces its edge — a node waits on one holder at a time.
    pub fn wait(mut self, waiter: NodeId, holder: NodeId) -> Self {
        self.edges.insert(waiter, holder);
        self
    }

    /// How many wait-edges have been recorded.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// **Search the graph for a cycle.** Consumes the graph. Returns an [`Acyclic<E>`]
    /// witness if the observed graph has no cycle, or a [`Cycle<E>`] certificate naming
    /// the nodes that form a cycle and a victim to abort. With multiple disjoint cycles it
    /// reports the first (in node-id order); resolve it and re-run to find the next.
    ///
    /// The verdict is fused to the search — there is no detachable token that could be
    /// replayed against a graph it did not come from.
    pub fn detect(self) -> Result<Acyclic<E>, Cycle<E>> {
        // Functional graph (out-degree <= 1): follow each chain; a cycle exists iff a
        // walk revisits a node already on its current path. Nodes proven to reach a
        // dead-end (or a known-safe node) are marked safe so each is walked once.
        let mut safe: BTreeSet<NodeId> = BTreeSet::new();
        for &start in self.edges.keys() {
            if safe.contains(&start) {
                continue;
            }
            let mut path: Vec<NodeId> = Vec::new();
            let mut on_path: BTreeSet<NodeId> = BTreeSet::new();
            let mut cur = start;
            loop {
                if on_path.contains(&cur) {
                    // Cycle: the segment from the first occurrence of `cur` to the end.
                    let from = path.iter().position(|&n| n == cur).unwrap();
                    let members: BTreeSet<NodeId> = path[from..].iter().copied().collect();
                    let victim = *members.iter().max().expect("a cycle has members");
                    return Err(Cycle { members, victim, _priv: () });
                }
                if safe.contains(&cur) {
                    break; // reaches a node already proven acyclic
                }
                match self.edges.get(&cur) {
                    Some(&next) => {
                        path.push(cur);
                        on_path.insert(cur);
                        cur = next;
                    }
                    None => break, // `cur` holds but waits on no one — a dead-end
                }
            }
            safe.extend(path);
        }
        Ok(Acyclic { _priv: () })
    }
}

impl<const E: u64> Default for WaitForGraph<E> {
    fn default() -> Self {
        Self::new()
    }
}

/// A witness that the observed wait-for graph at epoch `E` contains **no cycle** — no
/// deadlock *in the graph as submitted*.
///
/// A *fact* about the graph as detected — hence `Clone`. Unforgeable: the private field
/// means the only source is a [`detect`](WaitForGraph::detect) that found no cycle. It
/// attests the *observed* graph was acyclic, not that the graph reflected a consistent
/// global cut (that is the detection seam).
#[derive(Debug, Clone)]
#[must_use = "an Acyclic witness records a deadlock-freedom (safety) check; dropping it discards the evidence"]
pub struct Acyclic<const E: u64> {
    _priv: (),
}

impl<const E: u64> Acyclic<E> {
    /// The configuration epoch this deadlock-freedom check was made at (mirrors `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }
}

/// A certificate that the observed wait-for graph at epoch `E` contains a **cycle** —
/// nodes each waiting on the next, a deadlock under the single-outstanding-request model.
///
/// Unforgeable (private fields, minted only by [`detect`](WaitForGraph::detect)) and
/// move-only — [`abort_victim`](Cycle::abort_victim) consumes it, so the *same* certificate
/// cannot be resolved twice through that path. But [`victim`](Cycle::victim) exposes the
/// target non-destructively, so this *steers* toward one abort per detection rather than
/// enforcing it (Rust affinity, not true linearity — see the module seam). That the victim
/// is the *right* one to abort is a policy, not a guarantee.
#[derive(Debug)]
#[must_use = "a detected deadlock must be resolved — consume the Cycle by aborting its victim"]
pub struct Cycle<const E: u64> {
    members: BTreeSet<NodeId>,
    victim: NodeId,
    _priv: (),
}

impl<const E: u64> Cycle<E> {
    /// The nodes forming the cycle in the observed graph.
    pub fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }

    /// The node selected to abort (highest id in the cycle — a deterministic policy;
    /// see the module seam). Guaranteed to be a member of [`members`](Cycle::members).
    pub const fn victim(&self) -> NodeId {
        self.victim
    }

    /// The configuration epoch this cycle was detected at (mirrors `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// **Resolve the deadlock** by aborting the victim. Consumes the certificate, steering
    /// toward a single resolution per detection — though [`victim`](Cycle::victim) exposes
    /// the same id non-destructively (see the module seam). Returns the victim's id.
    pub fn abort_victim(self) -> NodeId {
        self.victim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_three_node_cycle_is_a_deadlock() {
        let g = WaitForGraph::<1>::new().wait(1, 2).wait(2, 3).wait(3, 1);
        let cycle = g.detect().expect_err("1→2→3→1 is a cycle");
        assert_eq!(cycle.members(), &BTreeSet::from([1, 2, 3]));
        assert_eq!(cycle.victim(), 3, "highest id in the cycle");
        assert_eq!(cycle.abort_victim(), 3);
    }

    #[test]
    fn a_terminating_chain_is_not_a_deadlock() {
        // 1→2→3, and 3 waits on no one.
        let g = WaitForGraph::<1>::new().wait(1, 2).wait(2, 3);
        let ok = g.detect().expect("a chain that ends is acyclic");
        assert_eq!(ok.epoch(), 1);
    }

    #[test]
    fn an_empty_graph_is_acyclic() {
        assert!(WaitForGraph::<1>::new().detect().is_ok());
    }

    #[test]
    fn a_two_node_cycle_is_detected() {
        let g = WaitForGraph::<1>::new().wait(1, 2).wait(2, 1);
        let cycle = g.detect().expect_err("1↔2 is a cycle");
        assert_eq!(cycle.members(), &BTreeSet::from([1, 2]));
        assert_eq!(cycle.victim(), 2);
    }

    #[test]
    fn a_cycle_plus_an_innocent_tail_only_flags_the_cycle() {
        // 4→1, and 1→2→3→1 is the cycle. Node 4 is not part of it.
        let g = WaitForGraph::<1>::new().wait(4, 1).wait(1, 2).wait(2, 3).wait(3, 1);
        let cycle = g.detect().expect_err("there is a cycle among 1,2,3");
        assert_eq!(cycle.members(), &BTreeSet::from([1, 2, 3]));
        assert!(!cycle.members().contains(&4), "the tail node 4 is not in the cycle");
    }

    #[test]
    fn two_independent_chains_no_cycle() {
        let g = WaitForGraph::<1>::new().wait(1, 2).wait(3, 4).wait(5, 6);
        assert!(g.detect().is_ok());
        assert_eq!(WaitForGraph::<1>::new().wait(1, 2).edge_count(), 1);
    }

    #[test]
    fn acyclic_witness_is_clone_a_fact() {
        let ok = WaitForGraph::<9>::new().wait(1, 2).detect().unwrap();
        let copy = ok.clone();
        assert_eq!(ok.epoch(), copy.epoch());
    }
}

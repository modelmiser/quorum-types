# quorum-types

[![CI](https://github.com/modelmiser/quorum-types/actions/workflows/ci.yml/badge.svg)](https://github.com/modelmiser/quorum-types/actions/workflows/ci.yml)

*Can the compile-time safety of [`warp-types`](https://github.com/modelmiser/warp-types)
survive the move from a GPU warp to a distributed system?*

**Status: feasibility exploration.** Every piece here is a deliberately small
toy built to answer one question and stop. Nothing is production code, the formal
model is bounded, and the tests exercise small domains. What it offers is an
*arc* — a chain of small results, each verified, that together map where
structural typing helps in a distributed setting and where it must hand off to a
runtime check.

## The idea

Verification is becoming the bottleneck of software: generation outpaces review.
Of the styles that have historically *composed and scaled*, structural
(type-level) enforcement stands out — you get the guarantee by construction, not
by a global proof.

`warp-types` is a structural type system for GPU warps: a diverged warp *cannot*
call a shuffle because the method does not exist on its type. The observation
behind this repo is that **a GPU warp is a degenerate best-case distributed
system** — fixed membership, lockstep, no partitions, no failure. So `warp-types`
is already a session/ownership type system specialized to the friendliest
distributed system in existence. What has to change to handle a *real* one?

The answer, built up step by step below, is: keep the type carrying the
*relations* (epoch, complementarity, majority), push the *elements* and the
*temporal* facts to runtime certificates at a `gradual` boundary — and accept
that some safety is structural while some is irreducibly temporal.

## The arc

| # | Layer | Question | Result |
|---|-------|----------|--------|
| 1 | `lib.rs` (base) | Does epoch-indexing compose with compile-time complement proofs? | **Yes.** Cross-epoch `merge` is a *type error* — split-brain unrepresentable at compile time. |
| 2 | `tla/quorum.tla` | Is the type-level epoch *sufficient* for safety? | **No.** Split-brain is temporal; a bounded TLA+ check finds it as soon as the lease guard is dropped. Epoch is *necessary but not sufficient*. |
| 3 | `failover.rs` | How does the missing temporal guard look in Rust? | A **runtime** lease check (`reconfigure` returns `Result`) — the `gradual` boundary the model proved unavoidable. |
| 4 | `tests/partition_heal.rs` | Does the real API hold across a failure cycle? | A deterministic crash→partition→heal sim keeps `NoSplitBrain` throughout, delegating the decision to the real `reconfigure`. |
| 5 | `membership.rs` | How does membership go dynamic and unbounded? | By *flipping the set relation*: warp complements are **disjoint**; distributed quorums must **intersect**. The type stays relational; the members become a runtime set. |
| 6 | `reconfig.rs` | Are the temporal and structural guards redundant? | **No** — they split safety by regime. *Within* an epoch, intersection. *Across* an epoch, quorums can be disjoint, so only the lease is safe. |
| 7 | `consistency.rs` | Can the *data* be typed, not just the membership? | A value's consensus strength becomes a lattice `Local` → `At<T,E>` → `Agreed<T>`. Moving *up* requires a `&Quorum` as evidence; moving *down* is free — so a committed value is *unforgeable*, and acting on an uncommitted one is a type error. |
| 8 | `reconcile.rs` | Can the *merge* of divergent committed values be typed? | **Partly.** `Diverged` → `Reconciled` is an evidence-gated typestate: the merge demands a `Lawful` witness minted by property-checking the merge function's semilattice laws at a runtime boundary (*sampled evidence, not proof* — [Propel](https://dl.acm.org/doi/10.1145/3591276) does this soundly, statically). And the merged result re-enters the lattice at the **bottom**: in a consensus system a merge is a new proposal, not a decision. |

Read top to bottom, that is the whole story: a structural guarantee (1), a proof
that it is not enough (2), the runtime guard that completes it (3), evidence it
works end-to-end (4), the generalization to real membership (5), the composition
showing the two guards are complementary (6), the same discipline turned on the
*values* rather than the membership (7), and finally the merge of values that
disagree — where the discipline survives but its evidence weakens from counted
majorities to sampled laws (8).

## Key findings

- **Split-brain unrepresentable is a *type-error* claim.** Lifting the epoch into
  the type makes `merge(q@epoch3, q@epoch4)` fail to unify — the guarantee is
  discharged by the compiler's unifier, nothing runs.
- **The epoch is necessary but not sufficient.** Safety against a stale leader is
  *temporal* — no type can say "wait for the old lease to expire." A bounded TLA+
  model (guarded: no violation over 36 states; negative control: split-brain at
  depth 4) pins this down.
- **Dynamic membership is a sign flip, not a scale-up.** Divergence *partitions*
  lanes into disjoint complements; failure-tolerant consensus needs the opposite
  relation — any two quorums must *intersect*. Overlap is *necessary* for
  agreement (though not alone sufficient — the shared member must also refuse to
  double-vote, which this toy does not model).
- **The two guards partition safety by regime.** Within a configuration, safety is
  structural (intersection). Across a configuration change, quorums can be
  disjoint, so intersection guarantees nothing — safety must come from elsewhere.
  This is the known cross-configuration hazard, and real systems answer it two
  ways: force overlapping joint majorities across configs (Raft's `C_old,new`),
  *or* sequence configs with a leader lease (this toy's choice, in the
  Chubby/Boxwood lineage; Vertical Paxos uses an external reconfiguration
  authority). Arriving at that fork from "generalize warp-types" is a
  faithfulness signal, not a novelty claim.
- **The value lattice is asymmetric: up is quorum-gated, down is free.** Consensus
  strength is itself a type — `Local` (a proposal) below `At<T,E>`/`Agreed<T>`
  (committed). The only move *up* consumes a `Local` and demands a `&Quorum` as
  evidence, so a committed value is *unforgeable*; weakening back *down* needs
  nothing, because discarding a guarantee is always sound. "Act only on decided
  values" is then a compile error, not a runtime flag anyone can set.
- **A merge is a proposal, not a decision.** Reconciling two divergent
  committed values is evidence-gated like everything else (`Lawful<M>`, a
  witness that the merge's semilattice laws held on samples), but its output
  re-enters the lattice at the bottom: no quorum has witnessed the merged
  value *as the resolution*, so it must be committed again. In a pure CRDT the
  merge *is* the truth — that difference is the seam between the two worlds.
  The witness here is the runtime-cheap cousin of what Propel (PLDI 2023)
  proves statically, and it gates *standing*, not computation: sampling cannot
  exclude a lawless function the samples missed, and nothing stops code from
  hand-merging readable values off-path — what is unforgeable is the
  `Reconciled` badge itself.
- **`gradual` boundaries are where structure ends.** `Config::certify` and
  `reconfigure` are runtime-checked edges that mint typed tokens trusted
  structurally inside. `N > E` across a reconfiguration, and true linear
  must-consume, are *not* expressible on stable Rust — documented as boundary
  invariants rather than faked.

## Layout

```
src/lib.rs               base: Quorum<const E, S> — compile-time epoch + complement
src/failover.rs          gap #1: Lease, reconfigure — runtime temporal guard
src/membership.rs        gap #2: Config/Quorum<const E> — dynamic intersecting quorums
src/reconfig.rs          unified: LeasedQuorum — both guards composed
src/consistency.rs       value lattice: Local/At/Agreed — consensus strength as a type
src/reconcile.rs         divergence: Diverged/Lawful/Reconciled — evidence-gated merge
tests/partition_heal.rs  deterministic crash/partition/heal simulation
tla/quorum.tla           bounded TLA+ model of the failover discipline
    quorum_guarded.cfg   lease guard on  — invariants hold
    quorum_noguard.cfg   lease guard off — split-brain counterexample
```

## Running it

```sh
cargo test                 # 46 tests: unit + integration + doctest + compile-fail
cargo clippy --all-targets -- -D warnings

# The formal model (needs Java + tla2tools.jar):
java -cp tla2tools.jar tlc2.TLC -deadlock -config tla/quorum_guarded.cfg tla/quorum.tla   # holds
java -cp tla2tools.jar tlc2.TLC -deadlock -config tla/quorum_noguard.cfg tla/quorum.tla   # finds split-brain
```

## Scope and non-goals

This is a research feasibility study, not a consensus library. It does **not**
provide: a network/transport layer, a message protocol, real-time leases, more
than two-way static splits, or Byzantine tolerance. The TLA+ model is bounded
(`MaxEpoch = 2`, two halves) and the property tests cover small domains. Treat
every result as "holds in the checked domain," not "proven in general."

## Relationship to warp-types

`warp-types` is a published, independent crate and is treated here as a
read-only reference — it is *not* a dependency. The `ActiveSet` / `ComplementOf`
traits in the base module are a minimal *model* of its concept, kept
self-contained so this experiment varies only the distributed dimensions.

## License

MIT.

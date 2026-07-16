# The result: what structural typing can and cannot carry in a distributed system

*A standalone taxonomy over the 43 modules of `quorum-types`. This is the
reference companion to `PAPER.md` (the experience report) and the crate-level
`## The result` section in `lib.rs` (the front-page summary). Where they narrate,
this document tabulates.*

**Status:** thesis-complete. Nineteen pre-registered loops, 43 modules, 73
build-and-verify results — none abandoned or inconclusive (the one pre-registered
hypothesis that was *refuted*, the cross-process ceiling, counts as a result, not a
gap). Each rung ships `compile_fail` negatives and was cold-reviewed to
convergence; the epoch/lease boundary ships a bounded TLA+/TLC model and a
deterministic network simulation, and many rungs additionally carry out-of-tree z3
and/or TLC models in a research harness (not shipped in-crate). This document is
the synthesis pass — it states the taxonomy the loops converged on and argues why
the crate stops here.

---

## The one-sentence thesis

Carrying distributed-consensus invariants in stable Rust's type system splits
every concern into **two species**, and the split *coincides with the
coordination-free (CALM) boundary*: a **structural** guarantee is compile-time-
local and needs no evidence from other nodes (the CALM side), while a **witness**
guarantee rests on *trusted runtime evidence* assembled from other nodes (the
coordinated side). The type system reaches exactly as far as the coordination-
free part goes, and hands off — to a construction-time certificate, a runtime
guard, or beyond a per-process type system entirely — exactly where coordination
begins.

Of the 43 modules, **20 are structural** and **23 are witness**. The boundary is
not an artifact of how the modules were written; it tracks *compile-time-local vs.
runtime-input*, orthogonal to which distributed concern is being typed, and it was
observed to hold intact on eight independent axes (below).

---

## Table 1 — The four compile-error primitives (a closed set)

Every structural guarantee in the crate, and the compile-time skeleton of every
witness, is enforced by one of **exactly four** Rust compile errors. The claim is
not a theorem but a reasoned closure: these are the only ways Rust's *stable* type
system turns a distributed invariant into a rejected program — unforgeability
(privacy), linearity (affine moves), identity (unification), and monotone
arithmetic (const-eval). No fifth mechanism appeared across nineteen loops.

| Error | Mechanism | Distributed invariant it enforces | Modules |
|---|---|---|---|
| **E0451** | private field / sealed trait → **unforgeability** | "this certificate exists only if the protocol minted it" | 29 |
| **E0382** | move / use-after-move → **linearity** | "this capability/vote/view is consumed exactly once" | 18 |
| **E0308** | type mismatch / const-generic brand → **unification** | "these two facts share an epoch / shard / node / term" | 34 |
| **E0080** | const-eval panic → **monotone-arithmetic wall** | "these ranks / windows / quorum sizes obey an ordering law" | 7 |

Most modules use several; E0308 (branding) is the workhorse, E0080 the rarest.
E0080's seven users are `crdt`, `fifo`, `flex`, `lockorder`, `reconfig_safety`,
`staleness`, `term` — of which `crdt`'s use is an *incidental* index-bound check
(its convergence guarantee rests on trusted semilattice laws, not a compile
error), making `crdt` arguably the one structural module whose real guarantee no
compile primitive carries. The other six use E0080 as a genuine arithmetic wall
(rank monotonicity, FIFO contiguity, `R+W>N`, intersection-preserving reconfig,
staleness bound, term supersession).

---

## Table 2 — The five witness families (countable cross-node evidence), and the residue

A **witness** is a typed certificate mintable only from trusted runtime evidence.
Across the 23 witness modules, the evidence falls into **five countable
families** — five ways to turn messages from other nodes into a fact a type can
carry. These are *families*, not a crisp partition: a few modules straddle two
(noted below). The claim is that the families jointly exhaust *countable cross-node
evidence* (evidence assembled by counting, comparing, collecting, or predicating
over node reports), not that every module lands in exactly one.

| Family | What it counts / checks | Witness modules |
|---|---|---|
| **(a) quorum / threshold-count** | a counted threshold of distinct members — majority or masking **intersection**, an f+1 **existence** corroboration, or a consumed quorum certificate | `membership`, `flex`, `consistency`, `attest`, `byzantine`, `election`, `detector`, `reconfig` (8) |
| **(b) unanimity-barrier** | *every* member of a roster/participant-set acked | `stability`, `cross_shard` (2) |
| **(c) pairwise compare** | two versions/values/tokens compared for order, dominance, or equality | `vclock`, `occ`, `fencing`, `reconcile` (4) |
| **(d) peer-issued linear token** | a token the peer mints and the sender must hold — a return **ack** (`at_least_once`) or a forward **credit** (`backpressure`) | `at_least_once`, `backpressure` (2) |
| **(e) global-predicate / meet over assembled state** | a predicate checked (or a meet computed) over a frontier/graph assembled from many nodes | `consistent_cut`, `recovery_line`, `deadlock` (3) |

**Straddlers and gates (why these are families, not a partition).** `attest` spans
(a): its `Attested` clears an f+1 *existence* threshold (possibly a minority —
"existence, not uniqueness"), while its sibling `Committed` clears the
masking-*intersection* threshold. `consistency` is quorum-*gated* — it consumes a
`&Quorum<E>` and discards it (crash-model style, `_witness`) rather than *computing*
an intersection — so it sits in (a) by the evidence it rests on, not a mechanism it
runs. `reconcile`'s placement in (c) is its `Diverged` pairwise compare; its
`Lawful` merge witness is a distinct *sampled-law* form (property-tested semilattice
laws — evidence, not proof, and not cross-node; see limits), the same law-trust
`crdt` rests on. `reconfig` sits in (a) for its majority `certify`, but its
distinctive cross-epoch guarantee is a *temporal lease* (cross-epoch quorums are
deliberately disjoint) — an (a)+temporal hybrid. And `flex`'s `R+W>N` intersection
condition is enforced at *compile* time (E0080), the runtime witness being the
certified quorum it sizes.

**The residue (4 modules) — deliberately outside the five, and why that is the
point.** Four witness modules carry evidence that is *not* countable cross-node
data, so the five-family schema does not cover them — and should not:

| Module | Evidence | Why it is not one of the five |
|---|---|---|
| `commit_wait` | a physical-time interval elapsed (TrueTime ε) | **temporal**, not countable — "you cannot type what time it is, only that you waited" |
| `failover` | the prior leader's lease has lapsed | **temporal** — a runtime *guard*, the Boundary-I handoff; "no type can carry 'wait'" |
| `staleness` | measured lag ≤ Δ (and/or a leader lease) | **temporal / physical** — a clock fact, not a node count |
| `total_order` | a trusted sequencer assigned this position | **delegated** — consensus as a black box, or a degenerate 1-authority "quorum" |

This residue is not a gap in the taxonomy; it is the taxonomy naming its own edge.
Three of the four are **temporal** evidence, which `PAPER.md` maps as a distinct
handoff (temporal facts → a runtime guard, never a witness at all), and the fourth
(`total_order`, the agreement axis ≡ consensus) delegates to a sequencer the
quorum apparatus is itself a decomposition of.

---

## Table 3 — The eight paired axes (the structural/witness split, demonstrated)

The cleanest evidence that the two species track the CALM line is eight
independent distributed-systems concerns, each built as a **structural rung
immediately followed by its witness dual** — the same concern answered once
coordination-free and once with cross-node evidence, the pair straddling the CALM
boundary at a named crossing.

| Axis | Structural rung (CALM side) | Witness rung (coordinated side) | The split |
|---|---|---|---|
| **order** (delivery) | `fifo` — per-sender contiguity (E0080) | `total_order` — sequencer agreement | source order is local; total order ≡ consensus |
| **count** (delivery) | `at_most_once` — single-use Effect (E0382) | `at_least_once` — ack'd retransmit (d) | ≤once is a local token; ≥once needs a round trip |
| **occupancy** (flow control) | `send_window` — bound *own* in-flight | `backpressure` — spend receiver Credit (d) | limiting yourself is free; protecting the peer needs its evidence |
| **leadership** | `term` — term-scoped decree | `election` — win a vote-quorum (a) | the term discipline is local; winning it is global |
| **liveness** (failure detection) | `suspicion` — local timeout alarm | `detector` — corroborated death (a) | suspecting is local; confirming death needs a quorum |
| **GC** (safe-to-forget) | `compaction` — forget own prefix (E0382) | `stability` — unanimity barrier (b) | forgetting is local; certifying it safe needs *everyone* |
| **data-partitioning** | `sharding` — disjoint key brand (E0308) | `cross_shard` — participant barrier (b) | single-shard is I-confluent; cross-shard is coordinated |
| **deadlock** | `lockorder` — rank-monotone acquire (E0080) | `deadlock` — global wait-for graph (e) | avoidance is local; detection needs the global graph |

The remaining 27 modules are the **foundation** (loops 1–11): the epoch mechanism
(`lib.rs` `Quorum`/`merge`), membership and reconfiguration, the Byzantine and
attestation ladder, the consistency lattice, physical-time rungs, the transaction/
isolation family, the snapshot/recovery duals, and the causal/session/CRDT floor.
They establish the mechanism the eight axes then generalize.

Note two hybrids flagged during inventory: `twophase` and `session` frame their
guarantees as structural (a linear session typestate) though each has a genuine
runtime seam (cross-network atomicity; the freshness read-witness). They are
counted structural by dominant framing, with the seam named in-module.

---

## The saturation argument (why the crate stops here)

Three independent closures, each an *argument* rather than a proof, met at Loop 20:

1. **The compile-primitive set is closed at four.** E0451/E0382/E0308/E0080 are
   the only stable-Rust compile errors that encode a distributed invariant
   (unforgeability, linearity, identity, monotone arithmetic). Nineteen loops
   surfaced no fifth. A dependently-typed or prover-backed language would add more
   (that is exactly the `PAPER.md` §6 ceiling — value-uniqueness across a
   partition is reachable *only* by a prover over an intact session, which a
   partition removes); stable Rust does not.

2. **The five witness families exhaust countable cross-node evidence.** Every way
   to turn node reports into a typed fact is a threshold-count (a/b), a comparison
   (c), a peer-issued token (d), or a predicate over assembled state (e) — families
   with a few straddlers, not a crisp partition, but jointly covering the countable
   space. The residue is non-countable evidence — temporal
   (`commit_wait`/`failover`/`staleness`) or delegated-consensus (`total_order`) —
   which hands off to a guard or a black box, not to a sixth family.

3. **The remaining axes are present, re-skins, or off-thesis.** A research gate at
   each recent loop found the untried concerns either already modelled
   (read-repair = `reconcile`+`vclock`), a composition of existing rungs
   (eviction-gated-on-`Confirmed`), or off-thesis (threshold cryptography — a
   *different* evidence model, secret-sharing over finite fields, adjourned to a
   separate crate).

**What escapes the argument, stated honestly.** The saturation is scoped: (i)
*uncountable / continuous* evidence (probabilistic or measure-theoretic quorum
witnesses) is outside the "countable" qualifier by construction; (ii)
*cryptographic* shapes (threshold signatures, erasure-coded shares, k-of-n
reconstruction) are a distinct evidence model held off-thesis for a separate
crate; (iii) the eight-axis demonstration is a *sample*, not an enumeration of all
distributed concerns — the claim is that the species split held on every axis
tried, not that no axis exists that would break it. The crate stops because the
*mechanism* space (primitives × shapes) closed, not because the *concern* space is
finite.

---

## Honest limits

The whole point of the crate is knowing where structure ends, so the limits are a
section, not a footnote:

- **Toy scale.** Memberships are small (n = 5 in worked examples), epochs bounded,
  property tests exercise small domains. There is no transport stack — the wire is
  a 25-byte test harness over a deterministic simulator.
- **No cryptography.** Corroboration is byte-equality of votes, not signatures, so
  the fault budget `f` is a declared operator axiom no type checks. This is the
  boundary the off-thesis crypto crate would cross.
- **Law-trusted floors.** `crdt`'s convergence and `reconcile`'s `Lawful` witness
  rest on *sampled* semilattice laws — evidence, not proof.
- **Construction-time, not compile-time, uniqueness.** Value-uniqueness across a
  partition is enforced by a smart constructor returning `None`, not by a type
  error — the `PAPER.md` §6 ceiling, a scoped structural result, not a failure.
- **Roots of trust.** The types verify chains; operators choose roots — membership
  (`Config::new`), sampled laws, the fault budget `f`, the deserializer
  (`promote`). Naming them is how the discipline stays honest about its TCB.

---

*Artifact: a dependency-free stable-Rust crate. See `PAPER.md` for the experience
report and `src/lib.rs` for the module-by-module ladder.*

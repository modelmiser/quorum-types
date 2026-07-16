# How Far Does Structural Typing Reach Into a Distributed System?

*An experience report on carrying consensus evidence in stable Rust's type
system — where it reaches by construction, and the boundary, coinciding with
coordination-freedom, at which it must hand off.*

**Draft — 2026-07-16. Content draft for review; not a submission.**

## Abstract

Structural type systems buy their guarantees *by construction*: a value of the
wrong type cannot be named, so no proof obligation is discharged and no check
runs. `warp-types` uses this to make GPU-warp safety structural — a diverged warp
cannot call a shuffle because the method does not exist on its type. A GPU warp
is a degenerate best-case distributed system: fixed membership, lockstep, no
partitions, no failure. This report asks what happens to that structural
discipline as the system becomes real — dynamic membership, partitions, crash
faults, lying nodes, an actual wire, and then the full ledger of distributed
concerns: ordering, delivery, flow control, leadership, failure detection,
garbage collection, sharding, deadlock. We built `quorum-types`, a dependency-free
stable-Rust crate, as a ladder of 43 small pre-registered experiments across
nineteen development loops, each shipping executable `compile_fail` negatives and
answering one question before the next. The finding is a split — clean but for two hybrid modules we flag
explicitly (§4). **Every distributed concern we
typed falls into one of two species, and the split coincides with the
coordination-free (CALM) boundary:** a *structural* guarantee is compile-time-
local and needs no evidence from other nodes; a *witness* guarantee rests on
trusted runtime evidence assembled from other nodes at a single named
construction crossing. The type system reaches exactly as far as the
coordination-free part goes and hands off — to a construction-time certificate, a
runtime guard, or beyond a per-process type system entirely — exactly where
coordination begins. Two closures make the finding load-bearing rather than
anecdotal: the crate's *compile-time* enforcement uses exactly four Rust compile
errors (unforgeability, linearity, unification, monotone arithmetic) and no
fifth appeared; and its *witnesses* assemble node reports in five countable
families (a few modules straddle two), with a named residue (temporal and
delegated-consensus evidence) that is non-countable by design. The shipped artifact is 202 library
tests, 144 doctests, `compile_fail` negatives, a bounded TLA+/TLC model of the
epoch/lease boundary, and a deterministic network simulation; many rungs' core
lemmas were additionally cross-checked by out-of-tree z3 and/or TLC models in a
research harness — developed alongside the crate but not shipped in it.

## 1. Introduction

As machine-assisted code generation outpaces review, the enforcement styles that
*compose and scale* are worth revisiting. Structural (type-level) enforcement is
distinctive among them — the guarantee comes from the shape of the types,
discharged by the compiler's unifier, rather than from a global proof re-run on
every change. This is a report on the *design experience* of pushing that style
into a domain it was not built for — distributed consensus — until we could map
precisely where it reaches and where it stops. The artifact is a deliberately
small crate built to illustrate the boundary, not a system anyone operates; the
"experience" is the design exploration, and we are explicit (§11) about the toy
scale that buys the clarity.

The starting point is `warp-types`, a structural type system for GPU warps. Its
observation is that a warp is a *degenerate* distributed system — the friendliest
one in existence: membership is fixed, execution is lockstep, there are no
partitions and no failures. `warp-types` is therefore already a session/ownership
type system specialized to that best case. The natural experiment is to relax the
best-case assumptions one at a time and watch what the type system can still
carry.

We report the result of doing exactly that, as a ladder of layers built up in
development order (the artifact labels them "rungs"). Rather than recount 43 rungs
one by one, we organize the report around the finding they converged on:

**The two species.** Every concern we typed split into a *structural* guarantee
(compile-time-local, discharged by the compiler, no evidence from other nodes
required) or a *witness* guarantee (a typed certificate mintable only from
trusted runtime evidence collected from other nodes). The split is not an artifact
of how we wrote the modules; it tracks compile-time-local versus runtime-input,
orthogonal to which concern is being typed. And it *coincides with the
coordination-free (CALM) boundary* (Hellerstein and Alvaro's consistency-as-
logical-monotonicity): the structural species is exactly the coordination-free
side, the witness species exactly the side that pays for coordination. We
demonstrate the coincidence directly (§5) by building eight distributed concerns
*twice each* — once structural, once as its witness dual — and watching the split
land on the CALM line every time.

**The handoff has three shapes, and the mechanism space is closed.** Where a
witness is required, the type system hands off in one of three ways (§6): temporal
facts hand off to a runtime *guard*; wire and value data hand off to a
construction-time *certificate*; and cross-process disagreement hands off past a
per-process type system *entirely* — the ceiling. Underneath, the compile-time
enforcement turns out to use exactly **four** Rust compile errors, and the
witnesses exactly **five** countable evidence shapes (§7). That the mechanism
space closed — not that the concern space is finite — is why the crate stops.

Our primary contributions are the **two-species/CALM mapping** and the **closed
mechanism space** (four primitives, five witness shapes) that makes it a taxonomy
rather than a list. The three-handoff analysis, the cross-process ceiling, a
witness whose weight changes with the fault model, and an evidence-strength
gradient are the supporting observations the mapping surfaces. We are explicit
throughout about what we do *not* claim: the consensus algorithms are decades old
and we claim none of them; the contribution is the type-level *encoding* and the
map of where it reaches. A companion `THESIS.md` carries the full per-module
tables; this report narrates them.

## 2. Background: the degenerate best case

`warp-types` encodes GPU-warp divergence as typestate (Strom and Yemini 1986; the
typestate-oriented lineage). A warp has a type that records which lanes are
active; a shuffle or ballot is a method that exists only on the type of a
*converged* warp. Calling it on a diverged warp is not a runtime error caught by a
guard — it is a name that does not resolve, rejected by the compiler.
Complementary lane masks are related structurally: two sub-warps that partition an
active set carry types that a `merge` can recombine, and only those.

The distributed reading is direct. A warp's lane mask is a membership set; a
converged warp is a configuration with no partition; `merge` of complementary
masks is a reconfiguration. What `warp-types` never has to face is the part that
makes distribution hard: the set changing over time, two halves that cannot
communicate, a member that has crashed, a member that lies, and data that arrives
as bytes with no type. Each rung relaxes one of these and reports what the type
discipline retains.

## 3. The shape that transfers

Before the boundary, the shape that survives relaxation: the load-bearing trick
carries over from warps to consensus provided the type holds *relations* rather
than *elements*.

**Epoch as a type index.** We index every certificate by a compile-time epoch,
`const E: u64`. Two certificates from different configuration generations carry
different types, so a `merge` of an epoch-3 half with an epoch-4 half fails to
unify `E` — it is a compile error, not a runtime check. Split-brain across a
reconfiguration is *unrepresentable*, discharged by the unifier. This is the
crate's founding structural guarantee, and — as §6 shows with a bounded TLA+ model
— it is necessary but not sufficient.

**Disjoint becomes intersecting.** Generalizing from fixed warp lanes to dynamic
membership is not a scale-up but a *sign flip* in the set relation. Warp
complements are **disjoint** — two sub-warps share no lane. Failure-tolerant
quorums must be the opposite: any two quorums must **intersect**, because a member
common to both is what carries agreement across them. The type stays relational —
it certifies the intersection property — while the members themselves become a
runtime `BTreeSet`. The certificate is minted at a runtime-checked boundary
(`Config::certify`) and then trusted structurally inside. This is the first
*witness*: the intersection is a fact about runtime membership, so it cannot be a
type, but it can be a certificate mintable only at one crossing.

The pattern already visible here — a relation the compiler discharges, elements at
a runtime-checked boundary — is the seed of the two-species split. The epoch is
structural; the quorum is a witness. §4 states the split as the report's spine.

## 4. The two species, and the CALM coincidence

Across all 43 modules, every concern resolved into one of two kinds.

**Structural (20 modules).** The guarantee is compile-time-local: it depends only
on facts a single process's compiler already has, and a violating program does not
typecheck. No evidence from any other node is consulted. Examples: a cross-epoch
merge (§3), FIFO per-sender contiguity (`fifo`), rank-monotone lock acquisition
(`lockorder`), the two-phase-locking growing/shrinking wall (`two_phase_lock`),
single-shard routing (`sharding`), a single-use delivery capability
(`at_most_once`).

**Witness (23 modules).** The guarantee rests on *trusted runtime evidence*
assembled from other nodes, lifted into a typed certificate at a single named
construction crossing, and trusted structurally only downstream of there.
Examples: a quorum certificate (`membership`), an elected leader (`election`), a
confirmed failure (`detector`), a stable-prefix barrier (`stability`), an
atomic-commit barrier (`cross_shard`), an acked delivery (`at_least_once`).

**Two hybrids, flagged.** Two modules resist a clean assignment. `twophase` and
`session` frame their guarantees structurally — a linear session typestate — yet
each has a genuine runtime seam (`twophase`'s cross-network atomicity; `session`'s
freshness read-witness). We count them structural by *dominant* framing and name
the seam in-module rather than pretend the split is frictionless; they are the two
places the otherwise-clean bifurcation required a judgment call.

**The coincidence.** The split lands on the coordination-free (CALM) boundary, and
part of this is analytic rather than discovered: we *define* a structural
guarantee as one a node establishes *alone*, which is close to CALM's
local-decidability, so some alignment is built into the definitions. What the eight
axes (§5) add is that the mapping is non-trivial and consistent — compile-time-
locality could have cut *across* the coordination-free line (a locally-checkable
guarantee that still needed coordination, or a coordinated one that was locally
checkable) and never did. We built the CALM classifier (`calm`, `crdt`) as its own
rung and then watched every subsequent axis land on the side its coordination cost
predicted. The
sharpest statement of it is `total_order`: total (atomic) broadcast is a *witness*
rung and not a structural one *because* atomic broadcast is equivalent to
consensus (Chandra–Toueg) and therefore not coordination-free — the type system
cannot mint ordering strength it has to pay for in the coin of global agreement.

The two species are stable under the relaxations of §2: crash faults, Byzantine
faults, an actual wire, and dynamic membership each move a concern along the
axis, but never off it. That stability is what earns the word *taxonomy*.

## 5. The eight paired axes

The cleanest evidence for the coincidence is eight independent distributed
concerns, each built **twice**: a structural rung and then its witness dual — the
same concern answered once coordination-free and once with cross-node evidence,
the pair straddling the CALM boundary at a named crossing.

| Axis | Structural (CALM side) | Witness (coordinated side) | The split |
|---|---|---|---|
| order (delivery) | `fifo` — per-sender contiguity | `total_order` — sequencer agreement | source order is local; total order ≡ consensus |
| count (delivery) | `at_most_once` — single-use `Effect` | `at_least_once` — ack'd retransmit | ≤once is a local token; ≥once needs a round trip |
| occupancy (flow control) | `send_window` — bound *own* in-flight | `backpressure` — spend receiver `Credit` | limiting yourself is free; guarding the peer needs its evidence |
| leadership | `term` — term-scoped decree | `election` — win a vote-quorum | the term discipline is local; winning it is global |
| liveness (failure detection) | `suspicion` — local timeout alarm | `detector` — corroborated death | suspecting is local; confirming death needs a quorum |
| GC (safe-to-forget) | `compaction` — forget own prefix | `stability` — unanimity barrier | forgetting is local; certifying it safe needs *everyone* |
| data-partitioning | `sharding` — disjoint key brand | `cross_shard` — participant barrier | single-shard is I-confluent; cross-shard is coordinated |
| deadlock | `lockorder` — rank-monotone acquire | `deadlock` — global wait-for graph | avoidance is local; detection needs the global graph |

Each pair was pre-registered as a dual, and each half ships `compile_fail`
doctests for its negatives; many halves were additionally cross-checked out-of-tree
by z3 (the core lemma) and/or a bounded TLC model. Two axes are worth their own sentence
because they invert the naive expectation. **GC** (`compaction`/`stability`) puts
the *cheap* act (forgetting your own prefix) on the structural side and the *most
expensive* evidence on the witness side — its witness is the crate's first
**unanimity barrier** (every member must ack), because a mere majority strands the
lagging minority that still needs the prefix. **Deadlock** (`lockorder`/`deadlock`)
splits the two textbook answers to one hazard: avoidance is a local ordering
discipline (structural), detection requires assembling the global wait-for graph
(witness) — no single node can see a cycle it is only one edge of.

The remaining 27 modules are the **foundation** (loops 1–11): the epoch mechanism,
membership and reconfiguration, the Byzantine and attestation ladder, the
consistency lattice, the physical-time rungs, the transaction/isolation family,
the snapshot/recovery duals, and the causal/session/CRDT floor. They establish the
mechanism the eight axes generalize.

## 6. The three handoffs a witness makes

A witness marks where the structural species ends. The handoff itself takes three
shapes — the three boundaries an earlier, seven-rung form of this report treated
as its whole finding, now seen as the three *kinds* of witness crossing.

### 6.1 Temporal facts hand off to a runtime guard

The epoch type index is necessary but not sufficient, and the insufficiency is a
handoff that is not even a witness — it is a runtime *guard*. A stale leader is a
safety problem a type cannot express. Suppose configuration 4 is elected while the
configuration-3 leader still holds a time-bounded lease. Nothing about the types of
their certificates is wrong — they are honestly different epochs — yet both serve,
and that is split-brain. We confirmed this with a bounded TLA+ model: with the
lease guard in place, no violation over the reachable state space; with it removed
as a negative control, split-brain appears at depth four. The property that fails
is *temporal* — "the new configuration must wait out the old lease" — and no type
can carry "wait." The handoff is a runtime check (`reconfigure` returns a `Result`,
refusing the new authority until the prior lease has demonstrably lapsed): the
classical leader-lease/fencing guard (Chubby-style leases; IronFleet's IronKV). It
is a *gradual* boundary in the precise sense of gradual typing — a runtime-checked
edge between a statically-typed region and a fact the static region cannot
establish. This is why `commit_wait`, `failover`, and `staleness` are witnesses
that fit *none* of the five countable shapes (§7): their evidence is a clock, not a
count. "You cannot type what time it is, only that you waited for the uncertainty
to close."

### 6.2 Wire and value data hand off to a construction-time certificate

The unifying principle: runtime data — whatever arrives off a wire — cannot be
lifted *into* a type; it becomes typed evidence only at a single named construction
crossing, and the type system trusts it structurally only downstream of there.

**The epoch instance: the wire.** We drive the real API over a deterministic
network simulator (turmoil), three hosts exchanging 25-byte UDP datagrams through
crash, partition, and heal. Building the wire *located* a limit rather than
removing one: every certificate is indexed by `const E: u64`, and a const generic
must be a compile-time constant, so an epoch that arrives as eight bytes off a
socket cannot become one — there is no `match` over 2^64 monomorphizations. The
resolution is folklore with no canonical name: a runtime value cannot *become* a
const generic, but it can *select among* monomorphizations that already exist, at
one visible `match`. All wire trust concentrates in one `promote()` function — the
sole crossing from bytes to typed values — where arithmetic is checked before any
consumer sees an operand.

**The value instance: corroboration.** A value's consensus strength becomes a
lattice: `Local` (a proposal) moves *up* to a committed value only when witnessed
by a quorum, and *down* for free. Under crash faults the up-move discards its
witness — an honest majority's mere existence suffices. Under Byzantine faults the
same witness becomes **load-bearing**, and that change of weight is a result in
itself: `attest` has no `value: T` parameter — the value is *extracted* from the
votes — so a value can inhabit `Attested<T,E>` only if `f+1` distinct members voted
for it. Value-blindness is not a check that can be forgotten; it is
unrepresentable. The `f+1` threshold buys *existence* but not *uniqueness*;
uniqueness requires the masking threshold `⌈(n+2f+1)/2⌉`, and the reduction runs at
construction time, in ordinary Rust, with no external prover — a smart constructor
whose success is the certificate.

Down this boundary the evidence weakens in a documented pattern: a counted
majority, then a sampled semilattice law (`reconcile`'s `Lawful` — evidence, not
proof), then a counted supermajority conditional on a declared fault budget, then
the same majority with its provenance demoted to bytes an adversarial network
delivered. The discipline survives at every step; the gradient is how much it buys
at each.

### 6.3 The cross-process ceiling

The ladder's capstone asks whether the construction-time `None` that enforces
value-uniqueness could be *replaced* by a compile error — value-uniqueness as the
value-level twin of the epoch type error of §3. We pre-registered that optimistic
hypothesis specifically so a refutation would count, and it was refuted. The
distinct claim is not "uniqueness is enforced at construction" (that is §6.2) but
"*no per-process compile-time enforcement can replace that construction check*,"
for a reason independent of the enforcement mechanism.

Rust is not dependently typed: a type cannot be indexed by a runtime term, and a
vote's value is runtime data, so "these two values differ" is not a type-level
proposition. But the deep reason is not that shallow wall. The disagreement that
constitutes split-brain lives in *two different processes* — two partitioned
collectors each minting a `Committed`. No single per-process typechecker observes
both, and there is no build step at which all participants' code is checked
together against a shared partition-time state. Mechanisms that *do* reach
cross-participant agreement — refined multiparty session types, Session★ — escape
precisely by discharging it through a prover over an *intact* session, which is
exactly what a partition removes. So the precise, scoped statement is:
*value-uniqueness across a partition is not reachable by ordinary host-language
typechecking; it is enforced at construction because the threat is cross-process
while each participant's type system is per-process.* The construction-time `None`
is not a weaker stand-in for a compile-time guarantee reachable with more
cleverness; it is the maximal enforcement the deployed per-process model admits.
This is where the ladder tops out, and it is the same seam — typecheck /
construction-time certificate / prover-obligation — the rest of the crate drew.

## 7. The mechanism space is closed

What turns the two-species map into a taxonomy is that both species are built from
a small, closed set of mechanisms. Full tables are in `THESIS.md`; the closures:

**Four compile-error primitives.** Every structural guarantee, and the
compile-time skeleton of every witness, is one of exactly four Rust compile errors:
**E0451** (private field / sealed trait → *unforgeability*), **E0382**
(move / use-after-move → *linearity*), **E0308** (type mismatch / const-generic
brand → *unification*), and **E0080** (const-eval panic → *monotone-arithmetic
wall*). E0308 is the workhorse (34 modules); E0080 the rarest (7, and one of those
— `crdt`'s — is an incidental index check, its real guarantee resting on trusted
semilattice laws). No fifth mechanism appeared across nineteen loops. This is a
reasoned closure over stable Rust's compile-time enforcement surface, not a
theorem; a dependently-typed or prover-backed language would add more, which is
exactly the §6.3 ceiling restated as a language-capability fact.

**Five witness families, exhausting countable cross-node evidence.** The 23
witnesses assemble node reports in five countable families — families rather than a
crisp partition, since a few modules straddle two: **quorum / threshold-count**
(8 modules — a counted threshold of distinct members: a majority or masking
*intersection*, an f+1 *existence* corroboration, or a consumed quorum
certificate), **unanimity-barrier** (2 — everyone acked), **pairwise compare** (4 —
two versions or values compared for order, dominance, or equality),
**peer-issued linear token** (2 — a token the peer mints and the sender must hold:
a return *ack* in `at_least_once`, a forward *credit* in `backpressure`), and
**global-predicate / meet over assembled state** (3 — a predicate checked or a meet
computed over a frontier or graph built from many nodes). The claim is that these
families exhaust *countable* cross-node evidence — evidence assembled by counting,
comparing, collecting, or predicating — not that every module lands in exactly one:
`attest` spans threshold-count's existence (f+1, possibly a minority) and
intersection (masking) ends, and `reconcile`'s merge-lawfulness rests on a distinct
*sampled-law* witness (property-tested semilattice laws — evidence, not proof, and
not cross-node; §11) beneath its pairwise divergence compare.

**The residue, named honestly.** Four witnesses fit none of the five, by design:
`commit_wait`, `failover`, and `staleness` carry *temporal* evidence (a clock
interval or lease elapsed — the §6.1 handoff, which is a guard, not a countable
witness at all), and `total_order` carries *delegated* evidence (a trusted
sequencer, i.e. consensus as a black box the quorum apparatus is itself a
decomposition of). The residue is not a gap; it is the taxonomy reporting its own
edge.

**Why the crate stops (saturation).** Three closures met: the primitive set is
four, the countable-evidence shapes are five, and a research gate at each recent
loop found the remaining axes either already modelled (read-repair =
`reconcile`+`vclock`), a composition of existing rungs, or off-thesis. What escapes
the argument is stated plainly: *uncountable/continuous* evidence (probabilistic
quorums) is outside the "countable" qualifier; *cryptographic* shapes (threshold
signatures, erasure coding, k-of-n reconstruction) are a distinct evidence model
adjourned to a separate crate; and the eight-axis demonstration is a *sample*, not
an enumeration of all distributed concerns. The crate stops because the *mechanism*
space closed, not because the *concern* space is finite.

## 8. Cross-cutting findings

**Primary contributions.**

1. *The two-species / CALM mapping.* Every distributed concern typed here is
   structural (compile-time-local, coordination-free) or witness (trusted runtime
   evidence, coordinated), and the split coincides with the CALM boundary —
   demonstrated on eight paired axes and stable under crash, Byzantine, wire, and
   dynamic-membership relaxations.
2. *The closed mechanism space.* Four compile-error primitives and five countable
   witness shapes account for all 43 modules, with a named non-countable residue.
   This is what makes the mapping a taxonomy and what justifies stopping.

**Supporting observations** (evidence for the mapping, not co-equal results).

- *The three handoff shapes.* Where a witness is required, the crossing is a
  runtime guard (temporal), a construction-time certificate (wire/value), or the
  cross-process ceiling — §6.
- *Witness weight changes with the fault model.* A quorum witness is discardable
  under crash faults and load-bearing under Byzantine faults, where the value is
  extractable only from `f+1` corroborating votes.
- *The evidence-strength gradient.* How much typing buys weakens down the ladder:
  counted majority → sampled law → conditional supermajority → wire-demoted
  majority. The discipline never breaks; its evidence weakens predictably.
- *Roots of trust.* Operator-chosen roots recur — membership (`Config::new`),
  sampled laws, the declared fault budget `f`, the deserializer (`promote`).
  **Types verify chains; operators choose roots** — the type-level analogue of a
  trusted computing base.
- *The recurring witness trap.* A private-field "unforgeable" certificate is only
  as strong as the *most permissive* way to obtain what it gates; a leaked accessor
  or an unconstrained constructor input silently demotes a linear capability to an
  advisory one. This failure mode recurred ~20 times across the loops and was
  caught each time by adversarial cold review — its own small methodological result.

**A methodological note.** Each rung was pre-registered — hypothesis and falsifiers
written before code — so the §6.3 null counts as a result rather than a failure,
and several parked blockers (the const-generic obstacle to dynamic membership, the
missing wire protocol, the compile-error ceiling) each turned out to *be* the
finding rather than an obstacle to it.

## 9. Method and artifact

The artifact is a dependency-free stable-Rust crate — 43 modules across nineteen
development loops — with 202 library tests, 144 doctests, clippy- and
rustdoc-clean under `-D warnings`. The negative results are carried by
`compile_fail` doctests (a cross-epoch merge, an out-of-epoch vote, the
runtime-value-to-const-generic wall, and each rung's characteristic negatives), so
the report's "you cannot write this" claims are executable. The shipped
model-checking is a bounded TLA+/TLC model of the epoch/lease boundary (no
invariant violation with the guard, split-brain at depth four without it) and a
deterministic `turmoil` network simulation that replays the counterexample as real
partition events. Many rungs' core lemmas were additionally cross-checked by
out-of-tree z3 and/or TLC models in a research harness — evidence developed
alongside the crate but not shipped in it.
Determinism is a deliberate buy-in — ordered collections only, virtual clock, no
ambient nondeterminism — checked by asserting the same seed reproduces the same
event trace. Two methodological commitments carry the credibility: every rung's
hypothesis and falsifiers were written before its code, and each rung was
cold-reviewed by fresh adversarial agents to convergence before merge. The
per-module taxonomy tables are in `THESIS.md`.

## 10. Related work

*Source system.* `warp-types` (GPU-warp typestates) is the system this report
generalizes.

*Behavioral and session types.* Ferrite (ECOOP 2022) and Rumpsteak (PPoPP 2022)
embed session types in Rust but transport native Rust values between participants
compiled into one program — adversarial bytes never arrive. Multiparty session
types type protocol structure, not payload agreement; refined MPST / Session★
(OOPSLA 2020) do type cross-participant value agreement, but via a prover over an
intact session (see §6.3). Choreographic programming (Choral, TOPLAS 2024; HasChor,
ICFP 2023) compiles cooperating endpoints from one global program, not partitioned
adversaries.

*Consistency and coordination-freedom.* The CALM theorem (Hellerstein and Alvaro,
CACM 2020) — consistency as logical monotonicity — is the boundary our two-species
split is observed to coincide with; the Bailis et al. I-confluence analysis (VLDB
2015) is the per-operation form we lean on for single-shard and single-key rungs.

*Verified consensus.* IronFleet (SOSP 2015, crash-fault Paxos in Dafny),
Velisarios (ESOP 2018, PBFT safety in Coq), and Verus (OOPSLA 2023, Rust via SMT)
discharge quorum-intersection safety as a lemma in a prover. This report's move is
smaller and different in kind: the same reductions hold *by construction* — epoch
and complementarity by ordinary typechecking, quorum-intersection at a
runtime-checked smart-constructor boundary whose success is the certificate —
rather than as a global proof re-run on every change. §6.3 marks where even the
construction-time form runs out.

*Byzantine quorums and safety.* Malkhi and Reiter's Byzantine quorum systems
(Distributed Computing 1998) and the safety argument of Castro and Liskov's PBFT
(OSDI 1999) are the classical results we encode and claim none of.

*Adjacent techniques.* Recalling a Witness (POPL 2018) gives F★ a first-class
`witnessed` token over monotonic state — the nearest witness-as-a-type neighbor to
our attested values. GhostCell (ICFP 2021) and generativity brand values by
provenance, not by value equality. Gradual Session Types (ICFP 2017) frames the
runtime-checked boundary as casts with blame. Propel (PLDI 2023) verifies
semilattice laws soundly by *type-checking* them — the sound sibling of our
sampled/property-tested `Lawful` merge witness. Mocket (EuroSys 2023), SandTable
(EuroSys 2024), and the CCF verifiers (NSDI 2025) replay model-checker
counterexamples against implementations — the lineage our turmoil-based replay
joins.

## 11. Limitations

The report's honesty is its method, and the limitations are a section rather than a
footnote. The artifact is a toy: memberships are small (n = 5 in the worked
examples), epochs bounded, and the property tests exercise small domains. There is
no transport layer — the wire is a test harness over a 25-byte toy protocol, not a
network stack. There is no cryptography; corroboration is byte-equality of votes,
not signatures, so the fault budget `f` is a declared operator axiom no type
checks. Value-uniqueness is construction-time, not compile-time, for the structural
reason of §6.3. The four-primitive and five-shape closures are *reasoned arguments*
over what nineteen loops surfaced, not proofs of impossibility — a sceptic who
produces a sixth mechanism or a sixth countable shape refutes them, which is the
intended standard. The eight-axis demonstration is a sample, not an enumeration of
all distributed concerns. Every one of these is a place the type discipline names
where it stops; none is faked.

## 12. Conclusion

Structural typing reaches further into a distributed system than its GPU-warp
origin would suggest — it carries epoch, complementarity, majority, masking-quorum
overlap, value corroboration, ordering, delivery, flow control, leadership, failure
detection, garbage collection, and sharding. But *how* it carries them divides
cleanly, and that division is the report: every concern is either structural
(compile-time-local, discharged by the compiler, coordination-free) or a witness
(trusted runtime evidence, coordinated), and the two species fall on opposite sides
of the CALM line. The type system reaches exactly as far as coordination-freedom
goes and hands off — to a runtime guard, a construction-time certificate, or, at
the ceiling, past a per-process type system entirely — exactly where coordination
begins. The taxonomy is closed at four compile primitives and five countable
witness shapes, with a named residue of temporal and delegated evidence that is
non-countable by design. The guarantee the whole ladder bought was never
"split-brain is impossible in the world"; it was "split-brain is unrepresentable
in any process that keeps its promises at the roots." Knowing precisely where
structure ends — and that it ends on the coordination-free boundary — is the
result.

---

*Artifact: a dependency-free stable-Rust crate (43 modules, 202 library tests, 144
doctests, `compile_fail` negatives, a bounded TLA+/TLC model of the epoch/lease
boundary, and a deterministic network simulation; z3/TLC cross-checks for many
rungs live out-of-tree in a research harness) developed alongside a companion essay
series. Full per-module
taxonomy in `THESIS.md`.*

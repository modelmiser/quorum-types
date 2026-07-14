# How Far Does Structural Typing Reach Into a Distributed System?

*An experience report on carrying consensus evidence in stable Rust's type
system, and the three boundaries where it must hand off.*

**Draft — 2026-07-14. Content draft for review; not a submission.**

## Abstract

Structural type systems buy their guarantees *by construction*: a value of the
wrong type cannot be named, so no proof obligation is discharged and no check
runs. `warp-types` uses this to make GPU-warp safety structural — a diverged warp
cannot call a shuffle because the method does not exist on its type. A GPU warp
is a degenerate best-case distributed system: fixed membership, lockstep, no
partitions, no failure. This report asks what happens to that structural
discipline as the system becomes real — dynamic membership, partitions, crash
faults, then lying nodes, then an actual wire.

We built `quorum-types`, a dependency-free stable-Rust crate, as a ladder of
small pre-registered experiments, each verified and each answering one question
before the next. The finding is not that structural typing fails, but that it
*hands off* in three places, each for a structural reason: temporal facts hand
off to a runtime guard; runtime data arriving off a wire — whether an epoch or a
corroborated value — hands off to a construction-time certificate at a single
named crossing (one per instance); and cross-process invariants are unreachable by *ordinary
host-language typechecking*, the enforcement model a real deployment actually runs
process by process. The last is a *scoped* ceiling: value-uniqueness across a
partition cannot be established by ordinary typechecking (a prover over an
*intact* session can — but a partition is exactly what removes the session), so
it is enforced at construction, because the threat is cross-process and a
host-language type system is per-process. Along the way the
discipline yields a witness whose weight changes with the fault model (discardable
under crash faults, load-bearing under Byzantine), an evidence-strength gradient
that weakens predictably down the ladder, and a recurring "roots of trust"
pattern: types verify chains, operators choose roots. The artifact is 80 tests, a
bounded TLA+ model, and a deterministic network simulation.

## 1. Introduction

As machine-assisted code generation outpaces review, the enforcement styles that
*compose and scale* are worth revisiting. Structural (type-level) enforcement is
distinctive among them — the guarantee comes from the shape of the types,
discharged by the compiler's unifier, rather than from a global proof re-run on
every change. This is a report on the *design experience* of pushing that style
into a domain it was not built for — distributed consensus — until it breaks, and
on what the breaking points turn out to be. The artifact is a deliberately small
crate built to illustrate the boundaries, not a system anyone operates; the
"experience" is the design exploration, and we are explicit (§10) about the toy
scale that buys the clarity.

The starting point is `warp-types`, a structural type system for GPU warps. Its
observation is that a warp is a *degenerate* distributed system — the friendliest
one in existence: membership is fixed, execution is lockstep, there are no
partitions and no failures. `warp-types` is therefore already a session/ownership
type system specialized to that best case. The natural experiment is to relax the
best-case assumptions one at a time and watch what the type system can still
carry.

We report the result of doing exactly that, as a ladder of layers built up in
development order (the artifact labels them "rungs"; this report regroups them
thematically — see the map in §7). Rather than recount the ladder rung by rung,
we organize the report around its central finding: **structural typing carries
the *relational* skeleton of consensus — epoch, complementarity, majority,
masking-quorum overlap, value corroboration — and hands off, to a runtime check
or beyond a type system entirely, at three boundaries.**

1. **Temporal facts hand off to a runtime guard** (§4). A type can carry "these
   two certificates belong to the same epoch"; it cannot carry "wait until the
   previous leader's lease expires."
2. **Runtime data hands off to a construction-time certificate** (§5). Whatever
   arrives off a wire — an epoch as bytes, a value as a vote — cannot be lifted
   *into* a type; it becomes typed evidence only at a single named construction
   boundary, and only there. This is one boundary with two instances (the epoch
   and the value), unified by that principle, not two separate hand-offs.
3. **Cross-process invariants are unreachable by ordinary host-language
   typechecking** (§6). The disagreement that constitutes split-brain lives in two
   different processes; no single per-process typechecker observes both. This is a
   ceiling — and, because it turns on the *scope* of the checker rather than on
   any missing feature, a structural one; §6 argues why that scope is the one
   that matters and what escapes it.

Three lenses recur and are *subordinate* to this spine, offered as evidence for it
rather than as competing theses: an evidence-strength gradient that weakens down
the ladder (§5), a witness whose weight changes with the fault model (§5), and a
"roots of trust" pattern naming where the discipline must trust an operator (§7).

Our primary contributions (§7 collects them) are the three-boundary mapping and
the cross-process ceiling result; the witness-weight finding, the evidence-strength
gradient, and the roots-of-trust pattern are the supporting observations that the
mapping surfaces. We are explicit throughout about what we do *not* claim: the
consensus algorithms are decades old and we claim none of them; the contribution
is the type-level *encoding* and the map of where it reaches.

## 2. Background: the degenerate best case

`warp-types` encodes GPU-warp divergence as typestate (Strom and Yemini 1986; the
typestate-oriented lineage). A warp has a type that records which lanes are
active; a shuffle or ballot is a method that exists only on the type of a
*converged* warp. Calling it on a diverged warp is not a runtime
error caught by a guard — it is a name that does not resolve, rejected by the
compiler. Complementary lane masks are related structurally: two sub-warps that
partition an active set carry types that a `merge` can recombine, and only those.

The distributed reading is direct. A warp's lane mask is a membership set; a
converged warp is a configuration with no partition; `merge` of complementary
masks is a reconfiguration. What `warp-types` never has to face is the part that
makes distribution hard: the set changing over time, two halves that cannot
communicate, a member that has crashed, a member that lies, and data that arrives
as bytes with no type. Each of the following sections relaxes one of these and
reports what the type discipline retains.

## 3. The shape of the transfer

Before the boundaries, the shape that survives relaxation: the load-bearing trick
carries over from warps to consensus provided the type holds *relations* rather
than *elements*. This section states that shape; §4–§6 are where it hands off.
The shape is not itself hand-off-free — its very first instance, the epoch index,
is exactly the fact §4 shows to be insufficient — so we introduce the mechanism
here and pick up its limit there, rather than pretend §3 is the part that "just
works."

**Epoch as a type index.** We index every certificate by a compile-time epoch,
`const E: u64`. Two certificates from different configuration generations carry
different types, so a `merge` of an epoch-3 half with an epoch-4 half fails to
unify `E` — it is a compile error, not a runtime check. Split-brain across a
reconfiguration is *unrepresentable*, discharged by the unifier. This is the one
place a distributed-safety fact is enforced purely by typechecking — and, as §4
shows with a bounded TLA+ model, it is necessary but not sufficient.

**Disjoint becomes intersecting.** Generalizing from fixed warp lanes to
dynamic membership is not a scale-up but a *sign flip* in the set
relation. Warp complements are **disjoint** — two sub-warps share no lane.
Failure-tolerant quorums must be the opposite: any two quorums must **intersect**,
because a member common to both is what carries agreement across them. The type
stays relational — it certifies the intersection property — while the members
themselves become a runtime `BTreeSet`. The certificate is minted at a
runtime-checked boundary (`Config::certify`) and then trusted structurally
inside.

**Two guards, partitioned by regime.** Within one configuration, safety
is the intersection property (structural). Across a configuration change, two
quorums can be legitimately disjoint, so intersection guarantees nothing and
safety must come from the temporal guard of §4. The two are not redundant; they
divide the safety argument by regime. This is the known cross-configuration
hazard, and the type system makes the division explicit rather than papering over
it.

The pattern already visible here — a relation in the type, elements at a
runtime-checked boundary — recurs at every subsequent rung.

## 4. Boundary I: temporal facts hand off to a runtime guard

The epoch type index is *necessary but not sufficient*, and the insufficiency is
the first hand-off.

A stale leader is a safety problem a type cannot express. Suppose configuration 4
is elected while the configuration-3 leader still holds a time-bounded lease.
Nothing about the types of their certificates is wrong — they are honestly
different epochs — yet both serve, and that is split-brain. We confirmed this with
a bounded TLA+ model: with the lease guard in place, no violation over the
reachable state space; with it removed as a negative control, split-brain appears
at depth four. The property that fails is *temporal* — "the new configuration must
wait out the old lease" — and no type can carry "wait."

The hand-off is a runtime check: `reconfigure` returns a `Result`, refusing to
mint the new authority until the prior lease has demonstrably lapsed. Such
leader-lease / fencing guards are classical (Chubby-style leases; IronFleet's
verified lease-based store, IronKV). This is a *gradual* boundary in the precise sense of
gradual typing — a runtime-checked edge between a statically-typed region and a
fact the static region cannot establish. A deterministic in-process simulation
drives
the full crash → partition → heal cycle and holds the no-split-brain invariant
throughout, delegating every safety decision to the real `reconfigure`. The type
carries the structural half; the guard carries the temporal half; neither is
optional.

## 5. Boundary II: runtime data hands off to a construction-time certificate

The unifying principle of this boundary is one sentence: runtime data — whatever
arrives off a wire — cannot be lifted *into* a type; it becomes typed evidence
only at a single named construction crossing, and the type system trusts it
structurally only downstream of there. That principle has two instances, an epoch
and a value, and this section is one boundary because both obey it, not two
because the instances differ. It is also where the "roots of trust" pattern (§7)
becomes explicit.

**The epoch instance: the wire.** We drive the real API over a deterministic
network simulator (turmoil), three hosts exchanging 25-byte UDP datagrams
through crash, partition, and heal, with the partition induced as a network event
rather than a boolean. Building the wire *located* a limit rather than removing
one: every certificate is indexed by `const E: u64`, and a const generic must be a
compile-time constant, so an epoch that arrives as eight bytes off a socket cannot
become one — there is no `match` over 2^64 monomorphizations. The resolution is
folklore with no canonical name: a runtime value cannot *become* a const generic,
but it can *select among* monomorphizations that already exist, at one visible
`match`. Inside an arm the epoch is a type again; outside the compiled arms, an
unknown epoch gets refusal. This carries an honest tension worth stating: dynamism
in *membership* is genuine (the member set is a runtime `BTreeSet`), but dynamism
in the *epoch count* is bounded by monomorphization — the system speaks only the
epochs compiled into it. That bound is itself an instance of this very boundary:
the epoch, like any wire datum, cannot be lifted into the type at runtime. All
*wire* trust concentrates in one `promote()` function — the sole crossing from
bytes to typed values — where arithmetic is checked before any consumer sees an
operand (a
hostile datagram of two `u64::MAX` words must die at the boundary, not panic
inside a lease constructor). A negative control that bypasses the lease guard via
a legitimate `genesis` constructor split-brains at the exact tick the TLA+
counterexample predicts.

**The value instance: corroboration.** A value's consensus strength becomes a
lattice: `Local` (a proposal) moves *up* to a committed value — `At<T,E>`, the
crash-model committed type, distinct from the Byzantine `Committed<T,E>` below —
only when witnessed by a quorum, and *down* for free. Under crash faults, the
up-move discards its witness
— `commit(self, _witness: &Quorum<E>)` binds the quorum with a leading underscore
and never inspects it — because an honest majority's mere existence suffices, and
whatever value the committer holds is the agreed one. This is sound only until a
node can lie.

Under Byzantine faults the same witness becomes **load-bearing**, and that change
of weight is a result in itself. A minority of `f` liars can hand the committer a
fabricated value, so the value must be *corroborated*. We make corroboration
structural: `attest` has **no `value: T` parameter** — the value is extracted from
the votes — and the resulting `Attested<T,E>` has a private field and no
constructor, so a value can inhabit it only if `f+1` distinct configuration
members voted for it. Value-blindness is not a check that can be forgotten; it is
unrepresentable. The `f+1` threshold buys *existence* (at least one correct
voucher, so the value was not fabricated by the liars alone) but not *uniqueness*:
two conflicting values can each reach it. Uniqueness requires the masking
threshold `⌈(n+2f+1)/2⌉`, well-defined when `n ≥ 4f+1` — the masking-quorum
existence bound the worked example (`n = 5, f = 1`) sits exactly on. At that
threshold, two supporting quorums intersect in `≥ 2f+1` members, of which `≥ f+1`
are correct; a correct member votes once, so two `Committed<T,E>` cannot disagree
within the fault budget. The reduction runs at construction time, in
ordinary Rust, with no external prover: a smart constructor whose success is the
certificate. Driven through the wire under an equivocating host, the two thresholds
sign-flip cleanly — the same adversary that forces two conflicting `Attested` at
`f+1` is denied two `Committed` at the masking threshold.

Down this boundary the *evidence weakens in a documented pattern*: a counted
majority, then a sampled law (a `Lawful` witness minted by property-checking a
merge function's semilattice laws — evidence, not proof), then a counted
supermajority conditional on a declared fault budget, then the same majority again
with its provenance demoted to bytes an adversarial network delivered. The type
discipline survives at every step; this gradient — how much the discipline buys at
each layer — is a supporting observation for the boundary thesis, not a separate
result.

## 6. Boundary III: the cross-process ceiling

§5 already enforces value-uniqueness — `commit_masking` returns `None` when a
second, conflicting value cannot reach the masking threshold. This section asks a
different question, and its answer is the ladder's capstone: could that
construction-time `None` be *replaced* by a compile error — value-uniqueness as
the value-level twin of the epoch type error from §3 — so that a program minting
two conflicting `Committed` fails to typecheck? We pre-registered that optimistic
hypothesis specifically so a refutation would count, and it was refuted. The
distinct claim of this section is not "uniqueness is enforced at construction"
(that is §5) but "*no per-process compile-time enforcement can replace that
construction check*," for a reason independent of the enforcement mechanism.

**The shallow wall.** Rust is not dependently typed. A type cannot be indexed by a
runtime term, and a vote's value is runtime data, so "these two values differ" is
not a type-level proposition. A `compile_fail` test witnesses this concretely — a
runtime value cannot be assigned to a const generic (error E0435) — and a survey
of the escape hatches (stable const generics, nightly `adt_const_params`,
`typenum`, generativity/branding, associated types, the absent `TestEquality`)
finds that none of *these* bridge a runtime value into a type. This wall is real
but shallow; taken alone it says only "the mechanisms we tried route around it,"
which is not yet a ceiling.

**The deep wall, and why it is not a tautology.** The disagreement that
constitutes split-brain lives in *two different processes* — two partitioned
collectors each minting a `Committed`. A caveat first, since §5 proved two
`Committed` cannot disagree: that guarantee holds *within* the declared fault
budget, and two conflicting same-epoch `Committed` are runtime-reachable only when
the budget is exceeded (they need more than `f` members to equivocate across the
two views). But §6's question is orthogonal to whether that state is reachable. A
test makes the type-erasure concrete: two `Committed<u64, 3>` carrying different
values — each minted by `commit_masking` from a separate view, as two partitioned
collectors would — share one type and coexist in one homogeneous `Vec`; the
disagreement survives only in the runtime values, invisible to the compiler. The
point is not that the discipline permits them — it does not, within budget — but
that the *type* could never have distinguished them regardless of how they arose,
so type-level uniqueness enforcement was never on the table. Now, a skeptic will
object that this is true by definition — define the checker as one process's
`rustc`, name a two-process property, and of course
it is out of reach; that is just "a per-process typechecker is not a whole-system
verifier," which is folklore. The objection is correct about the mechanism and
wrong about the point. What makes the per-process scope the *interesting* one is
that it is the scope a real deployment actually runs: each node compiles and
typechecks *its own* binary, in isolation, and there is no build step at which all
participants' code is checked together against a shared partition-time state.
Mechanisms that *do* reach cross-participant agreement escape precisely by leaving
that scope — and the escape is exactly what a partition breaks. That is the
content of the ceiling: not "types can't," but "the enforcement model deployments
run can't, and every model that can, a partition removes."

Concretely: refined multiparty session types and Session★ *do* verify
cross-participant value agreement statically — but they discharge it through a
prover (F★ + SMT), over an *intact* session, which is exactly what a partition
removes; choreographic languages (Choral, HasChor) compile cooperating projected
endpoints, not two partitioned Byzantine minters. So the precise, scoped statement
— the one the abstract now carries — is: *value-uniqueness across a partition is
not reachable by ordinary host-language typechecking; it is enforced at
construction because the threat is cross-process while each participant's type
system is per-process.* The construction-time `None` is therefore not a weaker
stand-in for a compile-time guarantee reachable with more cleverness; it is the
maximal enforcement the deployed model admits. This lands exactly on the
typecheck / prover-obligation / construction-time seam the rest of the ladder
drew — and it is the reason the ladder tops out here.

## 7. Cross-cutting findings

**The map.** The artifact is built in development order as "rungs"; this report
regroups them by the boundary they illustrate. The correspondence:

| Boundary / section | Rung (artifact) | File | What it establishes |
|---|---|---|---|
| §3 shape | 1 | `lib.rs` | epoch as type index; cross-epoch merge is a type error |
| §3 shape | 2 (membership) | `membership.rs` | disjoint→intersecting; members a runtime set |
| §3 shape | 2 (reconfig) | `reconfig.rs` | the two guards partition safety by regime |
| §4 temporal | — (TLA+) | `tla/` | epoch necessary but not sufficient; split-brain at depth 4 |
| §4 temporal | 1 (failover) | `failover.rs` | the runtime lease guard |
| §5 wire+value | 3 | `reconcile.rs` | sampled-law `Lawful` merge witness |
| §5 wire+value | 4 | `byzantine.rs` | masking quorum; witness weight flips |
| §5 wire+value | 5 | `tests/wire_sim.rs` | the `promote` deserialization crossing |
| §5 wire+value | 6 | `attest.rs` | value corroboration; existence vs uniqueness |
| §6 ceiling | 7 | `attest.rs` | value-uniqueness cannot be a compile error |

**Primary contributions.**

1. *The three-boundary mapping.* A worked account of what transfers from a
   GPU-warp type system to distributed consensus (the relational skeleton) and
   what hands off (temporal, wire/value, cross-process), using the
   degenerate-best-case framing as the lens.
2. *The cross-process ceiling.* No per-process compile-time enforcement can
   replace `commit_masking`'s construction check, for the scope reason of §6 —
   the boundary at which the ladder tops out.

**Supporting observations** (evidence for the mapping, not co-equal results).

- *Witness weight changes with the fault model.* A quorum witness is discardable
  under crash faults (`commit`'s unused `_witness`) and load-bearing under
  Byzantine faults (`attest`/`commit_masking`, where the value is extractable only
  from `f+1` corroborating votes). Fresh framing, not a claimed theorem — the
  underlying algorithm is classical.
- *The evidence-strength gradient.* How much typing buys weakens down the ladder:
  counted majority → sampled law → conditional supermajority → wire-demoted
  majority. The discipline never breaks; its evidence weakens predictably.
- *Roots of trust.* Four operator-chosen roots recur — membership (`Config::new`),
  sampled laws, the declared fault budget `f`, and the deserializer (`promote`).
  **Types verify chains; operators choose roots.** This is the type-level analogue
  of a *trusted computing base*: the types propagate each root's consequences
  faithfully but cannot check the root itself, and naming them is how the design
  stays honest about where trust sits.

**A methodological note.** Each rung was pre-registered — hypothesis and
falsifiers written before code — and three rungs turned a *parked blocker* into
the finding: the const-generic obstacle to dynamic membership, the missing wire
protocol, and the compile-error ceiling were each a result in a to-do's clothing.
Pre-registration is what let the null of §6 count as a result rather than a
failure.

## 8. Method and artifact

The artifact is a dependency-free stable-Rust crate — seven results across the
layers of §7's map — with 80 tests, clippy- and rustdoc-clean. The negative
results are carried by `compile_fail` doctests (a cross-epoch merge, an
out-of-epoch vote, the runtime-value-to-const-generic wall), so the report's "you
cannot write this" claims are executable. A bounded TLA+ model cross-checks the
temporal guard — no invariant violation with the guard, split-brain at depth four
without it. A deterministic `turmoil` simulation replays the TLA+ counterexample
as real partition events; determinism is a deliberate buy-in (ordered collections
only, virtual clock, no ambient nondeterminism), checked by asserting that the
same seed reproduces the same event trace byte-for-byte. Two methodological
commitments carry the credibility rather than the authors' word: every rung's
hypothesis and falsifiers were written before its code (so §6's null is a
pre-registered outcome, not a rationalized failure), and the artifact — tests,
model, and simulation — is available for inspection.

## 9. Related work

*Source system.* `warp-types` (GPU-warp typestates) is the system this report
generalizes.

*Behavioral and session types.* Ferrite (ECOOP 2022) and Rumpsteak (PPoPP 2022)
embed session types in Rust but transport native Rust values between participants
compiled into one program — adversarial bytes never arrive. Multiparty session
types type protocol structure, not payload agreement; refined MPST / Session★
(OOPSLA 2020) do type cross-participant value agreement, but via a prover over an
intact session (see §6). Choreographic programming (Choral, TOPLAS 2024; HasChor,
ICFP 2023) compiles cooperating endpoints from one global program, not
partitioned adversaries.

*Verified consensus.* IronFleet (SOSP 2015, crash-fault Paxos in Dafny),
Velisarios (ESOP 2018, PBFT safety in Coq), and Verus (OOPSLA 2023, Rust via SMT)
discharge quorum-intersection safety as a lemma in a prover. This report's move is
smaller and different in kind: the same reductions hold *by construction* — epoch
and complementarity by ordinary typechecking (the unifier rejects a mismatch),
and quorum-intersection at a runtime-checked smart-constructor boundary whose
success is the certificate — rather than as a global proof re-run on every change.
The distinction is precisely the point, and §6 marks where even the
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
sampled/property-tested `Lawful` merge witness. Mocket
(EuroSys 2023), SandTable (EuroSys 2024), and the CCF verifiers (NSDI 2025) replay
model-checker counterexamples against implementations — the lineage our
turmoil-based replay joins.

## 10. Limitations

The report's honesty is its method, and the limitations are a section rather than
a footnote. The artifact is a toy: memberships are small (n = 5 in the worked
examples), epochs bounded, and the property tests exercise small domains. There is
no transport layer — the wire is a test harness over a 25-byte toy protocol, not a
network stack. There is no cryptography; corroboration is byte-equality of votes,
not signatures, so the fault budget `f` is a declared operator axiom no type
checks, and `N > E` across a reconfiguration is a documented boundary invariant
rather than an enforced one. Value-uniqueness is construction-time, not
compile-time, for the structural reason of §6. Every one of these is a place the
type discipline names where it stops; none is faked.

## 11. Conclusion

Structural typing reaches further into a distributed system than its GPU-warp
origin would suggest — it carries epoch, complementarity, majority, masking-quorum
overlap, and value corroboration. But *how* it carries them divides cleanly, and
that division is the report: epoch and complementarity are discharged by the
compiler; majority, masking overlap, and corroboration by runtime-checked
construction boundaries whose success is the certificate — neither a global prover
nor a runtime guard on every operation. It reaches exactly as far as the
*relational* structure goes, and hands off three times for three structural
reasons: temporal facts to a runtime guard, wire data to a construction-time
certificate, and — across a partition — value agreement to a check no per-process
typechecker can perform. The last is the ceiling, and it is a clean one: the
construction-time `None` that enforces value-uniqueness is not a weaker stand-in
for a guarantee we failed to reach, but the most the deployed per-process model
can offer against a cross-process threat. The guarantee the whole ladder bought
was never "split-brain is impossible in the world"; it was "split-brain is
unrepresentable in any process that keeps its promises at the roots" — four of
them, ending at the bytes. Knowing precisely where structure ends is the result.

---

*Artifact: a dependency-free stable-Rust crate (80 tests, bounded TLA+ model,
deterministic network simulation) developed alongside eight cold-reviewed essays.*

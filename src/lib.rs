//! # quorum-types (feasibility toy)
//!
//! **Question this crate answers:** does the *complement-proof* mechanism from
//! `warp-types` survive being indexed by a type-level **epoch**, such that two
//! stale halves from *different* epochs cannot typecheck a `merge`?
//!
//! This is the 30-minute feasibility gate for a distributed-verification
//! direction ("quorum-types"). It is **not** a distributed system. It models
//! the friendliest possible one — fixed membership, no failure — exactly as a
//! GPU warp does, and asks only whether epoch-indexing composes cleanly with
//! compile-time complement proofs.
//!
//! ## What is (deliberately) modelled
//!
//! * [`Quorum<E, S>`] — a membership token at **type-level epoch `E`** over an
//!   active member-set `S`. Move-only (no `Copy`/`Clone`): a token is a *linear*
//!   resource, consumed by exactly one `merge`.
//! * [`split`](Quorum::split) — a reconfiguration that partitions an `All`
//!   quorum into complementary `Lo`/`Hi` halves at the **same** epoch.
//! * [`merge`] — recombines two halves, but *only* when the type system can
//!   prove (a) they carry the **same epoch `E`** and (b) their sets are
//!   **complementary** ([`ComplementOf`]).
//!
//! ## The load-bearing property: split-brain is unrepresentable
//!
//! Because the epoch is a *type* parameter, a merge of two halves from
//! different epochs fails to unify `E` — it is a **compile error**, not a
//! runtime check:
//!
//! ```compile_fail
//! use quorum_types::{Quorum, All, merge};
//! let (lo3, _) = Quorum::<3, All>::genesis(0xFFFF).split();
//! let (_, hi4) = Quorum::<4, All>::genesis(0xFFFF).split();
//! let _ = merge(lo3, hi4); // E: 3 vs 4 do not unify — split-brain unrepresentable
//! ```
//!
//! Two *non-complementary* halves are likewise rejected. `Lo`'s only registered
//! complement is `Hi`, so `merge`'s `A: ComplementOf<B>` bound forces the second
//! argument to be `Quorum<E, Hi>` — a second `Lo` fails to unify:
//!
//! ```compile_fail
//! use quorum_types::{Quorum, All, merge};
//! let (lo_a, _) = Quorum::<3, All>::genesis(0xFFFF).split();
//! let (lo_b, _) = Quorum::<3, All>::genesis(0xFFFF).split();
//! let _ = merge(lo_a, lo_b); // expected `Quorum<3, Hi>`, found `Quorum<3, Lo>`
//! ```
//!
//! And a token cannot be merged twice — linearity is enforced by move
//! semantics, so the second use is a use-after-move:
//!
//! ```compile_fail
//! use quorum_types::{Quorum, All, merge};
//! let (lo, hi) = Quorum::<3, All>::genesis(0).split();
//! let _ = merge(lo, hi);
//! let _ = merge(lo, hi); // `lo`/`hi` already moved — a token is consumed once
//! ```
//!
//! The happy path — same epoch, complementary sets — compiles and recombines
//! the runtime membership mask:
//!
//! ```
//! use quorum_types::{Quorum, All, merge};
//! let (lo, hi) = Quorum::<7, All>::genesis(0xDEAD_BEEF).split();
//! let whole = merge(lo, hi);
//! assert_eq!(whole.members(), 0xDEAD_BEEF);
//! assert_eq!(whole.epoch(), 7);
//! ```
//!
//! ## The failure layer: [`mod@failover`]
//!
//! This module's `merge` **cannot fail** — the lockstep assumption a real
//! distributed system violates. The [`failover`] module adds the lease-degraded
//! complement (validated first in `tla/quorum.tla`): a runtime lease guard for
//! reconfiguration, because the TLA+ model proved the type-level epoch is
//! *necessary but not sufficient* — split-brain is temporal and cannot be
//! discharged structurally.
//!
//! ## Dynamic, unbounded membership: [`mod@membership`]
//!
//! The `All`/`Lo`/`Hi` sets above are static and *disjoint* (a partition, like a
//! GPU warp's lanes). The [`membership`] module generalizes to dynamic, unbounded
//! clusters by flipping the set relation: distributed safety needs *intersecting*
//! quorums (any two majorities overlap), not disjoint complements. Membership
//! becomes a runtime set; the type carries only the epoch and the majority
//! property (the `gradual` boundary).
//!
//! ## Both mechanisms together: [`mod@reconfig`]
//!
//! [`reconfig`] unifies the temporal lease with the dynamic quorum. Composing
//! them shows they are not redundant: within an epoch, safety is *structural*
//! (quorum intersection); across an epoch, quorums can be disjoint, so safety is
//! *temporal* (the lease). Each covers exactly where the other fails.
//!
//! ## Typing the data, not just the membership: [`mod@consistency`]
//!
//! The modules above type *who* is in a quorum. [`consistency`] types the
//! *values*: a small lattice recording how much consensus a value carries,
//! whose moves run `Local` → `At` → `Agreed` (in *strength*, `Agreed` sits
//! between `Local` and `At`). Its one asymmetry mirrors the whole crate —
//! moving *up* the lattice ([`Local::commit`](consistency::Local::commit))
//! requires a [`membership::Quorum`] as evidence, while moving *down*
//! ([`At::forget_epoch`](consistency::At::forget_epoch)) is free. Acting on an
//! uncommitted `Local` where a decision is required is a compile error.
//!
//! ## Reconciling divergence: [`mod@reconcile`]
//!
//! [`consistency`] stops where two `Agreed` values *disagree*. [`reconcile`]
//! extends the evidence discipline to the merge: a [`reconcile::Diverged`]
//! conflict (minted by comparing committed values) is consumed by an
//! evidence-gated merge demanding a [`reconcile::Lawful`] witness — the merge
//! function's semilattice laws property-checked at a runtime boundary
//! (*sampled evidence, not proof*; Propel does this soundly and statically).
//! The merged result re-enters the lattice at the **bottom**: a merge is a
//! new proposal, and only a quorum lifts it back up.
//!
//! ## Byzantine evidence: [`mod@byzantine`]
//!
//! Everything above assumes nodes fail by *stopping*. [`byzantine`] asks what
//! survives when they fail by **lying**: the crash majority's one-node
//! intersection is worthless if that node is the liar, so the certificate
//! changes — a masking quorum (`n ≥ 4f+1`, overlap `≥ 2f+1`; Malkhi–Reiter)
//! whose [`ByzQuorum`](byzantine::ByzQuorum) is a *distinct type* from
//! [`membership::Quorum`]. Supplying crash evidence where Byzantine evidence
//! is required is a compile error. The fault budget `f` is an operator-declared
//! axiom the types propagate but cannot check.
//!
//! ## Attested values: [`mod@attest`]
//!
//! [`byzantine`] types *who* may be believed; [`consistency`] types a value's
//! consensus strength but commits with the witness **discarded** (`_witness`) —
//! sound under crash faults, forgeable under Byzantine ones. [`attest`] binds
//! the two: [`Attested`](attest::Attested) has no caller-supplied value and no
//! constructor, so a value inhabits it only when `f+1` distinct members voted
//! for it — value-blindness is unrepresentable. `f+1` buys *existence*, not
//! *uniqueness*; [`Committed`](attest::Committed) at the masking threshold buys
//! uniqueness, reduced to [`Overlap`](byzantine::Overlap) at construction time
//! rather than in a prover. `tests/attest_wire.rs` drives it under an
//! equivocating host: existence splits, the masking threshold denies the split.
//!
//! ## The coordination-free floor: [`mod@crdt`]
//!
//! Every module above types a *boundary* where a runtime fact earns a guarantee.
//! [`crdt`] types the place where there is **none**: a state-based CRDT is a value
//! in a join-semilattice, and its [`join`](crdt::JoinSemilattice::join) —
//! commutative, associative, idempotent — is total, infallible, and witness-free.
//! Replicas converge under *any* delivery order, with duplicates, needing no
//! quorum, lease, or causal typestate (Hellerstein's CALM: monotone = coordination
//! -free). It is the mirror of [`causal`]: that rung makes out-of-order delivery a
//! compile error (it *enforces* an order); this one makes order *irrelevant*. The
//! compiler enforces the interface (the only combinator is `join`); the tests
//! discharge the algebraic laws, with a non-idempotent negative control.
//!
//! ## Buying back coordination-freedom: [`mod@escrow`]
//!
//! [`crdt`] types operations that are *already* coordination-free. [`escrow`] types
//! the harder case: an operation that is **not** invariant-confluent — a bounded
//! counter ("stock `≥ 0`") — made local by *pre-partitioning* the budget (Bailis's
//! demarcation). A [`Reservation<BUDGET>`](escrow::Reservation) is a linear token;
//! capacity enters via `grant` (once per tree; `grant` roots independent trees),
//! moves only by conserving `split`/`merge`/`spend`, so within a tree `Σ remaining
//! + Σ spent == BUDGET` always and the bound cannot be crossed by any interleaving.
//! Spending your own reservation is free; needing
//! *more* is the seam — the through-line inverted: pay coordination **once** at
//! partition time, then act freely. Budgets are epoch-like — reservations from
//! different `BUDGET`s cannot `merge` (compile error).
//! ## Flexible read/write quorums: [`mod@flex`]
//!
//! [`membership`] types one quorum kind (majorities, `2·maj(N) > N`). [`flex`]
//! types the whole **Flexible Paxos** frontier: [`ReadQuorum<N, R>`](flex::ReadQuorum)
//! and [`WriteQuorum<N, W>`](flex::WriteQuorum) are distinct types, and the only
//! way to witness that a read observes a write ([`read_sees_write`](flex::read_sees_write))
//! carries an inline `const {}` assertion that `R + W > N` — so obtaining the
//! intersection witness for a miss-prone sizing is a **compile error**. The types
//! enforce the *sufficient* direction (`R + W > N` ⇒ every read meets every write);
//! z3 established the frontier's exactness separately (strict majority is its
//! symmetric instance). The same `const {}`
//! threshold-lift the reconfiguration rung uses, now for the flexible frontier.
//! ## Classifying coordination-freedom: [`mod@calm`]
//!
//! The rungs above each sit on one side of a cut — a `crdt` join is coordination
//! -free, quorum commits / `escrow` top-ups need a seam. [`calm`] types the cut as
//! a compositional **effect**: an [`Op<C>`](calm::Op) is tagged [`Free`](calm::Free)
//! or [`Coordinated`](calm::Coordinated), a [`Pipeline`](calm::Pipeline) threads the
//! **join** of its steps' levels (`Coordinated` is sticky), and
//! [`deploy_coordinator_free`](calm::deploy_coordinator_free) compiles only for an
//! all-`Free` pipeline — one coordinated step anywhere is a compile error. This is
//! Hellerstein's CALM theorem (coordination-free iff monotone) as a type-level
//! classifier; the coordination lattice is itself the two-element join-semilattice
//! `crdt` types over data. It *propagates* declared monotonicity labels, it does not
//! *prove* them — like [`byzantine`]'s fault budget, the labels are axioms.
//!
//! ## The blocking transaction: [`mod@twophase`]
//!
//! Every rung above hands you a token you *can* spend. [`twophase`] types the
//! **opposite** move, the one that defines two-phase atomic commit: a participant that
//! votes YES enters an in-doubt [`Prepared`](twophase::Prepared) state that exposes
//! **no** `commit`/`abort` — it has *surrendered* unilateral choice, and its only exit
//! [`decide`](twophase::Participant::decide)s by consuming a coordinator
//! [`Decision`](twophase::Decision). AC3/AC2 are structural (a `Final<Commit>` needs a
//! `Decision<Commit>`; `Final` is terminal), but uniform agreement (AC1) is a
//! coordinator obligation, not a compile-time guarantee — `decide` accepts any decision,
//! so a single decision per transaction is trusted, not enforced. The famous *blocking*
//! hazard then falls out for free as a
//! `#[must_use]` linear `Prepared` token you hold but **cannot discharge** if the
//! decision never arrives. The type system's inability to let you proceed *is* the
//! model of the protocol's weakness. It inverts [`escrow`]: escrow pays coordination
//! once then spends freely (non-blocking); 2PC blocks *at* the seam. (A companion TLC
//! model checks exactly where cooperative termination lifts the block.)
//! ## Detecting concurrency: [`mod@vclock`]
//!
//! [`causal`] *enforces* an order and [`reconcile`] merges divergent *values*; neither
//! answers the question between them — *do two updates actually conflict, or does one
//! supersede the other?* [`vclock`] types the vector-clock decision: [`compare`](vclock::VClock::compare)
//! mints an [`Ordered`](vclock::Ordered) or [`Concurrent`](vclock::Concurrent) witness,
//! and the lossy last-writer-wins shortcut [`take_dominant`](vclock::take_dominant)
//! *requires* `Ordered` — so silently dropping a concurrent update (the classic
//! lost-update bug) is a **compile error**; concurrent clocks force a lossless
//! [`merge`](vclock::VClock::merge) (pairwise max — the clock is itself a [`crdt`]
//! join-semilattice). It is the honest precondition-detector for [`reconcile`]: a
//! `Concurrent` witness is the evidence that a merge is *warranted*. Sharper witness
//! than most of the crate — `compare` is a pure decision procedure over the clocks, not
//! a trusted `bool`; the only residual trust is that the clocks faithfully count events.
//! ## Physical time — the write path: [`mod@commit_wait`]
//!
//! Every clock above is *logical* ([`causal`], [`session`] watermarks) — honest by
//! construction, because it counts real events. [`commit_wait`] types the one clock that
//! can be *wrong*: physical time. It models Spanner's **TrueTime**, where
//! [`now`](commit_wait::TrueTime::now) returns an uncertainty *interval* `[earliest, latest]`,
//! and buys **external consistency** by **commit-wait**: a [`Pending<T>`](commit_wait::Pending)
//! commit exposes no value — the committed value is reachable only through
//! [`Externalized<T>`](commit_wait::Externalized), and the only route there
//! ([`try_release`](commit_wait::Pending::try_release)) succeeds once a later interval's
//! `earliest` has passed the commit timestamp (the ε window closed). So *externalizing a
//! write before its uncertainty window closes is a compile error*. The honest seam is
//! sharp here: the guarantee is *global* (every writer must wait, and ε must truly bracket
//! the clock), and the types own only *this* commit's local wait — you cannot type what
//! time it is, only that you waited for the uncertainty to close.
//!
//! ## Physical time — the read path: [`mod@staleness`]
//!
//! [`staleness`] types the mirror choice on reads. A **linearizable** read needs the leader
//! ([`LeaderLease`](staleness::LeaderLease) — coordination); a **bounded-staleness** read is
//! served locally from a lagging follower *if the client accepts an age bound `Δ`*, which
//! rides in the type as a const generic. [`read_within::<Δ>`](staleness::Replica::read_within)
//! mints a [`Staleness<Δ, T>`](staleness::Staleness) only when the measured lag is within
//! `Δ`, and [`require::<TOL>`](staleness::Staleness::require) carries a `const { Δ ≤ TOL }`
//! gate — a read looser than the tolerance is a compile error (the same const-assert
//! [`flex`] uses). It is physical *recency*, orthogonal to [`session`]'s logical
//! read-your-writes: `Δ` old, not "reflects your writes."
//!
//! ## Making a lost lock safe: [`mod@fencing`]
//!
//! [`failover`]'s TLA+ verdict was that the type-level epoch is *necessary but not
//! sufficient*: an old leader keeps serving until its lease lapses — a temporal hole
//! no type can plug. [`fencing`] closes that residual from the **resource** side
//! (Kleppmann): the lock service issues [`FencingToken`](fencing::FencingToken)s with
//! strictly increasing numbers, and a [`FencedStore`](fencing::FencedStore)'s only
//! mutator *requires* one, rejecting any write whose token is older than the newest it
//! has accepted. Two clients can *both* believe they hold the lock — mutual exclusion
//! has already failed — and writes *to this store* are still safe, because the stale
//! write is fenced there (the store needs no lease, clock, or quorum of its own — it
//! pushes those into the token authority). An [`Accepted`](fencing::Accepted)
//! receipt is minted only by the store's own compare, and the token is unforgeable
//! (private field) — but the guarantee is *mostly* a runtime monotone check, and
//! honestly so: the types forbid an unfenced write and a forged token, while total
//! mediation and cross-failure monotonicity stay operator obligations. It trades
//! [`failover`]'s clock-trust for coverage-trust. (A TLC discriminant confirms the
//! reject is load-bearing — remove it and a stale token overwrites a newer write.)
//!
//! ## The compensating transaction: [`mod@saga`]
//!
//! [`twophase`] buys atomicity *and* isolation by **blocking** — a prepared
//! participant holds its choice and its locks until the coordinator decides. [`saga`]
//! is the non-blocking dual (Garcia-Molina & Salem): each local step commits
//! immediately, and a later failure recovers atomicity of *outcome* by running the
//! completed steps' compensations in **reverse**. That reverse order is typed
//! *structurally* — the pending compensations are a type-level cons-list
//! ([`Cons`](saga::Cons)/[`Nil`](saga::Nil)), and
//! [`compensate_next`](saga::Aborting::compensate_next) peels only the head, burying
//! the rest in the tail type, so compensating out of order is *unrepresentable* and
//! [`done`](saga::Aborting::done) exists only on the empty stack. It mirrors
//! [`twophase`] inverted: 2PC's headline is a token you *cannot discharge* (block); the
//! saga's is a stack you are *steered* (`#[must_use]`, not forced — affinity lets you
//! drop instead) to discharge in reverse (unwind) — both linearity stories, opposite
//! ends of the blocking/isolation trade. The types own the control flow;
//! whether each compensation *semantically* undoes its step, and the lost isolation of
//! intermediate reads, are the runtime seam. (A z3 model checks the ordering claim
//! under step dependencies: reverse restores the pre-saga state, out-of-order can
//! violate it.)
//!
//! ## Linearizability from a line, not a set: [`mod@chain`]
//!
//! Every consistency rung above earns its guarantee from a *set* that intersects —
//! a quorum ([`membership`], [`flex`], [`byzantine`]), a value lattice, or a clock.
//! [`chain`] takes the other road (van Renesse & Schneider): the replicas are a
//! **line** Head → Mid → Tail, an update is applied at the head and forwarded one
//! hop at a time, and it is committed *exactly when it reaches the tail* — by which
//! point every upstream replica has it. Position is a type parameter; the [`Forward`](chain::Forward)
//! trait carries the successor as an associated type (`Tail` has none), so
//! [`forward`](chain::Update::forward) advances one hop and [`commit`](chain::Update::commit)
//! is reachable *only* from `Update<`[`Tail`](chain::Tail)`>` — committing a
//! half-replicated update is a compile error, not a runtime check. Strong
//! consistency comes from the *order of traversal*, with no quorum anywhere. The
//! types own one update's Head→Tail walk; per-node durability, cross-update per-link
//! FIFO order (the linearizability-across-updates seam a TLC discriminant covers),
//! and chain repair on failure stay runtime.
//!
//! ## Deadlock made unrepresentable: [`mod@lockorder`]
//!
//! [`failover`] and [`fencing`] type a *single* lock; [`lockorder`] types the
//! *multi-lock* hazard's classic fix (Havender/Dijkstra): give every resource a
//! **rank** and acquire in strictly increasing rank order, so a hold-and-wait cycle
//! cannot close. A [`Held`](lockorder::Held)`<HI>` carries the highest rank held as
//! a const generic, and [`acquire`](lockorder::Held::acquire)`::<R>` guards its body
//! with `const { assert!(R > HI) }` — an out-of-order acquire fails at
//! monomorphization, so the circular wait that could deadlock is a compile error.
//! A move-only [`Guard`](lockorder::Guard) makes [`release`](lockorder::Held::release)
//! strictly LIFO (the mirror of the increasing-rank acquire). Its seam is a distinct
//! *species*: not a trusted runtime witness but a trusted global **ordering** — that
//! the ranks form one consistent order every thread obeys is a declared axiom (the
//! [`byzantine`] `f` / [`calm`]-label shape), and the monotone watermark is
//! sufficient-not-exact ([`flex`]'s trade). A z3 discriminant confirms rank-ordered
//! acquisition is the sole thing preventing a wait-for cycle.
//!
//! ## Serializability by locking — the pessimistic road: [`mod@two_phase_lock`]
//!
//! [`twophase`] and [`saga`] type the **A** of ACID (atomicity); the quorum, lattice,
//! and clock rungs type **C** (consistency). [`two_phase_lock`] types the letter the
//! ladder had skipped — **I**solation. Two-phase *locking* (not to be confused with
//! two-phase *commit* in [`twophase`]) makes an interleaved schedule
//! conflict-serializable with one monotone rule: a transaction may acquire locks (the
//! **growing** phase) then release them (the **shrinking** phase), but *never acquire
//! after releasing*. The phase is a type parameter, and
//! [`acquire`](two_phase_lock::Txn::acquire) exists **only** on
//! `Txn<`[`Growing`](two_phase_lock::Growing)`>` — so an acquire-after-release, the
//! one move that breaks serializability, is a compile error, not a runtime check. The
//! types own one transaction's phase discipline; that *every* transaction is two-phase
//! (the global obligation), the real lock table, and the *deadlock* 2PL can cause
//! (the price [`lockorder`] pays to avoid) stay runtime seams. A TLC discriminant
//! confirms the hold-across-read-modify-write is load-bearing: drop it and a lost
//! update appears.
//!
//! ## Serializability by validation — the optimistic road: [`mod@occ`]
//!
//! [`occ`] is `two_phase_lock`'s dual (Kung & Robinson): take **no** locks, read a
//! private snapshot, and only at the end **validate** that nothing you read has
//! changed — commit if so, abort and retry if not. Same guarantee, opposite bet on
//! contention (the [`twophase`]↔[`saga`], [`crdt`]↔[`causal`] shape). The unsafe move
//! is *committing without validating*, and it is unrepresentable:
//! [`commit`](occ::Txn::commit) exists only on `Txn<`[`Valid`](occ::Valid)`>`, and the
//! only door into that phase is [`validate`](occ::Txn::validate) — which **consumes**
//! the transaction and re-emits it validated, so there is no detachable "validated"
//! token to replay onto a different transaction (the deliberate fix for the crate's
//! recurring witness-not-unforgeable hazard). Unlike `two_phase_lock`'s purely
//! structural phase machine, OCC's Reading→Valid door is a *runtime* check, so its
//! guarantee rests on a trusted witness ([`vclock`]/[`fencing`] species): that the
//! version counter is honest, and that no writer slips into the validate→commit
//! window, are the seams. A z3 discriminant confirms validation is the sole thing
//! standing between OCC and a stale-write anomaly.
//!
//! ## Consistent global state by protocol — the operational road: [`mod@snapshot`]
//!
//! Every rung so far typed something *per transaction*. [`snapshot`] steps to a
//! different axis: recording a consistent state of *every* process at one logical
//! instant (Chandy & Lamport, 1985), without stopping the system. The move that
//! manufactures an **orphan** — recording an incoming channel before recording your own
//! process state — is unrepresentable: [`record_channel`](snapshot::Snapshot::record_channel)
//! exists only on `Snapshot<`[`Recording`](snapshot::Recording)`>`, and the sole door
//! into that phase is [`begin`](snapshot::Snapshot::begin), which records own-state
//! first. Like `two_phase_lock`, it is a purely **structural** guarantee (no trusted
//! witness, only a type-level phase); FIFO channels, one-marker-per-channel, and
//! global consistency across processes are the documented runtime seams. A TLC
//! discriminant confirms the record-self-first order is load-bearing against orphans.
//!
//! ## Consistent global state by predicate — the denotational road: [`mod@consistent_cut`]
//!
//! [`consistent_cut`] is `snapshot`'s dual — not a rival algorithm but the *property*
//! its output must satisfy. A cut (a [`vclock`]-shaped frontier of per-process event
//! counts) is **consistent** iff it is causally left-closed: every message it records
//! as *received* also has its *send* inside the cut. The forbidden shape is an orphan
//! (received inside, sent outside). [`verify`](consistent_cut::Cut::verify) — defined
//! only on `Cut<`[`Unverified`](consistent_cut::Unverified)`, N>` — **consumes** the cut
//! and re-emits it [`Consistent`](consistent_cut::Consistent) iff no orphan exists, so
//! the proof is fused to the frontier (no detachable witness — the same fix `occ` uses).
//! Its Unverified→Consistent door is a *runtime* check, so like [`vclock`] it trusts the
//! message log to be complete and its indices real. A z3 discriminant confirms causal
//! closure is the sole thing separating a cut from the orphan anomaly. The two rungs
//! compose: `snapshot` produces by protocol what `consistent_cut` certifies by
//! predicate — Chandy–Lamport's theorem is exactly that identity.
//!
//! ## Recovering a failed process, forward — by replay: [`mod@message_log`]
//!
//! The snapshot axis records a consistent state; *recovery* is what you do with one
//! after a crash, and it splits into two families (Elnozahy et al., ACM CSUR 2002).
//! [`message_log`] types the **forward** family: log each message's determinant
//! *before* delivering it, so a crashed process is replayed forward from its last
//! checkpoint with no surviving process rolled back. The pessimistic-logging rule —
//! never let state depend on an unlogged delivery (Alvisi & Marzullo's
//! *always-no-orphans*) — is made **structural**: [`deliver`](message_log::Msg::deliver)
//! exists only on `Msg<`[`Logged`](message_log::Logged)`>`, and a caused send must cite
//! the [`Stable`](message_log::Stable) witness a delivery mints, so depending on an
//! unlogged message is unrepresentable. Like `snapshot` it is purely **structural**;
//! log stability and the true dependency graph are the runtime seams. A TLC
//! discriminant confirms log-before-deliver is load-bearing against orphan processes.
//!
//! ## Recovering a failed process, backward — by rollback: [`mod@recovery_line`]
//!
//! [`recovery_line`] types the **backward** family and its hazard. With uncoordinated
//! checkpoints, the latest ones need not form a consistent cut, so recovery must
//! *search* for a **recovery line** — the greatest consistent cut ≤ the failure
//! frontier — by rolling back every orphan, a cascade that is Randell's **domino
//! effect** (1975). [`resolve`](recovery_line::Line::resolve) — defined only on
//! `Line<`[`Tentative`](recovery_line::Tentative)`, N>` — **consumes** the frontier and
//! re-emits the [`Resolved`](recovery_line::Resolved) fixpoint, the result fused to the
//! search (no detachable witness, the `occ`/`consistent_cut` fix). It reuses
//! [`consistent_cut`]'s orphan predicate but *computes the meet* rather than checking
//! membership; its runtime search rests on the same log-honesty seam. A z3 discriminant
//! confirms rollback-closure is the sole thing standing between a recovery line and an
//! orphan. The two recovery rungs bound the design space — pay at logging time
//! (forward) or at recovery time (backward) — the crate's fourth structural/witness
//! dual, on the backward-time mirror of the snapshot axis.
//!
//! ## Delivery order by source, at the floor — FIFO: [`mod@fifo`]
//!
//! [`causal`] types the *middle* of the delivery-ordering hierarchy; [`fifo`] types its
//! **floor** — the weakest non-trivial order, per-sender FIFO. A sender's own messages
//! deliver in send order and nothing is promised across senders (that cross-sender
//! constraint is exactly what [`causal`] adds). The rule is a compile-time **arithmetic**
//! wall: [`Msg::deliver`](fifo::Msg::deliver) carries `const { assert!(N == PREV + 1) }`, so
//! a gap in a sender's stream fails const-evaluation (E0080) — the [`lockorder`]/[`staleness`]
//! mechanism, not [`causal`]'s type-identity unification. FIFO is **coordination-free**
//! (I-confluent), so like [`causal`], [`crdt`], and the [`calm`] floor the type system can
//! own it. This rung also *discharges* the untyped "FIFO channels" assumption [`chain`] and
//! [`snapshot`] rest on. The seam is the same as [`causal`]'s: the transport supplies the true
//! sequence numbers.
//!
//! ## Delivery order by agreement — total (atomic) broadcast: [`mod@total_order`]
//!
//! [`total_order`] types the orthogonal *agreement axis*: all correct processes deliver in one
//! common order. This is a different kind of guarantee — total-order broadcast is **equivalent
//! to consensus** (Chandra–Toueg), so it is *not* coordination-free, and that is exactly why it
//! is a runtime **witness** rather than a structural rung: the structural/witness split
//! coincides with the [`calm`]/[`crdt`] coordination-free boundary. A [`Sequencer`](total_order::Sequencer)
//! is the sole minter of [`Ordered`](total_order::Ordered) messages (private fields ⇒
//! unforgeable), and a linear [`Cursor`](total_order::Cursor) delivers them in position order —
//! once, serial, no forking. What the types own is local (unforgeable, once, serial); what they
//! cannot own is the crux — that every receiver shares one sequencer, i.e. **consensus** — which
//! is the trusted runtime seam. `fifo` (own nothing to a witness) and `total_order` (own the
//! whole order to a witness) are the two poles of the ordering-strength spectrum, with [`causal`]
//! between them.
//!
//! ## Delivery count — at-most-once, by a single-use capability: [`mod@at_most_once`]
//!
//! Loop's ordering rungs type *when* a message is delivered relative to others; [`at_most_once`]
//! and `at_least_once` type *how many times* its effect fires — the reliability axis orthogonal
//! to order. [`at_most_once`] gates an effect on a move-only, unforgeable [`Effect`](at_most_once::Effect)
//! capability minted once per operation: a retransmitted duplicate carries no fresh token (E0451)
//! and the original is consumed by [`apply`](at_most_once::Effect::apply) (reuse is E0382), so the
//! effect fires **≤ 1 time** whatever the delivery count. It is the non-idempotent sibling of
//! [`crdt`]'s road (make the effect idempotent so duplicates are harmless); both are the
//! at-most-once corner of exactly-once *processing*. Purely **structural** — at-most-once by
//! linearity is local, no agreement. The seam: recognizing which wire copies are duplicates is the
//! transport's job, as `fifo` trusts it for sequence numbers.
//!
//! ## Delivery count — at-least-once, by ack'd retransmit: [`mod@at_least_once`]
//!
//! [`at_least_once`] is the dual: the sender keeps a [`Pending`](at_least_once::Pending) message and
//! retransmits it until the receiver returns an [`Ack`](at_least_once::Ack) — the only key that
//! [`retire`](at_least_once::Pending::retire)s it. The ack is unforgeable (E0451) and identity-tied,
//! so an ack for one message cannot retire another (E0308). This is a runtime **witness** while
//! `at_most_once` is structural, because at-least-once needs a **round trip** — evidence from the
//! other side — which is the coordination the type system cannot mint: the structural/witness split
//! is the coordination-free (CALM) boundary once more. Composing the two — at-least-once delivery of
//! an at-most-once (or [`crdt`]-idempotent) effect — gives exactly-once *processing*; exactly-once
//! *delivery* is the documented impossibility (you cannot type "arrived once", only "applied once").
//! The seam is liveness: if every copy is lost, no ack is ever minted.
//!
//! ## Flow control by a local send window — occupancy, structural: [`mod@send_window`]
//!
//! Order types *when* a message is delivered and count *how many times* its effect fires;
//! [`send_window`] and `backpressure` type *how many may be in flight at once* — the occupancy
//! axis. [`send_window`] bounds the **sender's own** outstanding messages to a compile-time
//! window `N`: [`send`](send_window::Window::send) mints a linear, unforgeable
//! [`Slot`](send_window::Slot) and [`complete`](send_window::Window::complete) returns it, so at
//! most `N` slots exist and **in-flight ≤ N by construction**. A full window refuses further
//! sends locally (`Err`) — backpressure computed with no coordination. It is a *semaphore* (slots
//! regenerate), where [`escrow`]'s reservation is a *drained budget* (occupancy vs volume). Purely
//! **structural**: bounding your own work is local, no agreement.
//!
//! ## Flow control by receiver credit — occupancy, witness: [`mod@backpressure`]
//!
//! [`backpressure`] is the dual: a `send_window` protects the sender but says nothing about the
//! **receiver's** buffer. A [`Credit`](backpressure::Credit) is minted only by the receiver's
//! [`grant`](backpressure::Receiver::grant) (unforgeable, E0451) reflecting real free buffer, and
//! the sender may transmit only by [`spend`](backpressure::Credit::spend)ing one, so messages
//! outstanding toward the receiver never exceed the buffer it advertised — **the receiver never
//! overflows**. This is a runtime **witness** while `send_window` is structural, because not
//! overrunning the *peer* needs evidence *from* the peer — the coordination the type system
//! cannot mint: the structural/witness split is the coordination-free (CALM) boundary a third
//! time, after order and count. Composing the two — send the minimum of the local window and the
//! outstanding credits — is TCP's `min(self-limit, rwnd)` flow control. The seam is that
//! [`grant`](backpressure::Receiver::grant) honestly reflects free memory (a declared axiom) and
//! liveness: a receiver that never grants starves the sender.
//!
//! ## Leadership authority by term, structural: [`mod@term`]
//!
//! The axes above type *messages*; this one types *who is in charge*. Most of the crate presupposes a
//! leader ([`failover`] takes a lease "at the boundary", [`staleness`]'s `LeaderLease::acquire` takes
//! a trusted bool). [`term`] and `election` type the two halves nobody else does. [`term`] is the
//! **structural** half — the discipline of *holding* leadership: a [`Reign<T>`](term::Reign) is a
//! linear, unforgeable token for the leader of term `T`, gating leader-only
//! [`decree`](term::Reign::decree)s; a [`Decree<X, T>`](term::Decree) commits only under the
//! matching-term reign, so a stale-term command cannot be committed under a later term (E0308, the
//! crate's founding epoch-unification trick, not [`lockorder`]'s arithmetic rank); and
//! [`superseded_by`](term::Reign::superseded_by) carries `const { U > T }` (E0080), so authority never
//! moves backward. Purely **structural**: one node enforces its own term discipline, no coordination.
//!
//! ## Leadership by winning a majority, witness: [`mod@election`]
//!
//! [`election`] is the dual — the act of *becoming* leader. A candidate's [`Ballot<T>`](election::Ballot)
//! records votes and [`close`](election::Ballot::close)s against the electorate, reusing
//! [`membership`]'s [`certify`](membership::Config::certify) to mint an
//! [`Elected<T, E>`](election::Elected) only from a majority vote-[`Quorum`](membership::Quorum). This
//! is a runtime **witness** while `term` is structural, because winning needs evidence from a quorum of
//! the electorate — the coordination the type system cannot mint: the structural/witness split is the
//! coordination-free (CALM) boundary a fourth time, after order, count, and occupancy. **At most one
//! leader per term** (a runtime property, not a type guarantee) rides quorum
//! [`intersect`](membership::Quorum::intersect): two winners of one term would share a voter who voted
//! twice (forbidden by the one-vote-per-term seam). `Elected<T, E>` supplies the provenance a
//! `term::Reign<T>` install *should* be gated on — the lease `failover` takes on faith (the coupling is
//! conventional, not type-enforced). The seams: one vote per voter per term (a declared axiom) and one
//! electorate per uniqueness argument ([`reconfig_safety`] governs across configurations).
//!
//! ## Liveness by local suspicion — failure detection, structural: [`mod@suspicion`]
//!
//! The leadership axis presupposes knowing a leader *failed*; every lease/failover module takes that
//! judgement on faith ([`failover`] consumes a lease expiry, [`staleness`]'s leader check is "crude").
//! This axis types the judgement itself — liveness, the one dimension the rest of the crate declares
//! out of scope. `suspicion` is the **structural** half: a [`Monitor<NODE>`](suspicion::Monitor)
//! watching one peer mints a linear, unforgeable [`Suspected<NODE>`](suspicion::Suspected) iff the
//! silence since it was last `heard_at` **exceeds** a runtime budget (an `Option`-returning local
//! boundary, like [`staleness`]'s `read_within`). The primary mechanism is **node identity** — the
//! watched node rides in the type, so `Suspected<3>` and `Suspected<4>` are different types (E0308) —
//! deliberately *not* a timeout `const` gate (which would re-skin [`staleness`]'s `Δ ≤ TOL`); the
//! timeout is a runtime value. Purely **structural**: one node judges its own clock, no coordination —
//! but a slow node is indistinguishable from a dead one (FLP), so this is *suspicion*, not death.
//!
//! ## Liveness by corroborated death — failure detection, witness: [`mod@detector`]
//!
//! `detector` is the dual — *confirming* a death. A [`Report<NODE>`](detector::Report) collects the
//! reporters that [`corroborate`](detector::Report::corroborate) node `NODE`'s silence and
//! [`close`](detector::Report::close)s against the electorate, reusing [`membership`]'s
//! [`certify`](membership::Config::certify) to mint a [`Confirmed<NODE, E>`](detector::Confirmed) only
//! from a majority. This is a runtime **witness** while `suspicion` is structural, because a single
//! vantage cannot tell slow from dead — confirming death needs evidence from a quorum, the coordination
//! the type system cannot mint: the structural/witness split is the coordination-free (CALM) boundary a
//! fifth time, after order, count, occupancy, and leadership. **No contradictory verdict** (a node is
//! not confirmed dead and alive at once — a runtime property, not a type guarantee) rides quorum
//! [`intersect`](membership::Quorum::intersect): a dead-majority and an alive-majority of one electorate
//! share a node that would hold both opinions (forbidden by the one-opinion-per-node seam). A
//! corroboration is a **claim** — a bare `NodeId`, like a vote — because a peer's local
//! `suspicion::Suspected` cannot cross the wire; `Confirmed<NODE, E>` supplies the death-provenance
//! [`failover`]/[`reconfig`] take on faith. The seams: one opinion per node (a declared axiom) and one
//! electorate ([`reconfig_safety`] governs across configurations).
//!
//! ## Reclaiming storage by unanimous stability — garbage collection, witness: [`mod@stability`]
//!
//! `stability` is the dual — certifying that forgetting is *safe*. Every other witness rung earns its
//! guarantee from quorum **intersection** ([`membership`], [`election`], [`detector`]); safe-to-forget
//! is the first axis where intersection is the *wrong* primitive, because a majority reporting "applied
//! up to `W`" says nothing about the lagging minority that still needs the prefix. So a
//! [`Barrier<STREAM, E>`](stability::Barrier) collects one watermark [`ack`](stability::Barrier::ack)
//! per member and [`seal`](stability::Barrier::seal) mints a
//! [`StableUpTo<STREAM, E>`](stability::StableUpTo) **only if every member has acked** (unanimity, not a
//! majority), carrying the `min` of their watermarks as the frontier. This is a runtime **witness**
//! while `compaction` is structural, because knowing the drop is safe needs evidence from *everyone* —
//! the strongest coordination in the crate, strictly beyond a quorum: the structural/witness split is
//! the coordination-free (CALM) boundary a sixth time, after order, count, occupancy, leadership, and
//! liveness, and the first whose witness is a **barrier** rather than a quorum. `StableUpTo` supplies
//! the frontier a `compaction` should compact up to (conventional, not type-enforced). The seams: a
//! watermark is a trusted claim, and unanimity's liveness cost is that one silent node halts GC forever
//! — escaped in practice by excluding a [`detector`]-confirmed-dead node from the roster.
//!
//! ## Still out of scope (parking lot → later versions)
//!
//! Benchmarks. (The deterministic network simulation formerly parked here
//! shipped as `tests/wire_sim.rs` — the note's "needs a wire protocol first"
//! turned out to be the finding, not the blocker.)
//!
//! ## Relationship to `warp-types`
//!
//! The [`ActiveSet`] / [`ComplementOf`] traits here are a **minimal
//! reimplementation** of the concept in the published `warp-types` crate — a
//! model, not the real GPU trait surface — kept self-contained so this
//! experiment varies only the *epoch* dimension. `warp-types` is treated as a
//! read-only reference and is not a dependency.

#![forbid(unsafe_code)]

pub mod attest;
pub mod byzantine;
pub mod calm;
pub mod causal;
pub mod commit_wait;
pub mod consistency;
pub mod crdt;
pub mod escrow;
pub mod failover;
pub mod flex;
pub mod membership;
pub mod reconcile;
pub mod reconfig;
pub mod reconfig_safety;
pub mod session;
pub mod twophase;
pub mod vclock;
pub mod staleness;
pub mod fencing;
pub mod saga;
pub mod chain;
pub mod lockorder;
pub mod two_phase_lock;
pub mod occ;
pub mod snapshot;
pub mod consistent_cut;
pub mod message_log;
pub mod recovery_line;
pub mod fifo;
pub mod total_order;
pub mod at_most_once;
pub mod at_least_once;
pub mod send_window;
pub mod backpressure;
pub mod term;
pub mod election;
pub mod suspicion;
pub mod detector;
pub mod stability;

use core::marker::PhantomData;

mod sealed {
    /// Prevents downstream code from asserting bogus member-sets or false
    /// complement proofs — the guarantees are only as trustworthy as the set
    /// of impls, so the set of impls is closed.
    pub trait Sealed {}
}

/// A type-level active member-set. Sealed: the only inhabitants are the ones
/// defined in this crate.
pub trait ActiveSet: sealed::Sealed {}

/// The full membership (a quorum of everyone).
#[derive(Debug)]
pub struct All;
/// The low complementary half produced by [`Quorum::split`].
#[derive(Debug)]
pub struct Lo;
/// The high complementary half produced by [`Quorum::split`].
#[derive(Debug)]
pub struct Hi;

impl sealed::Sealed for All {}
impl sealed::Sealed for Lo {}
impl sealed::Sealed for Hi {}
impl ActiveSet for All {}
impl ActiveSet for Lo {}
impl ActiveSet for Hi {}

/// A compile-time proof that `Self` and `Other` are the two complementary
/// halves of a common parent — i.e. their union is the parent and their
/// intersection is empty. Sealed: no downstream code can fabricate a
/// complement relation that does not hold.
///
/// The only proofs that exist are `Lo ⟂ Hi` and `Hi ⟂ Lo`. There is
/// deliberately no `All: ComplementOf<_>` and no `Lo: ComplementOf<Lo>`.
pub trait ComplementOf<Other: ActiveSet>: ActiveSet + sealed::Sealed {}

impl ComplementOf<Hi> for Lo {}
impl ComplementOf<Lo> for Hi {}

/// A membership token at type-level epoch `E` over member-set `S`.
///
/// Move-only by construction (no `Copy`/`Clone`): the token is a *linear*
/// resource. `members` is the runtime membership bitmask; the type-level `E`
/// and `S` are what the compiler reasons about.
#[must_use = "a Quorum token is a linear resource; dropping it silently loses membership"]
pub struct Quorum<const E: u64, S: ActiveSet> {
    members: u32,
    _set: PhantomData<S>,
}

impl<const E: u64, S: ActiveSet> Quorum<E, S> {
    /// The runtime membership bitmask carried by this token.
    pub const fn members(&self) -> u32 {
        self.members
    }

    /// The epoch this token belongs to (mirrors the type-level `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }
}

impl<const E: u64> Quorum<E, All> {
    /// Mint a full quorum at epoch `E` with the given membership mask.
    ///
    /// The epoch is supplied via turbofish, e.g. `Quorum::<3, All>::genesis(m)`.
    pub const fn genesis(members: u32) -> Self {
        Quorum { members, _set: PhantomData }
    }

    /// Partition a full quorum into complementary `Lo`/`Hi` halves at the
    /// **same** epoch. Consumes `self` (the whole can't coexist with its parts).
    ///
    /// The bit-16 split point is a toy stand-in for a real membership partition.
    pub fn split(self) -> (Quorum<E, Lo>, Quorum<E, Hi>) {
        let lo = self.members & 0x0000_FFFF;
        let hi = self.members & 0xFFFF_0000;
        (
            Quorum { members: lo, _set: PhantomData },
            Quorum { members: hi, _set: PhantomData },
        )
    }
}

/// Recombine two halves into a full quorum.
///
/// Compiles **only** when the type system can prove both preconditions:
/// * `a` and `b` share the same type-level epoch `E` (unifying the two `E`s), and
/// * `A: ComplementOf<B>` (the sets are complementary halves).
///
/// Both tokens are consumed. In this toy `merge` is total (it cannot fail) —
/// the lockstep assumption a real distributed `merge` does not get to make.
pub fn merge<const E: u64, A, B>(a: Quorum<E, A>, b: Quorum<E, B>) -> Quorum<E, All>
where
    A: ComplementOf<B>,
    B: ActiveSet,
{
    Quorum { members: a.members | b.members, _set: PhantomData }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_epoch_complements_split_and_merge() {
        let all = Quorum::<3, All>::genesis(0xDEAD_BEEF);
        assert_eq!(all.epoch(), 3);

        let (lo, hi) = all.split();
        assert_eq!(lo.members(), 0x0000_BEEF, "lo half keeps low 16 bits");
        assert_eq!(hi.members(), 0xDEAD_0000, "hi half keeps high 16 bits");
        assert_eq!(lo.epoch(), 3, "split preserves epoch");
        assert_eq!(hi.epoch(), 3, "split preserves epoch");

        let merged = merge(lo, hi);
        assert_eq!(merged.members(), 0xDEAD_BEEF, "merge is the union of halves");
        assert_eq!(merged.epoch(), 3, "merge preserves epoch");
    }

    #[test]
    fn merge_is_order_independent() {
        // Hi: ComplementOf<Lo> also holds, so halves may be merged either way.
        let (lo, hi) = Quorum::<9, All>::genesis(0xABCD_1234).split();
        let merged = merge(hi, lo);
        assert_eq!(merged.members(), 0xABCD_1234);
        assert_eq!(merged.epoch(), 9);
    }

    #[test]
    fn empty_membership_round_trips() {
        let (lo, hi) = Quorum::<0, All>::genesis(0).split();
        assert_eq!(lo.members(), 0);
        assert_eq!(hi.members(), 0);
        assert_eq!(merge(lo, hi).members(), 0);
    }
}

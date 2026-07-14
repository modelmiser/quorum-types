# Experience-report skeleton — quorum-types

*Draft outline, 2026-07-14. Not committed; for direction. The claim, the section
arc, and which existing evidence (rungs, tests, posts) feeds each section.
Content only — no venue/submission logistics.*

## Working title

**"How far does structural typing reach into a distributed system?"**
— an experience report on carrying consensus evidence in stable Rust's type
system, and finding the three boundaries where it must hand off.

## The claim (one sentence)

The *relational* structure of distributed consensus — epoch, complementarity,
majority, masking-quorum overlap, value corroboration — can be carried in stable
Rust's type system and discharged by ordinary typechecking at construction-time
boundaries; and the exact points where it must hand off to a runtime certificate,
or beyond a type system entirely, are *principled and predictable*, not
incidental gaps.

## The spine: three hand-off boundaries

The paper's argument is that structural typing hands off in three places, each
for a structural reason, and the ladder discovered them in order:

1. **Temporal facts → runtime guard.** A type can carry "these two certificates
   are the same epoch"; it cannot carry "wait for the old lease to expire."
   *Evidence:* rung 1 (epoch as type error) → rung 2 (TLA+: epoch necessary but
   not sufficient, split-brain at depth 4) → rung 3 (`reconfigure` returns
   `Result`).
2. **Values off the wire → construction-time certificate.** A `const E: u64`
   cannot be lifted from runtime bytes; a value cannot be corroborated by the
   type system. One `promote()` / `certify` / `commit_masking` boundary is where
   bytes become typed evidence. *Evidence:* rung 5 (`promote`, the deserializer
   as the 4th root of trust), rung 6 (`attest`/`commit_masking`: value-blindness
   unrepresentable, but enforcement is a construction-time `None`).
3. **Cross-process invariants → unreachable by any per-process type system.**
   Value-uniqueness (no two conflicting committed values) is a property of two
   processes; no single typechecker observes both. *Evidence:* rung 7, the
   ceiling — conflicting `Committed` cannot be a compile error, and the reason is
   structural, not a Rust limitation.

## Contributions

1. **A worked warp-types → distributed mapping.** What transfers from a GPU-warp
   ownership/session type system to real consensus (the relational skeleton) and
   what does not (temporal, cross-process). The degenerate-best-case framing (a
   warp is the friendliest distributed system) is the lens.
2. **The evidence-strength ladder.** Down the rungs the evidence weakens in a
   documented pattern: counted majority (rung 2) → sampled law (rung 3
   reconciliation, `Lawful` witness) → counted supermajority conditional on a
   declared budget (rung 4 Byzantine) → the same majority with its provenance
   demoted to bytes off a wire (rung 5). A map of *how much* typing buys at each
   layer, not just whether it type-checks.
3. **The crash-vs-Byzantine witness-weight result** (rung 6). A quorum witness is
   *discardable* under crash faults (`commit`'s `_witness` is unused — honest
   majority existence suffices) and *load-bearing* under Byzantine faults
   (`attest`/`commit_masking` extract the value from `f+1` corroborating votes;
   value-blindness is unrepresentable). Framed as fresh, not a claimed theorem.
4. **The ceiling result** (rung 7). Value-uniqueness cannot be a compile error in
   a per-process type system; construction-time enforcement is maximal. Precise:
   not reachable by *ordinary host-language typechecking* (refined MPST/Session★
   can, but via a prover over an intact session a partition destroys).
5. **The "roots of trust" refrain.** Four operator-chosen roots — membership
   (`Config::new`), samples (rung 3), fault budget `f` (rung 4), the deserializer
   (`promote`, rung 5). *Types verify chains; operators choose roots.* A design
   principle for where trust concentrates when you type distributed evidence.

## Method / artifact (the "experience" the report reports)

- A dependency-free stable-Rust crate: 7 rungs / 11 layers, 80 tests, clippy +
  rustdoc clean; `compile_fail` doctests carry the negative results.
- A bounded TLA+ model (guarded: no violation; negative control: split-brain at
  depth 4) cross-checking the runtime guard.
- A deterministic `turmoil` network simulation replaying the TLA+ counterexample
  as real partition events; a sign-flip control (equivocating host splits at
  `f+1`, denied at the masking threshold).
- Pre-registration discipline: each rung wrote H1/H0 falsifiers before code; two
  rungs (4 const-generic blocker, 5 wire, 7 ceiling) turned a *parked blocker*
  into the finding.

## Related work (already assembled across the posts — to formalize)

- **Source system:** warp-types (GPU warp typestates).
- **Session/behavioral types:** Ferrite (ECOOP'22), Rumpsteak (PPoPP'22),
  multiparty session types, refined MPST / Session★ (OOPSLA'20), choreographies
  (Choral TOPLAS'24, HasChor ICFP'23) — none face untrusted bytes / partitioned
  Byzantine minters at compile time.
- **Verified consensus (prover, not typecheck):** IronFleet (SOSP'15, crash
  Paxos), Velisarios (ESOP'18, PBFT/Coq), Verus (OOPSLA'23).
- **Byzantine quorums / safety:** Malkhi–Reiter (DC'98), PBFT (OSDI'99).
- **Witness-as-a-type:** Recalling a Witness (POPL'18, F★ monotonic state).
- **Branding / provenance:** GhostCell (ICFP'21), generativity.
- **Gradual boundary:** Gradual Session Types (ICFP'17) — casts + blame.
- **Spec-counterexample replay:** Mocket (EuroSys'23), SandTable (EuroSys'24),
  CCF verifiers (NSDI'25). **Sampled-law merge:** Propel (PLDI'23).

## Limitations (the honest fences — a section, not a footnote)

Toy scale (n=5, bounded epochs); no transport layer (the wire is a test harness
over a 25-byte toy protocol); no crypto/signatures; `f` is a declared axiom no
type checks; `N > E` across reconfiguration unenforced; value-uniqueness is
construction-time, not compile-time. The report's honesty *is* its method — every
guarantee names where it stops.

## Open questions / future work

Larger memberships and real transport; a signed variant (does crypto move any of
the runtime roots back into the type?); whether a prover-backed refinement layer
(Session★-style) could reclaim cross-process value-uniqueness for the
*non-partitioned* case; the timed-session-types thread (parked).

---

## Status

The full prose draft this skeleton planned is now `PAPER.md` (~4,500 words,
cold-reviewed to clean at moderate-or-worse severity across technical, claims, and
editorial passes). This file remains as the planning artifact — the claim, the
three-boundary spine, and the evidence map. Remaining, if the draft is taken
further: figures (the ladder, the three boundaries, the sign-flip trace) and an
artifact-evaluation appendix.

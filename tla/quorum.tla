---------------------------- MODULE quorum ----------------------------
(***************************************************************************)
(* Protocol model for the *lease-degraded complement* — the failure case  *)
(* the Rust toy (`quorum-types`) deliberately omits.                       *)
(*                                                                         *)
(* The toy proved: a `merge` of two halves requires the SAME type-level    *)
(* epoch, so cross-epoch merge is a compile error (split-brain             *)
(* unrepresentable at the type level). But the toy's `merge` cannot fail.  *)
(* Real splits are *discovered* (crash/partition) and a complement token   *)
(* can die with its holder — strict linearity then deadlocks. The rescue   *)
(* is a lease: a leader steps down when its lease lapses, and a new leader  *)
(* may not form until then.                                                *)
(*                                                                         *)
(* This spec model-checks that failover discipline. `serving` is the set   *)
(* of epochs whose leader currently believes it is authoritative — latched *)
(* state, because a partitioned old leader keeps serving until IT notices. *)
(*                                                                         *)
(* EnforceLeaseGuard toggles the single load-bearing precondition:         *)
(*   TRUE  (quorum_guarded.cfg) — a new leader forms only when no prior     *)
(*         leader is still serving  =>  NoSplitBrain holds.                 *)
(*   FALSE (quorum_noguard.cfg)  — that wait is skipped  =>  TLC finds a    *)
(*         split-brain counterexample. This negative control is what makes  *)
(*         the model earn its keep: it proves the guard is load-bearing,    *)
(*         not decorative.                                                  *)
(*                                                                         *)
(* Partition is modelled abstractly (a token holder becomes unreachable),  *)
(* not as a message layer. Epochs are logical (no real-time clock).        *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS MaxEpoch,           \* bound on configuration generations
          EnforceLeaseGuard   \* TRUE = safe failover discipline; FALSE = negative control

ASSUME MaxEpoch \in Nat
ASSUME EnforceLeaseGuard \in BOOLEAN

Halves  == {"lo", "hi"}       \* the two complementary membership halves
Epochs  == 0..MaxEpoch

VARIABLES
    epoch,      \* current configuration generation (monotonic)
    alive,      \* [Halves -> BOOLEAN]  is this half's holder reachable?
    mintedAt,   \* [Halves -> Epochs]   epoch the current half-tokens were minted at
    serving     \* SUBSET Epochs        epochs whose leader still believes it is authoritative

vars == <<epoch, alive, mintedAt, serving>>

TypeOK ==
    /\ epoch \in Epochs
    /\ alive \in [Halves -> BOOLEAN]
    /\ mintedAt \in [Halves -> Epochs]
    /\ serving \subseteq Epochs

Init ==
    /\ epoch    = 0
    /\ alive    = [h \in Halves |-> TRUE]
    /\ mintedAt = [h \in Halves |-> 0]
    /\ serving  = {}

(* A leader forms a full quorum at epoch e by merging both complementary   *)
(* halves: both must be alive and minted at the SAME epoch e (the toy's     *)
(* merge constraint). The lease guard forbids forming while a prior leader  *)
(* is still serving.                                                        *)
Form(e) ==
    /\ \A h \in Halves : alive[h] /\ mintedAt[h] = e
    /\ e \notin serving
    /\ (EnforceLeaseGuard => serving = {})
    /\ serving' = serving \cup {e}
    /\ UNCHANGED <<epoch, alive, mintedAt>>

(* A token holder becomes unreachable (crash / partition). *)
Crash(h) ==
    /\ alive[h]
    /\ alive' = [alive EXCEPT ![h] = FALSE]
    /\ UNCHANGED <<epoch, mintedAt, serving>>

(* A leader of an older generation steps down: once superseded by a newer   *)
(* epoch it can no longer renew its lease with the quorum, so it lapses.     *)
LeaseLapse(e) ==
    /\ e \in serving
    /\ e < epoch
    /\ serving' = serving \ {e}
    /\ UNCHANGED <<epoch, alive, mintedAt>>

(* Install a new configuration after a suspected failure: advance the epoch  *)
(* and re-mint both halves. Old `serving` leaders persist until they lapse   *)
(* (they don't yet know they were superseded — the source of split-brain).   *)
Reconfigure ==
    /\ epoch < MaxEpoch
    /\ epoch'    = epoch + 1
    /\ mintedAt' = [h \in Halves |-> epoch + 1]
    /\ alive'    = [h \in Halves |-> TRUE]
    /\ UNCHANGED serving

Next ==
    \/ \E e \in Epochs  : Form(e)
    \/ \E h \in Halves  : Crash(h)
    \/ \E e \in serving : LeaseLapse(e)
    \/ Reconfigure

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* Safety                                                                  *)
(***************************************************************************)

\* No two distinct authoritative leaders at once.
NoSplitBrain == Cardinality(serving) <= 1

\* No leader claims authority for a future (unformed) generation.
ServingWithinEpoch == \A e \in serving : e <= epoch

\* Tokens never claim a future epoch; epoch never runs backwards.
EpochMonotone == \A h \in Halves : mintedAt[h] <= epoch
=========================================================================

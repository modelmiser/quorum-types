//! The saga — a long-lived transaction that compensates in **reverse**, the
//! non-blocking dual of [`twophase`](crate::twophase).
//!
//! Two-phase commit buys atomicity *and* isolation by **blocking**: a prepared
//! participant surrenders its choice and holds locks until the coordinator decides.
//! A saga (Garcia-Molina & Salem, 1987) makes the opposite trade. It is a sequence of
//! local transactions `T1..Tn`, each with a *compensation* `C1..Cn` that semantically
//! undoes it. Each `Ti` commits **locally and immediately** — no global lock, no
//! in-doubt state, never blocking. If a later step fails, the saga recovers atomicity
//! of *outcome* (all-forward or all-undone) by running the compensations for the
//! steps that did complete — and it must run them in **reverse order** `Cj..C1`,
//! because a later step can depend on an earlier one.
//!
//! This module types that reverse-order discipline **structurally**: compensating out
//! of order is not a runtime bug to guard against — it is *unrepresentable*.
//!
//! ## The mechanism — a type-level compensation stack
//!
//! The pending compensations are a type-level cons-list: [`Nil`] is empty, and each
//! completed step wraps the prior stack in one more [`Cons`] layer. A three-step saga
//! carries `Cons<Cons<Cons<Nil>>>`.
//!
//! * [`Saga::begin`] starts at `Saga<`[`Nil`]`>`.
//! * [`try_step`](Saga::try_step) runs a forward action. On `Ok` it records the step's
//!   compensation id and grows the saga to `Saga<Cons<S>>`. On `Err` the forward
//!   action is *assumed* to have had no effect (so there is nothing to compensate *for
//!   it* — see the forward-atomicity seam), and the saga transitions to
//!   [`Aborting`]`<S>` to unwind the steps that *did* complete.
//! * [`commit`](Saga::commit) ends a fully-forward saga: the compensations are
//!   discarded.
//! * [`compensate_next`](Aborting::compensate_next) — defined **only** on
//!   `Aborting<`[`Cons`]`<S>>` — peels the **head** (most recent) compensation and
//!   returns the unwinder over the *tail*. The deeper compensations are buried in the
//!   tail type: you cannot reach `C1` before `C2` before `C3`. Reverse order is a type
//!   invariant, not a convention.
//! * [`done`](Aborting::done) — defined **only** on `Aborting<`[`Nil`]`>` — closes a
//!   fully-unwound saga. You cannot declare "compensated" with steps remaining.
//!
//! ## Forward to commit
//!
//! ```
//! use quorum_types::saga::Saga;
//! let (saga, _) = Saga::begin().try_step(1, || Ok::<_, ()>(())).unwrap();
//! let (saga, _) = saga.try_step(2, || Ok::<_, ()>(())).unwrap();
//! let (saga, _) = saga.try_step(3, || Ok::<_, ()>(())).unwrap();
//! let committed = saga.commit();
//! assert_eq!(committed.steps(), 3);
//! ```
//!
//! ## A failed step compensates the completed steps in reverse (3, 2, 1)
//!
//! ```
//! use quorum_types::saga::Saga;
//! // Three steps complete, then a fourth forward action fails → unwind.
//! let (saga, _) = Saga::begin().try_step(1, || Ok::<_, ()>(())).unwrap();
//! let (saga, _) = saga.try_step(2, || Ok::<_, ()>(())).unwrap();
//! let (saga, _) = saga.try_step(3, || Ok::<_, ()>(())).unwrap();
//!
//! // The 4th forward action fails; its own effect never happened, so only 1..3 unwind.
//! let (aborting, err) = saga.try_step(4, || Err::<(), _>("boom")).unwrap_err();
//! assert_eq!(err, "boom");
//!
//! // Compensations come out head-first — the reverse of completion. The type will not
//! // let you take them in any other order.
//! let mut order = Vec::new();
//! let (c, aborting) = aborting.compensate_next(); order.push(c);
//! let (c, aborting) = aborting.compensate_next(); order.push(c);
//! let (c, aborting) = aborting.compensate_next(); order.push(c);
//! let _ = aborting.done();
//! assert_eq!(order, vec![3, 2, 1]);
//! ```
//!
//! ## You cannot declare "compensated" with work remaining — a compile error
//!
//! [`done`](Aborting::done) exists only on `Aborting<`[`Nil`]`>`; a non-empty stack
//! has no such method, and its only compensation exit hands you the *head* first:
//!
//! ```compile_fail
//! use quorum_types::saga::Saga;
//! let (saga, _) = Saga::begin().try_step(1, || Ok::<_, ()>(())).unwrap();
//! let (saga, _) = saga.try_step(2, || Ok::<_, ()>(())).unwrap();
//! let (aborting, _) = saga.try_step(3, || Err::<(), ()>(())).unwrap_err();
//! // aborting: Aborting<Cons<Cons<Nil>>> — two compensations still pending.
//! let _ = aborting.done(); // no `done` on a non-empty stack: steps remain
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! The types own the *order*: if you choose to unwind, the compensation ids are forced
//! out most-recent-first. They do not own whether you unwind at all (affine — you may
//! drop the handle), whether executing each id succeeds (liveness), or whether `Ci`
//! semantically undoes `Ti`:
//!
//! * **A compensation must truly undo its step.** The type sequences `Ci` correctly;
//!   it cannot check that `Ci` actually reverses `Ti`'s effect — a no-op compensation
//!   typechecks. Same trust shape as [`reconcile`](crate::reconcile)'s `Lawful`
//!   witness: semantic correctness is assumed, not proved. (An out-of-tree z3 model, in
//!   the research harness and not shipped in this crate, checks the *ordering* claim
//!   under a dependency model: reverse order restores the pre-saga state, out-of-order
//!   can violate it.)
//! * **Forward-step atomicity is assumed.** [`try_step`](Saga::try_step) drops the
//!   failed step's compensation on `Err`, which is correct *only* if a failing
//!   `forward` left no effect. A forward action that mutates state and *then* returns
//!   `Err` leaks an uncompensated effect; such a step must instead return `Ok`
//!   (recording its compensation) and signal failure through the compensation path.
//! * **No isolation.** Every `Ti` commits immediately, so a concurrent reader can
//!   observe an intermediate state (after `T1`, before `T2`) — the classic saga
//!   weakness that 2PC pays blocking to avoid. This rung types the sequence of steps,
//!   not concurrency isolation.
//! * **Compensations are assumed to succeed.** [`compensate_next`](Aborting::compensate_next)
//!   yields the id for the caller to run; a real saga needs each `Ci` to be retriable
//!   /idempotent until it does succeed. That liveness is out of scope — the same
//!   visible-but-unremoved hazard as [`twophase`](crate::twophase)'s block.
//! * **Backward recovery only.** This models compensation (backward recovery). Sagas
//!   also admit *pivot* and *retriable* steps (forward recovery); not modelled here.
//! * **Affine, not linear.** An `Aborting<Cons<_>>` still *pending* can be dropped
//!   rather than unwound — Rust affinity permits it, and `#[must_use]` only flags an
//!   ignored value, so the API steers toward full unwinding without a panicking
//!   `Drop` forcing it. Same caveat as [`failover`](crate::failover)'s leased token.

/// An empty compensation stack — a saga with no completed steps to undo.
#[derive(Debug)]
pub struct Nil;

/// One compensation atop the tail stack `T`. The runtime `id` labels the step's
/// compensation; the *type* nesting records the stack's depth and forces the LIFO
/// unwind order. Constructed only inside this module (private fields).
#[derive(Debug)]
pub struct Cons<T> {
    id: u32,
    tail: T,
}

/// A saga in its forward phase, carrying the type-level stack `S` of compensations
/// accumulated by the steps completed so far.
///
/// Move-only and `#[must_use]`: a saga in progress has committed local effects that
/// must be resolved one way or the other — driven forward to [`commit`](Saga::commit)
/// or unwound through [`Aborting`] — or those effects dangle half-done.
#[must_use = "a Saga in progress has committed steps; commit it or compensate it, don't drop it"]
#[derive(Debug)]
pub struct Saga<S> {
    stack: S,
    // Invariant: `depth` == the number of `Cons` layers in `S`. Kept in sync only by
    // construction (`begin` = 0, `try_step` = +1); the private fields make it
    // unforgeable, but any future constructor must preserve it or `Committed::steps`
    // desyncs from the type-level depth.
    depth: u32,
}

impl Saga<Nil> {
    /// Begin a saga with no steps yet completed.
    pub const fn begin() -> Self {
        Saga { stack: Nil, depth: 0 }
    }
}

impl<S> Saga<S> {
    /// Attempt the next forward step, identified for compensation by `comp_id`.
    ///
    /// Runs `forward`. On `Ok(v)` the step's effect is now committed locally: push its
    /// compensation and advance to `Saga<Cons<S>>`, handing back `v`. On `Err(e)` the
    /// forward action is **assumed** not to have taken effect (see the module's
    /// forward-atomicity seam) — there is nothing to compensate for *this* step — so the
    /// saga enters compensation, returning an [`Aborting`]`<S>` that will unwind the
    /// *previously* completed steps in reverse, along with `e`.
    #[allow(clippy::type_complexity)]
    pub fn try_step<T, E>(
        self,
        comp_id: u32,
        forward: impl FnOnce() -> Result<T, E>,
    ) -> Result<(Saga<Cons<S>>, T), (Aborting<S>, E)> {
        match forward() {
            Ok(v) => Ok((
                Saga {
                    stack: Cons { id: comp_id, tail: self.stack },
                    depth: self.depth + 1,
                },
                v,
            )),
            Err(e) => Err((Aborting { stack: self.stack }, e)),
        }
    }

    /// The number of forward steps completed so far.
    pub const fn depth(&self) -> u32 {
        self.depth
    }

    /// All forward steps succeeded — commit the saga, discarding the compensations.
    pub fn commit(self) -> Committed {
        Committed { steps: self.depth }
    }
}

/// A committed saga: every forward step ran and none were compensated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Committed {
    steps: u32,
}

impl Committed {
    /// How many forward steps the saga committed.
    pub const fn steps(&self) -> u32 {
        self.steps
    }
}

/// A saga in its **compensation** phase, unwinding the type-level stack `S`.
///
/// The only exits are [`compensate_next`](Aborting::compensate_next) (on a non-empty
/// stack — peels the head and continues) and [`done`](Aborting::done) (on [`Nil`] —
/// closes a fully-unwound saga). There is no method to reach a buried compensation,
/// so the reverse order is structural.
#[must_use = "an Aborting saga has compensations still pending; unwind them via compensate_next"]
#[derive(Debug)]
pub struct Aborting<S> {
    stack: S,
}

impl<S> Aborting<Cons<S>> {
    /// Run the **most recent** uncompensated step's compensation (LIFO). Returns its
    /// `id` — for the caller to execute the actual compensating effect — and the
    /// unwinder over the *tail*. The deeper compensations remain buried in `S` until
    /// this layer is peeled, so they cannot run first.
    pub fn compensate_next(self) -> (u32, Aborting<S>) {
        let Cons { id, tail } = self.stack;
        (id, Aborting { stack: tail })
    }
}

impl Aborting<Nil> {
    /// Close a fully-unwound saga: every completed step has been compensated.
    pub fn done(self) -> Compensated {
        Compensated { _priv: () }
    }
}

/// A fully-compensated saga: every completed step was undone, in reverse order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compensated {
    _priv: (),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fully_forward_saga_commits() {
        let (s, _) = Saga::begin().try_step(1, || Ok::<_, ()>(())).unwrap();
        let (s, _) = s.try_step(2, || Ok::<_, ()>(())).unwrap();
        assert_eq!(s.depth(), 2);
        let committed = s.commit();
        assert_eq!(committed.steps(), 2);
    }

    #[test]
    fn try_step_threads_the_forward_value() {
        // The forward action's Ok value is handed back for the next step to use.
        let (s, first) = Saga::begin().try_step(1, || Ok::<_, ()>(41)).unwrap();
        let (_s, second) = s.try_step(2, || Ok::<_, ()>(first + 1)).unwrap();
        assert_eq!(second, 42);
    }

    #[test]
    fn a_failed_step_unwinds_completed_steps_in_reverse() {
        let (s, _) = Saga::begin().try_step(10, || Ok::<_, ()>(())).unwrap();
        let (s, _) = s.try_step(20, || Ok::<_, ()>(())).unwrap();
        let (s, _) = s.try_step(30, || Ok::<_, ()>(())).unwrap();

        // The 4th forward action fails; its effect never happened, so it is NOT on the
        // stack — only 10, 20, 30 unwind.
        let (aborting, e) = s.try_step(40, || Err::<(), _>("boom")).unwrap_err();
        assert_eq!(e, "boom");

        let mut order = Vec::new();
        let (c, aborting) = aborting.compensate_next();
        order.push(c);
        let (c, aborting) = aborting.compensate_next();
        order.push(c);
        let (c, aborting) = aborting.compensate_next();
        order.push(c);
        let _ = aborting.done();

        assert_eq!(order, vec![30, 20, 10], "compensations fire in reverse of completion");
    }

    #[test]
    fn an_immediate_failure_leaves_nothing_to_compensate() {
        // First forward action fails: the saga is at Nil, so it unwinds to Compensated
        // with no compensations run.
        let (aborting, e) = Saga::begin().try_step(1, || Err::<(), _>(7)).unwrap_err();
        assert_eq!(e, 7);
        let _done: Compensated = aborting.done();
    }
}

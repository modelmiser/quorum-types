//! A type-level **CALM classifier** — tracking coordination-freedom as an effect.
//!
//! The research loops kept circling one cut: *ordering your own actions is free;
//! confirming you have observed someone else's is coordination.* The `crdt` rung
//! is the free side (monotone joins); [`consistency::Local::commit`](crate::consistency::Local::commit),
//! acquiring more `escrow` capacity, and a stale-read obligation
//! are the coordinated side. This module makes that cut a **compile-time property
//! of a whole computation**, not just of one operation.
//!
//! It is the type-level form of Hellerstein's **CALM** theorem (*Consistency As
//! Logical Monotonicity*): a program is coordination-free **iff** it is monotone.
//! Equivalently — a pipeline is coordination-free iff *every* operation in it is
//! monotone; a single non-monotone operation forces coordination on the whole
//! thing. That "sticky" propagation is exactly a join on a two-element lattice.
//!
//! ## The coordination lattice (which is itself a join-semilattice)
//!
//! `Free < Coordinated`. Composing two operations takes the **join** of their
//! levels: `Free ⊔ Free = Free`, but anything joined with `Coordinated` is
//! `Coordinated`. So the classifier's own algebra is a join-semilattice — the same
//! structure the `crdt` rung types over *data*, here lifted to *effects*.
//! [`JoinLevel`] computes that join at the type level, and a [`Pipeline<C>`] threads
//! the running level through composition.
//!
//! ## The gate: only a `Free` pipeline deploys without a coordinator
//!
//! [`CoordinationFree`] is a marker implemented **only** for [`Free`]. A function
//! that may run without a coordinator ([`deploy_coordinator_free`]) bounds its
//! argument on it, so handing it a pipeline that contains even one coordinated step
//! is a **compile error**:
//!
//! ```compile_fail
//! use quorum_types::calm::{Pipeline, Op, deploy_coordinator_free};
//! let p = Pipeline::start()
//!     .then(Op::monotone("crdt::join"))
//!     .then(Op::coordinated("consistency::commit")); // poisons the level
//! deploy_coordinator_free(&p); // Pipeline<Coordinated>: not CoordinationFree → compile error
//! ```
//!
//! ## What this is — and is NOT
//!
//! This **propagates** a declared monotonicity label; it does not **prove** it.
//! [`Op::monotone`] trusts the caller that the named operation really is monotone
//! (establishing that is out of scope here — it is the job of a CRDT / monotone-op
//! layer and its law-based property tests, which *sample* rather than prove). So "this
//! pipeline is coordination-free" is sound *relative to correct labels*, exactly as
//! [`byzantine`](crate::byzantine)'s fault budget `f` is an operator-declared axiom
//! the types propagate but cannot check. The value added is compositional: label
//! the leaves once, and the coordination-freedom of every pipeline built from them
//! is computed — and enforced — by the compiler.
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::calm::{Pipeline, Op, deploy_coordinator_free};
//!
//! // An all-monotone pipeline is coordination-free — deploys with no coordinator.
//! let p = Pipeline::start()
//!     .then(Op::monotone("gcounter::increment"))
//!     .then(Op::monotone("gset::insert"))
//!     .then(Op::monotone("crdt::join"));
//! assert!(p.coordination_free());
//! assert_eq!(deploy_coordinator_free(&p), &["gcounter::increment", "gset::insert", "crdt::join"]);
//! ```

use core::marker::PhantomData;

type Phantom<T> = PhantomData<fn() -> T>;

mod sealed {
    pub trait Sealed {}
}

/// A coordination level. Sealed: the only levels are [`Free`] and [`Coordinated`].
pub trait Coordination: sealed::Sealed {
    /// Whether a computation at this level runs without any cross-replica
    /// coordination. `true` for [`Free`], `false` for [`Coordinated`].
    const COORDINATION_FREE: bool;
}

/// **Bottom of the coordination lattice.** A monotone, coordination-free operation
/// (a CRDT join, a grow-only insert).
pub enum Free {}

/// **Top of the coordination lattice.** An operation that needs a seam — a quorum
/// commit, acquiring more escrow capacity, a freshness-gated read.
pub enum Coordinated {}

impl sealed::Sealed for Free {}
impl sealed::Sealed for Coordinated {}
impl Coordination for Free {
    const COORDINATION_FREE: bool = true;
}
impl Coordination for Coordinated {
    const COORDINATION_FREE: bool = false;
}

/// The type-level **join** on the coordination lattice: `Self ⊔ Other`. `Free` is
/// absorbed, `Coordinated` dominates — so composing any operation with a
/// coordinated one yields a coordinated result.
pub trait JoinLevel<Other: Coordination>: Coordination {
    /// The join `Self ⊔ Other`.
    type Out: Coordination;
}
impl JoinLevel<Free> for Free {
    type Out = Free;
}
impl JoinLevel<Coordinated> for Free {
    type Out = Coordinated;
}
impl JoinLevel<Free> for Coordinated {
    type Out = Coordinated;
}
impl JoinLevel<Coordinated> for Coordinated {
    type Out = Coordinated;
}

/// Marker for levels safe to run **without a coordinator** — implemented only for
/// [`Free`]. This is the compile-time gate: [`deploy_coordinator_free`] bounds on
/// it, so a [`Coordinated`] pipeline cannot satisfy it.
pub trait CoordinationFree: Coordination {}
impl CoordinationFree for Free {}

/// A single operation tagged with its coordination level `C`.
#[must_use]
pub struct Op<C: Coordination> {
    name: &'static str,
    _c: Phantom<C>,
}

impl Op<Free> {
    /// Declare a **monotone**, coordination-free operation. Trusts the caller that
    /// `name` is genuinely monotone — this labels, it does not verify.
    pub const fn monotone(name: &'static str) -> Self {
        Op { name, _c: PhantomData }
    }
}

impl Op<Coordinated> {
    /// Declare an operation that **needs coordination** (a seam).
    pub const fn coordinated(name: &'static str) -> Self {
        Op { name, _c: PhantomData }
    }
}

impl<C: Coordination> Op<C> {
    /// The operation's label.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// A composed computation whose coordination level `C` is the join of its
/// operations' levels. Built with [`start`](Pipeline::start) and
/// [`then`](Pipeline::then); the level is recomputed by the compiler at every step.
#[must_use]
pub struct Pipeline<C: Coordination> {
    ops: Vec<&'static str>,
    _c: Phantom<C>,
}

impl Pipeline<Free> {
    /// The empty pipeline — vacuously coordination-free.
    pub fn start() -> Self {
        Pipeline { ops: Vec::new(), _c: PhantomData }
    }
}

impl Default for Pipeline<Free> {
    fn default() -> Self {
        Self::start()
    }
}

impl<C: Coordination> Pipeline<C> {
    /// Append an operation. The result's level is `C ⊔ D` — so once a
    /// [`Coordinated`] step is added, the pipeline stays coordinated.
    pub fn then<D: Coordination>(mut self, op: Op<D>) -> Pipeline<C::Out>
    where
        C: JoinLevel<D>,
    {
        self.ops.push(op.name);
        Pipeline { ops: self.ops, _c: PhantomData }
    }

    /// Whether the whole pipeline is coordination-free (a runtime mirror of the
    /// type-level level, from the `const`).
    #[must_use]
    pub const fn coordination_free(&self) -> bool {
        C::COORDINATION_FREE
    }

    /// The operation labels, in composition order.
    #[must_use]
    pub fn steps(&self) -> &[&'static str] {
        &self.ops
    }
}

/// Run a pipeline with **no coordinator**. Compiles only for a [`CoordinationFree`]
/// (i.e. [`Free`]) pipeline; a pipeline containing any [`Coordinated`] step is
/// rejected at compile time. Returns the steps it would run.
pub fn deploy_coordinator_free<C: CoordinationFree>(pipeline: &Pipeline<C>) -> &[&'static str] {
    pipeline.steps()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_monotone_pipeline_is_free() {
        let p = Pipeline::start()
            .then(Op::monotone("crdt::join"))
            .then(Op::monotone("gset::insert"));
        assert!(p.coordination_free());
        assert_eq!(deploy_coordinator_free(&p), &["crdt::join", "gset::insert"]);
    }

    #[test]
    fn one_coordinated_step_poisons_the_pipeline() {
        let p = Pipeline::start()
            .then(Op::monotone("crdt::join"))
            .then(Op::coordinated("consistency::commit"));
        assert!(!p.coordination_free(), "a coordinated step makes the whole pipeline coordinated");
        assert_eq!(p.steps(), &["crdt::join", "consistency::commit"]);
    }

    #[test]
    fn coordination_is_sticky_across_later_monotone_steps() {
        // Free ⊔ Coordinated ⊔ Free = Coordinated — once coordinated, stays so.
        let p = Pipeline::start()
            .then(Op::monotone("gcounter::increment"))
            .then(Op::coordinated("escrow::acquire_more"))
            .then(Op::monotone("crdt::join"));
        assert!(!p.coordination_free());
        assert_eq!(p.steps().len(), 3);
    }

    #[test]
    fn empty_pipeline_is_vacuously_free() {
        let p = Pipeline::start();
        assert!(p.coordination_free());
        assert_eq!(deploy_coordinator_free(&p), &[] as &[&'static str]);
    }

    #[test]
    fn join_level_absorbs_free_and_is_dominated_by_coordinated() {
        // A direct check of the type-level join's runtime shadow.
        fn out_free<A, B>() -> bool
        where
            A: JoinLevel<B>,
            B: Coordination,
        {
            <A::Out as Coordination>::COORDINATION_FREE
        }
        assert!(out_free::<Free, Free>(), "Free ⊔ Free = Free");
        assert!(!out_free::<Free, Coordinated>(), "Free ⊔ Coordinated = Coordinated");
        assert!(!out_free::<Coordinated, Free>(), "Coordinated ⊔ Free = Coordinated");
    }
}

//! Reconciliation: typing the merge of divergent committed values (rung 3).
//!
//! [`consistency`](crate::consistency) ended with an honest gap: nothing
//! merges two [`Agreed`] values that *disagree*. This module extends the
//! crate's evidence discipline one step further out — the same shape, a new
//! object:
//!
//! ```text
//!   Diverged<T> ──reconcile(&Lawful<M, T>)──▶ Reconciled<T> ──into_local──▶ Local<T>
//!        ▲                     ▲                                               │
//!   runtime boundary     runtime boundary                        the merge re-enters the
//!   (detect: values      (certify: sampled                       lattice at the BOTTOM —
//!    compared)            semilattice laws)                      a merge is a new PROPOSAL
//! ```
//!
//! The asymmetry recurs: producing a [`Reconciled`] demands evidence (a
//! [`Lawful`] witness that the merge function's semilattice laws held on
//! sampled inputs), while *possessing* one proves the check ran — there is no
//! other constructor. And [`Reconciled`] deliberately does **not** implement
//! [`Committed`](crate::consistency::Committed): in a consensus system a
//! local merge is a new proposal, not a decision — only a quorum can lift it
//! back up. (In a pure CRDT the merge *is* the truth; that difference is the
//! seam between the two worlds.)
//!
//! ## Attribution — the composition is the experiment; the parts are not new
//!
//! - The witness mechanism (unforgeable, runtime-minted, phantom-typed) is
//!   *Ghosts of Departed Proofs* (Noonan, Haskell Symposium 2018) / Haskell's
//!   `refined` — "parse, don't validate."
//! - Merge laws carried in types, **soundly and statically**, already exist:
//!   **Propel** (Zakhour, Weisenburger, Salvaneschi, PLDI 2023) proves
//!   commutativity/associativity/idempotence of merge functions at compile
//!   time. This module is the deliberately cheaper runtime-boundary variant.
//! - Typed containment of divergent replica state: Gallifrey's branches
//!   (Milano et al., SNAPL 2019).
//! - Property-testing CRDT merge laws is routine (`cvrdt-exposition` etc.);
//!   there, results are discarded — here the passing check mints a value.
//!
//! ## Soundness disclaimer
//!
//! [`Lawful`] is *sampled evidence*, not proof — and it gates **standing**,
//! not **computation**. Property-checking cannot establish a ∀-law, so the
//! witness cannot exclude a lawless `M` whose violations the samples missed.
//! And no typestate can stop out-of-band computation over readable values:
//! code can always read two `Agreed`s, hand-merge them with anything, and
//! re-commit the result through a real quorum. What the types exclude is
//! narrower and sharper — **no `Reconciled` exists without a certified
//! witness**: the badge of lawful reconciliation is unforgeable even where
//! the computation is free. And the samples are **caller-chosen**, so
//! certification is self-attestation: a constant merge certified over the one
//! sample it returns passes all three laws trivially and launders any value
//! into a `Reconciled`. The witness proves a check ran for this merge —
//! evidence, not proof; it does not even retain the samples — and never that
//! the check was adversarially meaningful. This is
//! [`Config::new`](crate::membership::Config::new)'s root-of-trust hole one
//! rung up: types verify chains; operators choose roots. Propel/VeriFx make
//! the law half sound statically — this toy prices the cheap version and
//! says so.
//!
//! ## The whole loop
//!
//! ```
//! use quorum_types::consistency::Local;
//! use quorum_types::membership::Config;
//! use quorum_types::reconcile::{Detection, Diverged, Lawful, Merge};
//!
//! struct MaxMerge;
//! impl Merge<i64> for MaxMerge {
//!     fn merge(&self, a: &i64, b: &i64) -> i64 { *a.max(b) }
//! }
//!
//! // Two replicas each cleared a real quorum — and disagree.
//! let config = Config::<7>::new([1, 2, 3].into());
//! let quorum = config.certify([1, 2].into()).expect("majority");
//! let ours = Local::new(4_i64).commit(&quorum).forget_epoch();
//! let theirs = Local::new(9_i64).commit(&quorum).forget_epoch();
//!
//! let Detection::Diverged(conflict) = Diverged::detect(ours, theirs) else {
//!     unreachable!("4 != 9");
//! };
//!
//! // The witness: laws checked on samples ONCE, at the boundary.
//! let law = Lawful::certify(MaxMerge, &[0, 1, 2, 5, -3]).expect("max is a semilattice");
//!
//! // Evidence-gated merge — and the result is a new PROPOSAL.
//! let merged = conflict.reconcile(&law);
//! assert_eq!(*merged.value(), 9);
//! let proposal = merged.into_local();
//! let recommitted = proposal.commit(&quorum); // only a quorum lifts it back up
//! assert_eq!(recommitted.epoch(), 7);
//! ```
//!
//! Forging a reconciled value is rejected — the field is private and there is
//! no public constructor:
//!
//! ```compile_fail
//! use quorum_types::reconcile::Reconciled;
//!
//! let forged = Reconciled { value: 7 }; // ERROR: field `value` is private
//! ```
//!
//! The witness is pinned to the value type it was checked on — a `Lawful`
//! minted over `i64` samples cannot reconcile diverged `String`s:
//!
//! ```compile_fail
//! use quorum_types::consistency::Local;
//! use quorum_types::membership::Config;
//! use quorum_types::reconcile::{Detection, Diverged, Lawful, Merge};
//!
//! struct MaxMerge;
//! impl Merge<i64> for MaxMerge {
//!     fn merge(&self, a: &i64, b: &i64) -> i64 { *a.max(b) }
//! }
//!
//! let config = Config::<7>::new([1, 2, 3].into());
//! let quorum = config.certify([1, 2].into()).unwrap();
//! let a = Local::new("x".to_string()).commit(&quorum).forget_epoch();
//! let b = Local::new("y".to_string()).commit(&quorum).forget_epoch();
//! let Detection::Diverged(conflict) = Diverged::detect(a, b) else { unreachable!() };
//!
//! let law = Lawful::certify(MaxMerge, &[0_i64, 1, 2]).unwrap();
//! let merged = conflict.reconcile(&law); // ERROR: expected `String` evidence, found `i64`
//! ```
//!
//! A conflict is consumed by its reconciliation — merging it twice is a move
//! error:
//!
//! ```compile_fail
//! use quorum_types::consistency::Local;
//! use quorum_types::membership::Config;
//! use quorum_types::reconcile::{Detection, Diverged, Lawful, Merge};
//!
//! struct MaxMerge;
//! impl Merge<i64> for MaxMerge {
//!     fn merge(&self, a: &i64, b: &i64) -> i64 { *a.max(b) }
//! }
//!
//! let config = Config::<7>::new([1, 2, 3].into());
//! let quorum = config.certify([1, 2].into()).unwrap();
//! let a = Local::new(4_i64).commit(&quorum).forget_epoch();
//! let b = Local::new(9_i64).commit(&quorum).forget_epoch();
//! let Detection::Diverged(conflict) = Diverged::detect(a, b) else { unreachable!() };
//!
//! let law = Lawful::certify(MaxMerge, &[0, 1, 2]).unwrap();
//! let first = conflict.reconcile(&law);
//! let second = conflict.reconcile(&law); // ERROR: use of moved value
//! ```
//!
//! And reconciliation does not manufacture consensus — a [`Reconciled`] is
//! rejected wherever a committed value is demanded:
//!
//! ```compile_fail
//! use quorum_types::consistency::{Committed, Local};
//! use quorum_types::membership::Config;
//! use quorum_types::reconcile::{Detection, Diverged, Lawful, Merge};
//!
//! struct MaxMerge;
//! impl Merge<i64> for MaxMerge {
//!     fn merge(&self, a: &i64, b: &i64) -> i64 { *a.max(b) }
//! }
//!
//! fn apply<C: Committed>(_decided: C) {}
//!
//! let config = Config::<7>::new([1, 2, 3].into());
//! let quorum = config.certify([1, 2].into()).unwrap();
//! let a = Local::new(4_i64).commit(&quorum).forget_epoch();
//! let b = Local::new(9_i64).commit(&quorum).forget_epoch();
//! let Detection::Diverged(conflict) = Diverged::detect(a, b) else { unreachable!() };
//!
//! let law = Lawful::certify(MaxMerge, &[0, 1, 2]).unwrap();
//! let merged = conflict.reconcile(&law);
//! apply(merged); // ERROR: `Reconciled<i64>: Committed` is not satisfied
//! ```

use core::marker::PhantomData;

use crate::consistency::{Agreed, Local};

/// A candidate merge function. Deliberately an **open** trait — anyone can
/// write one, which is exactly why using one requires a [`Lawful`] witness.
pub trait Merge<T> {
    /// Combine two conflicting values into one.
    fn merge(&self, a: &T, b: &T) -> T;
}

/// The semilattice law a candidate merge violated on the samples, reported by
/// [`Lawful::certify`]. Checked cheapest-first (idempotence → commutativity →
/// associativity); a multi-law violator reports the *first* — the order is
/// contractual, and a test pins it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LawViolation {
    /// No samples were provided — zero checks run is not evidence.
    NoSamples,
    /// `merge(a, a) != a` for some sample `a`.
    Idempotence,
    /// `merge(a, b) != merge(b, a)` for some samples `a, b`.
    Commutativity,
    /// `merge(a, merge(b, c)) != merge(merge(a, b), c)` for some samples.
    Associativity,
}

/// Evidence that `M`'s semilattice laws held on sampled inputs of type `T`.
///
/// Minted only by [`certify`](Lawful::certify) — the runtime boundary that
/// actually runs the checks. The `T` parameter pins the witness to the value
/// type it was checked on. **Sampled evidence, not proof**: see the module
/// docs' soundness disclaimer.
#[must_use = "a law witness exists to gate a reconcile — dropping it unused is suspicious"]
pub struct Lawful<M, T> {
    merge: M,
    _checked_on: PhantomData<T>,
}

impl<M, T> Lawful<M, T>
where
    M: Merge<T>,
    T: PartialEq,
{
    /// The runtime boundary: check idempotence, commutativity, and
    /// associativity of `merge` over every combination of `samples`, and mint
    /// a witness only if no counterexample is found.
    ///
    /// The witness *holds* the merge function — a certified merge is spent
    /// through its certificate, so nothing uncertified can ever *produce* a
    /// [`Reconciled`]. (It can still touch values — the gate is on standing,
    /// not computation; see the module docs.) The empty sample set is
    /// rejected: zero checks run is not evidence. The samples are the
    /// caller's choice — see the module docs' trust model: this is
    /// self-attestation, auditable but gameable by a caller who controls
    /// both the merge and the samples.
    pub fn certify(merge: M, samples: &[T]) -> Result<Self, LawViolation> {
        if samples.is_empty() {
            return Err(LawViolation::NoSamples);
        }
        for a in samples {
            if merge.merge(a, a) != *a {
                return Err(LawViolation::Idempotence);
            }
        }
        for a in samples {
            for b in samples {
                if merge.merge(a, b) != merge.merge(b, a) {
                    return Err(LawViolation::Commutativity);
                }
            }
        }
        for a in samples {
            for b in samples {
                for c in samples {
                    let left = merge.merge(&merge.merge(a, b), c);
                    let right = merge.merge(a, &merge.merge(b, c));
                    if left != right {
                        return Err(LawViolation::Associativity);
                    }
                }
            }
        }
        Ok(Lawful { merge, _checked_on: PhantomData })
    }
}

/// The verdict of [`Diverged::detect`]: either the two committed values
/// already agree, or they genuinely conflict.
#[must_use = "a detection verdict says whether reconciliation is needed"]
pub enum Detection<T> {
    /// The values were equal — no conflict; the (single) agreed value.
    Consistent(Agreed<T>),
    /// The values differ — a conflict that must be reconciled to proceed.
    Diverged(Diverged<T>),
}

/// Two committed values known to disagree. **Bottom of the reconciliation
/// path** — the analogue of [`Local`] one module over: a conflict you cannot
/// *resolve* until evidence lifts it.
///
/// Move-only. The only way out is [`reconcile`](Diverged::reconcile), which
/// consumes the conflict.
#[must_use = "a detected conflict must be reconciled (or explicitly dropped)"]
pub struct Diverged<T> {
    left: T,
    right: T,
}

impl<T> Diverged<T> {
    /// The runtime boundary that mints a conflict: compare two committed
    /// values. Divergence is a fact about *values*, so — like every other
    /// boundary in this crate — the check runs once, here, and the type
    /// carries the verdict afterward.
    pub fn detect(a: Agreed<T>, b: Agreed<T>) -> Detection<T>
    where
        T: PartialEq,
    {
        if a.value() == b.value() {
            Detection::Consistent(a)
        } else {
            Detection::Diverged(Diverged { left: a.into_value(), right: b.into_value() })
        }
    }

    /// One side of the conflict.
    pub fn left(&self) -> &T {
        &self.left
    }

    /// The other side.
    pub fn right(&self) -> &T {
        &self.right
    }

    /// The evidence-gated merge: consumes the conflict, demands a [`Lawful`]
    /// witness for the same value type, and applies the *certified* merge.
    /// No check runs here — like `commit`, this spends evidence minted at the
    /// earlier boundary.
    pub fn reconcile<M>(self, law: &Lawful<M, T>) -> Reconciled<T>
    where
        M: Merge<T>,
    {
        Reconciled { value: law.merge.merge(&self.left, &self.right) }
    }
}

/// A conflict that has been merged under a certified law. Unforgeable:
/// private field, no public constructor — possessing one is proof the
/// lawfulness check ran.
///
/// Deliberately **not** [`Committed`](crate::consistency::Committed): a local
/// merge is a new proposal, not a decision. [`into_local`](Reconciled::into_local)
/// re-enters the consistency lattice at the bottom.
#[must_use = "a reconciled value should re-enter the lattice via into_local"]
pub struct Reconciled<T> {
    value: T,
}

impl<T> Reconciled<T> {
    /// Read the merged value.
    pub fn value(&self) -> &T {
        &self.value
    }

    /// Down to the lattice floor: a merge is a new proposal, and only a
    /// quorum ([`Local::commit`]) can lift it back up.
    pub fn into_local(self) -> Local<T> {
        Local::new(self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::Config;

    struct MaxMerge;
    impl Merge<i64> for MaxMerge {
        fn merge(&self, a: &i64, b: &i64) -> i64 {
            *a.max(b)
        }
    }

    /// Not idempotent: `a + a != a` (for `a != 0`).
    struct AddMerge;
    impl Merge<i64> for AddMerge {
        fn merge(&self, a: &i64, b: &i64) -> i64 {
            a + b
        }
    }

    /// Idempotent and associative but not commutative: `first(a, b) = a`.
    struct FirstMerge;
    impl Merge<i64> for FirstMerge {
        fn merge(&self, a: &i64, _b: &i64) -> i64 {
            *a
        }
    }

    /// Idempotent and commutative but not associative (even in exact arithmetic — `(a@b)@c = (a+b+2c)/4` vs `a@(b@c) = (2a+b+c)/4`):
    /// `(1 @ 1) @ 5 = 3` but `1 @ (1 @ 5) = 2`.
    struct MidpointMerge;
    impl Merge<i64> for MidpointMerge {
        fn merge(&self, a: &i64, b: &i64) -> i64 {
            (a + b) / 2
        }
    }

    /// Violates all three laws — exists to pin the contractual check order,
    /// which single-law violators cannot (they report the same law under any
    /// order).
    struct SubMerge;
    impl Merge<i64> for SubMerge {
        fn merge(&self, a: &i64, b: &i64) -> i64 {
            a - b
        }
    }

    const SAMPLES: [i64; 5] = [0, 1, 2, 5, -3];

    fn agreed(value: i64) -> Agreed<i64> {
        let config = Config::<7>::new([1, 2, 3].into());
        let quorum = config.certify([1, 2].into()).expect("2 of 3 is a majority");
        crate::consistency::Local::new(value).commit(&quorum).forget_epoch()
    }

    #[test]
    fn max_certifies_over_samples() {
        assert!(Lawful::certify(MaxMerge, &SAMPLES).is_ok());
    }

    #[test]
    fn a_multi_law_violator_reports_the_first_checked_law() {
        // Subtraction breaks idempotence, commutativity, AND associativity;
        // the verdict pins the check order. Reorder the checks and this test
        // goes red — the single-law controls above would all stay green.
        assert_eq!(Lawful::certify(SubMerge, &SAMPLES).err(), Some(LawViolation::Idempotence));
    }

    #[test]
    fn empty_samples_mint_nothing() {
        // Even a genuinely lawful merge earns no witness from zero checks.
        assert_eq!(Lawful::certify(MaxMerge, &[]).err(), Some(LawViolation::NoSamples));
    }

    #[test]
    fn each_negative_control_is_rejected_for_its_own_law() {
        assert_eq!(Lawful::certify(AddMerge, &SAMPLES).err(), Some(LawViolation::Idempotence));
        assert_eq!(Lawful::certify(FirstMerge, &SAMPLES).err(), Some(LawViolation::Commutativity));
        assert_eq!(Lawful::certify(MidpointMerge, &SAMPLES).err(), Some(LawViolation::Associativity));
    }

    #[test]
    fn detect_separates_agreement_from_conflict() {
        assert!(matches!(Diverged::detect(agreed(4), agreed(4)), Detection::Consistent(_)));
        match Diverged::detect(agreed(4), agreed(9)) {
            Detection::Diverged(conflict) => {
                assert_eq!((*conflict.left(), *conflict.right()), (4, 9));
            }
            Detection::Consistent(_) => panic!("4 != 9 must diverge"),
        }
    }

    #[test]
    fn reconcile_applies_the_certified_merge() {
        let law = Lawful::certify(MaxMerge, &SAMPLES).unwrap();
        let Detection::Diverged(conflict) = Diverged::detect(agreed(4), agreed(9)) else {
            panic!("must diverge");
        };
        assert_eq!(*conflict.reconcile(&law).value(), 9);
    }

    #[test]
    fn reconciled_reenters_the_lattice_as_a_proposal() {
        let law = Lawful::certify(MaxMerge, &SAMPLES).unwrap();
        let Detection::Diverged(conflict) = Diverged::detect(agreed(4), agreed(9)) else {
            panic!("must diverge");
        };
        let proposal = conflict.reconcile(&law).into_local();
        assert_eq!(*proposal.peek(), 9);
        // Only a quorum lifts it back up.
        let config = Config::<7>::new([1, 2, 3].into());
        let quorum = config.certify([2, 3].into()).unwrap();
        assert_eq!(proposal.commit(&quorum).epoch(), 7);
    }
}

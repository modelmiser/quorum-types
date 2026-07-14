//! # rung 6 — attested values: Byzantine value corroboration as a type
//!
//! The crash lattice ([`consistency`](crate::consistency)) commits a value the
//! moment a quorum *exists*: `Local::commit(self, _witness: &Quorum<E>)` stamps
//! the committer's own value and **discards** the witness (`_`). Under crash
//! faults that is sound — an honest majority's mere existence is enough, and the
//! value the committer holds is the agreed one.
//!
//! Under Byzantine faults it is not. A minority of `≤ f` liars can hand the
//! committer a *fabricated* value. So the witness must become **load-bearing**:
//! a value is believable only if it was *corroborated* by enough distinct
//! members that at least one of them is correct. This module makes that a
//! **type** — [`Attested<T, E>`] has no caller-supplied value and no bypass
//! constructor, so value-blindness is not a runtime check that can be forgotten;
//! it is unrepresentable.
//!
//! ## Existence, not (yet) uniqueness
//!
//! [`attest`] mints at the **dissemination** threshold `f+1`. That buys
//! *existence* — with `≥ f+1` distinct voters, at least one is correct, so the
//! value was not fabricated by the liars alone. It does **not** buy *uniqueness*:
//! two *different* values can each reach `f+1` (one correct voucher apiece, plus
//! the same `f` liars equivocating). That is not a bug — it is the
//! dissemination/masking split from [`byzantine`](crate::byzantine), surfaced
//! here as a value-level fact and demonstrated in the tests. Uniqueness needs
//! the *masking* threshold [`BftConfig::threshold`] and reduces to
//! [`Overlap`](crate::byzantine::Overlap) — the next rung, not this one.
//!
//! ## What stays a declared seam
//!
//! Corroboration is byte-equality of `f+1` votes (`T: Eq`), **not** signatures —
//! consistent with [`byzantine`](crate::byzantine)'s no-crypto axiom. `f` is an
//! operator-declared budget; if it is wrong, the guarantee is wrong and no type
//! here can tell. Distinct voters are counted by [`NodeId`]: a correct node
//! votes once, a Byzantine node double-voting for one value counts once, and a
//! non-member's vote is dropped.
//!
//! ## The value is load-bearing by construction
//!
//! There is no `Attested::new`, and the field is private, so the only value that
//! can inhabit an `Attested<T, E>` is one that `f+1` votes carried. You cannot
//! ignore the votes the way `commit` ignores its witness — ignoring them leaves
//! no value to return:
//!
//! ```compile_fail
//! use quorum_types::attest::Attested;
//! // No public constructor: the value cannot enter except through `attest`.
//! let _ = Attested::<u64, 3> { value: 7, support: Default::default(), f: 1 };
//! ```
//!
//! A vote from a different epoch cannot even be offered — `E` fails to unify:
//!
//! ```compile_fail
//! use quorum_types::attest::{attest, Vote};
//! use quorum_types::byzantine::BftConfig;
//! let cfg = BftConfig::<3>::new([0, 1, 2, 3, 4].into_iter().collect(), 1).unwrap();
//! let votes = vec![Vote::<u64, 4>::new(0, 7)]; // epoch 4 …
//! let _ = attest(votes, &cfg);                 // … against a config at epoch 3
//! ```

use std::collections::BTreeSet;

use crate::byzantine::{BftConfig, ByzQuorum, Overlap};
use crate::membership::NodeId;

/// A single node's claim, at type-level epoch `E`, that a value holds.
///
/// A vote is *evidence*, not authority: it counts only if its `voter` is a
/// member of the configuration it is weighed against, and only once per value.
#[derive(Debug, Clone)]
pub struct Vote<T, const E: u64> {
    voter: NodeId,
    value: T,
}

impl<T, const E: u64> Vote<T, E> {
    /// A vote from `voter` for `value`, at epoch `E`.
    pub const fn new(voter: NodeId, value: T) -> Self {
        Vote { voter, value }
    }

    /// The node that cast this vote.
    pub const fn voter(&self) -> NodeId {
        self.voter
    }

    /// The value voted for.
    pub const fn value(&self) -> &T {
        &self.value
    }
}

/// A value corroborated by `≥ f+1` **distinct** members of configuration `E`.
///
/// **Existence guarantee** (conditional on the declared `f`): at least one of
/// the supporting members is correct, so the value was not fabricated by the
/// `≤ f` liars alone. This is *not* a uniqueness guarantee — see the module
/// docs. The value is private and constructor-less by design: the only way to
/// obtain an `Attested<T, E>` is [`attest`], so no value ever enters it without
/// `f+1` votes behind it.
#[derive(Debug, Clone)]
#[must_use = "an Attested is a corroborated value; use it or the corroboration was pointless"]
pub struct Attested<T, const E: u64> {
    value: T,
    support: BTreeSet<NodeId>,
    f: usize,
}

impl<T, const E: u64> Attested<T, E> {
    /// The corroborated value.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// The distinct members whose matching votes corroborated the value —
    /// `≥ f+1` of them.
    pub const fn support(&self) -> &BTreeSet<NodeId> {
        &self.support
    }

    /// The minimum number of **correct** supporters — `≥ 1` at the existence
    /// threshold, conditional on the declared budget. (`support.len() - f`,
    /// floored at zero; at `f+1` support this is `≥ 1`.)
    pub fn min_correct(&self) -> usize {
        self.support.len().saturating_sub(self.f)
    }

    /// The epoch that corroborated this value (mirrors `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// Take ownership of the corroborated value.
    pub fn into_value(self) -> T {
        self.value
    }
}

/// Corroborate a value from a bag of votes: mint an [`Attested<T, E>`] iff some
/// value was voted for by `≥ f+1` **distinct members** of `cfg`.
///
/// Note the shape of the signature — there is **no `value: T` parameter**. The
/// corroborated value is *extracted from the votes*, never supplied by the
/// caller. That is what makes the witness load-bearing where `commit`'s is
/// discardable: ignore the votes and there is nothing to return.
///
/// Non-member votes are dropped; a member voting twice for the same value is
/// counted once. Among values that clear `f+1`, the one with the most distinct
/// support is returned (deterministic; ties resolve to the last such value).
pub fn attest<T: Eq, const E: u64>(
    votes: Vec<Vote<T, E>>,
    cfg: &BftConfig<E>,
) -> Option<Attested<T, E>> {
    let f = cfg.fault_budget();

    // Group votes by value, counting distinct member-voters. `T: Eq` only, so
    // this is O(n·groups) — fine for a feasibility toy.
    let mut groups: Vec<(T, BTreeSet<NodeId>)> = Vec::new();
    for vote in votes {
        if !cfg.is_member(vote.voter) {
            continue; // a non-member's vote is not evidence
        }
        match groups.iter_mut().find(|(v, _)| *v == vote.value) {
            Some((_, support)) => {
                support.insert(vote.voter);
            }
            None => {
                let mut support = BTreeSet::new();
                support.insert(vote.voter);
                groups.push((vote.value, support));
            }
        }
    }

    // Existence threshold: f+1 distinct members. Pick the best-supported value
    // that clears it (any one is a valid existence certificate).
    groups
        .into_iter()
        // `> f` is the f+1 existence threshold, written wrap-free: `f` is an
        // operator-declared budget, so `f + 1` must not be trusted to not overflow.
        .filter(|(_, support)| support.len() > f)
        .max_by_key(|(_, support)| support.len())
        .map(|(value, support)| Attested { value, support, f })
}

/// A value corroborated by a **masking quorum** of configuration `E` — unique
/// per epoch (tier 6b).
///
/// Where [`Attested`] clears the `f+1` *existence* threshold (≥1 correct
/// voucher), a `Committed` clears the *masking* threshold
/// [`BftConfig::threshold`] `⌈(n+2f+1)/2⌉` and carries a [`ByzQuorum<E>`] as its
/// support. That is what buys **uniqueness**: any two masking quorums of the
/// same epoch intersect in `≥ 2f+1` members, of which `≥ f+1` are correct
/// ([`Overlap::min_correct`]) — and a correct member votes once. So two
/// `Committed<T, E>` cannot carry *different* values without a set of liars
/// larger than the declared `f`. The reduction is [`agreement_witness`], and it
/// is the value-level analogue of the split-brain-unrepresentable property from
/// rung 1: no external prover, just the intersection an `Overlap` already
/// certifies.
#[derive(Debug, Clone)]
#[must_use = "a Committed is a uniquely-corroborated value; use it or the masking quorum was pointless"]
pub struct Committed<T, const E: u64> {
    value: T,
    quorum: ByzQuorum<E>,
}

impl<T, const E: u64> Committed<T, E> {
    /// The uniquely-corroborated value.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// The certified masking quorum that corroborated the value.
    pub const fn quorum(&self) -> &ByzQuorum<E> {
        &self.quorum
    }

    /// The epoch that committed this value (mirrors `E`).
    pub const fn epoch(&self) -> u64 {
        E
    }

    /// **The uniqueness reduction.** Intersect this commit's masking quorum with
    /// another same-epoch commit's. The returned [`Overlap`] certifies
    /// `≥ f+1` *correct* members shared by both quorums — and a correct member
    /// does not vote for two different values. Its mere existence is therefore a
    /// witness that the two committed values agree (conditional on the declared
    /// `f`); if they did not, no honest population could have produced both.
    /// `None` is unreachable for two genuine masking quorums (they always
    /// intersect above `2f`), but the boundary returns it rather than panic —
    /// the same relocate-don't-delete discipline `ByzQuorum::intersect` uses.
    pub fn agreement_witness(&self, other: &Committed<T, E>) -> Option<Overlap<E>> {
        self.quorum.intersect(other.quorum())
    }

    /// Take ownership of the committed value.
    pub fn into_value(self) -> T {
        self.value
    }
}

/// Corroborate a value at the **masking** threshold: mint a [`Committed<T, E>`]
/// iff some value was voted for by a masking quorum's worth
/// (`⌈(n+2f+1)/2⌉`) of **distinct members** of `cfg`.
///
/// Same shape as [`attest`] — no `value: T` parameter — so the value is
/// extracted from the votes, never supplied. The winning support set is handed
/// to [`BftConfig::certify`], so the resulting quorum is a genuine masking
/// certificate, not a bare set. Uniqueness follows: a *second* value cannot also
/// reach this threshold without `> f` members voting twice.
pub fn commit_masking<T: Eq, const E: u64>(
    votes: Vec<Vote<T, E>>,
    cfg: &BftConfig<E>,
) -> Option<Committed<T, E>> {
    let threshold = cfg.threshold();

    let mut groups: Vec<(T, BTreeSet<NodeId>)> = Vec::new();
    for vote in votes {
        if !cfg.is_member(vote.voter) {
            continue;
        }
        match groups.iter_mut().find(|(v, _)| *v == vote.value) {
            Some((_, support)) => {
                support.insert(vote.voter);
            }
            None => {
                let mut support = BTreeSet::new();
                support.insert(vote.voter);
                groups.push((vote.value, support));
            }
        }
    }

    let (value, support) = groups
        .into_iter()
        .filter(|(_, support)| support.len() >= threshold)
        .max_by_key(|(_, support)| support.len())?;
    // `certify` re-checks subset-of-members and threshold: the masking quorum is
    // minted by the library's own boundary, never fabricated here.
    let quorum = cfg.certify(support)?;
    Some(Committed { value, quorum })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_5_1() -> BftConfig<3> {
        // n = 5, f = 1 satisfies n ≥ 4f+1 = 5. Existence threshold f+1 = 2.
        BftConfig::<3>::new([0, 1, 2, 3, 4].into_iter().collect(), 1).unwrap()
    }

    #[test]
    fn f_plus_one_distinct_members_corroborate_the_value() {
        let cfg = cfg_5_1();
        let votes = vec![Vote::new(0, "commit-x"), Vote::new(1, "commit-x")];
        let a = attest(votes, &cfg).expect("2 = f+1 distinct members agree");
        assert_eq!(*a.value(), "commit-x");
        assert_eq!(a.support(), &[0, 1].into_iter().collect());
        assert_eq!(a.epoch(), 3);
        assert!(a.min_correct() >= 1, "existence: at least one correct voucher");
    }

    #[test]
    fn f_votes_are_below_the_existence_threshold() {
        // sign-flip A: only f = 1 vote — one liar could have produced it alone.
        let cfg = cfg_5_1();
        let votes = vec![Vote::new(0, "commit-x")];
        assert!(attest(votes, &cfg).is_none());
    }

    #[test]
    fn a_double_voting_member_counts_once() {
        // sign-flip B: f+1 = 2 votes for one value, but from ONE node twice.
        // Distinct-member dedup leaves 1 < f+1 → no corroboration.
        let cfg = cfg_5_1();
        let votes = vec![Vote::new(4, "forge"), Vote::new(4, "forge")];
        assert!(attest(votes, &cfg).is_none());
    }

    #[test]
    fn non_member_votes_are_not_evidence() {
        let cfg = cfg_5_1();
        // Node 9 is not in the config; only member 0 remains → 1 < f+1.
        let votes = vec![Vote::new(0, "x"), Vote::new(9, "x")];
        assert!(attest(votes, &cfg).is_none());
    }

    #[test]
    fn existence_is_not_uniqueness_two_values_both_attest_at_f_plus_one() {
        // THE BOUNDARY FINDING (pre-registered falsifier 4, fired as a
        // refinement). An equivocating Byzantine node (4) sends "A" to one
        // collector's view and "B" to another's. Each view also has one correct
        // voucher (0 for A, 1 for B). Each reaches f+1 = 2 → each attests. Two
        // DIFFERENT values are corroborated at the same epoch. f+1 buys
        // existence, never uniqueness — which is exactly why the masking tier
        // (rung 6b) must exist.
        let cfg = cfg_5_1();

        let view_a = vec![Vote::new(0, "A"), Vote::new(4, "A")]; // correct 0 + liar 4
        let view_b = vec![Vote::new(1, "B"), Vote::new(4, "B")]; // correct 1 + liar 4

        let a = attest(view_a, &cfg).expect("A reaches f+1");
        let b = attest(view_b, &cfg).expect("B reaches f+1");

        assert_ne!(a.value(), b.value(), "two conflicting values, both attested");
        assert_eq!(a.epoch(), b.epoch(), "at the same epoch");
        // The liar sits in both support sets — that IS the equivocation.
        assert!(a.support().contains(&4) && b.support().contains(&4));
    }

    // --- tier 6b: masking threshold and uniqueness -------------------------

    #[test]
    fn masking_quorum_commits_a_value() {
        // threshold at n=5,f=1 is ⌈(5+2+1)/2⌉ = 4.
        let cfg = cfg_5_1();
        let votes = (0..4).map(|n| Vote::new(n, "A")).collect();
        let c = commit_masking(votes, &cfg).expect("4 distinct members = masking threshold");
        assert_eq!(*c.value(), "A");
        assert_eq!(c.quorum().members().len(), 4);
        assert_eq!(c.epoch(), 3);
    }

    #[test]
    fn a_value_below_the_masking_threshold_does_not_commit() {
        // Existence-strong (3 ≥ f+1) but masking-weak (3 < 4): no Committed.
        let cfg = cfg_5_1();
        let votes = (0..3).map(|n| Vote::new(n, "A")).collect();
        assert!(commit_masking(votes, &cfg).is_none());
    }

    #[test]
    fn two_masking_commits_agree_via_an_overlap_witness() {
        // Two views both see "A" reach the masking threshold, over different
        // quorums {0,1,2,3} and {1,2,3,4}. The reduction certifies a shared
        // correct voucher — the uniqueness witness.
        let cfg = cfg_5_1();
        let a = commit_masking((0..4).map(|n| Vote::new(n, "A")).collect(), &cfg).unwrap();
        let b = commit_masking((1..5).map(|n| Vote::new(n, "A")).collect(), &cfg).unwrap();
        let overlap = a.agreement_witness(&b).expect("masking quorums always intersect above 2f");
        assert_eq!(overlap.members(), &[1, 2, 3].into_iter().collect());
        assert!(overlap.min_correct() >= 1, "≥1 correct member vouched for both");
        assert_eq!(a.value(), b.value(), "the shared correct member forces agreement");
    }

    #[test]
    fn one_equivocator_cannot_mint_a_conflicting_masking_commit() {
        // The uniqueness guarantee, negatively: honest {0,1,2,3} commit "A".
        // The single liar (4) trying to forge "B" is 1 vote — nowhere near the
        // masking threshold of 4. With each honest node voting once, no second
        // value can reach the threshold; a conflicting commit needs `> f`
        // double-voters, which the declared budget forbids.
        let cfg = cfg_5_1();
        let mut votes: Vec<Vote<&str, 3>> = (0..4).map(|n| Vote::new(n, "A")).collect();
        votes.push(Vote::new(4, "B")); // the equivocator's forged value
        let c = commit_masking(votes, &cfg).unwrap();
        assert_eq!(*c.value(), "A", "the liar cannot displace the honest quorum");

        // And "B" alone genuinely does not commit.
        let b_only = vec![Vote::new(4, "B")];
        assert!(commit_masking(b_only, &cfg).is_none());
    }
}

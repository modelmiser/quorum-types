//! Term-scoped leader authority — the **structural** rung of the leadership-acquisition axis.
//!
//! Its runtime-witness dual is `election`. Most of the crate *presupposes* a leader —
//! [`failover`](crate::failover) receives a lease "at the boundary", [`staleness`](crate::staleness)'s
//! `LeaderLease::acquire` takes a *trusted bool*, [`fencing`](crate::fencing) issues monotone tokens
//! but "deliberately does not model" who won. This axis types the two halves nobody else does: the
//! **discipline** of holding leadership across terms (here, structural) and the **act** of winning it
//! (`election`, a witness). A *term* is a monotonically increasing election epoch (Raft's `currentTerm`).
//!
//! ## What the type owns
//!
//! A [`Reign<T>`] is a linear, unforgeable token meaning "the authority of the leader of term `T`".
//! Its *primary* mechanism is the crate's **founding** one — type-level epoch unification, exactly as
//! [`Quorum<E>`](crate::membership::Quorum) makes split-brain a type error (not
//! [`lockorder`](crate::lockorder)'s arithmetic rank gate); only the supersession check reuses the
//! monotone `const` gate [`lockorder`](crate::lockorder) also uses. Three structural guarantees follow:
//!
//! * **Authority gating.** A leader-only action ([`decree`](Reign::decree)) is reachable only through
//!   a `Reign<T>`; there is no other way to stamp a command with a term's authority.
//! * **Term-scoped commitment.** A leader commits its own decrees only *in its own term*: a
//!   [`Decree<X, T>`] is committed by presenting the matching-term `Reign<T>`, so a decree left over
//!   from an *earlier* term cannot be committed under the current one — the terms fail to unify
//!   (E0308). (This is Raft's "a leader only commits entries from its current term", as a type.)
//! * **Monotone supersession.** A reign is stepped down only *by a strictly greater term*:
//!   [`superseded_by`](Reign::superseded_by) carries `const { U > T }` (E0080), so authority never
//!   moves backward.
//!
//! ## Where the types stop (the runtime seam)
//!
//! * **The install root is trusted.** [`install`](Reign::install) mints a `Reign<T>` by assertion —
//!   like [`Quorum::genesis`](crate::Quorum::genesis) or [`escrow::grant`](crate::escrow::Reservation::grant),
//!   it is the boundary where authority *enters*. In a real system that boundary is an **election**:
//!   the witness rung's `Elected<T>` is exactly what should authorize an `install`, supplying the
//!   provenance `failover`/`staleness` take on faith. `term` types what a leader may *do* once it holds
//!   authority; `election` types how it legitimately *got* it.
//! * **`T` is a type-level class, not a value instance.** Two `Reign<T>` values of the same term `T`
//!   are indistinguishable to the type (the same limit `election`'s `Elected<T>` and the crate's other
//!   epoch brands carry). One reign per term is the property the *witness* rung earns, not this one.
//!
//! ## A reign cannot be forged — unforgeability
//!
//! ```compile_fail
//! use quorum_types::term::Reign;
//! let _ = Reign::<1> { _priv: () }; // private field — only `install` (or an election) mints a Reign
//! ```
//!
//! ## A stale-term decree cannot be committed under a later reign — epoch unification
//!
//! ```compile_fail
//! use quorum_types::term::Reign;
//! let old = Reign::<1>::install();
//! let decree = old.decree("x");            // Decree<&str, 1>
//! let now = Reign::<2>::install();          // the current term is 2
//! let _ = decree.commit(&now);              // Decree<_,1> vs Reign<2> — terms do not unify (E0308)
//! ```
//!
//! ## A reign is superseded only by a greater term — monotonicity
//!
//! ```compile_fail
//! use quorum_types::term::Reign;
//! let r = Reign::<3>::install();
//! let _ = r.superseded_by::<2>(); // const { 2 > 3 } fails after monomorphization (E0080)
//! ```
//!
//! ## A stepped-down reign has no authority — linearity
//!
//! ```compile_fail
//! use quorum_types::term::Reign;
//! let r = Reign::<3>::install();
//! let _deposed = r.superseded_by::<4>(); // consumes the reign
//! let _ = r.decree("late"); // `r` already moved — a superseded leader cannot still act (E0382)
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::term::Reign;
//!
//! // Win term 5 (here, asserted via `install`; really, via an election certificate).
//! let leader = Reign::<5>::install();
//!
//! // Issue two term-stamped decrees, then commit one in-term.
//! let d1 = leader.decree("set x = 1");
//! let d2 = leader.decree("set y = 2");
//! assert_eq!(d1.commit(&leader), "set x = 1"); // committed under the matching term
//!
//! // A higher term supersedes this reign; you no longer hold a `Reign<5>` in this scope, so the
//! // leftover decree d2 cannot be committed here — its `commit` needs `&Reign<5>`, which is gone.
//! // (Reviving it would mean re-asserting term-5 authority through the trusted `install` root,
//! // which a real election forbids — one leader per term. The type stops stale commits *under a
//! // later term*, E0308; it cannot stop a trusted root from re-minting the same term.)
//! let _deposed = leader.superseded_by::<6>();
//! let _ = d2; // still typed Decree<_, 5>
//! ```

/// The authority of the leader of term `T`. Linear (move-only, no `Copy`/`Clone`): leadership is a
/// resource, surrendered exactly once by [`superseded_by`](Self::superseded_by). Unforgeable outside
/// this module (private field); minted by [`install`](Self::install) — the trusted seam an election
/// discharges.
#[derive(Debug)]
#[must_use = "a Reign is leadership authority; hold it to act, or step down explicitly"]
pub struct Reign<const T: u64> {
    _priv: (),
}

/// A command stamped with the authority of term `T`'s leader. Move-only; committed exactly once, and
/// only by presenting the matching-term [`Reign<T>`](Reign) — a decree from a superseded term cannot
/// be committed under a later one.
#[derive(Debug)]
#[must_use = "a Decree is an un-committed leader command; commit it in its own term"]
pub struct Decree<X, const T: u64> {
    command: X,
}

/// A former leader that has stepped down. Terminal marker — carries no authority.
#[derive(Debug)]
pub struct Deposed {
    _priv: (),
}

impl<const T: u64> Reign<T> {
    /// Install the authority of term `T`. **Trusted root** — like [`Quorum::genesis`](crate::Quorum::genesis),
    /// this is where authority is *asserted*, not derived. In a real system an election certificate
    /// (`election::Elected<T>`) is what authorizes this call; see the module seam docs.
    pub const fn install() -> Self {
        Reign { _priv: () }
    }

    /// The term this reign holds.
    pub const fn term(&self) -> u64 {
        T
    }

    /// **Leader-only action.** Stamp a command with this term's authority, yielding a
    /// [`Decree<X, T>`](Decree). `&self`, so one leader may issue many decrees while it reigns.
    pub fn decree<X>(&self, command: X) -> Decree<X, T> {
        Decree { command }
    }

    /// **Monotone supersession.** Step down in favor of a strictly greater term `U`, consuming this
    /// reign. Compiles only if `U > T` — a reign never yields to an equal or earlier term (E0080).
    pub fn superseded_by<const U: u64>(self) -> Deposed {
        const { assert!(U > T, "a reign is superseded only by a strictly greater term") }
        let Reign { .. } = self; // consume this reign's authority
        Deposed { _priv: () }
    }
}

impl<X, const T: u64> Decree<X, T> {
    /// **Commit in-term.** Consume this decree, returning its command — but only by presenting the
    /// *current* [`Reign<T>`](Reign) of the **same** term. A decree left over from an earlier term
    /// needs that term's reign, which a later leader no longer holds, so stale-term commands cannot be
    /// committed under the current term (the two `T`s fail to unify — E0308).
    pub fn commit(self, _current: &Reign<T>) -> X {
        self.command
    }

    /// The term whose authority stamped this decree.
    pub const fn term(&self) -> u64 {
        T
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leader_issues_and_commits_in_term() {
        let leader = Reign::<5>::install();
        assert_eq!(leader.term(), 5);
        let d = leader.decree(42);
        assert_eq!(d.term(), 5);
        assert_eq!(d.commit(&leader), 42, "a decree commits under its own term's reign");
    }

    #[test]
    fn supersession_steps_down() {
        let leader = Reign::<5>::install();
        let d = leader.decree("pending");
        let _deposed = leader.superseded_by::<6>();
        // `d` is still a Decree<_, 5>; with Reign<5> given up it can no longer be committed.
        // (That un-committability is a compile-time fact — see the module's compile_fail docs.)
        assert_eq!(d.term(), 5);
    }

    #[test]
    fn many_decrees_one_reign() {
        let leader = Reign::<7>::install();
        let decrees: Vec<_> = (0..3).map(|i| leader.decree(i)).collect();
        for (i, d) in decrees.into_iter().enumerate() {
            assert_eq!(d.commit(&leader), i as i32, "each decree commits in-term");
        }
    }
}

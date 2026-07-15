//! Deadlock-free lock ordering — acquisition made **rank-monotone**, so a
//! circular wait is unrepresentable (Havender 1968; Dijkstra's resource ordering).
//!
//! [`failover`](crate::failover) and [`fencing`](crate::fencing) type a *single*
//! lock. The classic multi-lock hazard is different: two threads that each hold one
//! lock and wait for the other's deadlock. The classic *fix* is equally simple —
//! assign every lockable resource a unique **rank**, and require every thread to
//! acquire locks in **strictly increasing rank order**. Then no cycle of "holds one,
//! waits for a higher one" can close on itself, because that would demand a rank
//! strictly greater than itself all the way around the loop. No detector, no
//! rollback, no timeout: the discipline *prevents* deadlock structurally.
//!
//! This module lifts a thread's "highest rank currently held" into the type. A
//! [`Held`]`<HI>` token proves that, on this thread, locks were acquired in strictly
//! increasing rank order up to `HI`. Acquiring a lock whose rank does not exceed
//! `HI` fails a `const` assertion — a **compile error**, not a runtime guard. The
//! out-of-order acquisition that could deadlock is unrepresentable.
//!
//! ## The mechanism — a monotone watermark in a const generic
//!
//! * [`Held::base`] is the empty hold, [`Held`]`<0>` — nothing acquired yet.
//! * [`acquire`](Held::acquire)`::<R>` lifts the rank `R` into the type. Its body
//!   carries `const { assert!(R > HI) }`, so acquiring a rank at or below the current
//!   watermark does not compile. On success you get [`Held`]`<R>` (the new, higher
//!   watermark) **and** a [`Guard`]`<HI, R>` — a linear receipt remembering the rank
//!   just taken (`R`) and the watermark to restore beneath it (`HI`).
//! * [`release`](Held::release) consumes the current [`Held`]`<HI>` together with the
//!   guard for rank `HI` and returns [`Held`]`<UNDER>` — the watermark from *before*
//!   that lock. Because the guard's held-rank must match the current watermark, you
//!   can only release the lock on **top** of the stack: release is LIFO, the mirror
//!   of the rank-increasing acquire.
//!
//! ## Acquire in increasing rank order, release in reverse
//!
//! ```
//! use quorum_types::lockorder::Held;
//! let h0 = Held::base();                      // Held<0>
//! let (h1, g1) = h0.acquire::<1>();           // Held<1>, guard for rank 1 over 0
//! let (h3, g3) = h1.acquire::<3>();           // Held<3>: 3 > 1, ok
//! let (h7, g7) = h3.acquire::<7>();           // Held<7>: 7 > 3, ok
//! assert_eq!(h7.watermark(), 7);
//!
//! let h3 = h7.release(g7);                     // back to Held<3>
//! let h1 = h3.release(g3);                     // back to Held<1>
//! let _h0 = h1.release(g1);                    // back to Held<0>
//! ```
//!
//! ## Acquiring out of rank order is a compile error
//!
//! Acquiring a rank at or below the highest held violates `const { assert!(R > HI) }`
//! and fails to compile — the hold-and-wait cycle that could deadlock cannot be
//! written:
//!
//! ```compile_fail
//! use quorum_types::lockorder::Held;
//! let (h3, _g3) = Held::base().acquire::<3>();
//! let (_h2, _g2) = h3.acquire::<2>(); // 2 > 3 is false: out-of-order acquire, deadlock risk
//! ```
//!
//! ## Equal ranks are a compile error too (`>`, not `>=`)
//!
//! Two locks of the *same* rank are a genuine deadlock hazard (nothing orders them),
//! so the gate is strict — equal is rejected exactly as below-is:
//!
//! ```compile_fail
//! use quorum_types::lockorder::Held;
//! let (h3, _g3) = Held::base().acquire::<3>();
//! let (_h, _g) = h3.acquire::<3>(); // 3 > 3 is false: same-rank acquire is rejected
//! ```
//!
//! ## Releasing out of order is a compile error
//!
//! A [`Guard`]`<UNDER, RANK>` releases only the lock whose rank equals the current
//! watermark. Handing [`release`](Held::release) the *inner* guard (a lower rank than
//! the top) does not type-check, so locks come off strictly top-first:
//!
//! ```compile_fail
//! use quorum_types::lockorder::Held;
//! let (h1, g1) = Held::base().acquire::<1>();
//! let (h2, _g2) = h1.acquire::<2>();
//! // h2 is Held<2>; release wants a Guard<_, 2>, but g1 is Guard<0, 1>.
//! let _ = h2.release(g1); // rank mismatch: cannot release rank 1 while rank 2 is on top
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! This rung's seam is a different *species* from the crate's *witness*-based rungs —
//! a third member of the already-established declared-axiom family. Elsewhere the
//! seam is a trusted runtime **witness** (a lease, a monotone compare, a sampled
//! law). Here the type enforces conformance to an **order**, but assigning that
//! order is an operator axiom the types propagate yet cannot check:
//!
//! * **The global rank assignment is trusted.** Deadlock-freedom follows only if
//!   *every* thread agrees on one consistent total order over *all* resources.
//!   [`Held`]`<HI>` certifies that *this* thread acquired in increasing rank order —
//!   it says nothing about whether the ranks are globally consistent or whether
//!   other threads obey them. That the ranks form a sound global order is a declared
//!   axiom, the same shape as [`byzantine`](crate::byzantine)'s fault budget `f` and
//!   [`calm`](crate::calm)'s monotonicity labels — propagated, not proved. (An
//!   out-of-tree z3 model, in the research harness and not shipped here, is the
//!   discriminant: a rank-ordered circular wait is unsatisfiable for every cycle
//!   length checked, while dropping the ordering constraint alone makes a concrete
//!   deadlock cycle satisfiable.)
//! * **Prevention, not liveness.** This forbids circular wait — one of Coffman's
//!   four conditions — so no deadlock can form. It says nothing about *starvation*
//!   or *fairness*: a thread can still wait indefinitely for a lock another holds.
//!   Safety only; liveness is out of scope, as everywhere in this crate.
//! * **Sufficient, not exact — on the *release* side.** Because release is forced
//!   LIFO, the watermark `HI` always equals the *exact* maximum rank currently held,
//!   so the acquire gate `R > HI` is precisely Havender's rule, not stricter. The
//!   conservatism is entirely in release: LIFO forbids releasing an *inner* lock
//!   before the ones above it (hold {3, 5, 7}, drop the 5 while keeping 3 and 7) — a
//!   move a precise held-*set* tracker would allow and which is itself deadlock-safe.
//!   The watermark is the sound, conservative floor — the same sufficient-not-exact
//!   trade as [`flex`](crate::flex)'s `R + W > N` and [`staleness`](crate::staleness).
//! * **One stack, not one thread.** The type tracks a *single* acquisition chain. It
//!   does **not** stop a thread from minting several independent stacks (repeated
//!   [`base`](Held::base)) and acquiring across them out of global rank order —
//!   `base().acquire::<5>()` and `base().acquire::<3>()` on one thread both compile,
//!   a decreasing acquisition the single chain's `const` gate would reject. The
//!   sharper reason this discipline is load-bearing: a [`Guard`] carries no chain
//!   identity, so a `Guard<_, R>` minted from one root will [`release`](Held::release)
//!   a same-rank `Held<R>` from *another* — collapsing its watermark past ranks still
//!   held. A lone strictly-increasing chain holds each rank at most once, so same-rank
//!   guards never coexist to be swapped; multiple roots break exactly that. "Each
//!   thread threads exactly one `Held` chain" is therefore an operator obligation the
//!   types cannot express (Rust has no once-per-thread linear resource): the
//!   rank-order guarantee is *intra-chain*, and whole-thread acyclicity — let alone
//!   cross-thread — assumes the single-chain discipline on top of the global-order
//!   axiom above.
//! * **Affine, not linear.** [`Guard`] is `#[must_use]`, which steers you to release
//!   but does not force it (no panicking `Drop`). Dropping a guard is *safe* — you
//!   lose the exact guard [`release`](Held::release) demands, so the watermark can
//!   only ratchet *up*, never come off out of order — but the lock is then never
//!   given back (a leak, not an unsafe schedule). Same affinity caveat as
//!   [`saga`](crate::saga)'s pending compensations.

/// A proof that, on this thread, locks have been acquired in strictly increasing
/// rank order, with `HI` the highest rank currently held (the watermark). `Held<0>`
/// is the empty hold.
///
/// A zero-sized type: the whole guarantee lives in the const generic `HI` and in the
/// linearity of [`Guard`]. The private field blocks construction outside this
/// module, so the only `Held` values are the empty [`base`](Held::base) and those
/// threaded through [`acquire`](Held::acquire) / [`release`](Held::release).
///
/// `#[must_use]`: a non-empty `Held<HI>` is the only thing that can consume the
/// outstanding [`Guard`]s (release is a method on it); dropping it strands them, so
/// those locks can never be given back.
#[must_use = "a non-empty Held owns the release path for its Guards; dropping it strands the locks"]
pub struct Held<const HI: u32> {
    _priv: (),
}

/// A linear receipt for one acquired lock: `RANK` is the rank it holds, `UNDER` is
/// the watermark to restore when it is released. Minted only by
/// [`acquire`](Held::acquire), consumed only by [`release`](Held::release).
///
/// `#[must_use]`: a dropped guard is a lock never released — the watermark it guards
/// can never be lowered, so the thread's rank order is stuck above it.
#[must_use = "a Guard is a held lock; release it (LIFO) or the lock is never given back"]
pub struct Guard<const UNDER: u32, const RANK: u32> {
    _priv: (),
}

impl Held<0> {
    /// The empty hold: no locks acquired, watermark 0.
    pub const fn base() -> Self {
        Held { _priv: () }
    }
}

impl<const HI: u32> Held<HI> {
    /// The highest rank currently held on this thread.
    pub const fn watermark(&self) -> u32 {
        HI
    }

    /// Acquire a lock of rank `R`. Compiles **only** when `R > HI` (strictly above
    /// the current watermark) — the `const` block below is evaluated at
    /// monomorphization, so an out-of-order acquire is a compile error, not a
    /// runtime panic.
    ///
    /// Returns the raised watermark [`Held`]`<R>` and a [`Guard`]`<HI, R>` receipt
    /// that [`release`](Held::release) later consumes to restore `HI`.
    pub fn acquire<const R: u32>(self) -> (Held<R>, Guard<HI, R>) {
        const {
            assert!(
                R > HI,
                "lock rank must strictly exceed the highest rank currently held \
                 (acquire in increasing rank order to keep the wait-for graph acyclic)"
            );
        }
        (Held { _priv: () }, Guard { _priv: () })
    }

    /// Release the lock on **top** of the stack — the one whose rank equals the
    /// current watermark `HI` — and restore the watermark `UNDER` from beneath it.
    ///
    /// The guard's held-rank is fixed to `HI` by its type, so only the most recently
    /// acquired lock can be released here: release is LIFO, the exact reverse of the
    /// increasing-rank acquire.
    pub fn release<const UNDER: u32>(self, _guard: Guard<UNDER, HI>) -> Held<UNDER> {
        Held { _priv: () }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_increasing_then_release_reverse() {
        let h0 = Held::base();
        assert_eq!(h0.watermark(), 0);
        let (h2, g2) = h0.acquire::<2>();
        assert_eq!(h2.watermark(), 2);
        let (h5, g5) = h2.acquire::<5>();
        assert_eq!(h5.watermark(), 5);
        let (h9, g9) = h5.acquire::<9>();
        assert_eq!(h9.watermark(), 9);

        // Release strictly top-first: 9, then 5, then 2.
        let h5 = h9.release(g9);
        assert_eq!(h5.watermark(), 5);
        let h2 = h5.release(g5);
        assert_eq!(h2.watermark(), 2);
        let h0 = h2.release(g2);
        assert_eq!(h0.watermark(), 0);
    }

    #[test]
    fn adjacent_ranks_are_allowed() {
        // Strictly increasing by one is fine; only <= is forbidden.
        let (h1, g1) = Held::base().acquire::<1>();
        let (h2, g2) = h1.acquire::<2>();
        let h1 = h2.release(g2);
        let _h0 = h1.release(g1);
    }

    #[test]
    fn a_single_lock_round_trips() {
        let (h3, g3) = Held::base().acquire::<3>();
        assert_eq!(h3.watermark(), 3);
        let h0 = h3.release(g3);
        assert_eq!(h0.watermark(), 0);
    }
}

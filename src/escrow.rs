//! Escrow reservations — **moving an operation onto the coordination-free side**
//! by pre-partitioning a global budget (the escrow / *demarcation* technique — see
//! Bailis et al. on coordination avoidance).
//!
//! [`crdt`](crate::crdt) types operations that are *already* coordination-free
//! (monotone joins). This module types the harder case: an operation that is
//! **not** invariant-confluent, made local by construction.
//!
//! The canonical example is a bounded counter — "stock on hand must stay `≥ 0`",
//! or equivalently "total spend must stay `≤ BUDGET`". Two replicas that each
//! decrement independently can drive the total past the bound: the invariant is
//! not preserved by uncoordinated merges, so naively this operation *requires*
//! coordination on every step. The classic **escrow/demarcation** technique
//! (O'Neil's escrow method predates CRDTs; Bailis et al. recast it under invariant
//! confluence) buys it back: split the global budget into per-replica
//! **reservations** *once*, and then each replica spends only from its own
//! reservation — locally, with no coordination — because it physically cannot
//! spend capacity it does not hold.
//!
//! ## The load-bearing invariant: capacity is conserved, never created
//!
//! A [`Reservation<BUDGET>`] is a **linear** (move-only) token carrying a runtime
//! `remaining`. Capacity enters a reservation *tree* only at its root, and every
//! other operation conserves the total:
//!
//! * [`grant`](Reservation::grant) is the **trusted root** — it mints a fresh
//!   `BUDGET`. Like the crate's [`Quorum::genesis`](crate::Quorum::genesis), it is
//!   callable more than once; each call roots an *independent* tree. Conservation
//!   is a property *within* a tree, not across separate grants — `grant` is the
//!   boundary where the budget is asserted, not derived.
//! * [`split`](Reservation::split) carves a child out of a parent — the two halves'
//!   `remaining` sum to the original. It hands capacity *sideways*, and can never
//!   hand out more than it holds.
//! * [`spend`](Reservation::spend) moves capacity from a reservation into a
//!   [`Receipt`] — it can never exceed `remaining`, so it fails locally rather than
//!   overshoot.
//! * [`merge`](Reservation::merge) returns two reservations' capacity into one.
//!
//! Therefore, within one grant's tree, at every instant `Σ remaining + Σ spent ==
//! BUDGET` (where `Σ spent` is the total *decremented from reservations*, not a sum
//! over receipt records) — the bounded quantity cannot cross its bound **by
//! construction**: each of split/spend/merge conserves the sum, so no interleaving
//! of them sums past `BUDGET`. This is informal structural reasoning, not a
//! machine-checked proof. The floor holds on the split/spend/merge path; `grant`
//! is the one place a human asserts the starting budget.
//!
//! ## Where the seam is — and why this is the through-line inverted
//!
//! Spending *within* your reservation is free (no witness, no quorum). Acquiring
//! *more* than your reservation is the coordination — it means obtaining a
//! [`split`](Reservation::split) from a replica that still holds capacity. That is
//! the crate's recurring cut, seen from the constructive side: where committing a
//! value ([`consistency::Local::commit`](crate::consistency::Local::commit)) pays a
//! coordination cost — a [`Quorum`](crate::membership::Quorum) — *each* time it
//! moves up the lattice, escrow pays that cost **once** (at partition time) and then
//! buys unboundedly many coordination-free local spends.
//! *Coordinate rarely to re-partition; act freely within your share.*
//!
//! ## Two budgets do not mix
//!
//! `BUDGET` is a const type parameter, so reservations carved from *different*
//! budgets cannot be [`merge`](Reservation::merge)d — mixing them would break
//! conservation, and it is a **compile error**, not a runtime guard (the same
//! epoch-unification trick the rest of the crate uses):
//!
//! ```compile_fail
//! use quorum_types::escrow::Reservation;
//! let a = Reservation::<100>::grant();
//! let b = Reservation::<50>::grant();
//! let _ = a.merge(b); // BUDGET: 100 vs 50 do not unify — budgets cannot mix
//! ```
//!
//! ## A reservation is spent once — linearity
//!
//! Move semantics make a reservation a resource: `spend` consumes it and returns
//! the reduced remainder, so a stale copy cannot be spent again:
//!
//! ```compile_fail
//! use quorum_types::escrow::Reservation;
//! let r = Reservation::<10>::grant();
//! let _ = r.spend(3);
//! let _ = r.spend(3); // `r` already moved — a reservation is consumed once
//! ```
//!
//! ## The happy path
//!
//! ```
//! use quorum_types::escrow::Reservation;
//!
//! // A global budget of 10 units, granted once.
//! let global = Reservation::<10>::grant();
//!
//! // Hand 4 units to a replica; the origin keeps 6. Total conserved.
//! let (origin, replica) = global.split(4).unwrap();
//! assert_eq!(origin.remaining(), 6);
//! assert_eq!(replica.remaining(), 4);
//!
//! // The replica spends 3 locally — no coordination.
//! let (receipt, replica) = replica.spend(3).unwrap();
//! assert_eq!(receipt.amount(), 3);
//! assert_eq!(replica.remaining(), 1);
//!
//! // It cannot overshoot its reservation: spending 2 of a remaining 1 fails,
//! // and hands the reservation back intact (it must coordinate for more).
//! let replica = replica.spend(2).unwrap_err();
//! assert_eq!(replica.remaining(), 1, "an over-spend leaves the reservation untouched");
//! ```

/// A **linear escrow reservation** carved from a global budget of `BUDGET` units.
///
/// Move-only (no `Copy`/`Clone`): a reservation is a resource, not a value. Its
/// `remaining` is how much this holder may still [`spend`](Self::spend) without
/// coordinating. Capacity enters only via [`grant`](Self::grant) (once *per tree*)
/// and moves only by [`split`](Self::split)/[`merge`](Self::merge)/[`spend`](Self::spend),
/// all of which conserve the total — so within that tree the bound holds by construction.
#[derive(Debug)]
#[must_use = "a Reservation is capacity; dropping it silently forfeits the units it holds"]
pub struct Reservation<const BUDGET: u64> {
    remaining: u64,
}

/// A record that `amount` units were spent from some reservation. Holding a
/// `Receipt` witnesses that a spend of `amount` occurred — it cannot be conjured
/// (private field, no public constructor, mintable only inside [`spend`](Reservation::spend)).
///
/// Like a [`Reservation`] it is **move-only** (no `Copy`/`Clone`), so it cannot be
/// duplicated to double-count: summed over the distinct receipts a run retains,
/// `Σ amount` never exceeds the capacity actually drawn down. (It witnesses the
/// drawdowns; the *conserved* quantity is `Σ remaining` across live reservations.)
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct Receipt {
    amount: u64,
}

impl Receipt {
    /// The number of units this receipt accounts for.
    pub const fn amount(&self) -> u64 {
        self.amount
    }
}

impl<const BUDGET: u64> Reservation<BUDGET> {
    /// Mint a fresh budget as a single reservation — the **trusted root** of a
    /// reservation tree. Within that tree, capacity only ever moves by the
    /// conserving [`split`](Self::split)/[`merge`](Self::merge)/[`spend`](Self::spend),
    /// so no descendant can hold more than `BUDGET`. `grant` itself is the boundary
    /// where the budget is *asserted*: like [`Quorum::genesis`](crate::Quorum::genesis)
    /// it can be called repeatedly, each call rooting an independent tree, so
    /// conservation is a within-tree property — not a global cap on how much you mint.
    pub const fn grant() -> Self {
        Reservation { remaining: BUDGET }
    }

    /// The capacity this reservation may still spend without coordinating.
    pub const fn remaining(&self) -> u64 {
        self.remaining
    }

    /// **Escrow hand-out.** Carve `give` units into a new (child) reservation,
    /// keeping the rest in the parent. The two returned reservations' `remaining`
    /// sum to `self.remaining` — capacity moves sideways, never grows.
    ///
    /// Fails (returning `self` unchanged) if `give` exceeds what this reservation
    /// holds: you cannot hand out capacity you do not have.
    pub fn split(self, give: u64) -> Result<(Self, Self), Self> {
        if give <= self.remaining {
            let kept = self.remaining - give;
            Ok((Reservation { remaining: kept }, Reservation { remaining: give }))
        } else {
            Err(self)
        }
    }

    /// **The coordination-free local operation.** Draw `amount` units from this
    /// reservation, returning a [`Receipt`] and the reduced reservation. Requires
    /// no quorum, lease, or witness — spending your own reservation is free.
    ///
    /// Overspending your *local reservation* is refused at runtime: if `amount`
    /// exceeds `remaining`, the reservation is handed back intact via `Err` and no
    /// receipt is issued. What is *structurally* impossible is crossing the
    /// *global* bound — you cannot spend capacity you were never handed, so no
    /// interleaving of local spends sums past `BUDGET`. Getting more capacity means
    /// coordinating for another [`split`](Self::split) — the seam this design pushes
    /// to the rare path.
    pub fn spend(self, amount: u64) -> Result<(Receipt, Self), Self> {
        if amount <= self.remaining {
            let left = self.remaining - amount;
            Ok((Receipt { amount }, Reservation { remaining: left }))
        } else {
            Err(self)
        }
    }

    /// Recombine two reservations **of the same budget** into one — e.g. returning
    /// a replica's unused capacity to the origin. Conserves the total. Reservations
    /// from *different* budgets have different `BUDGET` type parameters and do not
    /// unify here, so mixing budgets is a compile error.
    pub fn merge(self, other: Self) -> Self {
        Reservation { remaining: self.remaining + other.remaining }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_conserves_and_spend_draws_down() {
        let global = Reservation::<10>::grant();
        assert_eq!(global.remaining(), 10);

        let (origin, replica) = global.split(4).expect("4 <= 10");
        assert_eq!(origin.remaining() + replica.remaining(), 10, "split conserves the total");
        assert_eq!(replica.remaining(), 4);

        let (receipt, replica) = replica.spend(3).expect("3 <= 4");
        assert_eq!(receipt.amount(), 3);
        assert_eq!(replica.remaining(), 1, "spend draws down remaining");
        // conservation across the whole system: kept + child-remaining + spent
        assert_eq!(origin.remaining() + replica.remaining() + receipt.amount(), 10);
    }

    #[test]
    fn overspend_is_refused_and_returns_the_reservation_intact() {
        let r = Reservation::<5>::grant();
        let r = r.spend(9).expect_err("9 > 5 cannot be spent");
        assert_eq!(r.remaining(), 5, "a refused spend leaves capacity untouched");
    }

    #[test]
    fn cannot_hand_out_more_than_held() {
        let r = Reservation::<5>::grant();
        let r = r.split(9).expect_err("cannot give away 9 of 5");
        assert_eq!(r.remaining(), 5);
    }

    #[test]
    fn merge_returns_capacity_to_one_reservation() {
        let (a, b) = Reservation::<10>::grant().split(6).unwrap();
        let whole = a.merge(b);
        assert_eq!(whole.remaining(), 10, "merge restores the full budget");
    }

    /// **The floor is structural** (see the module docs — split/spend/merge each
    /// conserve the sum). This test does not enumerate interleavings; it
    /// *spot-checks* the invariant `Σ remaining + Σ spent == BUDGET` along one
    /// fixed adversarial schedule (including refused over-spends), asserting the
    /// bound holds after every step. The guarantee comes from the conservation, not
    /// from this one sequence.
    #[test]
    fn conservation_holds_along_one_adversarial_schedule() {
        const BUDGET: u64 = 20;
        // Partition the budget across three replicas.
        let (rest, r_a) = Reservation::<BUDGET>::grant().split(7).unwrap();
        let (r_c, r_b) = rest.split(5).unwrap(); // r_c keeps 8, r_b gets 5

        // A deterministic but "adversarial" interleaving of local spends and an
        // over-spend attempt on each replica.
        let mut spent_total = 0u64;
        let mut live = Vec::new();

        for (res, asks) in [(r_a, [3u64, 5]), (r_b, [4, 4]), (r_c, [6, 9])] {
            let mut res = res;
            for ask in asks {
                match res.spend(ask) {
                    Ok((receipt, next)) => {
                        spent_total += receipt.amount();
                        res = next;
                    }
                    Err(unchanged) => res = unchanged, // over-spend refused locally
                }
                // Invariant holds after every single step: nothing crossed the bound.
                assert!(spent_total <= BUDGET, "global bound crossed: {spent_total} > {BUDGET}");
            }
            live.push(res);
        }

        // Conservation, exactly: everything still held plus everything spent = BUDGET.
        let held: u64 = live.iter().map(Reservation::remaining).sum();
        assert_eq!(held + spent_total, BUDGET, "capacity was created or destroyed");
    }
}

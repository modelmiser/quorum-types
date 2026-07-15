//! Fencing tokens — the resource-side monotone guard that makes a lost lock *safe*.
//!
//! [`failover`](crate::failover) types the *holder* of a lease and refuses to
//! surrender a still-valid one. But its TLA+ result was blunt: the type-level epoch
//! is **necessary, not sufficient**, because an old leader keeps serving until its
//! lease lapses — a *temporal* hole no type can plug. A lease cannot stop a client
//! that pauses (GC stall, network partition) while still believing it holds the lock.
//!
//! Fencing closes exactly that residual, and it does so from the **other side of the
//! wire** — at the *resource*, not the holder (Kleppmann, "How to do distributed
//! locking"). The lock service issues [`FencingToken`]s carrying strictly increasing
//! numbers; the protected [`FencedStore`] remembers the highest number it has ever
//! accepted and **rejects any write bearing an older one**. Two clients can *both*
//! believe they hold the lock — mutual exclusion has already failed — and writes *to
//! this store* are still safe, because the stale holder's write is fenced off. The
//! store itself needs no lease, clock, or quorum — it pushes those into the token
//! authority (see the seam below): just monotonicity, at the resource.
//!
//! The trust does not vanish, it *relocates*: from [`failover`](crate::failover)'s "the lease clock is
//! correct" to "every write goes through the fenced store, and the token source is
//! monotone across failures." ([`failover`](crate::failover) and this module are two
//! *independent* vantages on Kleppmann's argument — the holder side and the resource
//! side — not a composed code pipeline; no `Leased` token feeds a `FencingToken` here.)
//!
//! ## The mechanism
//!
//! * [`LockService::acquire`] mints a [`FencingToken`] whose number is strictly
//!   greater than every token issued before it. The token's field is private — a
//!   client cannot forge a higher number to jump the queue.
//! * [`FencedStore`]'s **only** mutator is [`write`](FencedStore::write), which
//!   *requires* a [`FencingToken`]: there is no token-free write path, so an unfenced
//!   write is unrepresentable. It accepts iff the token is **not older** than the
//!   newest write already accepted (`number >= high_water`), advancing the high-water
//!   and returning an [`Accepted`] receipt; an older token is refused with
//!   [`Fenced::Stale`] and the store is untouched.
//! * [`Accepted`] is minted **only** by a successful [`write`](FencedStore::write) —
//!   a caller cannot fabricate "my write landed."
//!
//! ## The paused holder is fenced off — a runtime reject
//!
//! ```
//! use quorum_types::fencing::{LockService, FencedStore, Fenced};
//! let mut svc = LockService::new();
//! let mut store = FencedStore::new("initial");
//!
//! let token1 = svc.acquire();                 // client 1 acquires (number 1)
//! // client 1 pauses (GC / network stall); client 2 acquires a newer lock and writes:
//! let token2 = svc.acquire();                 // number 2
//! let receipt = store.write(token2, "from client 2").unwrap();
//! assert_eq!(receipt.number(), 2);
//!
//! // client 1 wakes still believing it holds the lock and writes with its stale token
//! // — the store fences it off; the write never lands:
//! let err = store.write(token1, "from client 1").unwrap_err();
//! assert_eq!(err, Fenced::Stale { presented: 1, high_water: 2 });
//! assert_eq!(*store.get(), "from client 2");
//! ```
//!
//! ## A client cannot forge a token to jump the queue — a compile error
//!
//! The number lives in a private field with no public constructor, so the only
//! source of a `FencingToken` is [`LockService::acquire`]:
//!
//! ```compile_fail
//! use quorum_types::fencing::{FencedStore, FencingToken};
//! let mut store = FencedStore::new(0u32);
//! let forged = FencingToken { number: 999 }; // private field — no such constructor
//! let _ = store.write(forged, 1);
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! Fencing is the rung whose guarantee is *mostly* a runtime monotone compare — and
//! that is the honest point of it. What the types own is narrow but real: a write
//! *cannot bypass* the token, and [`Accepted`] *cannot be forged*. What they do not
//! own:
//!
//! * **Monotonicity is a runtime comparison.** `number >= high_water` — you cannot
//!   type "the largest number the store has seen." Same shape as [`failover`](crate::failover)'s
//!   lease-validity check and [`staleness`](crate::staleness)'s lag check.
//! * **Total mediation is trusted.** Fencing is only as strong as its coverage: any
//!   write path that reaches the resource *without* going through the [`FencedStore`]
//!   writes with no token and defeats the guard. The type gates *this* store's
//!   writes; it cannot force every access in the system through it — the same
//!   "reflect the full membership" trust as [`twophase`](crate::twophase)'s
//!   vote-completeness.
//! * **The token source must be monotone across failures.** [`LockService`]'s counter
//!   is in-memory; a service that restarts and resets it reissues low numbers,
//!   breaking the guarantee. Real deployments source tokens from a persistently
//!   monotone authority (ZooKeeper `zxid`, an etcd revision). The type propagates
//!   numbers; it does not prove global monotonicity — an operator-declared root of
//!   trust, like [`byzantine`](crate::byzantine)'s fault budget.
//!
//! An out-of-tree TLC model (in the research harness, not shipped in this crate) checks
//! the discriminant: the monotone reject is *sufficient* to prevent stale-holder
//! corruption, and load-bearing — remove the guard and a stale token overwrites a newer
//! write.

/// A monotonically increasing fencing-token number. The same logical-time domain as
/// [`failover`](crate::failover)'s ticks, but used for *ordering issuance*, not expiry.
pub type TokenNumber = u64;

/// The lock service — the sole source of [`FencingToken`]s, issuing strictly
/// increasing numbers.
///
/// Mutual exclusion *between acquirers* (only one client "holds" the lock at a time)
/// is the service's own job and is deliberately **not** modelled here: the whole point
/// of fencing is that safety survives even when that mutual exclusion fails (a paused
/// holder, a partition). This models only the monotone issuance the resource relies on.
#[derive(Debug, Clone)]
pub struct LockService {
    next: TokenNumber,
}

impl LockService {
    /// A fresh lock service. The first token it issues has number `1` (a store
    /// starts at `high_water == 0`, so the first real write is always accepted).
    pub const fn new() -> Self {
        LockService { next: 1 }
    }

    /// Acquire the lock: issue a [`FencingToken`] whose number is strictly greater
    /// than every token issued so far.
    pub fn acquire(&mut self) -> FencingToken {
        let number = self.next;
        self.next += 1;
        FencingToken { number }
    }

    /// The number the *next* [`acquire`](LockService::acquire) will issue.
    pub const fn peek_next(&self) -> TokenNumber {
        self.next
    }
}

impl Default for LockService {
    fn default() -> Self {
        Self::new()
    }
}

/// A fencing token: evidence of a lock acquisition, carrying that acquisition's
/// monotone [`TokenNumber`].
///
/// `Copy` on purpose — a holder reuses the same token for *every* write it makes while
/// it holds the lock. Minted **only** by [`LockService::acquire`]: the `number` field
/// is private, so a client cannot fabricate a higher number to jump ahead of a newer
/// holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FencingToken {
    number: TokenNumber,
}

impl FencingToken {
    /// This token's monotone number.
    pub const fn number(&self) -> TokenNumber {
        self.number
    }
}

/// Why a fenced write was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fenced {
    /// The presented token is older than a write the store has already accepted — a
    /// newer holder has superseded this one, so the write is dropped.
    Stale {
        /// The (too-old) number on the presented token.
        presented: TokenNumber,
        /// The highest number the store had already accepted.
        high_water: TokenNumber,
    },
}

/// Evidence that a write was accepted: at the moment it landed, the store had seen
/// nothing newer, and its high-water is now `>= number`.
///
/// Minted **only** by a successful [`FencedStore::write`] — a caller cannot forge "my
/// write landed." It certifies exactly that a write at `number` was applied when the
/// store had accepted nothing newer; it does **not** certify that this holder is still
/// current, nor that no *later* write has since superseded it (a newer token can land
/// the instant after).
#[must_use = "an Accepted receipt is your evidence the write was not fenced; check it"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    number: TokenNumber,
}

impl Accepted {
    /// The token number at which the accepted write landed — the store's high-water *at
    /// that instant* (it may have advanced since; this receipt is a `Copy` snapshot).
    pub const fn number(&self) -> TokenNumber {
        self.number
    }
}

/// The protected resource. Its **only** mutator is [`write`](FencedStore::write),
/// which requires a [`FencingToken`] — so an unfenced write is unrepresentable. The
/// store remembers the highest token number it has accepted and refuses anything
/// older. This monotone guard needs no lease and no clock.
#[derive(Debug, Clone)]
pub struct FencedStore<T> {
    value: T,
    high_water: TokenNumber,
}

impl<T> FencedStore<T> {
    /// A store initialized with `value`, having accepted no token yet
    /// (`high_water == 0`).
    pub const fn new(value: T) -> Self {
        FencedStore { value, high_water: 0 }
    }

    /// The highest token number this store has accepted.
    pub const fn high_water(&self) -> TokenNumber {
        self.high_water
    }

    /// The current value. Reads are unfenced — fencing guards *writes* (a stale read
    /// is a staleness concern, [`staleness`](crate::staleness), not a fencing one).
    pub const fn get(&self) -> &T {
        &self.value
    }

    /// **Fenced write.** Apply `value` iff `token` is not older than the newest write
    /// the store has accepted (`token.number() >= high_water`). On success the
    /// high-water advances to the token's number and an [`Accepted`] receipt is
    /// returned; on a stale token the write is refused with [`Fenced::Stale`] and the
    /// store is left **untouched** — the paused holder's write never lands, even
    /// though it still believes it holds the lock.
    ///
    /// Re-writing with the *same* token number succeeds (`>=`, not `>`): a holder may
    /// write repeatedly during a single hold.
    pub fn write(&mut self, token: FencingToken, value: T) -> Result<Accepted, Fenced> {
        if token.number < self.high_water {
            return Err(Fenced::Stale {
                presented: token.number,
                high_water: self.high_water,
            });
        }
        self.high_water = token.number;
        self.value = value;
        Ok(Accepted { number: token.number })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_issues_strictly_increasing_numbers() {
        let mut svc = LockService::new();
        let a = svc.acquire();
        let b = svc.acquire();
        let c = svc.acquire();
        assert_eq!((a.number(), b.number(), c.number()), (1, 2, 3));
    }

    #[test]
    fn a_newer_token_advances_the_high_water() {
        let mut svc = LockService::new();
        let mut store = FencedStore::new(0u32);
        let t1 = svc.acquire();
        let t2 = svc.acquire();
        assert_eq!(store.write(t1, 10).unwrap().number(), 1);
        assert_eq!(store.high_water(), 1);
        assert_eq!(store.write(t2, 20).unwrap().number(), 2);
        assert_eq!(store.high_water(), 2);
        assert_eq!(*store.get(), 20);
    }

    #[test]
    fn a_stale_token_is_fenced_and_the_store_is_untouched() {
        // Kleppmann's canonical trace: holder 1 pauses, holder 2 writes, holder 1
        // wakes and writes with its stale token.
        let mut svc = LockService::new();
        let mut store = FencedStore::new("initial");
        let t1 = svc.acquire();
        let t2 = svc.acquire();
        let _ = store.write(t2, "newer").unwrap();
        let err = store.write(t1, "stale overwrite").unwrap_err();
        assert_eq!(err, Fenced::Stale { presented: 1, high_water: 2 });
        assert_eq!(*store.get(), "newer", "the stale write must not land");
    }

    #[test]
    fn rewriting_with_the_same_token_succeeds() {
        // A holder writes repeatedly during one hold: `>=`, not `>`.
        let mut svc = LockService::new();
        let mut store = FencedStore::new(0u32);
        let t = svc.acquire();
        assert!(store.write(t, 1).is_ok());
        assert!(store.write(t, 2).is_ok(), "same-number rewrite is allowed");
        assert_eq!(*store.get(), 2);
    }

    #[test]
    fn the_first_write_is_always_accepted() {
        // A store starts at high_water 0 and the first issued number is 1, so the
        // first real write can never be spuriously fenced.
        let mut svc = LockService::new();
        let mut store = FencedStore::new(());
        let t = svc.acquire();
        assert!(store.write(t, ()).is_ok());
    }
}

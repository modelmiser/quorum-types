//! At-most-once effect — a **single-use capability** that a duplicate cannot re-fire.
//!
//! Loop 12 typed message *order*; this rung and `at_least_once` type the
//! orthogonal dimension of reliable broadcast: **delivery count** — how many times a message's
//! effect fires. Exactly-once *delivery* is impossible; exactly-once *processing* is achievable as
//! **at-least-once delivery ∘ an at-most-once effect**. This module types the at-most-once half.
//!
//! There are two roads to at-most-once. [`crdt`](crate::crdt) takes one — make the effect
//! **idempotent**, so applying it twice equals applying it once (`crdt` notes: idempotence makes
//! at-least-once delivery safe). This module takes the other, for effects that are *not*
//! idempotent: gate the effect on a **single-use linear capability**. A retransmitted duplicate
//! carries no fresh capability — and one cannot be forged — so the effect is inert on every copy
//! but the one that consumed the token.
//!
//! ## The mechanism — the effect is a linear resource
//!
//! * [`Effect<T>`] is a move-only, unforgeable capability (private field) authorizing **one**
//!   application of a logical operation carrying payload `T`. [`authorize`] mints exactly one per
//!   logical operation (the sender boundary).
//! * [`Effect::apply`] **consumes** the capability and invokes `f` once. A second `apply` on
//!   the same token does not compile — it was moved (E0382). A wire-duplicate carries no `Effect`
//!   (it cannot be constructed outside this module, E0451), so it can trigger nothing. So `f` is
//!   invoked **at most once per token**, whatever the delivery count.
//!
//! Two things are then the caller's contract, not the type's: that `f` *is* the whole effect (a
//! no-op `f` firing the real side effect elsewhere is outside what the token counts), and that the
//! sender does not **re-arm** — [`into_payload`](Effect::into_payload) hands the payload back, and
//! `authorize`-ing it again mints a *fresh* token for the same value. At-most-once therefore rests
//! on linearity **and** the "authorize once per operation" discipline; linearity alone bounds only
//! re-firing from *one* token (see the seam).
//!
//! ## A duplicate cannot fire the effect twice
//!
//! The capability is consumed by the first application; a second use is a use-after-move:
//!
//! ```compile_fail
//! use quorum_types::at_most_once::authorize;
//! let e = authorize(5_i32);
//! let _ = e.apply(|v| v + 1);
//! let _ = e.apply(|v| v + 1); // `e` was moved — the effect cannot fire twice
//! ```
//!
//! ## A duplicate cannot forge its own capability
//!
//! ```compile_fail
//! use quorum_types::at_most_once::Effect;
//! let forged: Effect<i32> = Effect { payload: 5 }; // private field — cannot construct
//! ```
//!
//! ## The happy path — one token, one application, duplicates inert
//!
//! ```
//! use quorum_types::at_most_once::{authorize, Effect};
//! // The transport may deliver a message many times; only one copy carries the capability.
//! let mut fired = 0;
//! let arrivals: Vec<Option<Effect<i32>>> = vec![None, Some(authorize(5)), None];
//! for arrival in arrivals {
//!     if let Some(effect) = arrival {
//!         effect.apply(|v| fired += v); // runs exactly once across the whole stream
//!     }
//! }
//! assert_eq!(fired, 5);
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! What is compile-time enforceable is that **one capability yields at most one application** —
//! and it is enforceable because at-most-once-by-linearity is **local**: no agreement, no runtime
//! state, just move semantics (the same reason the crate's other move-only tokens are structural).
//! What the types **cannot** own is *which* wire messages are duplicates of one another: the model
//! mints one [`Effect`] per logical operation, but recognizing that three arriving byte-strings are
//! copies of the same operation — and routing the single capability to the receiver — is the
//! transport's job, exactly as `fifo` trusts the transport for its sequence numbers. The type
//! guarantees the effect fires ≤ 1 time *given* a single-token-per-operation transport; it does not
//! deduplicate the wire.

/// A move-only, unforgeable capability authorizing **one** application of an effect carrying
/// payload `T`. Minted only by [`authorize`]; a duplicate message cannot construct one (private
/// field), and [`apply`](Effect::apply) consumes it, so the effect fires at most once.
#[must_use = "an Effect is a single-use capability; apply it (or take its payload) or the effect is lost"]
pub struct Effect<T> {
    payload: T,
}

impl<T> Effect<T> {
    /// **Invoke `f` once** on the payload, consuming the capability. There is no way to apply twice:
    /// the token is moved by this call. (The type bounds `f`'s invocation count to one *per token*;
    /// that `f` is the whole effect is the caller's contract — see the module docs.)
    pub fn apply<R>(self, f: impl FnOnce(T) -> R) -> R {
        f(self.payload)
    }

    /// Take the payload out without running an effect (still consumes the single-use capability).
    pub fn into_payload(self) -> T {
        self.payload
    }
}

/// Mint the single capability for one logical operation carrying `payload`. Called once per
/// operation at the sender boundary; the transport is trusted to route this one token to the
/// receiver and not to mint others for the same operation (see the module's runtime-seam note).
pub fn authorize<T>(payload: T) -> Effect<T> {
    Effect { payload }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_once() {
        let e = authorize(41_i32);
        assert_eq!(e.apply(|v| v + 1), 42);
    }

    #[test]
    fn duplicates_are_inert_only_the_token_bearer_fires() {
        // A stream of arrivals for the same logical op; only one copy carries the capability.
        let mut fired = 0;
        let arrivals: Vec<Option<Effect<i32>>> = vec![None, None, Some(authorize(7)), None];
        // None arrivals model duplicate copies carrying no capability; only the token-bearer fires.
        for effect in arrivals.into_iter().flatten() {
            effect.apply(|v| fired += v);
        }
        assert_eq!(fired, 7, "the effect fired exactly once across the duplicate stream");
    }

    #[test]
    fn into_payload_consumes_without_firing() {
        let e = authorize("payload");
        assert_eq!(e.into_payload(), "payload");
    }
}

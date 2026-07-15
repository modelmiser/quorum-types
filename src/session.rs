//! Session guarantees (Terry et al., Bayou) as typestate — atop the causal rung.
//!
//! The four *session guarantees* constrain a single client's view across replicas:
//!
//! * **Read-Your-Writes (RYW):** a read reflects this session's earlier writes.
//! * **Monotonic Reads (MR):** later reads reflect a superset of earlier reads.
//! * **Monotonic Writes (MW):** a write is ordered after this session's earlier writes.
//! * **Writes-Follow-Reads (WFR):** a write is ordered after the writes an earlier
//!   read observed.
//!
//! A [`Session<W>`] threads a type-level **watermark** `W` — the writes the session
//! has done or observed — through every operation. Move-only, so the operation
//! order (and thus the guarantees) is carried by the type.
//!
//! ## The cut: writes are structural, reads need runtime evidence
//!
//! The guarantees split cleanly, and the split *is* the invariant-confluence line:
//!
//! * **[`write`](Session::write) needs no witness.** It tags itself with the
//!   current watermark and advances it (`W → (New, W)`). MW and WFR are thereby
//!   enforced **structurally, at compile time** — a client always knows its own
//!   write order, so ordering a write after everything the session has done or
//!   read is *coordination-free* (I-confluent). No replica need be consulted.
//! * **[`read`](Session::read) demands a [`Fresh<W>`].** RYW and MR require that
//!   the replica actually *reflects* the session's watermark — and whether it does
//!   is **runtime data** (a version check, [`freshness_of`], the gradual boundary).
//!   The type forces you to hold the obligation; satisfying it observes remote
//!   state. That is the non-I-confluent half.
//!
//! So all four guarantees are *type-enforced as an obligation*, but the two
//! write-ordering ones bottom out in structure while the two read ones bottom out
//! at a runtime freshness check. Ordering your own writes is free; confirming a
//! read is fresh enough is coordination.
//!
//! ## Reading your own write without proving the replica has it is a compile error
//!
//! ```compile_fail
//! use quorum_types::session::{Session, Start, freshness_of};
//! enum W1 {}
//! let s = Session::<Start>::begin();
//! let stale = freshness_of::<Start>(true).unwrap(); // fresh for the EMPTY watermark
//! let s = s.write::<W1>();                           // watermark advances to (W1, Start)
//! // RYW violation: `stale` proves freshness for `Start`, not for `(W1, Start)` —
//! // this replica may not reflect the write. Does not typecheck.
//! let _ = s.read(stale, "x");
//! ```
//!
//! ## The happy path — prove freshness for the current watermark, then read
//!
//! ```
//! use quorum_types::session::{Session, Start, freshness_of};
//! enum W1 {}
//! let s = Session::<Start>::begin();
//! let s = s.write::<W1>();                                 // Session<(W1, Start)>
//! let fresh = freshness_of::<(W1, Start)>(true).unwrap();  // a replica reflects the write
//! let (obs, _s) = s.read(fresh, 42);                       // RYW satisfied
//! assert_eq!(*obs.value(), 42);
//! ```

use core::marker::PhantomData;

/// Phantom over the type-level watermark. `fn() -> T` keeps the wrappers covariant
/// and unconditionally `Send`/`Sync`/`Copy` — markers carry identity, not data.
type Phantom<T> = PhantomData<fn() -> T>;

/// The empty watermark — a fresh session has written and observed nothing.
pub enum Start {}

/// A per-client session whose watermark `W` (writes done or observed) is a
/// type-level marker. Move-only: each operation consumes it and returns its
/// successor, threading the operation order — and the session guarantees — through
/// the type.
#[must_use = "a Session is linear; thread it through your reads and writes"]
pub struct Session<W> {
    _w: Phantom<W>,
}

/// A witness that a replica reflects at least watermark `W`. `Copy` — a fact about
/// a replica's version, freely reusable. Its fields are private, so it is
/// obtainable only through [`freshness_of`] (the runtime gradual boundary) — a read
/// cannot conjure one inline. What it certifies is that a version check *was run and
/// returned `true`*, not an unforgeable fact: `freshness_of` trusts its `bool`, so an
/// honest replication layer supplies real freshness while a caller that blindly
/// passes `true` mints an unsound witness — the same declared-vs-true boundary as
/// [`causal`](crate::causal).
pub struct Fresh<W> {
    _w: Phantom<W>,
}

impl<W> Clone for Fresh<W> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<W> Copy for Fresh<W> {}

/// A value read in a session, tagged with the watermark `W` it was observed at.
#[must_use = "an Observed value carries a session-consistency guarantee; use it"]
pub struct Observed<T, W> {
    value: T,
    _w: Phantom<W>,
}

impl<T, W> Observed<T, W> {
    /// The observed value — reflects everything at or after watermark `W`.
    pub const fn value(&self) -> &T {
        &self.value
    }
}

impl Session<Start> {
    /// Begin a client session with an empty watermark.
    pub const fn begin() -> Self {
        Session { _w: PhantomData }
    }
}

impl<W> Session<W> {
    /// **Write** — enforces **MW** and **WFR** structurally. Advances the watermark
    /// to `(New, W)`: the write is ordered after every prior session write (MW) and
    /// after every write an earlier read observed (WFR), because those all live in
    /// `W`. Needs **no** freshness witness — ordering your own writes is
    /// coordination-free.
    pub fn write<New>(self) -> Session<(New, W)> {
        Session { _w: PhantomData }
    }

    /// **Read** — enforces **RYW** and **MR**. Requires a [`Fresh<W>`]: proof that
    /// the target replica reflects the session's current watermark, so the read
    /// observes everything at or after `W`. A stale witness (minted before a write
    /// advanced `W`) will not unify — the RYW/MR obligation is a compile-time type,
    /// but the freshness evidence it consumes is runtime data.
    pub fn read<T>(self, _fresh: Fresh<W>, value: T) -> (Observed<T, W>, Session<W>) {
        (Observed { value, _w: PhantomData }, Session { _w: PhantomData })
    }
}

/// The gradual boundary: mint a [`Fresh<W>`] iff the target replica's version
/// actually reflects watermark `W`. Whether it does is runtime data; here the
/// caller supplies the outcome of that version check.
pub fn freshness_of<W>(replica_reflects_watermark: bool) -> Option<Fresh<W>> {
    replica_reflects_watermark.then_some(Fresh { _w: PhantomData })
}

#[cfg(test)]
mod tests {
    use super::*;

    enum W1 {}
    enum W2 {}

    #[test]
    fn read_your_writes_after_proving_freshness() {
        let s = Session::<Start>::begin().write::<W1>();
        let fresh = freshness_of::<(W1, Start)>(true).unwrap();
        let (obs, _s) = s.read(fresh, 7);
        assert_eq!(*obs.value(), 7);
    }

    #[test]
    fn monotonic_writes_thread_the_order_structurally() {
        // Two writes need no witnesses; the marker chain (W2,(W1,Start)) carries MW.
        let s = Session::<Start>::begin().write::<W1>().write::<W2>();
        // A read now requires freshness for the FULL chain — MR/RYW over both writes.
        let fresh = freshness_of::<(W2, (W1, Start))>(true).unwrap();
        let (obs, _s) = s.read(fresh, "v");
        assert_eq!(*obs.value(), "v");
    }

    #[test]
    fn freshness_check_can_fail_at_the_runtime_seam() {
        // The replica does not reflect the watermark -> no witness -> cannot read.
        let _s = Session::<Start>::begin().write::<W1>();
        assert!(freshness_of::<(W1, Start)>(false).is_none());
    }

    #[test]
    fn fresh_witness_is_copy_reusable_across_reads() {
        let s = Session::<Start>::begin().write::<W1>();
        let fresh = freshness_of::<(W1, Start)>(true).unwrap();
        let (o1, s) = s.read(fresh, 1);
        let (o2, _s) = s.read(fresh, 2); // fresh reused — Copy (monotonic reads)
        assert_eq!((*o1.value(), *o2.value()), (1, 2));
    }
}

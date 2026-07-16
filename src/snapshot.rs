//! Chandy–Lamport **distributed snapshot** — recording a consistent global state
//! without stopping the system (Chandy & Lamport, 1985). This is the *operational*
//! half of a dual: the marker protocol that **produces** a consistent cut. Its
//! *denotational* half — the predicate that **certifies** one — is `consistent_cut`.
//!
//! Every prior rung typed something *per transaction* (the A/C/I of ACID). A
//! snapshot is a different axis: it captures the state of *every* process and *every*
//! channel at a single logical instant, so a crash-recovery layer, a deadlock
//! detector, or a distributed garbage collector can reason about "the whole system,
//! then". The one thing that must never happen is an **orphan**: the snapshot records
//! a message as *received* but not as *sent* — a global state no real execution ever
//! passed through. Chandy–Lamport rules that out with one discipline, and this rung
//! makes that discipline a type.
//!
//! ## The mechanism — record your own state *before* any channel
//!
//! The protocol runs as a phase machine over one process:
//!
//! * [`Snapshot`]`<`[`Idle`]`>` — the process is running normally, its channel
//!   topology known. [`new`](Snapshot::new) fixes how many `incoming` channels it
//!   has; nothing is recorded yet.
//! * [`begin`](Snapshot::begin) — **the first marker arrives.** This records the
//!   process's own state *now* and crosses into [`Recording`]. It is the only door
//!   into the recording phase, so own-state is recorded before anything else can be.
//! * [`record_channel`](Snapshot::record_channel) — defined **only** on
//!   `Snapshot<`[`Recording`]`>` — captures one incoming channel's in-flight messages
//!   (those that arrived after own-state was recorded but before that channel's
//!   marker). One call per incoming channel.
//! * [`seal`](Snapshot::seal) — defined **only** on `Snapshot<`[`Recording`]`>` —
//!   once every incoming channel is recorded, freezes the result into a [`Recorded`]
//!   global state. Sealing early returns [`Incomplete`].
//!
//! The load-bearing move — **recording a channel before recording your own state** —
//! is the one that manufactures orphans (a message counted in the channel that your
//! own pre-snapshot state already reflects sending, or vice versa). It is
//! *unrepresentable* here: [`record_channel`](Snapshot::record_channel) exists only on
//! [`Recording`], and the sole way to reach [`Recording`] is
//! [`begin`](Snapshot::begin), which records own-state first. Like
//! [`two_phase_lock`](crate::two_phase_lock)'s Growing→Shrinking wall, this is a
//! purely **structural** guarantee — a `Snapshot` carries no witness that must be
//! trusted, only a type-level phase — the pessimistic-lock species, not the
//! runtime-validation species of `occ`.
//!
//! ## The happy path — own state, then every channel, then seal
//!
//! ```
//! use quorum_types::snapshot::Snapshot;
//! // this process has two incoming channels.
//! let snap = Snapshot::new(2);
//! // first marker: record our own state (0x0042), enter the recording phase.
//! let mut snap = snap.begin(0x0042);
//! // capture the in-flight messages on each incoming channel (folded to a scalar here).
//! snap.record_channel(3); // channel 0 held 3 in-flight
//! snap.record_channel(0); // channel 1 was empty
//! let cut = snap.seal().expect("both channels recorded");
//! assert_eq!(cut.own_state(), 0x0042);
//! assert_eq!(cut.channel_state(), 3);
//! assert_eq!(cut.channels(), 2);
//! ```
//!
//! ## Sealing before every channel is recorded fails — no partial cut
//!
//! ```
//! use quorum_types::snapshot::Snapshot;
//! let mut snap = Snapshot::new(2).begin(1);
//! snap.record_channel(5); // only one of two channels recorded
//! match snap.seal() {
//!     Ok(_) => unreachable!("a cut missing a channel is not consistent"),
//!     Err(incomplete) => {
//!         assert_eq!(incomplete.recorded(), 1);
//!         assert_eq!(incomplete.expected(), 2);
//!     }
//! }
//! ```
//!
//! ## Recording a channel before recording own state is a compile error
//!
//! [`record_channel`](Snapshot::record_channel) exists only on
//! `Snapshot<`[`Recording`]`>`. A process still [`Idle`] — one that has not yet seen
//! a marker and recorded its own state — has no such method, so the orphan-making
//! order cannot be written:
//!
//! ```compile_fail
//! use quorum_types::snapshot::Snapshot;
//! let mut snap = Snapshot::new(2); // Idle: own state not yet recorded
//! snap.record_channel(3); // no `record_channel` on Snapshot<Idle>: begin() first
//! ```
//!
//! ## Sealing an Idle snapshot is a compile error
//!
//! [`seal`](Snapshot::seal) likewise exists only on [`Recording`] — you cannot freeze
//! a global state that never recorded its own process:
//!
//! ```compile_fail
//! use quorum_types::snapshot::Snapshot;
//! let snap = Snapshot::new(0);
//! let _ = snap.seal(); // no `seal` on Snapshot<Idle>: begin() first
//! ```
//!
//! ## You cannot fabricate a recorded cut
//!
//! [`Recorded`]'s fields are private, so a completed snapshot cannot be built by hand
//! — the only route to one is [`seal`](Snapshot::seal) succeeding:
//!
//! ```compile_fail
//! use quorum_types::snapshot::Recorded;
//! let forged = Recorded { own_state: 0, channel_state: 0, channels: 9 };
//! let _ = forged.channels(); // Recorded has private fields: no hand-built cut
//! ```
//!
//! ## Where the types stop (the runtime seam)
//!
//! The phase machine is structural, but it types the *protocol order*, not the
//! *content* of what was recorded. It does **not** own:
//!
//! * **FIFO channels.** Chandy–Lamport is correct only if channels deliver in order,
//!   so that a marker flushes exactly the messages that preceded it. This rung records
//!   whatever scalar [`record_channel`](Snapshot::record_channel) is handed; that the
//!   captured value really is "everything before the marker and nothing after" is the
//!   transport's obligation, the same declared-order trust
//!   [`chain`](crate::chain) places in in-order forwarding.
//! * **One marker per channel.** The completeness check counts
//!   [`record_channel`](Snapshot::record_channel) *calls*, not channel *identities* —
//!   recording channel 0 twice and channel 1 never
//!   still reaches `recorded == expected`. Binding each recording to a distinct channel
//!   would need a per-channel token, which is the crate's recurring detachable-witness
//!   hazard; instead the count is [fused to this snapshot] and one-marker-per-channel is
//!   the caller's discipline, exactly as `occ` collapses its whole read set to one
//!   scalar version.
//! * **That the recorded cut is globally consistent.** Own-state-before-channels makes
//!   *this process's* recording *order* orphan-safe (given FIFO channels) — a condition
//!   that is *necessary* for orphan avoidance but not, on its own, *sufficient*.
//!   Orphan-freeness is a **global** predicate over the union of every process's slice
//!   (a receive recorded inside implies its send recorded inside); no single slice can
//!   be "orphan-free" in isolation. That *every* process ran the protocol and the union
//!   is causally closed is the whole-system premise — precisely what `consistent_cut`
//!   verifies after the fact. The theorem (Chandy–Lamport) is that an honest run of this
//!   protocol on every process yields a [`Recorded`] state whose global union satisfies
//!   that predicate; the type owns the local order, not the global theorem.
//!
//! [fused to this snapshot]: Snapshot#structfield.recorded

use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A phase of the snapshot protocol. Sealed: the only phases are [`Idle`] and
/// [`Recording`], so `Snapshot<Recording>` can arise only through
/// [`begin`](Snapshot::begin), never by naming a phase type directly.
pub trait Phase: sealed::Sealed {}

/// The **idle** phase: the process is running and has not yet recorded its own
/// state. It can [`begin`](Snapshot::begin), but cannot record channels or seal.
#[derive(Debug)]
pub struct Idle;
/// The **recording** phase: own state is recorded and the snapshot is capturing
/// incoming channels. Only in this phase can channels be recorded or the cut sealed.
#[derive(Debug)]
pub struct Recording;

impl sealed::Sealed for Idle {}
impl sealed::Sealed for Recording {}
impl Phase for Idle {}
impl Phase for Recording {}

/// One process's participation in a Chandy–Lamport snapshot, in phase `PH`.
///
/// Move-only and `#[must_use]`: an in-progress snapshot is a linear resource,
/// resolved by [`seal`](Snapshot::seal) or dropped (abandoned snapshot). All fields
/// are private — a [`Recording`] snapshot cannot be forged, and the recorded-channel
/// count is fused to the token so no outside resource can advance it.
#[must_use = "a Snapshot is an in-progress recording; record its channels and seal it (or drop to abandon)"]
pub struct Snapshot<PH: Phase> {
    /// The process's own state, recorded at [`begin`](Snapshot::begin) (placeholder
    /// `0` while [`Idle`]).
    own_state: u64,
    /// How many incoming channels this process must record before it can seal.
    incoming: u32,
    /// How many channels have been recorded so far. Private and only ever advanced by
    /// this snapshot's own [`record_channel`](Snapshot::record_channel) — it is not a
    /// counter any external, identity-less resource can touch.
    recorded: u32,
    /// The folded in-flight state captured across recorded channels.
    channel_state: u64,
    _ph: PhantomData<PH>,
}

impl Snapshot<Idle> {
    /// Prepare a snapshot for a process with `incoming` incoming channels. The process
    /// keeps running; nothing is recorded until the first marker triggers
    /// [`begin`](Snapshot::begin).
    pub const fn new(incoming: u32) -> Self {
        Snapshot { own_state: 0, incoming, recorded: 0, channel_state: 0, _ph: PhantomData }
    }

    /// **The first marker arrives.** Record this process's own state and cross into the
    /// [`Recording`] phase. This is the only constructor of a recording snapshot, which
    /// is exactly why own-state is always recorded before any channel can be.
    pub fn begin(self, own_state: u64) -> Snapshot<Recording> {
        Snapshot {
            own_state,
            incoming: self.incoming,
            recorded: 0,
            channel_state: 0,
            _ph: PhantomData,
        }
    }
}

impl Snapshot<Recording> {
    /// Record one incoming channel's in-flight state — the messages that arrived after
    /// own-state was recorded but before that channel's marker. Call once per incoming
    /// channel. `captured` is folded into the running channel state.
    ///
    /// Recording *more* than `incoming` channels does not forge a cut: it drives the
    /// count past `incoming`, so [`seal`](Snapshot::seal) can never again reach equality
    /// and the snapshot becomes unsealable (returning [`Incomplete`]) — it fails safe,
    /// never producing a false [`Recorded`].
    ///
    /// # Panics
    /// In debug builds, on `u32`/`u64` counter overflow (as
    /// [`vclock::VClock::tick`](crate::vclock::VClock::tick)).
    pub fn record_channel(&mut self, captured: u64) {
        self.channel_state += captured;
        self.recorded += 1;
    }

    /// The process's own recorded state.
    pub const fn own_state(&self) -> u64 {
        self.own_state
    }

    /// How many incoming channels have been recorded so far.
    pub const fn recorded(&self) -> u32 {
        self.recorded
    }

    /// Freeze the snapshot into a [`Recorded`] global state — but **only** once every
    /// incoming channel has been recorded. If channels are still outstanding, returns
    /// [`Incomplete`] (and the snapshot is consumed — a partial cut is not a cut).
    pub fn seal(self) -> Result<Recorded, Incomplete> {
        if self.recorded == self.incoming {
            Ok(Recorded {
                own_state: self.own_state,
                channel_state: self.channel_state,
                channels: self.incoming,
            })
        } else {
            Err(Incomplete { recorded: self.recorded, expected: self.incoming })
        }
    }
}

/// A sealed Chandy–Lamport snapshot: this process's own state plus its incoming
/// channel state, recorded in the orphan-free order. Constructed only by
/// [`Snapshot::seal`] succeeding (private fields, no public constructor) — so its
/// existence certifies own-state was recorded before any channel.
///
/// By the Chandy–Lamport theorem, the union of every process's `Recorded` slice from
/// one protocol run is a consistent cut — the property `consistent_cut` verifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    own_state: u64,
    channel_state: u64,
    channels: u32,
}

impl Recorded {
    /// The process's own recorded state.
    pub const fn own_state(&self) -> u64 {
        self.own_state
    }
    /// The folded in-flight state captured across all incoming channels.
    pub const fn channel_state(&self) -> u64 {
        self.channel_state
    }
    /// How many incoming channels were recorded.
    pub const fn channels(&self) -> u32 {
        self.channels
    }
}

/// A [`seal`](Snapshot::seal) whose recorded-channel count did not equal the expected
/// number — normally too *few* channels recorded (a partial cut), or, if
/// [`record_channel`](Snapshot::record_channel) was miscalled more than once per
/// channel, too *many*. Either way the count is off, so no cut is produced. Carries
/// recorded versus expected; the snapshot it came from is consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Incomplete {
    recorded: u32,
    expected: u32,
}

impl Incomplete {
    /// How many channels had been recorded at the failed seal.
    pub const fn recorded(&self) -> u32 {
        self.recorded
    }
    /// How many channels the process was supposed to record.
    pub const fn expected(&self) -> u32 {
        self.expected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_state_then_channels_then_seal() {
        let mut snap = Snapshot::new(2).begin(0x0042);
        assert_eq!(snap.own_state(), 0x0042);
        snap.record_channel(3);
        snap.record_channel(0);
        assert_eq!(snap.recorded(), 2);
        let cut = snap.seal().expect("both channels recorded -> sealed");
        assert_eq!(cut.own_state(), 0x0042);
        assert_eq!(cut.channel_state(), 3);
        assert_eq!(cut.channels(), 2);
    }

    #[test]
    fn sealing_early_is_incomplete_and_consumes_the_snapshot() {
        let mut snap = Snapshot::new(3).begin(1);
        snap.record_channel(5);
        match snap.seal() {
            Ok(_) => panic!("a cut missing channels is not consistent"),
            Err(incomplete) => {
                assert_eq!(incomplete.recorded(), 1);
                assert_eq!(incomplete.expected(), 3);
            }
        }
    }

    #[test]
    fn a_process_with_no_incoming_channels_seals_immediately() {
        // A source process (no incoming channels) records only its own state.
        let cut = Snapshot::new(0).begin(7).seal().expect("no channels to record");
        assert_eq!(cut.own_state(), 7);
        assert_eq!(cut.channels(), 0);
        assert_eq!(cut.channel_state(), 0);
    }
}

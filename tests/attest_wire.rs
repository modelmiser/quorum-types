//! Rung 6b: uniqueness at the wire — an equivocating Byzantine host, and the
//! threshold that denies it.
//!
//! [`wire_sim`](../wire_sim.rs) drove the *crash* rung over turmoil: a lying
//! peer could shorten a lease, but every datagram carried one honest story.
//! Here the adversary **equivocates** — it tells two collectors two different
//! values for the same epoch — and the question is whether the *type* the
//! collectors mint can be forced to disagree.
//!
//! The answer is the whole point of tier 6b, and it is a **sign-flip**:
//!
//! * At the **existence** threshold (`f+1`, [`attest`]) the equivocator wins —
//!   each collector reaches `f+1` for a *different* value, and the two mint
//!   conflicting [`Attested`] values. Existence was never uniqueness.
//! * At the **masking** threshold (`⌈(n+2f+1)/2⌉`, [`commit_masking`]) the
//!   *same* adversary under the *same* schedule cannot force two commits: a
//!   minority value plus one liar never reaches the bar. [`Committed`] is
//!   unique per epoch — safety where existence had none.
//!
//! **The promotion boundary carries over from rung 5.** Votes arrive as bytes;
//! [`promote_vote`] is the sole crossing to a typed [`Vote`], and the epoch is
//! *checked against* the collector's compiled `E`, never *lifted* from the
//! bytes (a const generic cannot be — the rung-5 finding). The safety decision
//! is delegated to the real library; the harness only respects its `Option`.
//!
//! Determinism buy-in as before: `BTreeMap` only, virtual clock, no threads.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quorum_types::attest::{attest, commit_masking, Vote};
use quorum_types::byzantine::BftConfig;
use quorum_types::membership::NodeId;
use tokio::time::{sleep, timeout};
use turmoil::{net, Builder};

/// The compiled epoch every collector speaks. A vote for another epoch is
/// rejected at promotion, not lifted into the type.
const EPOCH: u64 = 3;
const PORT: u16 = 9998;
/// Two candidate values on the wire (`A` and `B`).
const A: u64 = 0xAA;
const B: u64 = 0xBB;
/// n = 5, f = 1 ⇒ existence threshold 2, masking threshold ⌈8/2⌉ = 4.
const MEMBERS: [NodeId; 5] = [0, 1, 2, 3, 4];
const F: usize = 1;

fn cfg() -> BftConfig<EPOCH> {
    BftConfig::<EPOCH>::new(MEMBERS.into_iter().collect(), F).unwrap()
}

fn tick() -> u64 {
    turmoil::elapsed().as_secs()
}

// ---------------------------------------------------------------------------
// The wire: 25-byte vote datagrams, and the ONE place bytes become a Vote.
// ---------------------------------------------------------------------------

fn frame(tag: u8, epoch: u64, voter: u64, value: u64) -> [u8; 25] {
    let mut f = [0u8; 25];
    f[0] = tag;
    f[1..9].copy_from_slice(&epoch.to_le_bytes());
    f[9..17].copy_from_slice(&voter.to_le_bytes());
    f[17..25].copy_from_slice(&value.to_le_bytes());
    f
}

/// The promotion boundary: wire bytes → a typed [`Vote<u64, EPOCH>`]. The epoch
/// word must *equal* `EPOCH` (a runtime `u64` selects the compiled epoch space;
/// it is never lifted into the const generic), and the voter id must fit
/// [`NodeId`]. A vote that fails either is not evidence and does not promote.
fn promote_vote(datagram: &[u8]) -> Option<Vote<u64, EPOCH>> {
    if datagram.len() != 25 || datagram[0] != 4 {
        return None;
    }
    let word = |i: usize| u64::from_le_bytes(datagram[i..i + 8].try_into().unwrap());
    let (epoch, voter, value) = (word(1), word(9), word(17));
    if epoch != EPOCH {
        return None; // epoch checked, not lifted
    }
    Some(Vote::new(NodeId::try_from(voter).ok()?, value))
}

// ---------------------------------------------------------------------------
// Observer: per-collector decisions, and the value-split violation.
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Registry {
    decided: BTreeMap<&'static str, u64>,
    violations: Vec<String>,
    trace: Vec<String>,
}

impl Registry {
    /// Record a collector's decision, flagging a split if a *different* value
    /// was already decided elsewhere at this epoch.
    fn decide(&mut self, host: &'static str, value: u64, t: u64) {
        self.trace.push(format!("t={t} {host} decided v{value:#x}"));
        for (other, &v) in &self.decided {
            if v != value {
                self.violations.push(format!(
                    "t={t} value-split: {other}=v{v:#x} vs {host}=v{value:#x}"
                ));
            }
        }
        self.decided.insert(host, value);
    }

    fn abstain(&mut self, host: &'static str, t: u64) {
        self.trace.push(format!("t={t} {host} committed nothing"));
    }

    fn note(&mut self, t: u64, msg: &str) {
        self.trace.push(format!("t={t} {msg}"));
    }
}

type Shared = Arc<Mutex<Registry>>;

/// What each voter tells collector-a and collector-b. Honest nodes tell both the
/// same value; the equivocator differs.
type Faces = BTreeMap<NodeId, (u64, u64)>;

/// Which threshold the collectors mint at.
#[derive(Clone, Copy)]
enum Tier {
    Existence,
    Masking,
}

// ---------------------------------------------------------------------------
// Hosts.
// ---------------------------------------------------------------------------

/// A voter: sends its (possibly two-faced) vote once, to each collector.
async fn voter(node: NodeId, faces: (u64, u64)) -> turmoil::Result {
    let sock = net::UdpSocket::bind(("0.0.0.0", PORT)).await?;
    // A small stagger so sends land before collectors decide; deterministic.
    sleep(Duration::from_millis(200)).await;
    let _ = sock.send_to(&frame(4, EPOCH, u64::from(node), faces.0), format!("collector-a:{PORT}")).await;
    let _ = sock.send_to(&frame(4, EPOCH, u64::from(node), faces.1), format!("collector-b:{PORT}")).await;
    Ok(())
}

/// A collector: gathers promoted votes for a fixed window, then delegates the
/// safety decision to the real library at the chosen tier and records it.
async fn collector(name: &'static str, reg: Shared, tier: Tier) -> turmoil::Result {
    let sock = net::UdpSocket::bind(("0.0.0.0", PORT)).await?;
    let cfg = cfg();
    let mut votes: Vec<Vote<u64, EPOCH>> = Vec::new();
    let mut buf = [0u8; 64];
    // Collect until t≈4, then decide.
    while tick() < 4 {
        if let Ok(Ok((n, _))) = timeout(Duration::from_millis(500), sock.recv_from(&mut buf)).await {
            if let Some(v) = promote_vote(&buf[..n]) {
                votes.push(v);
            }
        }
    }
    let now = tick();
    match tier {
        Tier::Existence => match attest(votes, &cfg) {
            Some(a) => reg.lock().unwrap().decide(name, *a.value(), now),
            None => reg.lock().unwrap().abstain(name, now),
        },
        Tier::Masking => match commit_masking(votes, &cfg) {
            Some(c) => reg.lock().unwrap().decide(name, *c.value(), now),
            None => reg.lock().unwrap().abstain(name, now),
        },
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario.
// ---------------------------------------------------------------------------

fn run(faces: Faces, tier: Tier, seed: u64) -> Registry {
    let reg: Shared = Arc::new(Mutex::new(Registry::default()));
    let mut sim = Builder::new()
        .simulation_duration(Duration::from_secs(20))
        .rng_seed(seed)
        .build();

    for node in MEMBERS {
        let f = faces[&node];
        sim.host(node_name(node), move || voter(node, f));
    }
    let r = reg.clone();
    sim.host("collector-a", move || collector("collector-a", r.clone(), tier));
    let r = reg.clone();
    sim.host("collector-b", move || collector("collector-b", r.clone(), tier));

    let r = reg.clone();
    sim.client("driver", async move {
        // The two views cannot reconcile: the equivocation lives in that gap.
        turmoil::partition("collector-a", "collector-b");
        r.lock().unwrap().note(tick(), "driver: collectors partitioned (two views)");
        sleep(Duration::from_secs(8)).await;
        Ok(())
    });

    sim.run().unwrap();
    #[allow(clippy::let_and_return)]
    let snapshot = reg.lock().unwrap().clone();
    snapshot
}

/// A voter host needs a `&'static str` name; map the five node ids.
fn node_name(n: NodeId) -> &'static str {
    match n {
        0 => "n0",
        1 => "n1",
        2 => "n2",
        3 => "n3",
        _ => "n4",
    }
}

/// The equivocation schedule: honest nodes split 2–2 between A and B (each
/// telling both collectors the same thing); the liar (node 4) tells
/// collector-a "A" and collector-b "B". At `f+1` each collector reaches its
/// side's value; at the masking threshold neither does.
fn equivocation_faces() -> Faces {
    BTreeMap::from([
        (0, (A, A)),
        (1, (B, B)),
        (2, (A, A)),
        (3, (B, B)),
        (4, (A, B)), // the equivocator
    ])
}

/// An honest supermajority for A; the liar forges B to everyone. Used to show
/// masking commits a *unique* value non-vacuously.
fn honest_majority_faces() -> Faces {
    BTreeMap::from([
        (0, (A, A)),
        (1, (A, A)),
        (2, (A, A)),
        (3, (A, A)),
        (4, (B, B)), // the liar, outvoted
    ])
}

// ---------------------------------------------------------------------------
// The gate's assertions.
// ---------------------------------------------------------------------------

/// The negative control: at the existence threshold the equivocator forces two
/// collectors onto different values. If this does NOT split, the sim has no
/// teeth and the masking result below is vacuous.
#[test]
fn equivocation_splits_at_the_existence_threshold() {
    let reg = run(equivocation_faces(), Tier::Existence, 42);
    assert!(
        !reg.violations.is_empty(),
        "existence control failed to split under equivocation\ntrace: {:#?}",
        reg.trace
    );
    assert!(
        reg.violations.iter().any(|v| v.contains("value-split")),
        "violation shape unexpected: {:?}",
        reg.violations
    );
}

/// The sign-flip: the SAME equivocation schedule at the masking threshold
/// cannot split — a minority value plus one liar never reaches ⌈(n+2f+1)/2⌉.
#[test]
fn the_masking_threshold_denies_the_same_equivocation() {
    let reg = run(equivocation_faces(), Tier::Masking, 42);
    assert!(
        reg.violations.is_empty(),
        "masking split under equivocation the existence tier fell to: {:?}\ntrace: {:#?}",
        reg.violations,
        reg.trace
    );
    // And it is safe *because nothing conflicting cleared the bar*, not because
    // the schedule was toothless — the existence twin above proves the teeth.
    assert!(
        reg.trace.iter().filter(|l| l.contains("committed nothing")).count() == 2,
        "expected both collectors to abstain at the masking threshold\ntrace: {:#?}",
        reg.trace
    );
}

/// Non-vacuous uniqueness: when an honest supermajority does clear the masking
/// threshold, both partitioned views commit the SAME value, and the liar's
/// forged value never commits. (Existence twin: at f+1 the honest four already
/// dominate, so no split there either — the interesting split needs the 2–2
/// schedule above.)
#[test]
fn honest_supermajority_commits_a_unique_value_across_both_views() {
    let reg = run(honest_majority_faces(), Tier::Masking, 42);
    assert!(reg.violations.is_empty(), "unexpected split: {:?}", reg.violations);
    assert_eq!(reg.decided.get("collector-a"), Some(&A), "view a commits A");
    assert_eq!(reg.decided.get("collector-b"), Some(&A), "view b commits A");
    assert!(
        !reg.decided.values().any(|&v| v == B),
        "the liar's forged value B must never commit"
    );
}

/// The promotion boundary keeps rung 5's discipline: malformed bytes, a foreign
/// epoch (checked, not lifted), and out-of-range ids are not evidence.
#[test]
fn promote_vote_rejects_malformed_and_foreign_epoch_votes() {
    assert!(promote_vote(&frame(4, EPOCH, 0, A)).is_some(), "honest vote promotes");
    assert!(promote_vote(&frame(4, EPOCH + 1, 0, A)).is_none(), "foreign epoch rejected");
    assert!(promote_vote(&frame(9, EPOCH, 0, A)).is_none(), "wrong tag rejected");
    assert!(promote_vote(&frame(4, EPOCH, u64::from(u32::MAX) + 1, A)).is_none(), "node id out of range");
    assert!(promote_vote(&[0u8; 24]).is_none(), "short datagram rejected");
}

/// Determinism buy-in: same seed reproduces the trace byte-for-byte, and the
/// splitting run reproduces its violations non-vacuously (the rung-5 lesson —
/// a determinism check over an empty violation set proves nothing).
#[test]
fn same_seed_reproduces_the_identical_trace() {
    let a = run(equivocation_faces(), Tier::Existence, 7);
    let b = run(equivocation_faces(), Tier::Existence, 7);
    assert_eq!(a.trace, b.trace, "same seed diverged: determinism buy-in violated");
    assert_eq!(a.violations, b.violations);
    assert!(!a.violations.is_empty(), "determinism check ran vacuously");
}

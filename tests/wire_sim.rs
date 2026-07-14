//! Rung 5: the wire — driving the typed API from a simulated network.
//!
//! `tests/partition_heal.rs` replayed the TLA+ counterexample *in process*,
//! with one shared clock and function calls between roles. Here the same
//! scenario runs under [turmoil]: three hosts exchanging UDP datagrams over a
//! simulated network, with the partition induced as a *network event* rather
//! than a bookkeeping flag. The safety decision is still delegated to the real
//! library — [`reconfigure`] — and the harness only respects its `Result`.
//!
//! **The promotion boundary.** Every quorum certificate in this crate is
//! indexed by `const E: u64`, and a const generic cannot be lifted from
//! runtime bytes.
//! [`promote`] is the single place where wire bytes become typed values, and
//! the epoch crossing happens at one visible `match` in [`candidate`]: a
//! runtime `u64` *chooses among monomorphizations compiled in advance* (the
//! community-standard answer to the const-generic lifting limit — there is no
//! `match` over 2^64 arms). Everything downstream of that match is carried by
//! the existing types; everything upstream is parsed data an adversarial
//! network delivered. This is the fourth root of trust in the arc: rung 2
//! trusts `Config::new` membership, rung 3 trusts caller-chosen samples,
//! rung 4 trusts the declared `f` — rung 5 trusts the deserializer. A lying
//! peer can announce a shorter lease and induce early failover; the types
//! verify the counting *after* promotion, never that the bytes told the truth.
//!
//! **Scope fences.** One scripted candidate (no competing elections); lease
//! renewal is unmodeled (the epoch-0 leader serves one lease and surrenders);
//! datagrams are unauthenticated (this is the crash rung over a wire, not the
//! Byzantine one); the candidate waits two ticks past the reported lease
//! expiry before retrying — the slack that stands in for bounded clock skew,
//! which the in-process sim got for free from its single `now` and a per-host
//! harness does not.
//!
//! **Determinism buy-in** (turmoil runs hosts on one thread, but requires the
//! harness to cooperate): `BTreeSet`/`BTreeMap` only, no `tokio::select!`, no
//! threads, no wall clock — ticks come from virtual [`turmoil::elapsed`]. The
//! `same_seed_reproduces_the_identical_trace` test checks the buy-in held.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quorum_types::All;
use quorum_types::failover::{FailoverError, Lease, Leased, Tick, reconfigure};
use quorum_types::membership::{Config, NodeId};
use tokio::time::{sleep, timeout};
use turmoil::{Builder, net};

/// Lease TTL in ticks (simulated seconds), matching `partition_heal.rs`.
const TTL: Tick = 5;
/// Ticks of leader silence before the candidate seeks election.
const SILENCE: Tick = 3;
/// Everyone speaks on one UDP port; hosts are addressed by turmoil name.
const PORT: u16 = 9999;

const LEADER: NodeId = 1;
const CANDIDATE: NodeId = 2;
const VOTER: NodeId = 3;

fn tick() -> Tick {
    turmoil::elapsed().as_secs()
}

// ---------------------------------------------------------------------------
// The wire: 25-byte datagrams, and the ONE place bytes become typed values.
// ---------------------------------------------------------------------------

/// A parsed message. Constructing one is a *trust event*, not a proof.
enum Msg {
    /// "I am serving `epoch` under a lease granted at `a` for `b` ticks."
    Heartbeat { epoch: u64, lease: Lease },
    /// "Vote for me at `epoch`."
    VoteReq { epoch: u64 },
    /// "Node `node` votes for you at `epoch`."
    Vote { epoch: u64, node: NodeId },
}

fn frame(tag: u8, a: u64, b: u64, c: u64) -> [u8; 25] {
    let mut f = [0u8; 25];
    f[0] = tag;
    f[1..9].copy_from_slice(&a.to_le_bytes());
    f[9..17].copy_from_slice(&b.to_le_bytes());
    f[17..25].copy_from_slice(&c.to_le_bytes());
    f
}

/// The promotion boundary: the sole crossing from wire bytes to typed values.
///
/// Sanitization happens HERE, before any consumer sees the data: lease
/// arithmetic is checked (`Lease::new` computes `granted_at + ttl`, so a
/// hostile frame of two `u64::MAX` words must die at the boundary, not panic
/// inside the library — the rung-4 lesson that guard arithmetic over untrusted
/// input is itself attack surface), the epoch must have a successor (the
/// candidate computes `epoch + 1`), and node ids must fit `NodeId`.
fn promote(datagram: &[u8]) -> Option<Msg> {
    if datagram.len() != 25 {
        return None;
    }
    let word = |i: usize| u64::from_le_bytes(datagram[i..i + 8].try_into().unwrap());
    let (a, b, c) = (word(1), word(9), word(17));
    match datagram[0] {
        1 => {
            b.checked_add(c)?; // reject leases whose expiry would overflow
            a.checked_add(1)?; // consumers compute the successor epoch; reject
            // a heartbeat whose epoch has none (u64::MAX would panic the
            // candidate's dispatch — a reviewer compiled exactly that frame)
            Some(Msg::Heartbeat { epoch: a, lease: Lease::new(b, c) })
        }
        2 => Some(Msg::VoteReq { epoch: a }),
        3 => Some(Msg::Vote { epoch: a, node: NodeId::try_from(b).ok()? }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Shared observer: the NoSplitBrain invariant and the deterministic trace.
// ---------------------------------------------------------------------------

/// Global bookkeeping the TLA+ model calls `serving` — trusted test
/// scaffolding, exactly as in `partition_heal.rs`. The types constrain each
/// node's local transitions; this registry observes whether the *protocol
/// driving them* composes. It records violations instead of panicking so the
/// negative control can assert that they occur.
#[derive(Default, Clone)]
struct Registry {
    serving: BTreeMap<&'static str, u64>,
    violations: Vec<String>,
    trace: Vec<String>,
}

impl Registry {
    fn set_serving(&mut self, host: &'static str, epoch: u64, t: Tick) {
        self.serving.insert(host, epoch);
        self.trace.push(format!("t={t} {host} serving e{epoch}"));
        if self.serving.len() > 1 {
            let who: Vec<String> =
                self.serving.iter().map(|(h, e)| format!("{h}=e{e}")).collect();
            self.violations.push(format!("t={t} split-brain: [{}]", who.join(", ")));
        }
    }

    fn clear_serving(&mut self, host: &'static str, t: Tick) {
        self.serving.remove(host);
        self.trace.push(format!("t={t} {host} stopped serving"));
    }

    fn note(&mut self, t: Tick, msg: &str) {
        self.trace.push(format!("t={t} {msg}"));
    }
}

type Shared = Arc<Mutex<Registry>>;

// ---------------------------------------------------------------------------
// Hosts.
// ---------------------------------------------------------------------------

/// The epoch-0 leader: serves while its lease is valid, heartbeats its lease
/// parameters, surrenders when the lease lapses, then follows. It can never
/// re-serve epoch 0: [`Leased::surrender`] consumed the token by move — the
/// stale-holder-revival bug is unrepresentable in this harness because the
/// authority value no longer exists.
async fn leader(reg: Shared) -> turmoil::Result {
    let sock = net::UdpSocket::bind(("0.0.0.0", PORT)).await?;
    let mut token = Some(Leased::<0, All>::genesis(0b111, Lease::new(0, TTL)));
    reg.lock().unwrap().set_serving("leader", 0, tick());
    let mut buf = [0u8; 64];
    let mut learned = false;
    loop {
        let now = tick();
        if let Some(t) = token.take() {
            match t.surrender(now) {
                Ok(()) => reg.lock().unwrap().clear_serving("leader", now),
                Err(t) => {
                    // Still authoritative: announce epoch and lease terms.
                    let hb = frame(1, 0, 0, TTL);
                    let _ = sock.send_to(&hb, format!("candidate:{PORT}")).await;
                    let _ = sock.send_to(&hb, format!("voter:{PORT}")).await;
                    token = Some(t);
                    sleep(Duration::from_secs(1)).await;
                }
            }
        } else {
            // Follower: learn who is serving now.
            if let Ok(Ok((n, _))) = timeout(Duration::from_millis(700), sock.recv_from(&mut buf)).await
            {
                if let Some(Msg::Heartbeat { epoch, .. }) = promote(&buf[..n]) {
                    if !learned {
                        learned = true;
                        reg.lock().unwrap().note(tick(), &format!("leader learned e{epoch} is serving"));
                    }
                }
            }
        }
    }
}

/// The failover candidate. On sustained leader silence it seeks election at
/// the next epoch. The `match` below IS the runtime→type-level bridge: the
/// wire delivered `epoch` as data, and data can only *select* among epochs
/// this binary was compiled to speak.
async fn candidate(reg: Shared, guarded: bool) -> turmoil::Result {
    let sock = net::UdpSocket::bind(("0.0.0.0", PORT)).await?;
    let mut buf = [0u8; 64];
    let mut last_heard: Option<Tick> = None;
    let mut known: Option<(u64, Lease)> = None;
    loop {
        if let Ok(Ok((n, _))) = timeout(Duration::from_millis(700), sock.recv_from(&mut buf)).await {
            if let Some(Msg::Heartbeat { epoch, lease }) = promote(&buf[..n]) {
                last_heard = Some(tick());
                known = Some((epoch, lease));
            }
        }
        let now = tick();
        if let (Some((epoch, prior)), Some(heard)) = (known, last_heard) {
            if now.saturating_sub(heard) >= SILENCE {
                reg.lock()
                    .unwrap()
                    .note(now, &format!("candidate: leader silent, seeking e{}", epoch + 1));
                match epoch + 1 {
                    1 => elect_and_serve::<1>(&sock, &reg, prior, guarded).await?,
                    2 => elect_and_serve::<2>(&sock, &reg, prior, guarded).await?,
                    other => {
                        reg.lock()
                            .unwrap()
                            .note(now, &format!("e{other} is outside this binary's compiled epoch space"));
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Election and, on success, the epoch-`N` leader loop. Monomorphic in `N`:
/// once inside, the epoch is a type again and all evidence is `E = N`-indexed.
async fn elect_and_serve<const N: u64>(
    sock: &net::UdpSocket,
    reg: &Shared,
    prior: Lease,
    guarded: bool,
) -> turmoil::Result {
    // Membership is operator-chosen (the rung-2 root of trust); the votes that
    // fill it arrive off the wire through `promote`.
    let cfg = Config::<N>::new(BTreeSet::from([LEADER, CANDIDATE, VOTER]));
    let _ = sock.send_to(&frame(2, N, 0, 0), format!("voter:{PORT}")).await;
    let mut votes: BTreeSet<NodeId> = BTreeSet::from([CANDIDATE]);
    let mut buf = [0u8; 64];
    for _ in 0..4 {
        if votes.len() >= cfg.threshold() {
            break;
        }
        if let Ok(Ok((n, _))) = timeout(Duration::from_millis(500), sock.recv_from(&mut buf)).await {
            if let Some(Msg::Vote { epoch, node }) = promote(&buf[..n]) {
                if epoch == N {
                    votes.insert(node);
                }
            }
        }
    }
    let Some(quorum) = cfg.certify(votes) else {
        reg.lock().unwrap().note(tick(), &format!("election at e{N} failed: no quorum"));
        return Ok(());
    };
    reg.lock()
        .unwrap()
        .note(tick(), &format!("quorum certified at e{N}: {:?}", quorum.members()));

    let (token, granted): (Leased<N, All>, Tick) = if guarded {
        loop {
            let now = tick();
            match reconfigure::<N>(prior, now, 0b110, Lease::new(now, TTL)) {
                Ok(t) => {
                    reg.lock().unwrap().note(now, "reconfigure permitted: prior lease lapsed");
                    break (t, now);
                }
                Err(FailoverError::LeaseStillValid { until }) => {
                    reg.lock()
                        .unwrap()
                        .note(now, &format!("reconfigure refused: prior lease valid until t={until}"));
                    // Wait out the reported expiry plus two ticks of slack —
                    // the stand-in for bounded clock skew between hosts.
                    // Saturating: `until` is wire-derived and may be u64::MAX.
                    sleep(Duration::from_secs(until.saturating_sub(now).saturating_add(2))).await;
                }
            }
        }
    } else {
        // NEGATIVE CONTROL: the genesis escape hatch, used away from genesis.
        // Same quorum, same schedule — only the lease guard is bypassed.
        let now = tick();
        reg.lock()
            .unwrap()
            .note(now, "UNGUARDED: minting authority via genesis, ignoring the prior lease");
        (Leased::<N, All>::genesis(0b110, Lease::new(now, TTL)), now)
    };

    reg.lock().unwrap().set_serving("candidate", N, tick());
    let _authority = token; // held for the duration of service
    let hb = frame(1, N, granted, TTL);
    loop {
        let _ = sock.send_to(&hb, format!("leader:{PORT}")).await;
        let _ = sock.send_to(&hb, format!("voter:{PORT}")).await;
        sleep(Duration::from_secs(1)).await;
    }
}

/// Grants at most one vote per epoch — the usual election-safety discipline.
async fn voter(reg: Shared) -> turmoil::Result {
    let sock = net::UdpSocket::bind(("0.0.0.0", PORT)).await?;
    let mut voted: BTreeSet<u64> = BTreeSet::new();
    let mut buf = [0u8; 64];
    loop {
        let (n, from) = sock.recv_from(&mut buf).await?;
        if let Some(Msg::VoteReq { epoch }) = promote(&buf[..n]) {
            if voted.insert(epoch) {
                reg.lock().unwrap().note(tick(), &format!("voter grants e{epoch}"));
                let _ = sock.send_to(&frame(3, epoch, u64::from(VOTER), 0), from).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The scripted schedule: the TLA+ counterexample as network events.
// ---------------------------------------------------------------------------

/// crash → partition → (refused failover) → lease lapse → failover → heal.
/// `quorum_noguard.cfg`'s State 3→4 is the moment the unguarded twin serves
/// two leaders; the guarded run is refused there and waits.
fn run_scenario(guarded: bool, seed: u64) -> Registry {
    let reg: Shared = Arc::new(Mutex::new(Registry::default()));
    let mut sim = Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .rng_seed(seed)
        .build();

    let r = reg.clone();
    sim.host("leader", move || leader(r.clone()));
    let r = reg.clone();
    sim.host("candidate", move || candidate(r.clone(), guarded));
    let r = reg.clone();
    sim.host("voter", move || voter(r.clone()));

    let r = reg.clone();
    sim.client("driver", async move {
        sleep(Duration::from_secs(2)).await;
        turmoil::partition("leader", "candidate");
        turmoil::partition("leader", "voter");
        r.lock().unwrap().note(tick(), "driver: leader isolated");
        sleep(Duration::from_secs(10)).await;
        turmoil::repair("leader", "candidate");
        turmoil::repair("leader", "voter");
        r.lock().unwrap().note(tick(), "driver: partitions healed");
        sleep(Duration::from_secs(4)).await;
        Ok(())
    });

    sim.run().unwrap();
    // Not a tail expression: that would extend the guard temporary past
    // `reg`'s drop (E0597) — the guard borrows the local Arc.
    #[allow(clippy::let_and_return)]
    let snapshot = reg.lock().unwrap().clone();
    snapshot
}

// ---------------------------------------------------------------------------
// The gate's assertions.
// ---------------------------------------------------------------------------

/// H1 bullets (i)–(iii): the typed API drives the whole cycle over the wire,
/// NoSplitBrain holds, and — vacuity check — the lease guard actually FIRED
/// (a green run in which nothing was refused would test nothing).
#[test]
fn guarded_wire_run_has_no_split_brain_and_the_guard_fired() {
    for seed in [1u64, 7, 42] {
        let reg = run_scenario(true, seed);
        assert!(
            reg.violations.is_empty(),
            "seed {seed}: split-brain in the guarded run: {:?}\ntrace: {:#?}",
            reg.violations,
            reg.trace
        );
        for must in [
            "reconfigure refused",   // the TLA+ guard fired at the wire
            "quorum certified at e1", // counting ran on wire-parsed votes
            "candidate serving e1",   // failover completed after the lapse
            "leader learned e1",      // the healed partition converged
        ] {
            assert!(
                reg.trace.iter().any(|l| l.contains(must)),
                "seed {seed}: missing {must:?} — vacuous or stuck run\ntrace: {:#?}",
                reg.trace
            );
        }
    }
}

/// H1 bullet (iv), the sign-flip: same schedule, same quorum, lease guard
/// bypassed via the `genesis` escape hatch — split-brain must occur, or the
/// sim has no teeth and every green run above is vacuous.
#[test]
fn unguarded_twin_split_brains_under_the_same_schedule() {
    let reg = run_scenario(false, 42);
    assert!(
        !reg.violations.is_empty(),
        "negative control failed to fail — the sim cannot detect split-brain\ntrace: {:#?}",
        reg.trace
    );
    assert!(
        reg.violations.iter().any(|v| v.contains("leader") && v.contains("candidate")),
        "violation shape unexpected: {:?}",
        reg.violations
    );
}

/// The boundary keeps its own promise: hostile arithmetic dies at `promote`,
/// not inside a consumer. A review of this rung compiled a heartbeat carrying
/// `epoch = u64::MAX` that passed the old boundary and panicked the
/// candidate's `epoch + 1` dispatch — the same attack class as the previous
/// rung's fault-budget overflow, one layer out.
#[test]
fn promote_rejects_wire_values_whose_arithmetic_would_overflow() {
    assert!(promote(&frame(1, u64::MAX, 0, TTL)).is_none(), "epoch with no successor");
    assert!(promote(&frame(1, 0, u64::MAX, u64::MAX)).is_none(), "lease expiry overflow");
    assert!(promote(&frame(3, 1, u64::from(u32::MAX) + 1, 0)).is_none(), "node id out of range");
    assert!(promote(&frame(1, 0, 0, TTL)).is_some(), "honest frame still promotes");
}

/// Falsifier #4: the determinism buy-in. Same seed, same trace — byte for
/// byte, including violation records. Without this, counterexamples found by
/// seed sweeps would be unactionable.
#[test]
fn same_seed_reproduces_the_identical_trace() {
    let a = run_scenario(true, 42);
    let b = run_scenario(true, 42);
    assert_eq!(a.trace, b.trace, "same seed diverged: determinism buy-in violated");
    assert_eq!(a.violations, b.violations);
    // The guarded run has no violations, so its equality check alone would be
    // vacuous for counterexamples — cover a run that actually produces one.
    let c = run_scenario(false, 42);
    let d = run_scenario(false, 42);
    assert_eq!(c.trace, d.trace, "unguarded twin diverged: counterexamples must reproduce");
    assert_eq!(c.violations, d.violations);
    assert!(!c.violations.is_empty(), "counterexample determinism check ran vacuously");
}

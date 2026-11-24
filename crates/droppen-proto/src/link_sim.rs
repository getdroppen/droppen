//! Links that are slow on purpose, under the real channel drivers.
//!
//! Two [`Channel`]s — an agent's and a device's, each with its [`Session`],
//! multiplexer and pace — run on tokio's paused clock, joined by links that
//! carry their frames through a bottleneck: a deep queue drained at the link's
//! rate, then the one-way delay. Nothing about the senders is modelled; only
//! the network is. Paused time makes every run the same: the clock moves only
//! when every task waits, to the next thing due.
//!
//! The scenario is a weak link's: one download (a `blob` stream) of several
//! megabytes from the agent while the page asks the agent a small question
//! (`rpc`, a 2 KB answer) every second. What counts is how long each question
//! takes and how fast the download goes, with the sender's pace
//! ([`crate::pace`]) and without it (as before; also what an old agent does).
//! The numbers are printed (`--no-capture`) for the record.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::channel::{PathOptions, Session};
use crate::driver::{Channel, ChannelOptions, Clock, DirectPipe, Incoming, StreamEvent};
use crate::frame::Frame;
use crate::keys::DhKeypair;
use crate::mux::{Role, StreamClass};
use crate::noise::tests::binding;
use crate::noise::{Initiator, RekeyPolicy, Responder, Transport};
use crate::testing::bytes32;

/// What a websocket (or a DataChannel's SCTP) adds to each message on the wire, about.
const FRAMING: usize = 8;
/// The answer to each question.
const ANSWER: usize = 2 * 1024;

#[derive(Debug, Clone, Copy)]
struct Link {
    bits_per_s: u64,
    one_way: Duration,
}

impl Link {
    fn mbps(mbps: f64, rtt_ms: u64) -> Self {
        Self {
            bits_per_s: (mbps * 1e6) as u64,
            one_way: Duration::from_micros(rtt_ms * 500),
        }
    }

    fn wire_time(&self, bytes: usize) -> Duration {
        Duration::from_nanos(((bytes + FRAMING) as u64 * 8).saturating_mul(1_000_000_000) / self.bits_per_s)
    }
}

/// A DataChannel's backlog as its owner keeps it: what the driver handed it
/// and is not on the wire yet ([`DirectPipe::queued`]), and the most it held.
#[derive(Clone, Default)]
struct Backlog {
    queued: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

/// Messages through `link`'s bottleneck: each leaves once those before it
/// have, at the link's rate (`left`, a DataChannel's backlog, is told how
/// many bytes, when), and arrives `one_way` later.
fn bottleneck<T: Send + 'static>(
    link: Link,
    mut from: mpsc::UnboundedReceiver<T>,
    to: mpsc::UnboundedSender<T>,
    size: fn(&T) -> usize,
    left: Option<Backlog>,
    // More one-way delay (µs) than the link's, from when it is set on: a route that changed.
    extra: Arc<AtomicU64>,
) {
    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<(Instant, usize, T)>();
    let seen = left.clone();
    tokio::spawn(async move {
        let mut free = Instant::now();
        while let Some(m) = from.recv().await {
            if let Some(b) = &seen {
                // The driver counts a message in before handing it over.
                b.peak.fetch_max(b.queued.load(Ordering::Relaxed), Ordering::Relaxed);
            }
            let bytes = size(&m);
            free = free.max(Instant::now()) + link.wire_time(bytes);
            if line_tx.send((free, bytes, m)).is_err() {
                break;
            }
        }
    });
    let (air_tx, mut air_rx) = mpsc::unbounded_channel::<(Instant, T)>();
    tokio::spawn(async move {
        while let Some((at, bytes, m)) = line_rx.recv().await {
            tokio::time::sleep_until(at).await;
            if let Some(left) = &left {
                left.queued.fetch_sub(bytes, Ordering::Relaxed);
            }
            let delay = link.one_way + Duration::from_micros(extra.load(Ordering::Relaxed));
            if air_tx.send((at + delay, m)).is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Some((at, m)) = air_rx.recv().await {
            tokio::time::sleep_until(at).await;
            if to.send(m).is_err() {
                break;
            }
        }
    });
}

fn frame_size(f: &Frame) -> usize {
    f.encode().map_or(0, |b| b.len())
}

fn transports(seed: u64) -> (Transport, Transport) {
    let device = DhKeypair::from_secret(bytes32(seed));
    let agent = DhKeypair::from_secret(bytes32(seed + 1));
    let mut i = Initiator::new(&device, &agent.public(), &binding()).unwrap();
    let mut r = Responder::new(&agent, &binding()).unwrap();
    r.read_hello(&i.write_hello(0, b"").unwrap()).unwrap();
    let (m2, rt) = r.write_reply(b"", RekeyPolicy::default(), 0).unwrap();
    let (it, _) = i.read_reply(&m2, RekeyPolicy::default(), 0).unwrap();
    (it, rt)
}

/// A direct path between the two drivers over `link` each way: the
/// DataChannel's buffer is what waits at the bottleneck (on a real one, its
/// `bufferedAmount` and SCTP's queue). The agent's backlog is the third.
fn direct_pipes(link: Link, extra: Arc<AtomicU64>) -> (DirectPipe, DirectPipe, Backlog) {
    let (d_tx, d_tx_rx) = mpsc::unbounded_channel();
    let (a_tx, a_tx_rx) = mpsc::unbounded_channel();
    let (d_rx_tx, d_rx) = mpsc::unbounded_channel();
    let (a_rx_tx, a_rx) = mpsc::unbounded_channel();
    let d = Backlog::default();
    let a = Backlog::default();
    bottleneck(link, d_tx_rx, a_rx_tx, |m: &Vec<u8>| m.len(), Some(d.clone()), extra.clone());
    bottleneck(link, a_tx_rx, d_rx_tx, |m: &Vec<u8>| m.len(), Some(a.clone()), extra);
    (
        DirectPipe {
            tx: d_tx,
            rx: d_rx,
            queued: d.queued,
        },
        DirectPipe {
            tx: a_tx,
            rx: a_rx,
            queued: a.queued.clone(),
        },
        a,
    )
}

#[derive(Debug, Clone, Copy)]
struct Run {
    link: Link,
    bulk: usize,
    /// How many downloads share it, at once (each `bulk / downloads`).
    downloads: usize,
    /// Each question's body, which the agent reads whole before it answers.
    body: usize,
    /// What the page uploads at the same time (a blob stream the agent reads whole; 0: nothing).
    upload: usize,
    /// Which ends pace (an old agent or page does not).
    agent_paces: bool,
    device_paces: bool,
    /// Carry the channel over a direct path on the same link instead of the hub.
    direct: bool,
    /// A terminal's output from the agent the whole time: so many bytes every so many ms (0: none).
    term_bytes: usize,
    term_every_ms: u64,
    /// At so many ms, the round trip becomes so many ms (a route that changed).
    rtt_change: Option<(u64, u64)>,
}

#[derive(Debug)]
struct Report {
    /// Each question's time to its whole answer, sorted.
    rpc_ms: Vec<f64>,
    /// The download's rate, the upload's (if any), and the link's.
    mbps: f64,
    up_mbps: Option<f64>,
    link_mbps: f64,
    /// The agent's pace as the download ended.
    cap: Option<u64>,
    /// Over a direct path, the most the agent's DataChannel held unsent.
    dc_peak: Option<usize>,
    /// Whether the page's pace deferred to the window at any time (looked at every 100 ms).
    page_deferred: bool,
}

impl Report {
    fn pct(&self, p: f64) -> f64 {
        if self.rpc_ms.is_empty() {
            return f64::NAN;
        }
        let i = ((self.rpc_ms.len() as f64 * p).ceil() as usize).clamp(1, self.rpc_ms.len()) - 1;
        self.rpc_ms[i]
    }

    fn line(&self, name: &str) -> String {
        format!(
            "{name:<28} rpc n={:<3} p50={:>6.0} ms  p95={:>6.0} ms  max={:>6.0} ms   bulk {:>6.2} Mbps ({:>5.1}% of the link){}   cap at the end {}{}",
            self.rpc_ms.len(),
            self.pct(0.5),
            self.pct(0.95),
            self.rpc_ms.last().copied().unwrap_or(0.0),
            self.mbps,
            100.0 * self.mbps / self.link_mbps,
            self.up_mbps.map_or(String::new(), |u| format!("   up {u:>6.2} Mbps ({:>5.1}%)", 100.0 * u / self.link_mbps)),
            self.cap.map_or("-".into(), |c| format!("{} KiB", c / 1024)),
            self.dc_peak.map_or(String::new(), |p| format!("   DataChannel backlog at most {} KiB", p / 1024)),
        )
    }
}

async fn run(r: Run) -> Report {
    let mut dc_peak: Option<Arc<AtomicUsize>> = None;
    let base = Instant::now();
    let clock: Clock = Arc::new(move || 1_000_000_000 + base.elapsed().as_micros() as u64);
    let (it, rt) = transports(1);
    let now = clock();
    let device = Session::with_paths(1, it, Role::Initiator, PathOptions { probe_relay: true }, now);
    let agent = Session::with_paths(1, rt, Role::Responder, PathOptions::default(), now);
    let (d_out, d_out_rx) = mpsc::unbounded_channel();
    let (a_out, a_out_rx) = mpsc::unbounded_channel();
    let (d_in, d_in_rx) = mpsc::unbounded_channel();
    let (a_in, a_in_rx) = mpsc::unbounded_channel();
    let extra = Arc::new(AtomicU64::new(0));
    bottleneck(r.link, d_out_rx, a_in, frame_size, None, extra.clone());
    bottleneck(r.link, a_out_rx, d_in, frame_size, None, extra.clone());
    if let Some((at_ms, rtt_ms)) = r.rtt_change {
        let base_rtt_us = r.link.one_way.as_micros() as u64 * 2;
        let extra = extra.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(at_ms)).await;
            extra.store((rtt_ms * 1000).saturating_sub(base_rtt_us) / 2, Ordering::Relaxed);
        });
    }
    let opts = |pace| ChannelOptions {
        pace,
        clock: Some(clock.clone()),
    };
    let (device, _d_acc) = Channel::spawn_with(device, d_in_rx, d_out, opts(r.device_paces));
    let (agent, mut a_acc) = Channel::spawn_with(agent, a_in_rx, a_out, opts(r.agent_paces));
    if r.direct {
        let (d_pipe, a_pipe, backlog) = direct_pipes(r.link, extra.clone());
        dc_peak = Some(backlog.peak);
        let (dt, at) = transports(10);
        agent.attach_direct(at, a_pipe).unwrap();
        device.attach_direct(dt, d_pipe).unwrap();
        let mut stats = device.path_stats();
        while !stats.borrow().direct {
            stats.changed().await.unwrap();
        }
    }

    // The agent: a download for a blob stream, a small answer for an rpc one.
    let bulk = r.bulk / r.downloads.max(1);
    let body = r.body;
    let upload = r.upload;
    let (term_bytes, term_every_ms) = (r.term_bytes, r.term_every_ms);
    let uploaded: Arc<std::sync::Mutex<Option<Instant>>> = Arc::default();
    let up_done = uploaded.clone();
    tokio::spawn(async move {
        while let Some(Incoming { class, meta, mut stream }) = a_acc.recv().await {
            let up_done = up_done.clone();
            tokio::spawn(async move {
                if class == StreamClass::Blob && meta.starts_with(b"POST") {
                    // An upload: read it whole, then answer.
                    let mut read = 0;
                    while read < upload {
                        match stream.recv().await {
                            Some(StreamEvent::Data(d)) => read += d.len(),
                            _ => return,
                        }
                    }
                    *up_done.lock().unwrap() = Some(Instant::now());
                    let _ = stream.send(b"ok").await;
                    stream.close(0);
                    while let Some(event) = stream.recv().await {
                        if matches!(event, StreamEvent::Closed(_)) {
                            break;
                        }
                    }
                    return;
                }
                if class == StreamClass::Term {
                    // A terminal's output, until the page closes it.
                    let chunk = vec![b'x'; term_bytes];
                    loop {
                        tokio::time::sleep(Duration::from_millis(term_every_ms)).await;
                        if stream.send(&chunk).await.is_err() {
                            return;
                        }
                    }
                }
                if class == StreamClass::Rpc {
                    let mut read = 0;
                    while read < body {
                        match stream.recv().await {
                            Some(StreamEvent::Data(d)) => read += d.len(),
                            _ => return,
                        }
                    }
                }
                let mut left = if class == StreamClass::Blob { bulk } else { ANSWER };
                let chunk = vec![0x5a; 64 * 1024];
                while left > 0 {
                    let n = left.min(chunk.len());
                    if stream.send(&chunk[..n]).await.is_err() {
                        return;
                    }
                    left -= n;
                }
                stream.close(0);
                while let Some(event) = stream.recv().await {
                    if matches!(event, StreamEvent::Closed(_)) {
                        break;
                    }
                }
            });
        }
    });

    // The page: the download, and a question every second while it lasts.
    let start = Instant::now();
    let mut blobs = Vec::new();
    for _ in 0..if r.bulk > 0 { r.downloads } else { 0 } {
        blobs.push(device.open(StreamClass::Blob, b"GET /api/fs/download?path=%2Ftmp%2Fbig").await.unwrap());
    }
    let term = if term_bytes > 0 {
        let mut term = device.open(StreamClass::Term, b"t-1").await.unwrap();
        let writer = term.writer();
        tokio::spawn(async move { while let Some(StreamEvent::Data(_)) = term.recv().await {} });
        Some(writer)
    } else {
        None
    };
    let deferred = Arc::new(AtomicBool::new(false));
    let watching = Arc::new(AtomicBool::new(true));
    {
        let (device, deferred, watching) = (device.clone(), deferred.clone(), watching.clone());
        tokio::spawn(async move {
            while watching.load(Ordering::Relaxed) {
                if device.pace().await.is_some_and(|p| p.deferring) {
                    deferred.store(true, Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }
    if upload > 0 {
        let up = device.open(StreamClass::Blob, b"POST /api/fs/upload?path=%2Ftmp%2Fbig").await.unwrap();
        tokio::spawn(async move {
            let mut up = up;
            let chunk = vec![0xa5; 64 * 1024];
            let mut left = upload;
            while left > 0 {
                let n = left.min(chunk.len());
                if up.send(&chunk[..n]).await.is_err() {
                    return;
                }
                left -= n;
            }
            while up.recv().await.is_some() {}
        });
    }
    let download = tokio::spawn(async move {
        for mut blob in blobs {
            let mut got = 0;
            while got < bulk {
                match blob.recv().await {
                    Some(StreamEvent::Data(d)) => got += d.len(),
                    other => panic!("{other:?} after {got} bytes"),
                }
            }
        }
        Instant::now()
    });
    let mut asks = Vec::new();
    let mut every = tokio::time::interval_at(start + Duration::from_millis(500), Duration::from_secs(1));
    let busy = |download: &tokio::task::JoinHandle<Instant>| !download.is_finished() || (upload > 0 && uploaded.lock().unwrap().is_none());
    while busy(&download) {
        every.tick().await;
        if !busy(&download) {
            break;
        }
        let device = device.clone();
        asks.push(tokio::spawn(async move {
            let asked = Instant::now();
            let mut s = device.open(StreamClass::Rpc, b"POST /api/screens").await.unwrap();
            if body > 0 {
                s.send(&vec![1; body]).await.unwrap();
            }
            let mut got = 0;
            while let Some(StreamEvent::Data(d)) = s.recv().await {
                got += d.len();
            }
            assert_eq!(got, ANSWER, "the whole answer");
            asked.elapsed().as_secs_f64() * 1000.0
        }));
    }
    let done = download.await.unwrap();
    let cap = agent.pace().await.map(|p| p.cap);
    watching.store(false, Ordering::Relaxed);
    if let Some(term) = term {
        term.close(0);
    }
    let mut rpc_ms = Vec::new();
    for ask in asks {
        rpc_ms.push(ask.await.unwrap());
    }
    rpc_ms.sort_by(f64::total_cmp);
    let secs = (done - start).as_secs_f64();
    let up_secs = uploaded.lock().unwrap().map(|at| (at - start).as_secs_f64());
    Report {
        rpc_ms,
        mbps: r.bulk as f64 * 8.0 / secs / 1e6,
        up_mbps: up_secs.map(|secs| r.upload as f64 * 8.0 / secs / 1e6),
        link_mbps: r.link.bits_per_s as f64 / 1e6,
        cap,
        dc_peak: dc_peak.map(|p| p.load(Ordering::Relaxed)),
        page_deferred: deferred.load(Ordering::Relaxed),
    }
}

const MIB: usize = 1024 * 1024;

/// 1 Mbps, a 70 ms round trip, a deep queue, a 3 MiB download and a question
/// a second: without a pace every answer waits behind the whole window of
/// bulk (seconds); with it, behind about half a second, and the download
/// keeps its speed.
#[tokio::test(start_paused = true)]
async fn on_a_slow_link_a_question_waits_behind_a_fraction_of_a_second_of_download() {
    let link = Link::mbps(1.0, 70);
    let base = Run {
        link,
        bulk: 3 * MIB,
        downloads: 1,
        body: 0,
        upload: 0,
        agent_paces: false,
        device_paces: false,
        direct: false,
        term_bytes: 0,
        term_every_ms: 0,
        rtt_change: None,
    };
    let before = run(base).await;
    let after = run(Run {
        agent_paces: true,
        device_paces: true,
        ..base
    })
    .await;
    println!("1 Mbps, 70 ms, a deep queue, 3 MiB download, an rpc (2 KB answer) every second, over the relay:");
    println!("{}", before.line("before (no pace)"));
    println!("{}", after.line("after (paced)"));
    assert!(before.pct(0.95) > 2000.0, "the problem this is about: {before:?}");
    assert!(after.pct(0.95) < 1000.0, "{after:?}");
    assert!(after.rpc_ms.last().copied().unwrap() < 1200.0, "{after:?}");
    assert!(after.mbps >= 0.85 * before.mbps, "{} against {}", after.mbps, before.mbps);
    assert!(after.mbps >= 0.9 * link.bits_per_s as f64 / 1e6, "{after:?}");
    assert_eq!(after.cap, Some(crate::pace::FLOOR), "the floor: 1 Mbps × 70 ms is 9 KB");
}

/// The same over a direct path on the same link: its own limits (a 1 MiB
/// DataChannel buffer, 4 MiB unacknowledged) let the agent's DataChannel
/// hold a megabyte — eight seconds at 1 Mbps in front of every answer and
/// acknowledgement on that path; paced, it holds about the cap.
#[tokio::test(start_paused = true)]
async fn a_slow_direct_path_is_paced_too() {
    // Four downloads at once: one stream's window alone (256 KiB) would stop short of the DataChannel's budget.
    let base = Run {
        link: Link::mbps(1.0, 70),
        bulk: 3 * MIB,
        downloads: 4,
        body: 0,
        upload: 0,
        agent_paces: false,
        device_paces: false,
        direct: true,
        term_bytes: 0,
        term_every_ms: 0,
        rtt_change: None,
    };
    let before = run(base).await;
    let after = run(Run {
        agent_paces: true,
        device_paces: true,
        ..base
    })
    .await;
    println!("1 Mbps, 70 ms, a deep queue, four downloads of 768 KiB at once, an rpc every second, over a direct path:");
    println!("{}", before.line("before (no pace)"));
    println!("{}", after.line("after (paced)"));
    assert!(before.dc_peak.unwrap() >= 900 * 1024, "{before:?}");
    let peak = after.dc_peak.unwrap() as u64;
    assert!(peak <= crate::pace::FLOOR + crate::pace::HEADROOM + 4096, "the DataChannel holds about the cap: {after:?}");
    assert!(before.pct(0.95) > 2000.0, "{before:?}");
    assert!(after.pct(0.95) < 1000.0, "{after:?}");
    assert!(after.mbps >= 0.85 * before.mbps, "{} against {}", after.mbps, before.mbps);
}

/// 50 Mbps, a 20 ms round trip: the cap grows to twice the product and the
/// download is as fast as without it.
#[tokio::test(start_paused = true)]
async fn on_a_fast_link_the_download_keeps_its_speed() {
    let link = Link::mbps(50.0, 20);
    let base = Run {
        link,
        bulk: 16 * MIB,
        downloads: 1,
        body: 0,
        upload: 0,
        agent_paces: false,
        device_paces: false,
        direct: false,
        term_bytes: 0,
        term_every_ms: 0,
        rtt_change: None,
    };
    let before = run(base).await;
    let after = run(Run {
        agent_paces: true,
        device_paces: true,
        ..base
    })
    .await;
    let short = run(Run { bulk: 2 * MIB, ..base }).await;
    let short_after = run(Run {
        bulk: 2 * MIB,
        agent_paces: true,
        device_paces: true,
        ..base
    })
    .await;
    println!("50 Mbps, 20 ms, a deep queue, an rpc every second, over the relay:");
    println!("{}", before.line("16 MiB before (no pace)"));
    println!("{}", after.line("16 MiB after (paced)"));
    println!("{}", short.line("2 MiB before (no pace)"));
    println!("{}", short_after.line("2 MiB after (paced)"));
    assert!(after.mbps >= 0.98 * before.mbps, "{} against {}", after.mbps, before.mbps);
    assert!(short_after.mbps >= 0.95 * short.mbps, "{} against {}", short_after.mbps, short.mbps);
    let cap = after.cap.unwrap();
    assert!(cap > 200 * 1024, "twice 125 KB, about: {cap}");
}

/// An agent that paces with a page that does not, and the other way round:
/// both carry the download and every answer whole. A page that does not pace
/// changes nothing for the download's answers — the agent sends them — and an
/// agent that does not gives the page nothing to answer but the probes it
/// always answered. (No pace on one end is exactly what an older version
/// does: answering a probe is the same code either way.)
#[tokio::test(start_paused = true)]
async fn an_end_that_paces_and_one_that_does_not_work_together() {
    let base = Run {
        link: Link::mbps(1.0, 70),
        bulk: 2 * MIB,
        downloads: 1,
        body: 0,
        upload: 0,
        agent_paces: true,
        device_paces: false,
        direct: false,
        term_bytes: 0,
        term_every_ms: 0,
        rtt_change: None,
    };
    let new_agent = run(base).await;
    let old_agent = run(Run {
        agent_paces: false,
        device_paces: true,
        ..base
    })
    .await;
    let new_agent_direct = run(Run { direct: true, ..base }).await;
    println!("mixed versions, 1 Mbps, 70 ms, 2 MiB:");
    println!("{}", new_agent.line("pacing agent, old page"));
    println!("{}", old_agent.line("old agent, pacing page"));
    println!("{}", new_agent_direct.line("same, direct path"));
    assert!(new_agent.pct(0.95) < 1000.0, "{new_agent:?}");
    assert!(new_agent_direct.pct(0.95) < 1000.0, "{new_agent_direct:?}");
    assert!(old_agent.mbps > 0.9, "{old_agent:?}");
    assert!(!old_agent.rpc_ms.is_empty() && !new_agent.rpc_ms.is_empty());
}

/// The side that reads a long download still writes: a credit for every
/// 128 KiB it reads on each stream, and a small request body every second.
/// Those writes are its own, and its pace must not hold them — the
/// multiplexer's credits do not count against it, and a drain for a clean
/// round trip ends with no more than a few KiB out. (A drain that waited for
/// nothing at all to be out never ended while credits kept coming faster than
/// one a round trip, and held every body until the download was over.)
/// A 200 ms round trip and one download for 15 s, which its stream's window
/// holds to about 10 Mbps — a credit every 100 ms — from an old agent and
/// from a pacing one.
#[tokio::test(start_paused = true)]
async fn a_page_reading_a_long_download_still_sends_its_small_requests_at_once() {
    let base = Run {
        link: Link::mbps(20.0, 200),
        bulk: 18 * MIB,
        downloads: 1,
        body: 100,
        upload: 0,
        agent_paces: false,
        device_paces: true,
        direct: false,
        term_bytes: 0,
        term_every_ms: 0,
        rtt_change: None,
    };
    let old_agent = run(base).await;
    let paced = run(Run { agent_paces: true, ..base }).await;
    println!("20 Mbps, 200 ms, an 18 MiB download, a POST (100 B body, read whole before the 2 KB answer) every second:");
    println!("{}", old_agent.line("old agent, pacing page"));
    println!("{}", paced.line("both pacing"));
    assert!(old_agent.rpc_ms.len() >= 14 && paced.rpc_ms.len() >= 14);
    // Behind an old agent's whole window (0.2 s of it at 20 Mbps), never behind the page's own drain.
    assert!(old_agent.rpc_ms.last().copied().unwrap() < 1_500.0, "{old_agent:?}");
    assert!(paced.rpc_ms.last().copied().unwrap() < 1_000.0, "{paced:?}");
}

/// Bulk both ways at once — a download and an upload of the same size — in
/// every pairing of an end that paces and one that does not, at 1 Mbps / 70 ms
/// and 4 Mbps / 50 ms: each direction keeps within a tenth of its speed with
/// neither end pacing.
#[tokio::test(start_paused = true)]
async fn both_ways_at_once_every_pairing_keeps_both_directions_busy() {
    for (mbps, rtt, size) in [(1.0, 70, 3 * MIB / 2), (4.0, 50, 4 * MIB)] {
        let base = Run {
            link: Link::mbps(mbps, rtt),
            bulk: size,
            downloads: 1,
            body: 0,
            upload: size,
            agent_paces: false,
            device_paces: false,
            direct: false,
            term_bytes: 0,
            term_every_ms: 0,
            rtt_change: None,
        };
        println!("{mbps} Mbps, {rtt} ms, a deep queue, a {} KiB download and upload at once, an rpc every second:", size / 1024);
        let unpaced = run(base).await;
        println!("{}", unpaced.line("neither paces"));
        for (agent_paces, device_paces, name) in [(true, true, "both pace"), (true, false, "pacing agent, old page"), (false, true, "old agent, pacing page")] {
            let r = run(Run {
                agent_paces,
                device_paces,
                ..base
            })
            .await;
            println!("{}", r.line(name));
            assert!(r.mbps >= 0.9 * unpaced.mbps, "{name}, down: {} against {}", r.mbps, unpaced.mbps);
            let (up, was) = (r.up_mbps.unwrap(), unpaced.up_mbps.unwrap());
            assert!(up >= 0.9 * was, "{name}, up: {up} against {was}");
        }
    }
}

/// The page uploads at 20 Mbps while the agent's terminal writes a little
/// output the whole time, and at 5 s the round trip goes from 20 ms to
/// 200 ms (a route that changed). The output is a trickle next to the
/// upload: it does not keep the upload's minimum round trip at the old 20 ms
/// (and its cap a tenth of what the new one needs) — the upload keeps
/// within a tenth of its speed with no output at all.
#[tokio::test(start_paused = true)]
async fn a_terminals_output_does_not_freeze_the_round_trip_an_upload_paces_by() {
    let base = Run {
        link: Link::mbps(20.0, 20),
        bulk: 0,
        downloads: 1,
        body: 0,
        upload: 16 * MIB,
        agent_paces: true,
        device_paces: true,
        direct: false,
        term_bytes: 0,
        term_every_ms: 0,
        rtt_change: Some((5_000, 200)),
    };
    println!("20 Mbps, a 16 MiB upload, the round trip 20 ms then 200 ms from 5 s, a terminal's output meanwhile:");
    let unpaced = run(Run {
        agent_paces: false,
        device_paces: false,
        ..base
    })
    .await;
    println!("{}", unpaced.line("no output, neither paces"));
    let quiet = run(base).await;
    println!("{}", quiet.line("no output"));
    for (bytes, every, name) in [(4 * 1024, 100, "output 40 KB/s"), (20 * 1024, 100, "output 200 KB/s")] {
        let r = run(Run {
            term_bytes: bytes,
            term_every_ms: every,
            ..base
        })
        .await;
        println!("{}", r.line(name));
        let (up, was) = (r.up_mbps.unwrap(), quiet.up_mbps.unwrap());
        assert!(up >= 0.9 * was, "{name}: {up} against {was} with no output");
    }
}

/// A pacing agent's small writes — 3 KB every 100 ms, each counted as
/// delivered before the next without a marker of its own — are still marked
/// every 16 KiB, so the page uploading meanwhile never takes the agent for an
/// older peer and never defers to the window: its questions keep their half
/// a second.
#[tokio::test(start_paused = true)]
async fn a_pacing_agents_small_writes_never_make_the_page_defer() {
    let r = run(Run {
        link: Link::mbps(1.0, 70),
        bulk: 0,
        downloads: 1,
        body: 0,
        upload: 3 * MIB / 2,
        agent_paces: true,
        device_paces: true,
        direct: false,
        term_bytes: 3 * 1024,
        term_every_ms: 100,
        rtt_change: None,
    })
    .await;
    println!("1 Mbps, 70 ms, a 1.5 MiB upload while the agent's terminal writes 3 KB every 100 ms:");
    println!("{}", r.line("both pace"));
    assert!(!r.page_deferred, "{r:?}");
    assert!(r.pct(0.95) < 1000.0, "{r:?}");
    assert!(r.up_mbps.unwrap() >= 0.85, "{r:?}");
}

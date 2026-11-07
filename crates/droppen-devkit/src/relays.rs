//! Relays for the end-to-end runs (`docs/relay-and-plans.md` §2): running a
//! `droppen-relay` as a child process, putting one behind toxiproxy, timing
//! relays the way a device does, and opening a channel through one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::process::{Child, Command};

use crate::{Device, Link, OpenedChannel, Result};

/// A relay this harness runs.
pub struct RelayProc {
    pub id: String,
    /// What clients dial (a toxiproxy port when it is behind one).
    pub origin: String,
    /// Where the process listens.
    pub port: u16,
    pub log: PathBuf,
    bin: PathBuf,
    env: Vec<(String, String)>,
    proc: Option<Child>,
}

impl RelayProc {
    /// A relay `id` listening on `port`, dialled at `origin`, registering
    /// with `hub` (a `ws://` origin) using `token` on its first start.
    pub fn new(bin: &Path, id: &str, port: u16, origin: &str, hub: &str, token: &str, log: PathBuf) -> RelayProc {
        let key = crate::hex(&crate::random_key());
        RelayProc {
            id: id.to_string(),
            origin: origin.to_string(),
            port,
            log,
            bin: bin.to_path_buf(),
            env: vec![
                ("DROPPEN_RELAY_HUB".into(), hub.to_string()),
                ("DROPPEN_RELAY_ORIGIN".into(), origin.to_string()),
                ("DROPPEN_RELAY_ID".into(), id.to_string()),
                ("DROPPEN_RELAY_KEY".into(), key),
                ("DROPPEN_RELAY_REGISTRATION_TOKEN".into(), token.to_string()),
                ("DROPPEN_RELAY_BIND".into(), "127.0.0.1".into()),
                ("PORT".into(), port.to_string()),
                ("DROPPEN_RELAY_DRAIN_SECS".into(), "3".into()),
                // Every frame at trace level: the canary search reads this log.
                ("RUST_LOG".into(), "trace".into()),
            ]
            .into_iter()
            // The hub-API capture (`docs/hub-api/check.mjs --traffic`), when this harness has it on.
            .chain(
                droppen_client::tap::traffic_path().map(|path| ("DROPPEN_HUB_API_TRAFFIC".to_string(), path.display().to_string())),
            )
            .collect(),
            proc: None,
        }
    }

    pub fn spawn(&mut self) -> Result<()> {
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&self.bin);
        cmd.env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdout(log_file.try_clone().map_err(|e| e.to_string())?)
            .stderr(log_file)
            .kill_on_drop(true);
        self.proc = Some(cmd.spawn().map_err(|e| format!("{}: {e}", self.bin.display()))?);
        Ok(())
    }

    /// Kill it outright (a crash).
    pub async fn kill(&mut self) {
        if let Some(mut p) = self.proc.take() {
            let _ = p.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), p.wait()).await;
        }
    }

    /// `SIGTERM`: it says `goaway`, drains and stops. Returns at once.
    pub fn terminate(&self) -> Result<()> {
        let pid = self.proc.as_ref().and_then(Child::id).ok_or("the relay is not running")?;
        std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Wait for the process to exit (after [`RelayProc::terminate`]).
    pub async fn wait_exit(&mut self, within: Duration) -> Result<()> {
        let Some(mut p) = self.proc.take() else { return Ok(()) };
        tokio::time::timeout(within, p.wait())
            .await
            .map_err(|_| format!("relay {} did not exit", self.id))?
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn running(&self) -> bool {
        self.proc.is_some()
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

/// A toxiproxy (its HTTP API) the harness may add latency with.
pub struct Toxiproxy {
    pub api: String,
}

impl Toxiproxy {
    /// Proxy `listen_port` (inside the container, published on the same
    /// port) to `upstream_port` on this machine, adding `latency_ms` each way.
    pub async fn proxy(&self, name: &str, listen_port: u16, upstream_port: u16, latency_ms: u64) -> Result<()> {
        let http = reqwest::Client::builder().no_proxy().build().map_err(|e| e.to_string())?;
        let res = http
            .post(format!("{}/proxies", self.api))
            .json(&json!({"name": name, "listen": format!("0.0.0.0:{listen_port}"), "upstream": format!("host.docker.internal:{upstream_port}"), "enabled": true}))
            .send()
            .await
            .map_err(|e| format!("toxiproxy: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("toxiproxy: creating the proxy: {}", res.status()));
        }
        for stream in ["upstream", "downstream"] {
            let res = http
                .post(format!("{}/proxies/{name}/toxics", self.api))
                .json(&json!({"name": format!("latency-{stream}"), "type": "latency", "stream": stream, "attributes": {"latency": latency_ms, "jitter": 0}}))
                .send()
                .await
                .map_err(|e| format!("toxiproxy: {e}"))?;
            if !res.status().is_success() {
                return Err(format!("toxiproxy: adding latency: {}", res.status()));
            }
        }
        Ok(())
    }
}

/// `wss://x` → `https://x`.
pub fn http_of(origin: &str) -> String {
    match origin.split_once("://") {
        Some(("ws", rest)) => format!("http://{rest}"),
        Some((_, rest)) => format!("https://{rest}"),
        None => origin.to_string(),
    }
}

/// What a device does to choose (§2.6): four `GET /v1/ping`, the first
/// dropped, the median of the rest, in ms.
pub async fn ping(origin: &str) -> Option<f64> {
    let http = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(5)).build().ok()?;
    let url = format!("{}/v1/ping", http_of(origin));
    let mut samples = Vec::new();
    for i in 0..4 {
        let t0 = Instant::now();
        let res = http.get(&url).send().await.ok()?;
        if !res.status().is_success() {
            return None;
        }
        if i > 0 {
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    Some(samples[samples.len() / 2])
}

/// Wait until the hub lists `ids` for this device's account.
pub async fn wait_listed(device: &Device, ids: &[&str], within: Duration) -> Result<Value> {
    let deadline = Instant::now() + within;
    loop {
        let listed = device.relays().await?;
        let have: Vec<&str> = listed["relays"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["id"].as_str())
            .collect();
        if ids.iter().all(|id| have.contains(id)) {
            return Ok(listed);
        }
        if Instant::now() > deadline {
            return Err(format!("the hub never listed {ids:?} (it lists {have:?})"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// A device's way onto one relay: its ticket and origin.
#[derive(Debug, Clone)]
pub struct Ticket {
    pub relay: String,
    pub origin: String,
    pub ticket: String,
}

/// Tickets for `ids` (an error names any refused).
pub async fn tickets(device: &Device, ids: &[&str]) -> Result<Vec<Ticket>> {
    let got = device.relay_tickets(ids).await?;
    if let Some(e) = got["errors"].as_array().and_then(|e| e.first()) {
        return Err(format!("no ticket for {}: {}", e["relay"], e["error"]));
    }
    Ok(got["tickets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some(Ticket {
                relay: t["relay"].as_str()?.to_string(),
                origin: t["origin"].as_str()?.to_string(),
                ticket: t["ticket"].as_str()?.to_string(),
            })
        })
        .collect())
}

/// Wait until the machine says (in the hub's presence) it is on `relays`.
pub async fn wait_attached(hub_link: &Link, machine: &str, relays: &[&str], within: Duration) -> Result<Value> {
    hub_link
        .wait_control(within, |v| {
            v["t"] == "presence"
                && v["machines"].as_array().is_some_and(|m| {
                    m.iter().any(|m| {
                        m["id"] == machine
                            && relays
                                .iter()
                                .all(|r| m["via"].as_array().is_some_and(|via| via.iter().any(|x| x["relay"] == *r)))
                    })
                })
        })
        .await
        .ok_or_else(|| format!("the machine never said it is on {relays:?}"))
}

/// Sign in on the relay of `ticket` and open a channel to `machine` there
/// (the device attested its head on the hub already).
pub async fn open(device: &mut Device, ticket: &Ticket, machine: &str) -> Result<(Link, OpenedChannel)> {
    let link = device.connect_relay(&ticket.origin, &ticket.ticket).await?;
    let opened = device.open_channel_info(&link, machine).await?;
    Ok((link, opened))
}

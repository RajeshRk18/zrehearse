//! A throwaway regtest Zebra node in Docker, and a small JSON-RPC client for it.
//!
//! We drive the `docker` CLI instead of the Docker API: it is already on every
//! machine that can run the rehearsal, and its errors are the ones users know.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Port zebrad listens on inside the container.
const RPC_PORT: u16 = 18232;
/// Label on every container we start, so leftovers are easy to find:
/// `docker ps -a --filter label=zrehearse`.
pub const LABEL: &str = "zrehearse";

pub struct Node {
    container: String,
    pub rpc_url: String,
    agent: ureq::Agent,
    keep: bool,
}

impl Node {
    /// Starts zebrad with `config` mounted as its `zebrad.toml`.
    pub fn start(image: &str, config: &Path, miner_address: &str, keep: bool) -> Result<Self> {
        let config = std::fs::canonicalize(config)
            .with_context(|| format!("resolving {}", config.display()))?;
        let container = format!("zrehearse-{}-{}", std::process::id(), unix_millis());
        let mount = format!("{}:/home/zebra/.config/zebrad.toml:ro", config.display());
        let started = docker(&[
            "run",
            "-d",
            "--name",
            &container,
            "--label",
            LABEL,
            "-p",
            &format!("127.0.0.1::{RPC_PORT}"),
            "-v",
            &mount,
            "-e",
            &format!("ZEBRA_RPC__LISTEN_ADDR=0.0.0.0:{RPC_PORT}"),
            "-e",
            "ZEBRA_RPC__ENABLE_COOKIE_AUTH=false",
            "-e",
            &format!("ZEBRA_MINING__MINER_ADDRESS={miner_address}"),
            image,
        ]);
        if let Err(e) = started {
            // `docker run` can fail after it created the container, for
            // example when the port bind fails.
            remove(&container);
            return Err(e.context(format!("starting {image}")));
        }

        // From here on, Drop removes the container even if setup fails.
        let mut node = Node {
            container,
            rpc_url: String::new(),
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(60)))
                .http_status_as_error(false)
                .build()
                .into(),
            keep,
        };
        let mapped = docker(&["port", &node.container, &RPC_PORT.to_string()])?;
        let host_port = mapped
            .lines()
            .find(|l| l.starts_with("127.0.0.1:"))
            .with_context(|| format!("no host port in `docker port` output: {mapped:?}"))?;
        node.rpc_url = format!("http://{host_port}");
        Ok(node)
    }

    /// Waits until the RPC server answers.
    pub fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.rpc("getblockcount", json!([])) {
                Ok(_) => return Ok(()),
                Err(e) if Instant::now() >= deadline => {
                    bail!("zebrad RPC not ready after {}s: {e:#}", timeout.as_secs())
                }
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    }

    pub fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut resp = self
            .agent
            .post(&self.rpc_url)
            .send_json(&body)
            .with_context(|| format!("calling {method}"))?;
        let reply: Value = resp
            .body_mut()
            .read_json()
            .with_context(|| format!("reading {method} reply"))?;
        if let Some(err) = reply.get("error").filter(|e| !e.is_null()) {
            bail!("{method} failed: {err}");
        }
        reply
            .get("result")
            .cloned()
            .with_context(|| format!("{method} reply has no result"))
    }

    pub fn height(&self) -> Result<u32> {
        let h = self.rpc("getblockcount", json!([]))?;
        h.as_u64()
            .and_then(|h| u32::try_from(h).ok())
            .context("getblockcount is not a height")
    }

    /// Mines `n` blocks with the regtest-only `generate` RPC.
    pub fn mine(&self, n: u32) -> Result<()> {
        if n > 0 {
            self.rpc("generate", json!([n]))?;
        }
        Ok(())
    }

    /// The last lines of zebrad's log, for the report when something breaks.
    pub fn logs(&self) -> String {
        Command::new("docker")
            .args(["logs", "--tail", "300", &self.container])
            .output()
            .map(|o| {
                let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                s.push_str(&String::from_utf8_lossy(&o.stderr));
                s
            })
            .unwrap_or_else(|e| format!("could not read logs: {e}"))
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if self.keep {
            eprintln!(
                "keeping container {} (RPC at {})",
                self.container, self.rpc_url
            );
            return;
        }
        remove(&self.container);
    }
}

/// Removes the container and reports a failure on stderr. A container that
/// does not exist is not a failure.
fn remove(container: &str) {
    match Command::new("docker")
        .args(["rm", "-f", container])
        .output()
    {
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            if !out.status.success() && !err.contains("No such container") {
                eprintln!(
                    "warning: could not remove container {container}: {}",
                    err.trim()
                );
            }
        }
        Err(e) => eprintln!("warning: could not run docker rm for {container}: {e}"),
    }
}

fn docker(args: &[&str]) -> Result<String> {
    let out = Command::new("docker")
        .args(args)
        .output()
        .context("running docker")?;
    if !out.status.success() {
        bail!(
            "docker {} failed: {}",
            args[0],
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

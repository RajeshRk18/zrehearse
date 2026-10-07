//! A throwaway regtest Zebra node in Docker, and a small JSON-RPC client for it.

use crate::docker::Container;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// Port zebrad listens on inside the container.
pub const RPC_PORT: u16 = 18232;

/// An error reply of the node. Code -32601 means that it has no such method.
#[derive(Debug)]
pub struct RpcError {
    method: String,
    pub code: i64,
    error: Value,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.method, self.error)
    }
}

impl std::error::Error for RpcError {}

pub struct Node {
    pub container: Container,
    pub rpc_url: String,
    agent: ureq::Agent,
}

impl Node {
    /// Starts zebrad with `config` mounted as its `zebrad.toml`. It also
    /// publishes `extra_ports`, for containers that share its network.
    pub fn start(
        run_id: &str,
        image: &str,
        config: &Path,
        miner_address: &str,
        extra_ports: &[u16],
        env: &std::collections::BTreeMap<String, String>,
        keep: bool,
    ) -> Result<Self> {
        let config = std::fs::canonicalize(config)
            .with_context(|| format!("resolving {}", config.display()))?;
        let mount = format!("{}:/home/zebra/.config/zebrad.toml:ro", config.display());
        let ports: Vec<String> = std::iter::once(&RPC_PORT)
            .chain(extra_ports)
            .map(|p| format!("127.0.0.1::{p}"))
            .collect();
        let env = crate::docker::env_args(env);
        let mut args: Vec<&str> = ports.iter().flat_map(|p| ["-p", p.as_str()]).collect();
        args.extend(env.iter().map(String::as_str));
        let rpc_listen = format!("ZEBRA_RPC__LISTEN_ADDR=0.0.0.0:{RPC_PORT}");
        let miner = format!("ZEBRA_MINING__MINER_ADDRESS={miner_address}");
        args.extend([
            "-v",
            &mount,
            "-e",
            &rpc_listen,
            "-e",
            "ZEBRA_RPC__ENABLE_COOKIE_AUTH=false",
            "-e",
            &miner,
            image,
        ]);
        let mut container = Container::run(&format!("{run_id}-zebrad"), &args, keep)
            .with_context(|| format!("starting {image}"))?;
        // zebrad can exit before Docker reports the port. The empty `rpc_url`
        // then makes `wait_ready` report the exit.
        let rpc_url = container
            .host_port(RPC_PORT)?
            .map(|hp| format!("http://{hp}"))
            .unwrap_or_default();
        container.note = format!(" (RPC at {rpc_url})");
        Ok(Node {
            container,
            rpc_url,
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(60)))
                .http_status_as_error(false)
                .build()
                .into(),
        })
    }

    /// Waits until the RPC server answers. Stops early if zebrad exits.
    pub fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.rpc("getblockcount", json!([])) {
                Ok(_) => return Ok(()),
                Err(e) if Instant::now() >= deadline => {
                    bail!("zebrad RPC not ready after {}s: {e:#}", timeout.as_secs())
                }
                Err(_) => {
                    if let (state, Some(code)) = self.container.status()? {
                        bail!("zebrad {state} with code {code} before its RPC answered");
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
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
            return Err(RpcError {
                method: method.to_string(),
                code: err["code"].as_i64().unwrap_or_default(),
                error: err.clone(),
            }
            .into());
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

    /// Mines `n` blocks to the miner address with the regtest-only `generate`
    /// RPC. Each call mines one block, so that a slow block does not reach
    /// the RPC timeout.
    pub fn mine(&self, n: u32) -> Result<()> {
        for _ in 0..n {
            self.rpc("generate", json!([1]))?;
        }
        Ok(())
    }

    /// Mines `n` blocks that pay `address`, with the regtest-only
    /// `generatetoaddress` RPC. A shielded coinbase needs a proof, which takes
    /// about 6 s on a CI runner, so each call mines one block.
    pub fn mine_to(&self, n: u32, address: &str) -> Result<()> {
        for _ in 0..n {
            self.rpc("generatetoaddress", json!([1, address]))?;
        }
        Ok(())
    }
}

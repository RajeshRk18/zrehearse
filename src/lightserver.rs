//! An optional light server (lightwalletd or Zaino) in front of the node, and
//! gRPC calls to it through grpcurl in Docker.
//!
//! The light server and grpcurl share zebrad's network namespace, so they all
//! meet on 127.0.0.1 and zrehearse creates no Docker network.

use crate::docker::{Container, docker};
use crate::node::{Node, RPC_PORT};
use crate::plan::{LightServerKind, LightServerSpec};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const GRPCURL_IMAGE: &str = "fullstorydev/grpcurl:v1.9.3";
const SERVICE: &str = "cash.z.wallet.sdk.rpc.CompactTxStreamer";
/// zcash/lightwallet-protocol v0.5.0 (commit ac7cee05), the protocol that
/// lightwalletd v0.5.4 and Zaino 0.10.1 serve. Zaino has no gRPC reflection,
/// so grpcurl needs the files.
const PROTOS: [(&str, &str); 2] = [
    ("service.proto", include_str!("../proto/service.proto")),
    (
        "compact_formats.proto",
        include_str!("../proto/compact_formats.proto"),
    ),
];

/// The gRPC port of the light server inside the container.
pub fn port(kind: LightServerKind) -> u16 {
    match kind {
        LightServerKind::Lightwalletd => 9067,
        LightServerKind::Zaino => 8137,
    }
}

pub struct LightServer {
    pub container: Container,
    /// `http://127.0.0.1:<port>` on the host, for projects.
    pub url: String,
    /// `container:<zebrad>`, the network grpcurl joins.
    network: String,
    /// `127.0.0.1:<port>` inside that network.
    target: String,
    proto_dir: PathBuf,
}

impl LightServer {
    /// Starts the light server in zebrad's network, pointed at its JSON-RPC.
    /// zebrad must publish `port(spec.kind)`.
    pub fn start(
        spec: &LightServerSpec,
        run_id: &str,
        node: &Node,
        out: &Path,
        keep: bool,
    ) -> Result<Self> {
        let proto_dir = out.join("proto");
        std::fs::create_dir_all(&proto_dir)
            .with_context(|| format!("creating {}", proto_dir.display()))?;
        for (name, text) in PROTOS {
            let path = proto_dir.join(name);
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        }
        let proto_dir = std::fs::canonicalize(&proto_dir)
            .with_context(|| format!("resolving {}", proto_dir.display()))?;

        let name = format!("{run_id}-lightserver");
        let network = format!("container:{}", node.container.name);
        let rpc = format!("127.0.0.1:{RPC_PORT}");
        let port = port(spec.kind);
        let container = match spec.kind {
            LightServerKind::Lightwalletd => {
                let server = [
                    "--no-tls-very-insecure",
                    "--grpc-bind-addr",
                    &format!("0.0.0.0:{port}"),
                    "--rpchost",
                    "127.0.0.1",
                    "--rpcport",
                    &RPC_PORT.to_string(),
                    // zebrad ignores them, but without all four lightwalletd
                    // looks for a zcash.conf.
                    "--rpcuser",
                    "zrehearse",
                    "--rpcpassword",
                    "zrehearse",
                    "--data-dir",
                    "/var/lib/lightwalletd",
                    "--log-file",
                    "/dev/stdout",
                ];
                let args = with_extras(&["--network", &network], &server, spec);
                Container::run(&name, &args, keep)
            }
            LightServerKind::Zaino => {
                // zainod needs a config file. The rest comes from ZAINO_* env.
                let config = out.join("zainod.toml");
                let mut zainod =
                    toml::Table::from_iter([("network".to_string(), "Regtest".into())]);
                crate::plan::merge(&mut zainod, &spec.config);
                std::fs::write(
                    &config,
                    toml::to_string(&zainod).context("serializing zainod.toml")?,
                )
                .with_context(|| format!("writing {}", config.display()))?;
                let config = std::fs::canonicalize(&config)
                    .with_context(|| format!("resolving {}", config.display()))?;
                let args = [
                    "--network",
                    &network,
                    "-v",
                    &format!("{}:/app/config/zainod.toml:ro", config.display()),
                    "-e",
                    &format!("ZAINO_VALIDATOR_SETTINGS__VALIDATOR_JSONRPC_LISTEN_ADDRESS={rpc}"),
                    "-e",
                    &format!("ZAINO_GRPC_SETTINGS__LISTEN_ADDRESS=0.0.0.0:{port}"),
                ];
                Container::run(&name, &with_extras(&args, &[], spec), keep)
            }
        };
        let mut container = container.with_context(|| format!("starting {}", spec.image))?;
        let url = node
            .container
            .host_port(port)?
            .map(|hp| format!("http://{hp}"))
            .unwrap_or_default();
        container.note = format!(" (gRPC at {url})");
        Ok(LightServer {
            container,
            url,
            network,
            target: format!("127.0.0.1:{port}"),
            proto_dir,
        })
    }

    /// Calls `method` with a JSON request and returns grpcurl's JSON output.
    /// A streaming method returns one JSON object per message.
    pub fn call(&self, method: &str, request: &str) -> Result<String> {
        let mount = format!("{}:/proto:ro", self.proto_dir.display());
        docker(&[
            "run",
            "--rm",
            "--network",
            &self.network,
            "-v",
            &mount,
            GRPCURL_IMAGE,
            "-plaintext",
            "-max-time",
            "30",
            "-import-path",
            "/proto",
            "-proto",
            "service.proto",
            "-d",
            request,
            &self.target,
            &format!("{SERVICE}/{method}"),
        ])
        // grpcurl spreads one error over several lines.
        .map_err(|e| {
            let text = format!("{e:#}");
            anyhow::anyhow!(
                "calling {method} on the light server: {}",
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
    }

    pub fn call_json(&self, method: &str, request: &str) -> Result<Value> {
        let out = self.call(method, request)?;
        serde_json::from_str(&out).with_context(|| format!("{method} returned {out:?}"))
    }

    /// Waits until the light server serves the block at `height`, and returns
    /// its `GetLightdInfo`. Zaino reports zebrad's height in `GetLightdInfo`
    /// before its own index has the block, so only `GetBlock` proves it.
    /// Stops early if the light server exits.
    pub fn wait_for_height(&self, height: u32, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let last = match self.call_json("GetBlock", &format!("{{\"height\":{height}}}")) {
                Ok(_) => return self.call_json("GetLightdInfo", "{}"),
                Err(e) => format!("{e:#}"),
            };
            if let (state, Some(code)) = self.container.status()? {
                bail!("light server {state} with code {code}. Last answer: {last}");
            }
            if Instant::now() >= deadline {
                bail!(
                    "light server did not serve block {height} within {}s. Last answer: {last}",
                    timeout.as_secs()
                );
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

/// The `docker run` arguments `docker`, `spec.env`, the image, `server` and
/// `spec.args`.
fn with_extras(docker: &[&str], server: &[&str], spec: &LightServerSpec) -> Vec<String> {
    let mut out: Vec<String> = docker.iter().map(|a| a.to_string()).collect();
    out.extend(crate::docker::env_args(&spec.env));
    out.push(spec.image.clone());
    out.extend(server.iter().map(|a| a.to_string()));
    out.extend(spec.args.iter().cloned());
    out
}

/// A compact block hash as RPC prints it. gRPC JSON gives the bytes in
/// base64, in internal order, and RPC shows them reversed in hex.
pub fn display_hash(base64: &str) -> Result<String> {
    let mut bytes = decode_base64(base64)?;
    bytes.reverse();
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn decode_base64(s: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for c in s.trim_end_matches('=').bytes() {
        let v = ALPHABET
            .iter()
            .position(|&a| a == c)
            .with_context(|| format!("{s:?} is not base64"))?;
        bits = (bits << 6) | v as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_hash_reverses_base64_bytes() {
        // base64 of the bytes 01 02 03 ff.
        assert_eq!(display_hash("AQID/w==").unwrap(), "ff030201");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello");
        assert!(decode_base64("a*b").is_err());
    }
}

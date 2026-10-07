//! The containers of one rehearsal, through the `docker` CLI.
//!
//! We drive the `docker` CLI instead of the Docker API. It is already on every
//! machine that can run the rehearsal, and its errors are the ones users know.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::process::Command;

/// Label on every container we start, so leftovers are easy to find with
/// `docker ps -a --filter label=zrehearse`.
const LABEL: &str = "zrehearse";

/// A name that is unique to this run. Container names start with it.
pub fn run_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("zrehearse-{}-{millis}", std::process::id())
}

/// A detached container that Drop removes, unless the run keeps it.
pub struct Container {
    pub name: String,
    keep: bool,
    /// Extra text for the message that says the container is kept.
    pub note: String,
}

impl Container {
    /// Runs `docker run -d --name <name> --label zrehearse <args>`.
    pub fn run<S: AsRef<str>>(name: &str, args: &[S], keep: bool) -> Result<Self> {
        let mut full = vec!["run", "-d", "--name", name, "--label", LABEL];
        full.extend(args.iter().map(AsRef::as_ref));
        if let Err(e) = docker(&full) {
            // `docker run` can fail after it created the container, for
            // example when the port bind fails.
            remove(name);
            return Err(e);
        }
        Ok(Container {
            name: name.to_string(),
            keep,
            note: String::new(),
        })
    }

    /// The `127.0.0.1:<port>` that Docker maps to `port` in the container.
    /// `None` when the container exited before Docker reported the port.
    pub fn host_port(&self, port: u16) -> Result<Option<String>> {
        let mapped = match docker(&["port", &self.name, &port.to_string()]) {
            Ok(mapped) => mapped,
            Err(_) if self.status()?.1.is_some() => return Ok(None),
            Err(e) => return Err(e),
        };
        let host_port = mapped
            .lines()
            .find(|l| l.starts_with("127.0.0.1:"))
            .with_context(|| format!("no host port in `docker port` output: {mapped:?}"))?;
        Ok(Some(host_port.to_string()))
    }

    /// Docker's state for the container, such as `running` or `exited`, and
    /// the exit code once it is not running.
    pub fn status(&self) -> Result<(String, Option<i64>)> {
        let out = docker(&[
            "inspect",
            "-f",
            "{{.State.Status}} {{.State.ExitCode}}",
            &self.name,
        ])?;
        let mut parts = out.split_whitespace();
        let state = parts
            .next()
            .with_context(|| format!("empty `docker inspect` output for {}", self.name))?
            .to_string();
        if state == "running" {
            return Ok((state, None));
        }
        let code = parts
            .next()
            .and_then(|c| c.parse().ok())
            .with_context(|| format!("no exit code in `docker inspect` output {out:?}"))?;
        Ok((state, Some(code)))
    }

    /// The last lines of the container's log, for the report.
    pub fn logs(&self) -> String {
        Command::new("docker")
            .args(["logs", "--tail", "300", &self.name])
            .output()
            .map(|o| {
                let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                s.push_str(&String::from_utf8_lossy(&o.stderr));
                s
            })
            .unwrap_or_else(|e| format!("could not read logs: {e}"))
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        if self.keep {
            eprintln!("keeping container {}{}", self.name, self.note);
            return;
        }
        remove(&self.name);
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

/// `-e NAME=value` pairs for `docker run`.
pub fn env_args(env: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    env.iter()
        .flat_map(|(k, v)| ["-e".to_string(), format!("{k}={v}")])
        .collect()
}

pub fn docker(args: &[&str]) -> Result<String> {
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

/// The lines of a container log that explain a failure. These are the
/// `error:` lines zebrad prints when it cannot start, panics, tracing `ERROR`
/// lines (zebrad and Zaino) and JSON lines at level error (lightwalletd).
pub fn error_lines(log: &str) -> Vec<String> {
    strip_ansi(log)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if let Some(msg) = line.strip_prefix("error: ") {
                return Some(msg.to_string());
            }
            // zebrad and Zakura log a rejected block at INFO level.
            if line.contains("panicked at") || line.contains("failed verification") {
                return Some(line.to_string());
            }
            if line.starts_with('{') {
                let v: Value = serde_json::from_str(line).ok()?;
                let level = v["level"].as_str()?;
                return matches!(level, "error" | "fatal" | "panic")
                    .then(|| v["msg"].as_str().unwrap_or(line).to_string());
            }
            // Tracing lines look like `<time> ERROR <target>: <message>`.
            let mut words = line.split_whitespace();
            let stamped = words.next().is_some_and(|w| w.contains(':'));
            if stamped && words.next() == Some("ERROR") {
                return Some(words.collect::<Vec<_>>().join(" "));
            }
            None
        })
        .collect()
}

/// Removes terminal color codes such as `\x1b[32m`, which Zaino writes.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip to the letter that ends the code.
            chars.by_ref().find(|c| c.is_ascii_alphabetic());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_lines_keeps_errors_and_panics_only() {
        let log = r#"
2026-10-06T04:02:42.926153Z  INFO zebrad::components::tracing: started
2026-10-06T04:02:43.000000Z  WARN zebra_network: no peers
2026-10-06T04:02:44.000000Z ERROR zebra_state: block rejected height=20
thread 'main' panicked at zebra-chain/src/parameters/network.rs:187:18:
error: zebrad fatal error: Configuration error: unknown field `Nu7`
                     continued text that is not an error line
  12:33:02.383  WARN zaino_chain_head_service::service: ChainHead failed to advance
  \x1b[2m12:37:18.948\x1b[0m \x1b[31mERROR\x1b[0m \x1b[1;31mzainodlib\x1b[0m: Zaino failed to start
{"app":"lightwalletd","level":"warning","msg":"Starting insecure no-TLS (plaintext) server"}
{"app":"lightwalletd","level":"error","msg":"getblock failed"}
2026-10-06T13:57:51.534853Z  INFO rpc_request: zakura_rpc::methods: submit block failed verification error=missing lockbox disbursements
"#
        .replace("\\x1b", "\x1b");
        assert_eq!(
            error_lines(&log),
            [
                "zebra_state: block rejected height=20",
                "thread 'main' panicked at zebra-chain/src/parameters/network.rs:187:18:",
                "zebrad fatal error: Configuration error: unknown field `Nu7`",
                "zainodlib: Zaino failed to start",
                "getblock failed",
                "2026-10-06T13:57:51.534853Z  INFO rpc_request: zakura_rpc::methods: submit block failed verification error=missing lockbox disbursements",
            ]
        );
    }
}

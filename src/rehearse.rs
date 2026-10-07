//! One rehearsal. Start the node, mine across the activation height, check the
//! upgrade really activated, then run each project against the node.

use crate::docker::{self, Container};
use crate::key::FundedKey;
use crate::lightserver::{self, LightServer, display_hash};
use crate::node::{Node, RpcError};
use crate::plan::{COINBASE_MATURITY, LightServerKind, PUBLIC_TEST_ADDRESS, Plan, Project};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Serialize)]
pub struct Report {
    pub plan: String,
    pub image: String,
    pub upgrade: String,
    pub activation_height: u32,
    /// Consensus branch ID of the upgrade, as zebrad reports it.
    pub branch_id: Option<String>,
    pub previous: String,
    /// Consensus branch ID of the previous upgrade, as zebrad reports it.
    pub previous_branch_id: Option<String>,
    /// Receives the rewards of blocks 1 to 100. Projects get its key.
    pub funded_address: String,
    /// Receives the rewards of every later block, as shielded outputs.
    /// `None` when the node has no `generatetoaddress`, so every block paid
    /// the transparent key.
    pub shielded_address: Option<String>,
    pub checks: Vec<Check>,
    pub projects: Vec<ProjectResult>,
    pub passed: bool,
    /// Why the rehearsal could not run. Its result then says nothing about
    /// the upgrade.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_error: Option<String>,
    pub node: Option<ContainerReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_server: Option<LightServerReport>,
}

#[derive(Serialize)]
pub struct ContainerReport {
    /// Docker's state for the container at the end of the run.
    pub state: String,
    pub exit_code: Option<i64>,
    /// Error lines from the container log, collected when the run failed.
    pub errors: Vec<String>,
    pub log: String,
}

#[derive(Serialize)]
pub struct LightServerReport {
    pub kind: LightServerKind,
    pub image: String,
    pub url: String,
    #[serde(flatten)]
    pub container: ContainerReport,
}

/// The node could not be prepared for the rehearsal.
#[derive(Debug)]
struct SetupError(String);

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SetupError {}

#[derive(Serialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Serialize)]
pub struct ProjectResult {
    pub name: String,
    pub passed: bool,
    /// `None` when the project timed out or never ran.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub skipped: bool,
    pub seconds: f64,
    pub log: String,
    /// Why zrehearse could not start, watch or stop the project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn run(plan: &Plan, out: &Path, keep: bool) -> Result<Report> {
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let config = out.join("zebrad.toml");
    write(&config, plan.zebrad_toml())?;

    let key = FundedKey::generate()?;
    let mut report = Report {
        plan: plan.name.clone(),
        image: plan.node.image.clone(),
        upgrade: plan.upgrade.name.clone(),
        activation_height: plan.upgrade.height,
        branch_id: None,
        previous: plan.upgrade.previous.clone(),
        previous_branch_id: None,
        funded_address: key.address.clone(),
        shielded_address: Some(plan.funding.shielded_address().to_string()),
        checks: Vec::new(),
        projects: Vec::new(),
        passed: false,
        setup_error: None,
        node: None,
        light_server: None,
    };

    let run_id = docker::run_id();
    let light_ports: Vec<u16> = plan
        .light_server
        .iter()
        .map(|s| lightserver::port(s.kind))
        .collect();
    let node = Node::start(
        &run_id,
        &plan.node.image,
        &config,
        &key.address,
        &light_ports,
        &plan.node.env,
        keep,
    )?;
    // Declared after the node, so Drop removes it first.
    let mut light = None;
    let start_light = || {
        plan.light_server
            .as_ref()
            .map(|spec| LightServer::start(spec, &run_id, &node, out, keep))
            .transpose()
    };
    let activated = activation_checks(plan, &node, &mut light, start_light, &mut report);
    if let Err(e) = &activated {
        if e.downcast_ref::<SetupError>().is_some() {
            report.setup_error = Some(format!("{e:#}"));
        } else {
            report.checks.push(Check {
                name: "node answered every request".into(),
                passed: false,
                detail: format!("{e:#}"),
            });
        }
    }
    let activated = activated.is_ok() && report.checks.iter().all(|c| c.passed);

    for project in &plan.projects {
        let result = if activated {
            run_project(project, plan, &node, light.as_ref(), &report, &key, out)
        } else {
            // The chain never reached the planned state, so a project result
            // would say nothing about the upgrade.
            ProjectResult {
                name: project.name.clone(),
                passed: false,
                exit_code: None,
                timed_out: false,
                skipped: true,
                seconds: 0.0,
                log: String::new(),
                error: None,
            }
        };
        report.projects.push(result);
    }

    report.passed = activated && report.projects.iter().all(|p| p.passed);
    report.node = Some(container_report(
        &node.container,
        &out.join("zebrad.log"),
        report.passed,
    )?);
    if let (Some(ls), Some(spec)) = (&light, &plan.light_server) {
        report.light_server = Some(LightServerReport {
            kind: spec.kind,
            image: spec.image.clone(),
            url: ls.url.clone(),
            container: container_report(
                &ls.container,
                &out.join("lightserver.log"),
                report.passed,
            )?,
        });
    }
    write(
        &out.join("report.json"),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    Ok(report)
}

/// Writes the container's log to `path` and describes the container.
fn container_report(container: &Container, path: &Path, passed: bool) -> Result<ContainerReport> {
    let mut log = container.logs();
    if !passed {
        // `docker logs` can lag behind the container's output, and the cause
        // of a failure is often in the last lines. Read until it is stable.
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(250));
            let again = container.logs();
            if again == log {
                break;
            }
            log = again;
        }
    }
    write(path, &log)?;
    let (state, exit_code) = container.status()?;
    Ok(ContainerReport {
        state,
        exit_code,
        errors: if passed {
            Vec::new()
        } else {
            docker::error_lines(&log)
        },
        log: path.display().to_string(),
    })
}

/// Mines up to the block before activation, then across it, checking what
/// zebrad (and the light server, if any) reports at each step. An `Err` means
/// the node itself misbehaved.
fn activation_checks(
    plan: &Plan,
    node: &Node,
    light: &mut Option<LightServer>,
    start_light: impl FnOnce() -> Result<Option<LightServer>>,
    report: &mut Report,
) -> Result<()> {
    let target = &plan.upgrade.name;
    let previous = &plan.upgrade.previous;
    let height = plan.upgrade.height;
    node.wait_ready(Duration::from_secs(plan.node.ready_timeout_secs))
        .map_err(|e| SetupError(format!("{e:#}")))?;

    // An image that does not know an upgrade leaves it out of this list.
    let info = node.rpc("getblockchaininfo", json!([]))?;
    for name in [previous, target] {
        if find_upgrade(&info, name).is_none() {
            return Err(SetupError(format!(
                "image {} does not support {name}, it is not in getblockchaininfo.upgrades",
                plan.node.image
            ))
            .into());
        }
    }

    let tip = node.height()?;
    mine(node, report, (height - 1).saturating_sub(tip))?;

    // The light server starts before the boundary, so it sees the
    // activation block arrive.
    *light = start_light().map_err(|e| SetupError(format!("{e:#}")))?;
    let light_timeout = plan
        .light_server
        .as_ref()
        .map(|s| Duration::from_secs(s.ready_timeout_secs));
    let mut light_follows = true;
    if let (Some(ls), Some(timeout)) = (light.as_ref(), light_timeout) {
        let reached = ls.wait_for_height(height - 1, timeout);
        light_follows = reached.is_ok();
        let detail = match reached {
            Ok(info) => format!(
                "height {}, branch {}",
                info["blockHeight"].as_str().unwrap_or("?"),
                info["consensusBranchId"].as_str().unwrap_or("?")
            ),
            Err(e) => format!("{e:#}"),
        };
        check(
            report,
            "light server follows the chain before activation",
            light_follows,
            detail,
        );
    }

    let info = node.rpc("getblockchaininfo", json!([]))?;
    let (branch, upgrade) = find_upgrade(&info, target).context("upgrade vanished")?;
    report.branch_id = Some(branch.clone());
    let (previous_branch, _) =
        find_upgrade(&info, previous).context("previous upgrade vanished")?;
    report.previous_branch_id = Some(previous_branch.clone());
    let tip_branch = info["consensus"]["chaintip"].as_str().unwrap_or_default();
    check(
        report,
        "previous upgrade in force before activation",
        info["blocks"] == height - 1 && tip_branch == previous_branch,
        format!(
            "tip {}, chaintip {tip_branch}, expected {previous} branch {previous_branch}",
            info["blocks"]
        ),
    );
    let next = info["consensus"]["nextblock"].as_str().unwrap_or_default();
    check(
        report,
        "pending one block before activation",
        info["blocks"] == height - 1
            && upgrade["status"] == "pending"
            && upgrade["activationheight"] == height
            && next == branch,
        format!(
            "tip {}, status {}, nextblock {next}, expected branch {branch}",
            info["blocks"], upgrade["status"]
        ),
    );

    mine(node, report, 1)?;
    let info = node.rpc("getblockchaininfo", json!([]))?;
    let (_, upgrade) = find_upgrade(&info, target).context("upgrade vanished after activation")?;
    let tip_branch = info["consensus"]["chaintip"].as_str().unwrap_or_default();
    check(
        report,
        "active at the activation height",
        info["blocks"] == height
            && upgrade["status"] == "active"
            && upgrade["activationheight"] == height
            && tip_branch == branch,
        format!(
            "tip {}, status {}, chaintip {tip_branch}",
            info["blocks"], upgrade["status"]
        ),
    );

    let block = node.rpc("getblock", json!([height.to_string(), 1]));
    check(
        report,
        "activation block is readable",
        block.is_ok(),
        match &block {
            Ok(b) => format!("hash {}", b["hash"].as_str().unwrap_or("?")),
            Err(e) => format!("{e:#}"),
        },
    );

    mine(node, report, plan.upgrade.blocks_after)?;
    let want = height + plan.upgrade.blocks_after;
    let got = node.height()?;
    check(
        report,
        "chain grows after activation",
        got == want,
        format!("tip {got}, expected {want}"),
    );

    // A block reward is spendable in block `height + COINBASE_MATURITY`.
    let address = report.funded_address.clone();
    let utxos = node.rpc("getaddressutxos", json!([{ "addresses": [address] }]))?;
    let spendable = utxos
        .as_array()
        .context("getaddressutxos did not return an array")?
        .iter()
        .filter(|u| {
            u["height"]
                .as_u64()
                .is_some_and(|h| h + u64::from(COINBASE_MATURITY) <= u64::from(got) + 1)
        })
        .count();
    check(
        report,
        "funded key has a spendable block reward",
        spendable > 0,
        format!("{spendable} spendable outputs for {address} at tip {got}"),
    );

    // The first COINBASE_MATURITY blocks fund the transparent key, so only
    // later blocks pay the shielded address.
    let shielded_boundary = report.shielded_address.is_some() && height - 1 > COINBASE_MATURITY;
    if shielded_boundary {
        record(report, "shielded rewards cross the boundary", || {
            let mut detail = Vec::new();
            let mut paid = true;
            for h in [height - 1, height] {
                let pools = coinbase_pools(&node.rpc("getblock", json!([h.to_string(), 2]))?);
                paid &= !pools.is_empty();
                detail.push(format!(
                    "{h} {}",
                    if pools.is_empty() {
                        "none".into()
                    } else {
                        pools.join(" ")
                    }
                ));
            }
            Ok((paid, detail.join(", ")))
        });
    }

    if let (Some(ls), Some(timeout), true) = (light.as_ref(), light_timeout, light_follows) {
        light_server_checks(
            ls,
            node,
            report,
            [height, got],
            &branch,
            timeout,
            shielded_boundary,
        );
    }
    Ok(())
}

/// Compares what the light server serves at `heights` (activation and tip)
/// with what zebrad reports.
fn light_server_checks(
    ls: &LightServer,
    node: &Node,
    report: &mut Report,
    heights: [u32; 2],
    branch: &str,
    timeout: Duration,
    shielded: bool,
) {
    let info = match ls.wait_for_height(heights[1], timeout) {
        Ok(info) => info,
        Err(e) => {
            return check(
                report,
                "light server reaches the tip",
                false,
                format!("{e:#}"),
            );
        }
    };
    let at = info["blockHeight"].as_str().unwrap_or("?");
    check(
        report,
        "light server reaches the tip",
        true,
        format!("height {at}"),
    );
    let reported = info["consensusBranchId"].as_str().unwrap_or_default();
    check(
        report,
        "light server reports the new branch",
        reported == branch,
        format!("consensusBranchId {reported}, expected {branch}"),
    );

    record(report, "light server serves blocks past activation", || {
        let mut detail = Vec::new();
        let mut same = true;
        for h in heights {
            let block = ls.call_json("GetBlock", &format!("{{\"height\":{h}}}"))?;
            let got = display_hash(block["hash"].as_str().context("GetBlock has no hash")?)?;
            let want = node.rpc("getblock", json!([h.to_string(), 1]))?;
            let want = want["hash"].as_str().unwrap_or_default();
            same &= got == want;
            detail.push(if got == want {
                format!("{h} {got}")
            } else {
                format!("{h} {got}, zebrad {want}")
            });
        }
        Ok((same, detail.join(", ")))
    });

    if shielded {
        record(report, "light server serves the shielded rewards", || {
            let mut detail = Vec::new();
            let mut served = true;
            for h in heights {
                let block = ls.call_json("GetBlock", &format!("{{\"height\":{h}}}"))?;
                let n: usize = block["vtx"].as_array().map_or(0, |txs| {
                    txs.iter()
                        .map(|tx| {
                            ["outputs", "actions", "ironwoodActions"]
                                .iter()
                                .map(|f| tx[*f].as_array().map_or(0, Vec::len))
                                .sum::<usize>()
                        })
                        .sum()
                });
                served &= n > 0;
                detail.push(format!("{h} {n} outputs"));
            }
            Ok((served, detail.join(", ")))
        });
    }

    record(report, "light server tree states match the node", || {
        let mut detail = Vec::new();
        let mut same = true;
        for h in heights {
            let got = ls.call_json("GetTreeState", &format!("{{\"height\":{h}}}"))?;
            let want = node.rpc("z_gettreestate", json!([h.to_string()]))?;
            let mut differ = Vec::new();
            for (ours, theirs) in [("saplingTree", "sapling"), ("orchardTree", "orchard")] {
                let state = &want[theirs]["commitments"]["finalState"];
                if got[ours].as_str().unwrap_or_default() != state.as_str().unwrap_or_default() {
                    differ.push(ours);
                }
            }
            if got["hash"] != want["hash"] {
                differ.push("hash");
            }
            same &= differ.is_empty();
            detail.push(if differ.is_empty() {
                format!("{h} same")
            } else {
                format!("{h} differs in {}", differ.join(" and "))
            });
        }
        Ok((same, detail.join(", ")))
    });

    record(report, "light server streams every subtree root", || {
        let mut detail = Vec::new();
        let mut same = true;
        for pool in ["sapling", "orchard"] {
            let request =
                format!("{{\"startIndex\":0,\"shieldedProtocol\":\"{pool}\",\"maxEntries\":0}}");
            let out = ls.call("GetSubtreeRoots", &request)?;
            let got = serde_json::Deserializer::from_str(&out)
                .into_iter::<Value>()
                .collect::<Result<Vec<_>, _>>()
                .with_context(|| format!("GetSubtreeRoots returned {out:?}"))?
                .len();
            let want = node.rpc("z_getsubtreesbyindex", json!([pool, 0]))?;
            let want = want["subtrees"].as_array().map_or(0, Vec::len);
            same &= got == want;
            detail.push(format!("{pool} {got} of {want}"));
        }
        Ok((same, detail.join(", ")))
    });
}

/// Records a check whose evaluation can fail. A failure fails the check.
fn record(report: &mut Report, name: &str, eval: impl FnOnce() -> Result<(bool, String)>) {
    match eval() {
        Ok((passed, detail)) => check(report, name, passed, detail),
        Err(e) => check(report, name, false, format!("{e:#}")),
    }
}

/// Mines `n` blocks. Blocks up to COINBASE_MATURITY pay the transparent key,
/// so they are spendable when projects run. Later blocks pay the shielded
/// address, because shielded rewards need no maturity (ZIP 213).
/// A node without `generatetoaddress`, such as Zakura 1.6.0, mines every
/// block to the transparent key, and the run has no shielded funding. A plan
/// that sets its own shielded address then fails.
fn mine(node: &Node, report: &mut Report, n: u32) -> Result<()> {
    let tip = node.height()?;
    let transparent = n.min(COINBASE_MATURITY.saturating_sub(tip));
    node.mine(transparent)?;
    let rest = n - transparent;
    let Some(address) = report.shielded_address.clone() else {
        return node.mine(rest);
    };
    match node.mine_to(rest, &address) {
        Err(e)
            if e.downcast_ref::<RpcError>()
                .is_some_and(|r| r.code == -32601) =>
        {
            if address != PUBLIC_TEST_ADDRESS {
                return Err(SetupError(format!(
                    "the node has no generatetoaddress, so it cannot pay funding.shielded_address {address}"
                ))
                .into());
            }
            report.shielded_address = None;
            node.mine(rest)
        }
        other => other,
    }
}

/// The shielded pools that a block's coinbase pays, with output counts, from
/// `getblock <h> 2`. For example `["ironwood 1"]`.
fn coinbase_pools(block: &Value) -> Vec<String> {
    let tx = &block["tx"][0];
    let count = |v: &Value| v.as_array().map_or(0, Vec::len);
    [
        ("sapling", count(&tx["vShieldedOutput"])),
        ("orchard", count(&tx["orchard"]["actions"])),
        ("ironwood", count(&tx["ironwood"]["actions"])),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .map(|(pool, n)| format!("{pool} {n}"))
    .collect()
}

fn find_upgrade(info: &Value, name: &str) -> Option<(String, Value)> {
    info["upgrades"]
        .as_object()?
        .iter()
        .find(|(_, u)| u["name"] == name)
        .map(|(branch, u)| (branch.clone(), u.clone()))
}

fn check(report: &mut Report, name: &str, passed: bool, detail: String) {
    report.checks.push(Check {
        name: name.into(),
        passed,
        detail,
    });
}

fn run_project(
    project: &Project,
    plan: &Plan,
    node: &Node,
    light: Option<&LightServer>,
    report: &Report,
    key: &FundedKey,
    out: &Path,
) -> ProjectResult {
    let log_path = out.join(format!("project-{}.log", project.name));
    let started = Instant::now();
    let mut result = ProjectResult {
        name: project.name.clone(),
        passed: false,
        exit_code: None,
        timed_out: false,
        skipped: false,
        seconds: 0.0,
        log: log_path.display().to_string(),
        error: None,
    };
    let spawned = File::create(&log_path).and_then(|log| {
        let err = log.try_clone()?;
        Command::new("sh")
            .arg("-c")
            .arg(&project.run)
            .current_dir(&plan.base_dir)
            .env("ZREHEARSE_RPC_URL", &node.rpc_url)
            .env("ZREHEARSE_UPGRADE", &plan.upgrade.name)
            .env(
                "ZREHEARSE_ACTIVATION_HEIGHT",
                plan.upgrade.height.to_string(),
            )
            .env(
                "ZREHEARSE_BRANCH_ID",
                report.branch_id.as_deref().unwrap_or_default(),
            )
            .env("ZREHEARSE_PREVIOUS_UPGRADE", &plan.upgrade.previous)
            .env(
                "ZREHEARSE_PREVIOUS_BRANCH_ID",
                report.previous_branch_id.as_deref().unwrap_or_default(),
            )
            .env("ZREHEARSE_FUNDED_ADDRESS", &key.address)
            .env("ZREHEARSE_FUNDED_KEY", &key.wif)
            .envs(
                report
                    .shielded_address
                    .as_deref()
                    .map(|a| ("ZREHEARSE_SHIELDED_ADDRESS", a)),
            )
            .envs(
                report
                    .shielded_address
                    .as_ref()
                    .and(plan.funding.mnemonic())
                    .map(|m| ("ZREHEARSE_SHIELDED_MNEMONIC", m)),
            )
            .envs(light.map(|ls| ("ZREHEARSE_LIGHTWALLETD_URL", ls.url.as_str())))
            .env(
                "ZREHEARSE_TIP",
                (plan.upgrade.height + plan.upgrade.blocks_after).to_string(),
            )
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err)
            .spawn()
    });
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            result.error = Some(format!("could not start project: {e}"));
            return result;
        }
    };

    let timeout = Duration::from_secs(project.timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                result.exit_code = status.code();
                result.passed = status.success();
                break;
            }
            Ok(None) if started.elapsed() >= timeout => {
                // ponytail: kills only the `sh` process. A project that forks
                // background jobs has to clean them up itself.
                if let Err(e) = child.kill().and_then(|()| child.wait().map(drop)) {
                    result.error = Some(format!("could not stop project after timeout: {e}"));
                }
                result.timed_out = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => {
                result.error = Some(format!("lost track of project: {e}"));
                break;
            }
        }
    }
    result.seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    result
}

fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

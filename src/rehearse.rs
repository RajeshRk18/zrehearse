//! One rehearsal. Start the node, mine across the activation height, check the
//! upgrade really activated, then run each project against the node.

use crate::node::Node;
use crate::plan::{Plan, Project};
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
    pub checks: Vec<Check>,
    pub projects: Vec<ProjectResult>,
    pub passed: bool,
}

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
}

pub fn run(plan: &Plan, out: &Path, keep: bool) -> Result<Report> {
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let config = out.join("zebrad.toml");
    std::fs::write(&config, plan.zebrad_toml())?;

    let mut report = Report {
        plan: plan.name.clone(),
        image: plan.node.image.clone(),
        upgrade: plan.upgrade.name.clone(),
        activation_height: plan.upgrade.height,
        branch_id: None,
        checks: Vec::new(),
        projects: Vec::new(),
        passed: false,
    };

    let node = Node::start(&plan.node.image, &config, &plan.node.miner_address, keep)?;
    let env_ok = activation_checks(plan, &node, &mut report);
    if let Err(e) = &env_ok {
        report.checks.push(Check { name: "rehearsal setup".into(), passed: false, detail: format!("{e:#}") });
    }
    let env_ok = env_ok.is_ok() && report.checks.iter().all(|c| c.passed);

    for project in &plan.projects {
        let result = if env_ok {
            run_project(project, plan, &node, &report, out)
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
            }
        };
        report.projects.push(result);
    }

    std::fs::write(out.join("zebrad.log"), node.logs())?;
    report.passed = env_ok && report.projects.iter().all(|p| p.passed);
    std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&report)? + "\n")?;
    Ok(report)
}

/// Mines up to the block before activation, then across it, checking what
/// zebrad reports at each step. An `Err` means the node itself misbehaved.
fn activation_checks(plan: &Plan, node: &Node, report: &mut Report) -> Result<()> {
    let target = &plan.upgrade.name;
    let height = plan.upgrade.height;
    node.wait_ready(Duration::from_secs(plan.node.ready_timeout_secs))?;

    let tip = node.height()?;
    node.mine((height - 1).saturating_sub(tip))?;
    let info = node.rpc("getblockchaininfo", json!([]))?;
    let (branch, upgrade) = find_upgrade(&info, target)
        .with_context(|| format!("zebrad does not list {target} in getblockchaininfo.upgrades"))?;
    report.branch_id = Some(branch.clone());
    let next = info["consensus"]["nextblock"].as_str().unwrap_or_default();
    check(report, "pending one block before activation",
        upgrade["status"] == "pending" && upgrade["activationheight"] == height && next == branch,
        format!("tip {}, status {}, nextblock {next}, expected branch {branch}", height - 1, upgrade["status"]));

    node.mine(1)?;
    let info = node.rpc("getblockchaininfo", json!([]))?;
    let (_, upgrade) = find_upgrade(&info, target).context("upgrade vanished after activation")?;
    let tip_branch = info["consensus"]["chaintip"].as_str().unwrap_or_default();
    check(report, "active at the activation height",
        upgrade["status"] == "active" && tip_branch == branch,
        format!("tip {height}, status {}, chaintip {tip_branch}", upgrade["status"]));

    let block = node.rpc("getblock", json!([height.to_string(), 1]));
    check(report, "activation block is readable", block.is_ok(),
        match &block { Ok(b) => format!("hash {}", b["hash"].as_str().unwrap_or("?")), Err(e) => format!("{e:#}") });

    node.mine(plan.upgrade.blocks_after)?;
    let want = height + plan.upgrade.blocks_after;
    let got = node.height()?;
    check(report, "chain grows after activation", got == want, format!("tip {got}, expected {want}"));
    Ok(())
}

fn find_upgrade(info: &Value, name: &str) -> Option<(String, Value)> {
    info["upgrades"]
        .as_object()?
        .iter()
        .find(|(_, u)| u["name"] == name)
        .map(|(branch, u)| (branch.clone(), u.clone()))
}

fn check(report: &mut Report, name: &str, passed: bool, detail: String) {
    report.checks.push(Check { name: name.into(), passed, detail });
}

fn run_project(project: &Project, plan: &Plan, node: &Node, report: &Report, out: &Path) -> ProjectResult {
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
    };
    let spawned = File::create(&log_path).and_then(|log| {
        let err = log.try_clone()?;
        Command::new("sh")
            .arg("-c")
            .arg(&project.run)
            .current_dir(&plan.base_dir)
            .env("ZREHEARSE_RPC_URL", &node.rpc_url)
            .env("ZREHEARSE_UPGRADE", &plan.upgrade.name)
            .env("ZREHEARSE_ACTIVATION_HEIGHT", plan.upgrade.height.to_string())
            .env("ZREHEARSE_BRANCH_ID", report.branch_id.as_deref().unwrap_or_default())
            .env("ZREHEARSE_TIP", (plan.upgrade.height + plan.upgrade.blocks_after).to_string())
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err)
            .spawn()
    });
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::write(&log_path, format!("could not start project: {e}\n"));
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
                let _ = child.kill();
                let _ = child.wait();
                result.timed_out = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => {
                let _ = std::fs::write(&log_path, format!("lost track of project: {e}\n"));
                break;
            }
        }
    }
    result.seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    result
}

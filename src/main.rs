//! zrehearse runs network upgrade rehearsals for downstream Zcash projects.
//!
//! It starts a throwaway regtest Zebra node with one upgrade set to activate at
//! a chosen height, mines across that height, checks the upgrade really took
//! effect, and then runs your project's own tests against the node.

mod node;
mod plan;
mod rehearse;

use anyhow::{Result, bail};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: zrehearse run <plan.toml> [--out <dir>] [--keep]

  --out <dir>   where to write report.json and logs (default: out/<plan name>)
  --keep        leave the zebrad container running afterwards";

fn main() -> ExitCode {
    match real_main() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// `Ok(passed)` for a finished rehearsal, `Err` when it could not run at all.
fn real_main() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => {}
        Some("--version") => {
            println!("zrehearse {}", env!("CARGO_PKG_VERSION"));
            return Ok(true);
        }
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return Ok(true);
        }
        _ => bail!("{USAGE}"),
    }

    let (mut plan_path, mut out, mut keep) = (None, None, false);
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--keep" => keep = true,
            "--out" => out = Some(PathBuf::from(rest.next().ok_or_else(|| anyhow::anyhow!("--out needs a directory"))?)),
            flag if flag.starts_with('-') => bail!("unknown flag {flag}\n{USAGE}"),
            path if plan_path.is_none() => plan_path = Some(PathBuf::from(path)),
            extra => bail!("unexpected argument {extra}\n{USAGE}"),
        }
    }
    let plan_path = plan_path.ok_or_else(|| anyhow::anyhow!("{USAGE}"))?;
    let plan = plan::Plan::load(&plan_path)?;
    let out = out.unwrap_or_else(|| {
        let stem = plan_path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        PathBuf::from("out").join(stem)
    });

    println!("rehearsing {} at height {} on {}", plan.upgrade.name, plan.upgrade.height, plan.node.image);
    let report = rehearse::run(&plan, &out, keep)?;

    for c in &report.checks {
        println!("{}  {}  ({})", mark(c.passed), c.name, c.detail);
    }
    for p in &report.projects {
        let how = if p.skipped {
            "skipped, the upgrade did not activate cleanly".to_string()
        } else if p.timed_out {
            format!("timed out after {}s, log {}", p.seconds, p.log)
        } else {
            format!("exit {}, {}s, log {}", p.exit_code.map_or("?".into(), |c| c.to_string()), p.seconds, p.log)
        };
        println!("{}  project {}  ({how})", mark(p.passed), p.name);
    }
    println!(
        "REHEARSAL {}  report: {}",
        if report.passed { "PASSED" } else { "FAILED" },
        out.join("report.json").display()
    );
    Ok(report.passed)
}

fn mark(passed: bool) -> &'static str {
    if passed { "PASS" } else { "FAIL" }
}

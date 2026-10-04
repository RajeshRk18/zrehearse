//! Real rehearsals. They need Docker and the zfnd/zebra:6.2.3 image, so they
//! run only with `cargo test -- --ignored`.

use serde_json::Value;
use std::process::{Command, Output, Stdio};

struct Run {
    output: Output,
    stdout: String,
    report: Value,
}

/// Runs `zrehearse run <plan> --out <out>` and checks that it left no container behind.
fn rehearse(plan: &str, out: &str) -> Run {
    let root = env!("CARGO_MANIFEST_DIR");
    let out = format!("{root}/{out}");
    let _ = std::fs::remove_dir_all(&out);
    let child = Command::new(env!("CARGO_BIN_EXE_zrehearse"))
        .args(["run", &format!("{root}/{plan}"), "--out", &out])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn zrehearse");
    // Container names start with `zrehearse-<pid>-`, so this sees only this run.
    let name = format!("name=zrehearse-{}-", child.id());
    let output = child.wait_with_output().expect("wait for zrehearse");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);

    let ps = Command::new("docker").args(["ps", "-aq", "--filter", &name]).output().expect("docker ps");
    assert!(ps.status.success(), "docker ps failed");
    assert!(ps.stdout.is_empty(), "container left behind\n{stdout}{stderr}");

    let report = std::fs::read_to_string(format!("{out}/report.json"))
        .unwrap_or_else(|e| panic!("reading report.json: {e}\n{stdout}{stderr}"));
    let report = serde_json::from_str(&report).expect("report.json is JSON");
    Run { output, stdout, report }
}

#[test]
#[ignore = "needs Docker and zfnd/zebra:6.2.3"]
fn nu6_3_rehearsal_passes() {
    let run = rehearse("examples/nu6_3.toml", "out/e2e");
    assert!(run.output.status.success(), "{}", run.stdout);
    assert!(run.stdout.contains("REHEARSAL PASSED"), "{}", run.stdout);
    assert_eq!(run.report["passed"], true);
    assert_eq!(run.report["projects"][0]["name"], "rpc-smoke");
    assert_eq!(run.report["projects"][0]["passed"], true);
}

#[test]
#[ignore = "needs Docker and zfnd/zebra:6.2.3"]
fn failing_project_fails_the_rehearsal() {
    let run = rehearse("examples/failing_project.toml", "out/neg");
    assert!(!run.output.status.success(), "{}", run.stdout);
    assert!(run.stdout.contains("REHEARSAL FAILED"), "{}", run.stdout);
    assert_eq!(run.report["passed"], false);
    assert_eq!(run.report["projects"][0]["exit_code"], 3);
}

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
fn run_zrehearse(plan: &str, out: &str) -> (Output, String) {
    let _ = std::fs::remove_dir_all(out);
    let child = Command::new(env!("CARGO_BIN_EXE_zrehearse"))
        .args(["run", plan, "--out", out])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn zrehearse");
    // Container names start with `zrehearse-<pid>-`, so this sees only this run.
    let name = format!("name=zrehearse-{}-", child.id());
    let output = child.wait_with_output().expect("wait for zrehearse");
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);

    let ps = Command::new("docker")
        .args(["ps", "-aq", "--filter", &name])
        .output()
        .expect("docker ps");
    assert!(ps.status.success(), "docker ps failed");
    assert!(ps.stdout.is_empty(), "container left behind\n{text}");
    (output, text)
}

/// Rehearses an example plan and reads its report.
fn rehearse(plan: &str, out: &str) -> Run {
    let root = env!("CARGO_MANIFEST_DIR");
    let out = format!("{root}/{out}");
    let (output, stdout) = run_zrehearse(&format!("{root}/{plan}"), &out);
    let report = std::fs::read_to_string(format!("{out}/report.json"))
        .unwrap_or_else(|e| panic!("reading report.json: {e}\n{stdout}"));
    let report = serde_json::from_str(&report).expect("report.json is JSON");
    Run {
        output,
        stdout,
        report,
    }
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

#[test]
#[ignore = "needs Docker and alpine:3.20"]
fn failed_container_start_leaves_no_container() {
    // A directory where zrehearse mounts zebrad.toml makes `docker run`
    // create the container and then fail to start it.
    let image = "zrehearse-test-badmount";
    let mut build = Command::new("docker")
        .args(["build", "-q", "-t", image, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("docker build");
    std::io::Write::write_all(
        build.stdin.as_mut().unwrap(),
        b"FROM alpine:3.20\nRUN mkdir -p /home/zebra/.config/zebrad.toml\nCMD [\"sleep\", \"60\"]\n",
    )
    .unwrap();
    assert!(build.wait().unwrap().success(), "docker build failed");

    let dir = format!("{}/badmount", env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let plan = format!("{dir}/plan.toml");
    std::fs::write(
        &plan,
        format!("name = \"bad mount\"\n[node]\nimage = \"{image}\"\n[upgrade]\nname = \"NU6.3\"\nheight = 20\n"),
    )
    .unwrap();
    let (output, text) = run_zrehearse(&plan, &format!("{dir}/out"));
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(text.contains("starting zrehearse-test-badmount"), "{text}");
}

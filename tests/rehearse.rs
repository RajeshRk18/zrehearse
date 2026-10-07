//! Real rehearsals. They need Docker and the zfnd/zebra:6.2.3 and
//! zfnd/zebra:7.0.0-rc.0 images, so they run only with `cargo test -- --ignored`.

use serde_json::Value;
use std::process::{Command, Output, Stdio};

struct Run {
    output: Output,
    stdout: String,
    report: Value,
}

/// Runs `zrehearse run <plan> --out <out>` and checks that it left no container behind.
fn run_zrehearse(plan: &str, out: &str) -> (Output, String) {
    run_zrehearse_with(plan, out, &[])
}

fn run_zrehearse_with(plan: &str, out: &str, extra: &[&str]) -> (Output, String) {
    let _ = std::fs::remove_dir_all(out);
    let child = Command::new(env!("CARGO_BIN_EXE_zrehearse"))
        .args(["run", plan, "--out", out])
        .args(extra)
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
    assert_eq!(run.report["previous_branch_id"], "5437f330");
    let shielded = check(&run.report, "shielded rewards cross the boundary");
    assert_eq!(shielded["detail"], "109 orchard 1, 110 ironwood 1");
    let checks = run.report["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c["name"] == "previous upgrade in force before activation")
    );
    assert_eq!(run.report["projects"][0]["name"], "rpc-smoke");
    assert_eq!(run.report["projects"][0]["passed"], true);
    assert_eq!(run.report["projects"][1]["name"], "spend");
    assert_eq!(run.report["projects"][1]["passed"], true);
}

#[test]
#[ignore = "needs Docker and zfnd/zebra:7.0.0-rc.0"]
fn nu7_rehearsal_passes() {
    let run = rehearse("examples/nu7.toml", "out/e2e-nu7");
    assert!(run.output.status.success(), "{}", run.stdout);
    assert_eq!(run.report["branch_id"], "77190ad9");
    assert_eq!(run.report["previous_branch_id"], "37a5165b");
    assert_eq!(run.report["projects"][0]["passed"], true);
    assert_eq!(run.report["projects"][1]["name"], "spend");
    assert_eq!(run.report["projects"][1]["passed"], true);
}

#[test]
#[ignore = "needs Docker and zfnd/zebra:7.0.0-rc.0"]
fn image_flag_replaces_the_plan_image() {
    let root = env!("CARGO_MANIFEST_DIR");
    let out = format!("{root}/out/e2e-image");
    let (output, text) = run_zrehearse_with(
        &format!("{root}/examples/nu6_3.toml"),
        &out,
        &["--image", "zfnd/zebra:7.0.0-rc.0"],
    );
    assert!(output.status.success(), "{text}");
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{out}/report.json")).unwrap())
            .unwrap();
    assert_eq!(report["image"], "zfnd/zebra:7.0.0-rc.0");
    assert_eq!(report["branch_id"], "37a5165b");
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
        format!("name = \"bad mount\"\n[node]\nimage = \"{image}\"\n[upgrade]\nname = \"NU6.3\"\nprevious = \"NU6.2\"\n"),
    )
    .unwrap();
    let (output, text) = run_zrehearse(&plan, &format!("{dir}/out"));
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(text.contains("starting zrehearse-test-badmount"), "{text}");
}

/// Rehearses a plan with this `[upgrade]` table on Zebra 6.2.3, written to a temp dir.
fn rehearse_upgrade(dir: &str, upgrade: &str) -> (Output, String, Value) {
    rehearse_toml(
        dir,
        &format!("name = \"t\"\n[node]\nimage = \"zfnd/zebra:6.2.3\"\n[upgrade]\n{upgrade}"),
    )
}

/// Rehearses this plan text, written to a temp dir.
fn rehearse_toml(dir: &str, plan_text: &str) -> (Output, String, Value) {
    let dir = format!("{}/{dir}", env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let plan = format!("{dir}/plan.toml");
    std::fs::write(&plan, plan_text).unwrap();
    let out = format!("{dir}/out");
    let (output, text) = run_zrehearse(&plan, &out);
    let report = std::fs::read_to_string(format!("{out}/report.json"))
        .unwrap_or_else(|e| panic!("reading report.json: {e}\n{text}"));
    (output, text, serde_json::from_str(&report).unwrap())
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no check {name:?} in {report:#}"))
}

const LIGHT_CHECKS: [&str; 6] = [
    "light server follows the chain before activation",
    "light server reaches the tip",
    "light server reports the new branch",
    "light server serves blocks past activation",
    "light server tree states match the node",
    "light server streams every subtree root",
];

#[test]
#[ignore = "needs Docker and zfnd/zebra:6.2.3"]
fn misspelled_upgrade_reports_the_zebrad_error() {
    let (output, text, report) =
        rehearse_upgrade("misspelled", "name = \"Nu7\"\nprevious = \"NU6.3\"\n");
    assert_eq!(output.status.code(), Some(2), "{text}");
    let setup = report["setup_error"].as_str().unwrap();
    assert!(setup.contains("exited with code 1"), "{setup}");
    assert_eq!(report["node"]["state"], "exited");
    assert_eq!(report["node"]["exit_code"], 1);
    let errors = report["node"]["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("unknown field `Nu7`")),
        "{errors:?}"
    );
    assert!(text.contains("zebrad error"), "{text}");
}

#[test]
#[ignore = "needs Docker and zfnd/zebra:6.2.3"]
fn upgrade_the_image_does_not_know_is_a_setup_error() {
    let (output, text, report) =
        rehearse_upgrade("unsupported", "name = \"NU7\"\nprevious = \"NU6.3\"\n");
    assert_eq!(output.status.code(), Some(2), "{text}");
    let setup = report["setup_error"].as_str().unwrap();
    assert!(
        setup.starts_with("image zfnd/zebra:6.2.3 does not support NU7"),
        "{setup}"
    );
    assert_eq!(report["passed"], false);
}

#[test]
#[ignore = "needs Docker, zfnd/zebra:7.0.0-rc.0 and electriccoinco/lightwalletd:v0.5.4"]
fn lightwalletd_serves_nu7() {
    let run = rehearse("examples/light_server.toml", "out/e2e-lightwalletd");
    assert!(run.output.status.success(), "{}", run.stdout);
    for name in LIGHT_CHECKS {
        assert_eq!(check(&run.report, name)["passed"], true, "{name}");
    }
    assert_eq!(run.report["light_server"]["kind"], "lightwalletd");
    let served = check(&run.report, "light server serves the shielded rewards");
    assert_eq!(served["passed"], true);
    assert_eq!(run.report["projects"][0]["passed"], true);
}

#[test]
#[ignore = "needs Docker, zfnd/zebra:6.2.3 and zingodevops/zaino:0.10.1-no-tls"]
fn zaino_serves_nu6_3() {
    let (output, text, report) = rehearse_toml(
        "zaino-nu6_3",
        "name = \"t\"\n[node]\nimage = \"zfnd/zebra:6.2.3\"\n\
         [upgrade]\nname = \"NU6.3\"\nprevious = \"NU6.2\"\n\
         [light_server]\nkind = \"zaino\"\nimage = \"zingodevops/zaino:0.10.1-no-tls\"\n",
    );
    assert!(output.status.success(), "{text}");
    for name in LIGHT_CHECKS {
        assert_eq!(check(&report, name)["passed"], true, "{name}");
    }
}

/// Zaino 0.10.1 pins zcash_protocol 0.10.6, which does not know the NU7
/// branch ID, so its index stops at the block before activation.
#[test]
#[ignore = "needs Docker, zfnd/zebra:7.0.0-rc.0 and zingodevops/zaino:0.10.1-no-tls"]
fn zaino_0_10_1_stops_at_nu7() {
    let (output, text, report) = rehearse_toml(
        "zaino-nu7",
        "name = \"t\"\n[node]\nimage = \"zfnd/zebra:7.0.0-rc.0\"\n\
         [upgrade]\nname = \"NU7\"\nprevious = \"NU6.3\"\n\
         [light_server]\nkind = \"zaino\"\nimage = \"zingodevops/zaino:0.10.1-no-tls\"\n\
         ready_timeout_secs = 20\n",
    );
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert_eq!(check(&report, LIGHT_CHECKS[0])["passed"], true);
    let tip = check(&report, LIGHT_CHECKS[1]);
    assert_eq!(tip["passed"], false);
    let detail = tip["detail"].as_str().unwrap();
    assert!(detail.contains("best chain tip [109]"), "{detail}");
}

#[test]
#[ignore = "needs Docker, zakuracore/zakura:1.6.0 and electriccoinco/lightwalletd:v0.5.4"]
fn zakura_rehearsal_passes() {
    let run = rehearse("examples/zakura.toml", "out/e2e-zakura");
    assert!(run.output.status.success(), "{}", run.stdout);
    for name in LIGHT_CHECKS {
        assert_eq!(check(&run.report, name)["passed"], true, "{name}");
    }
    assert_eq!(run.report["projects"][0]["passed"], true);
    // Zakura 1.6.0 has no generatetoaddress, so the run has no shielded funding.
    assert_eq!(run.report["shielded_address"], Value::Null);
    assert!(run.stdout.contains("no shielded funding"), "{}", run.stdout);
}

/// Without a lockbox disbursement in `node.config`, Zakura rejects the NU6.1
/// activation block, and the report names the cause.
#[test]
#[ignore = "needs Docker and zakuracore/zakura:1.6.0"]
fn zakura_without_lockbox_names_the_cause() {
    let (output, text, report) = rehearse_toml(
        "zakura-no-lockbox",
        "name = \"t\"\n[node]\nimage = \"zakuracore/zakura:1.6.0\"\n\
         [upgrade]\nname = \"NU6.3\"\nprevious = \"NU6.2\"\n",
    );
    assert_eq!(output.status.code(), Some(1), "{text}");
    let errors = report["node"]["errors"].as_array().unwrap();
    assert!(
        errors.iter().any(|e| e
            .as_str()
            .unwrap()
            .contains("missing lockbox disbursements")),
        "{errors:?}"
    );
}

use serde_json::Value;
use std::process::Command;

fn invoke(args: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_resolved-updater"))
        .args(args)
        .output()
        .unwrap();
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout.iter().filter(|&&c| c == b'\n').count(), 1);
    assert_eq!(output.stdout.last(), Some(&b'\n'));
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

#[test]
fn health_reports_desktop_identity_not_package_version() {
    let (success, value) = invoke(&["--protocol-version", "1", "health"]);
    assert!(success);
    assert_eq!(
        value,
        serde_json::json!({
            "protocol_version": 1, "kind": "health", "name": "resolved-updater",
            "app_version": env!("RESOLVED_UPDATER_APP_VERSION"),
            "build_version": env!("RESOLVED_BUILD_VERSION"),
            "build_number": env!("API_TESTER_BUILD_NUMBER"),
            "os": "macos", "arch": env!("RESOLVED_UPDATER_ARCH"),
            "feed_url": "https://apiworkbench.dev/downloads.json",
            "capabilities": ["health", "download", "verify", "verify-host", "install", "recover"], "installation_enabled": false
        })
    );
    assert!(["arm64", "x64"].contains(&value["arch"].as_str().unwrap()));
}

#[test]
fn failures_are_single_structured_json_lines() {
    for args in [
        vec![],
        vec!["health"],
        vec!["--protocol-version", "2", "health"],
        vec!["--protocol-version", "1", "health", "extra"],
        vec!["--protocol-version", "1", "check"],
        vec!["--protocol-version", "1", "download"],
        vec!["--protocol-version", "1", "install"],
    ] {
        let (success, value) = invoke(&args);
        assert!(!success);
        assert_eq!(value["kind"], "error");
        assert_eq!(value["protocol_version"], 1);
        assert_eq!(value["installation_enabled"], false);
        assert!(value["error"]["code"].is_string());
    }
}

#[test]
fn non_utf8_argument_is_a_structured_failure() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let output = Command::new(env!("CARGO_BIN_EXE_resolved-updater"))
        .args(["--protocol-version", "1", "health"])
        .arg(OsString::from_vec(vec![0xff]))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout.iter().filter(|&&c| c == b'\n').count(), 1);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["kind"], "error");
    assert_eq!(value["error"]["code"], "invalid_utf8");
}

#[test]
fn artifact_json_is_required_bounded_and_not_echoed() {
    for json in ["{secret-query", "null", "{}", &"x".repeat(16 * 1024 + 1)] {
        let (success, value) = invoke(&[
            "--protocol-version",
            "1",
            "download",
            "--artifact-json",
            json,
        ]);
        assert!(!success);
        assert_eq!(value["error"]["code"], "invalid_arguments");
        assert!(!value.to_string().contains("secret-query"));
    }
    let (success, _) = invoke(&[
        "--protocol-version",
        "1",
        "download",
        "--version",
        "2.0",
        "--sha256",
        &"a".repeat(64),
    ]);
    assert!(!success);
}

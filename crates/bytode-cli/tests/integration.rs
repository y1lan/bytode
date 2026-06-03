use std::process::Command;

#[test]
fn test_cli_help() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("bytode"), "help should mention bytode");
    assert!(stdout.contains("--project"), "should have --project flag");
    assert!(
        stdout.contains("--api-key"),
        "should mention --api-key option"
    );
}

#[test]
fn test_version_info() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Terminal coding agent"),
        "should describe itself"
    );
}

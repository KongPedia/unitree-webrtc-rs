// Scoped tracing subscribers still share static callsite interest with other test threads.
// Run log-capture fixtures alone rather than serializing the whole test suite.
pub(crate) fn run_isolated(test_name: &str) -> bool {
    const FIXTURE_ENV: &str = "UNITREE_ISOLATED_TEST_FIXTURE";
    if std::env::var(FIXTURE_ENV).ok().as_deref() == Some(test_name) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture"])
        .env(FIXTURE_ENV, test_name)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "isolated test {test_name} failed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed;"),
        "isolated test {test_name} did not run: {stdout}"
    );
    true
}

//! Size budget for the shipped binary (AGENTS.md). Runs only in release test builds, after `cargo build --release -p kite`.
#[cfg(not(debug_assertions))]
#[test]
fn release_binary_within_budget() {
    let exe = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/release/kite",
        env!("EXE_SUFFIX_FOR_BUDGET")
    );
    let size = std::fs::metadata(exe)
        .unwrap_or_else(|e| panic!("build the release binary first ({exe}): {e}"))
        .len();
    assert!(
        size <= 20 * 1024 * 1024,
        "kite binary is {:.1} MB, budget 20 MB",
        size as f64 / 1048576.
    );
}

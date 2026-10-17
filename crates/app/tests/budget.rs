//! Size budget for the shipped binary (AGENTS.md). Runs only in release test builds, after `cargo build --release -p kite`.
#[cfg(not(debug_assertions))]
#[test]
fn release_binary_within_budget() {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let exe = target
        .join("release")
        .join(concat!("kite", env!("EXE_SUFFIX_FOR_BUDGET")));
    let size = std::fs::metadata(&exe)
        .unwrap_or_else(|e| panic!("build the release binary first ({}): {e}", exe.display()))
        .len();
    assert!(
        size <= 20 * 1024 * 1024,
        "kite binary is {:.1} MB, budget 20 MB",
        size as f64 / 1048576.
    );
}

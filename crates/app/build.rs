fn main() {
    let suffix = if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        ".exe"
    } else {
        ""
    };
    println!("cargo:rustc-env=EXE_SUFFIX_FOR_BUDGET={suffix}");
}

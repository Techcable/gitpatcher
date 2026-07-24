pub fn main() {
    // does not need to rerun if source files change
    println!("cargo::rerun-if-changed=build.rs");

    println!("cargo::rustc-check-cfg=cfg(use_error_backtrace)");

    let version = rustversion_detect::detect_version().expect("failed to detect version");
    // enabling the backtrace feature requires nightly rust
    if version.is_nightly() && has_feature_enabled("backtrace") {
        println!("cargo::rustc-cfg=use_error_backtrace");
    }
}

fn has_feature_enabled(name: &str) -> bool {
    std::env::var_os(format!(
        "CARGO_FEATURE_{}",
        name.replace('-', "_").to_uppercase()
    ))
    .is_some()
}

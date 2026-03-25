fn main() {
    // Declare `cfg(coverage)` as an expected configuration key.
    // cargo-llvm-cov sets `--cfg coverage` when running coverage collection;
    // without this declaration the Rust compiler emits an `unexpected_cfgs` lint
    // (which the workspace escalates to an error via `warnings = "deny"`).
    println!("cargo::rustc-check-cfg=cfg(coverage)");
}

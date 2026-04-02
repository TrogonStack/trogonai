use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // Cargo.lock lives at the workspace root, two levels above this crate.
    let workspace_root = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let lockfile = workspace_root.join("Cargo.lock");

    // Re-run if the lockfile changes (e.g. wasmtime version bump).
    println!("cargo:rerun-if-changed={}", lockfile.display());

    let content = std::fs::read_to_string(&lockfile)
        .expect("Could not read Cargo.lock — is the workspace root correct?");

    let version = extract_version(&content, "wasmtime")
        .expect("Could not find 'wasmtime' package in Cargo.lock");

    println!("cargo:rustc-env=WASMTIME_VERSION={version}");
}

/// Extracts the version of `package` from a Cargo.lock v4 file.
/// Returns the first match (the canonical package, not transitive aliases).
fn extract_version(lockfile: &str, package: &str) -> Option<String> {
    let name_line = format!("name = \"{package}\"");
    let mut in_block = false;
    for line in lockfile.lines() {
        if line == "[[package]]" {
            in_block = false;
        }
        if line == name_line {
            in_block = true;
        }
        if in_block && line.starts_with("version = ") {
            let v = line
                .trim_start_matches("version = ")
                .trim_matches('"')
                .to_string();
            return Some(v);
        }
    }
    None
}

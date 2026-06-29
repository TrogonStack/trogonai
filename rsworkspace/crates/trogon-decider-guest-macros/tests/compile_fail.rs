#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn ui() {
    // `export_decider!` embeds `wit_bindgen::generate!({ path: "../../crates/trogon-decider-wit/wit" })`,
    // resolved relative to the crate being compiled. trybuild compiles each UI test from a
    // generated crate under `<target>/tests/trybuild/`, where that relative path would not
    // resolve — the WIT-read failure would then mask the very compile errors these tests
    // assert (e.g. the bundle shared-event bound). Provision the contract WIT at the
    // location trybuild's generated crates resolve against so the macro expands fully.
    provision_trybuild_wit();

    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}

fn provision_trybuild_wit() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wit_src = manifest.join("../trogon-decider-wit/wit/world.wit");
    // trybuild stages each generated crate under `<target>/tests/trybuild/<crate>/`, where the
    // macro's `wit_bindgen::generate!({ path: "../../crates/trogon-decider-wit/wit" })` resolves
    // to `<target>/tests/crates/trogon-decider-wit/wit`. Derive `<target>` from Cargo rather than
    // assuming the default tree so a custom CARGO_TARGET_DIR stages the WIT where trybuild looks.
    let dst_dir = cargo_target_dir().join("tests/crates/trogon-decider-wit/wit");
    fs::create_dir_all(&dst_dir).expect("create trybuild WIT dir");
    fs::copy(&wit_src, dst_dir.join("world.wit")).expect("copy world.wit into trybuild tree");
}

/// Resolve Cargo's target directory the same way trybuild does, so the staged WIT lands under the
/// root trybuild actually uses (it queries `cargo metadata`, which already honors CARGO_TARGET_DIR
/// and `build.target-dir`). Falls back to the default workspace tree only if the query fails.
fn cargo_target_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let metadata = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(manifest)
        .output();

    if let Ok(output) = metadata
        && output.status.success()
        && let Some(dir) = parse_target_directory(&output.stdout)
    {
        return dir;
    }

    manifest.join("../../target")
}

fn parse_target_directory(metadata: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(metadata).ok()?;
    let key = "\"target_directory\":\"";
    let start = text.find(key)? + key.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    Some(PathBuf::from(&rest[..end]))
}

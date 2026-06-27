#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;

#[test]
fn ui() {
    // `export_decider!` embeds `wit_bindgen::generate!({ path: "../trogon-decider-wit/wit" })`,
    // resolved relative to the crate being compiled. trybuild compiles each UI test from a
    // generated crate under `<target>/tests/trybuild/`, where that relative path would not
    // resolve — the WIT-read failure would then mask the very compile errors these tests
    // assert (e.g. the bundle shared-event bound). Provision the contract WIT at the sibling
    // location trybuild's generated crates resolve against so the macro expands fully.
    provision_trybuild_wit();

    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}

fn provision_trybuild_wit() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wit_src = manifest.join("../trogon-decider-wit/wit/world.wit");
    // trybuild resolves `../trogon-decider-wit/wit` from `<target>/tests/trybuild/<crate>/`.
    let dst_dir = manifest.join("../../target/tests/trybuild/trogon-decider-wit/wit");
    fs::create_dir_all(&dst_dir).expect("create trybuild WIT dir");
    fs::copy(&wit_src, dst_dir.join("world.wit")).expect("copy world.wit into trybuild tree");
}

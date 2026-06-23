use super::*;

#[test]
fn retains_path_identity() {
    let p = WasmBundlePath::new("/tmp/bundles");
    assert_eq!(p.as_path().as_os_str(), "/tmp/bundles");
}

#[test]
fn display_is_path_repr() {
    let p = WasmBundlePath::new("/x/y");
    assert!(format!("{p}").contains("x"));
}

#[test]
fn skill_wasm_path_appends_slug() {
    let p = WasmBundlePath::new("/b");
    let got = p.join_skill_wasm(&SkillId::new("risk").expect("valid"));
    assert_eq!(got.as_os_str(), "/b/risk.wasm");
}

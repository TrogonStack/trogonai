use super::demo_manifest;

#[test]
fn demo_manifest_builds_successfully() {
    let manifest = demo_manifest().unwrap();
    assert_eq!(manifest.entries().len(), 2);
    assert!(manifest.host().is_some());
}

#[test]
fn demo_manifest_wire_serializes_to_json() {
    let manifest = demo_manifest().unwrap();
    let wire = manifest.into_wire();
    let json = serde_json::to_string_pretty(&wire).unwrap();
    assert!(json.contains("Trogon ARD Demo Registry"));
    assert!(json.contains("urn:air:trogon.ai:mcp:weather"));
}

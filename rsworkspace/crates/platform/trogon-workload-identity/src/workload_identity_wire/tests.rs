use super::*;
use crate::workload_response_wire::WorkloadResponseWire;

#[test]
fn raw_workload_debug_does_not_disclose_identity_or_key_material() {
    let identity = WorkloadIdentityWire {
        spiffe_id: "spiffe://canary.example/private-selector".into(),
        x509_svid: Vec::new(),
        x509_svid_key: b"canary-not-a-real-key".to_vec(),
        bundle: Vec::new(),
        hint: "private-hint".into(),
    };
    assert_eq!(format!("{identity:?}"), "WorkloadIdentityWire { <redacted> }");
    let response = WorkloadResponseWire {
        svids: vec![identity],
        ..Default::default()
    };
    assert_eq!(format!("{response:?}"), "WorkloadResponseWire { <redacted> }");
}

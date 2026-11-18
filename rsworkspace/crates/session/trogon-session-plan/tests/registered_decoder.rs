#![allow(clippy::expect_used, clippy::unwrap_used)]

use base64::{Engine, engine::general_purpose::STANDARD};
use buffa::Message;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use trogon_session_plan::RegisteredPlanDecoder;
use trogonai_proto::session_contracts::trogonai::{agents::agents::v1alpha1 as agent, session::{plans::v1alpha1 as plan, sessions::v1alpha1 as session}};

fn digest(value: &[u8]) -> serde_json::Value {
    json!({"algorithm":"sha256", "value":STANDARD.encode(Sha256::digest(value))})
}

fn contract() -> agent::ConfigurationContractRef {
    serde_json::from_value(json!({
        "messageType":"trogonai.session.plans.v1alpha1.SessionExecutionPlan",
        "version":"1", "definitionDigest":digest(b"registered-plan-contract")
    })).expect("contract fixture")
}

fn valid_plan() -> plan::SessionExecutionPlan {
    let native = json!({
        "contract":{"messageType":"google.protobuf.Empty", "version":"1", "definitionDigest":digest(b"empty-settings-contract")},
        "configurationBytes":"", "configurationDigest":digest(b"")
    });
    let configuration = json!({"configurationDigest":digest(b"original-configuration")});
    serde_json::from_value(json!({
        "contractVersion":1, "contract":contract(), "sessionId":"parent-example", "projectId":{"value":"project-example"},
        "agentRevision":{"agentId":"agent-example", "revisionNumber":"1", "configuration":configuration},
        "effectiveConfiguration":configuration,
        "implementation":{"registeredExtension":{"extensionId":"example-runtime", "version":"1"}, "definitionDigest":digest(b"runtime-definition")},
        "runtimeSettings":native, "effectiveRuntimeConfiguration":native,
        "variables":{"values":{}}, "modelAccess":{"noManagedAccess":{}}, "compaction":{"noPlatformCompaction":{}},
        "executionContract":{"messageType":"example.Runtime", "version":"1", "definitionDigest":digest(b"execution-contract")},
        "workingDirectory":{"checkout":{"checkoutId":"checkout-example", "uri":"git+ssh://example.test/project", "revision":"revision-example"}, "relativePath":"", "filesystemContract":{"messageType":"example.Filesystem", "version":"1", "definitionDigest":digest(b"filesystem-contract")}}
    })).expect("plan fixture")
}

#[test]
fn exact_registered_plan_is_decoded_without_reconstructing_its_original_bytes() {
    let mut original = valid_plan().encode_to_vec();
    // An alternative field order is still the identity-bearing original artifact.
    assert_eq!(&original[..2], &[8, 1]);
    original.drain(..2);
    original.extend_from_slice(&[8, 1]);
    let stored = session::StoredSessionExecutionPlan {
        plan_digest: session::Digest { algorithm:"sha256".into(), value:Sha256::digest(&original).to_vec() }.into(),
        plan_bytes:original.clone(),
    };
    let decoder = RegisteredPlanDecoder::new(contract()).expect("registered contract");
    let decoded = decoder.decode(&stored).expect("registered valid plan must be supported");
    assert_eq!(decoded.value().session_id.as_deref(), Some("parent-example"));
    assert_eq!(decoded.original_bytes(), original);
}

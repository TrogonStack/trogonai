use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use trogon_session_plan::RegisteredPlanDecoder;
use trogonai_proto::session_contracts::trogonai::{
    agents::agents::v1alpha1 as agent,
    session::{plans::v1alpha1 as plan, sessions::v1alpha1 as session},
};

pub fn digest(value: &[u8]) -> serde_json::Value {
    json!({"algorithm":"sha256", "value":STANDARD.encode(Sha256::digest(value))})
}

pub fn contract() -> agent::ConfigurationContractRef {
    RegisteredPlanDecoder::compiled_contract()
}

pub fn valid_plan() -> plan::SessionExecutionPlan {
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

pub fn stored_bytes(bytes: Vec<u8>) -> session::StoredSessionExecutionPlan {
    session::StoredSessionExecutionPlan {
        plan_digest: session::Digest {
            algorithm: "sha256".into(),
            value: Sha256::digest(&bytes).to_vec(),
        }
        .into(),
        plan_bytes: bytes,
    }
}

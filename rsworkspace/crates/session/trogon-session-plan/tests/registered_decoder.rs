#![allow(clippy::expect_used, clippy::unwrap_used)]

use buffa::Message;
use sha2::{Digest as _, Sha256};
mod support;
use support::{contract, stored_bytes, valid_plan};
use trogon_session_plan::wire::plan;
use trogon_session_plan::{PlanError, RegisteredPlanDecoder};
use trogonai_proto::session_contracts::trogonai::{
    agents::agents::v1alpha1 as agent, session::sessions::v1alpha1 as session,
};

#[test]
fn exact_registered_plan_is_decoded_without_reconstructing_its_original_bytes() {
    let mut original = valid_plan().encode_to_vec();
    // An alternative field order is still the identity-bearing original artifact.
    assert_eq!(&original[..2], &[8, 1]);
    original.drain(..2);
    original.extend_from_slice(&[8, 1]);
    let stored = session::StoredSessionExecutionPlan {
        plan_digest: session::Digest {
            algorithm: "sha256".into(),
            value: Sha256::digest(&original).to_vec(),
        }
        .into(),
        plan_bytes: original.clone(),
    };
    let decoder = RegisteredPlanDecoder::new(contract()).expect("registered contract");
    let decoded = decoder
        .decode(&stored)
        .expect("registered valid plan must be supported");
    assert_eq!(decoded.value().session_id.as_deref(), Some("parent-example"));
    assert_eq!(decoded.original_bytes(), original);
}

#[test]
fn altered_original_bytes_and_unknown_plan_contracts_fail_closed() {
    let decoder = RegisteredPlanDecoder::new(contract()).unwrap();
    let mut stored = stored_bytes(valid_plan().encode_to_vec());
    stored.plan_bytes.push(0);
    assert_eq!(decoder.decode(&stored).unwrap_err(), PlanError::DigestMismatch);
    let mut value = valid_plan();
    value.contract.get_or_insert_default().version = "2".into();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::UnsupportedContract
    );
    let mut foreign = contract();
    foreign.definition_digest.get_or_insert_default().value[0] ^= 1;
    assert!(matches!(
        RegisteredPlanDecoder::new(foreign),
        Err(PlanError::UnsupportedContract)
    ));
    let mut registered = contract();
    registered.version = "2".into();
    assert!(matches!(
        RegisteredPlanDecoder::new(registered),
        Err(PlanError::UnsupportedContract)
    ));
}

#[test]
fn unknown_fields_duplicate_singular_fields_and_missing_capabilities_are_refused() {
    let decoder = RegisteredPlanDecoder::new(contract()).unwrap();
    for extension in [vec![0xf8, 0x7f, 1], vec![8, 1]] {
        let mut bytes = valid_plan().encode_to_vec();
        bytes.extend(extension);
        assert_eq!(decoder.decode(&stored_bytes(bytes)).unwrap_err(), PlanError::Wire);
    }
    let mut value = valid_plan();
    value.compaction = Default::default();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
}

#[test]
fn native_digest_directory_and_unpinned_platform_harness_are_refused() {
    let decoder = RegisteredPlanDecoder::new(contract()).unwrap();
    let mut value = valid_plan();
    value
        .runtime_settings
        .get_or_insert_default()
        .configuration_bytes
        .push(1);
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::DigestMismatch
    );
    let mut value = valid_plan();
    value.working_directory.get_or_insert_default().relative_path = Some("../other".into());
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
    let mut value = valid_plan();
    value.implementation.get_or_insert_default().implementation =
        Some(agent::agent_implementation::Implementation::PlatformHarness(Box::new(
            agent::PlatformHarnessVersionRef { version: "1".into() },
        )));
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::UnsupportedRuntime
    );
    let mut value = valid_plan();
    value.implementation.get_or_insert_default().implementation =
        Some(agent::agent_implementation::Implementation::RegisteredExtension(
            Box::new(agent::RegisteredExtensionVersionRef {
                extension_id: "example-runtime".into(),
                version: String::new(),
            }),
        ));
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
}

#[test]
fn variable_artifacts_obey_original_byte_and_total_value_budgets() {
    let decoder = RegisteredPlanDecoder::new(contract()).unwrap();
    let mut value = valid_plan();
    let entries = serde_json::Map::from_iter(
        (0..5).map(|index| (format!("name{index}"), serde_json::Value::String("x".repeat(4096)))),
    );
    value.variables = serde_json::from_value::<agent::VariableBindings>(serde_json::json!({"values":entries}))
        .unwrap()
        .into();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::Limit
    );
    let mut value = valid_plan();
    value.variables =
        serde_json::from_value::<agent::VariableBindings>(serde_json::json!({"values":{"items":vec![true;1000]}}))
            .unwrap()
            .into();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::Limit
    );
}

#[test]
fn decoder_bounds_original_artifacts_and_redacts_decoded_plan_diagnostics() {
    let decoder = RegisteredPlanDecoder::new(contract()).unwrap();
    for bytes in [Vec::new(), vec![0; 65537]] {
        assert_eq!(decoder.decode(&stored_bytes(bytes)).unwrap_err(), PlanError::Limit);
    }
    let decoded = decoder.decode(&stored_bytes(valid_plan().encode_to_vec())).unwrap();
    let debug = format!("{decoded:?}");
    assert!(debug.contains("DecodedPlan"));
    assert!(debug.contains("encoded_bytes"));
    assert!(!debug.contains("parent-example"));
    assert!(!debug.contains("runtime-definition"));
    for original in [Vec::new(), vec![0; 65537]] {
        assert_eq!(
            decoder.decode_agent_configuration(&original).unwrap_err(),
            PlanError::Limit
        );
    }
    let configuration = agent::AgentConfiguration {
        runtime: valid_plan().implementation,
        settings: buffa_types::google::protobuf::Any {
            type_url: "type.googleapis.com/google.protobuf.Empty".into(),
            ..Default::default()
        }
        .into(),
        contract_version: 2,
        description: String::new(),
        ..Default::default()
    };
    assert_eq!(
        decoder
            .decode_agent_configuration(&configuration.encode_to_vec())
            .unwrap_err(),
        PlanError::UnsupportedContract
    );
}

#[test]
fn immutable_plan_references_and_declared_collection_bounds_are_checked() {
    let decoder = RegisteredPlanDecoder::new(contract()).unwrap();
    let mut value = valid_plan();
    value.agent_revision.get_or_insert_default().revision_number = 0;
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
    let mut value = valid_plan();
    value
        .effective_configuration
        .get_or_insert_default()
        .configuration_digest
        .get_or_insert_default()
        .value[0] ^= 1;
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
    let mut value = valid_plan();
    value
        .runtime_settings
        .get_or_insert_default()
        .configuration_digest
        .get_or_insert_default()
        .algorithm = "sha1".into();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::DigestMismatch
    );
    let mut value = valid_plan();
    let native = value.runtime_settings.get_or_insert_default();
    native.configuration_bytes = vec![0; 32769];
    native.configuration_digest = agent::Digest {
        algorithm: "sha256".into(),
        value: Sha256::digest(&native.configuration_bytes).to_vec(),
    }
    .into();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::Limit
    );
    let mut value = valid_plan();
    for native in [&mut value.runtime_settings, &mut value.effective_runtime_configuration] {
        let native = native.get_or_insert_default();
        native.configuration_bytes = vec![0; 24577];
        native.configuration_digest = agent::Digest {
            algorithm: "sha256".into(),
            value: Sha256::digest(&native.configuration_bytes).to_vec(),
        }
        .into();
    }
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::Limit
    );
    let mut value = valid_plan();
    value.model_access = plan::ModelAccessProfile::default().into();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
    let mut value = valid_plan();
    value.dependencies = vec![plan::ResolvedDependency::default(); 257];
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::Limit
    );
    let mut value = valid_plan();
    value.dependencies = vec![plan::ResolvedDependency {
        declaring_resource: plan::DependencyDeclarer::default().into(),
        binding_name: Some("delegate".into()),
        ..Default::default()
    }];
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
    let mut value = valid_plan();
    value.variables = Default::default();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
    let mut value = valid_plan();
    value
        .working_directory
        .get_or_insert_default()
        .checkout
        .get_or_insert_default()
        .uri = "x".repeat(2049);
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::Limit
    );
    for path in ["folder\\outside", "folder%2Foutside", "C:folder", "folder\0outside"] {
        let mut value = valid_plan();
        value.working_directory.get_or_insert_default().relative_path = Some(path.into());
        assert_eq!(
            decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
            PlanError::InvalidPlan
        );
    }
}

#[test]
fn candidate_and_work_values_remain_structural_data_until_their_owners_admit_them() {
    let decoder = RegisteredPlanDecoder::new(contract()).unwrap();
    let mut value = valid_plan();
    value.candidate = serde_json::from_value::<session::CandidateRef>(
        serde_json::json!({"candidateId":{"value":"candidate-example"},"contentDigest":support::digest(b"candidate")}),
    )
    .unwrap()
    .into();
    assert!(decoder.decode(&stored_bytes(value.encode_to_vec())).is_ok());
    let mut value = valid_plan();
    value.work = plan::WorkInput {
        contract: contract().into(),
        inputs: Vec::new(),
    }
    .into();
    assert!(decoder.decode(&stored_bytes(value.encode_to_vec())).is_ok());
    let input: session::ClaimCheck = serde_json::from_value(
        serde_json::json!({"storageRef":{"value":"input-example"},"digest":support::digest(b"input")}),
    )
    .unwrap();
    value.work.get_or_insert_default().inputs = vec![input; 129];
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::Limit
    );
    let mut value = valid_plan();
    value.implementation.get_or_insert_default().implementation =
        Some(agent::agent_implementation::Implementation::PlatformHarness(Box::new(
            agent::PlatformHarnessVersionRef { version: "1".into() },
        )));
    value.model_access = plan::ModelAccessProfile {
        profile: Some(plan::model_access_profile::Profile::Managed(Box::default())),
    }
    .into();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::UnsupportedRuntime
    );
}

#[test]
fn working_directory_requires_a_complete_filesystem_contract() {
    let decoder = RegisteredPlanDecoder::new(contract()).unwrap();
    let mut value = valid_plan();
    value
        .working_directory
        .get_or_insert_default()
        .filesystem_contract
        .get_or_insert_default()
        .version = String::new();
    assert_eq!(
        decoder.decode(&stored_bytes(value.encode_to_vec())).unwrap_err(),
        PlanError::InvalidPlan
    );
}

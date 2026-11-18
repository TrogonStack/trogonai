#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use async_nats::jetstream;
use buffa::Message;
use sha2::{Digest as _, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use trogon_decider_nats::JetStreamStore;
use trogon_decider_runtime::{CommandId, ReadFrom, ReadStreamRequest, StreamRead};
use trogon_identity_types::SubjectPrincipal;
use trogon_nats::{NatsToken, test_support::JetStreamTestServer};
use trogon_session_plan::{
    DelegationOwnerContext, PlanAdmission, PlanError, PlanOwnerContext, PlanOwners, PlanSourceLease,
    RegisteredPlanDecoder, RuntimePlanInterpreter,
    wire::{agent, plan, session},
};
use trogon_session_store::{
    CreateAdmission, SessionAdmission, SessionAdmissionLease, SessionKey, SessionRuntime, SessionStreamProfile,
    SessionSubjectResolver,
};
use trogon_std::env::{ReadEnv, SystemEnv};

#[derive(Clone)]
struct FixtureLease {
    live: tokio::sync::watch::Receiver<bool>,
    deadline: Instant,
    revision: agent::AgentRevisionRef,
    original: Vec<u8>,
    settings: agent::ConfigurationContractRef,
    execution: agent::ConfigurationContractRef,
    effective: agent::NativeConfiguration,
    implementation: agent::AgentImplementation,
    configuration_readable: bool,
}

impl SessionAdmissionLease for FixtureLease {
    type Error = PlanError;
    fn require_live(&self) -> Result<(), Self::Error> {
        if *self.live.borrow() && Instant::now() < self.deadline {
            Ok(())
        } else {
            Err(PlanError::AuthorityWithdrawn)
        }
    }
    fn deadline(&self) -> Instant {
        self.deadline
    }
    async fn withdrawn(&self) {
        let mut live = self.live.clone();
        while *live.borrow_and_update() {
            if live.changed().await.is_err() {
                return;
            }
        }
    }
}

impl RuntimePlanInterpreter for FixtureLease {
    fn implementation(&self) -> &agent::AgentImplementation {
        &self.implementation
    }
    fn settings_contract(&self) -> &agent::ConfigurationContractRef {
        &self.settings
    }
    fn settings_type_url(&self) -> &str {
        "type.googleapis.com/google.protobuf.Empty"
    }
    fn execution_contract(&self) -> &agent::ConfigurationContractRef {
        &self.execution
    }
    fn validate_and_project(
        &self,
        original: &agent::AgentConfiguration,
        _plan: &plan::SessionExecutionPlan,
    ) -> Result<agent::NativeConfiguration, PlanError> {
        if original.settings.value.is_empty() {
            Ok(self.effective.clone())
        } else {
            Err(PlanError::UnsupportedRuntime)
        }
    }
}

impl PlanSourceLease for FixtureLease {
    fn revision(&self) -> &agent::AgentRevisionRef {
        &self.revision
    }
    fn original_configuration(&self) -> Result<&[u8], PlanError> {
        self.require_live()?;
        if !self.configuration_readable {
            return Err(PlanError::SourceUnavailable);
        }
        Ok(&self.original)
    }
    fn candidate(&self) -> Option<&session::CandidateRef> {
        None
    }
    fn project_id(&self) -> &str {
        "project-example"
    }
    fn interpreter(&self) -> &dyn RuntimePlanInterpreter {
        self
    }
}

struct FixtureOwners {
    sources: std::collections::BTreeMap<String, FixtureLease>,
    available: bool,
    delegation_allowed: bool,
}
impl PlanOwners for FixtureOwners {
    type SourceLease = FixtureLease;
    type PlanLease = FixtureLease;
    async fn authorize_source(&self, _caller: &SubjectPrincipal, _key: &SessionKey) -> Result<FixtureLease, PlanError> {
        if !self.available {
            return Err(PlanError::SourceUnavailable);
        }
        self.sources
            .values()
            .next()
            .cloned()
            .ok_or(PlanError::SourceUnavailable)
    }
    async fn admit_plan(&self, context: PlanOwnerContext<'_>) -> Result<FixtureLease, PlanError> {
        if !self.available {
            return Err(PlanError::SourceUnavailable);
        }
        self.sources
            .get(&context.plan.value().agent_revision.agent_id)
            .cloned()
            .ok_or(PlanError::SourceUnavailable)
    }
    async fn authorize_delegation(&self, context: DelegationOwnerContext<'_>) -> Result<FixtureLease, PlanError> {
        if !self.delegation_allowed {
            return Err(PlanError::AuthorityWithdrawn);
        }
        self.sources
            .get(&context.parent.value().agent_revision.agent_id)
            .cloned()
            .ok_or(PlanError::SourceUnavailable)
    }
}

fn adapter(source: FixtureLease) -> PlanAdmission<FixtureOwners> {
    let sources = std::collections::BTreeMap::from([(source.revision.agent_id.clone(), source)]);
    PlanAdmission::new(
        RegisteredPlanDecoder::new(support::contract()).unwrap(),
        Arc::new(FixtureOwners {
            sources,
            available: true,
            delegation_allowed: true,
        }),
    )
}

fn fixture_material() -> (FixtureLease, session::CreateSession, tokio::sync::watch::Sender<bool>) {
    let mut plan = support::valid_plan();
    let configuration = agent::AgentConfiguration {
        runtime: plan.implementation.clone(),
        settings: buffa_types::google::protobuf::Any {
            type_url: "type.googleapis.com/google.protobuf.Empty".into(),
            value: Vec::new().into(),
            ..Default::default()
        }
        .into(),
        contract_version: 1,
        description: "fixture configuration".into(),
        ..Default::default()
    };
    let original = configuration.encode_to_vec();
    let reference = agent::AgentConfigurationRef {
        configuration_digest: agent::Digest {
            algorithm: "sha256".into(),
            value: Sha256::digest(&original).to_vec(),
        }
        .into(),
    };
    plan.effective_configuration = reference.clone().into();
    plan.agent_revision.get_or_insert_default().configuration = reference.into();
    let (live, reader) = tokio::sync::watch::channel(true);
    let lease = FixtureLease {
        live: reader,
        deadline: Instant::now() + Duration::from_secs(30),
        revision: plan.agent_revision.as_option().unwrap().clone(),
        original,
        settings: plan.runtime_settings.contract.as_option().unwrap().clone(),
        execution: plan.execution_contract.as_option().unwrap().clone(),
        effective: plan.effective_runtime_configuration.as_option().unwrap().clone(),
        implementation: plan.implementation.as_option().unwrap().clone(),
        configuration_readable: true,
    };
    let command = session::CreateSession {
        session_id: "parent-example".into(),
        execution_plan: support::stored_bytes(plan.encode_to_vec()).into(),
        checkout_ref: plan.working_directory.checkout.clone(),
        candidate_ref: Default::default(),
    };
    (lease, command, live)
}

#[tokio::test]
async fn authenticated_original_configuration_and_registered_interpreter_admit_the_plan() {
    let (source, command, _live) = fixture_material();
    let admission = adapter(source);
    let caller = SubjectPrincipal::from_tokens("person", "fixture").unwrap();
    let key = SessionKey::new(NatsToken::new("fixture").unwrap(), "parent-example").unwrap();
    let original = command.encode_to_vec();
    let lease = admission
        .authorize_create(CreateAdmission {
            caller: &caller,
            key: &key,
            command: &command,
            original_command: &original,
        })
        .await;
    assert!(
        lease.is_ok(),
        "authenticated original configuration and exact runtime contract must admit the immutable plan: {:?}",
        lease.err()
    );
}

type AdmissionLease = trogon_session_plan::PlanAdmissionLease<FixtureLease, FixtureLease>;

async fn check(
    admission: &PlanAdmission<FixtureOwners>,
    command: &session::CreateSession,
) -> Result<AdmissionLease, PlanError> {
    let caller = SubjectPrincipal::from_tokens("person", "fixture").unwrap();
    let key = SessionKey::new(NatsToken::new("fixture").unwrap(), &command.session_id).unwrap();
    admission
        .authorize_create(CreateAdmission {
            caller: &caller,
            key: &key,
            command,
            original_command: &command.encode_to_vec(),
        })
        .await
}

fn update_configuration(
    source: &mut FixtureLease,
    command: &mut session::CreateSession,
    configuration: agent::AgentConfiguration,
) {
    source.original = configuration.encode_to_vec();
    let reference = agent::AgentConfigurationRef {
        configuration_digest: agent::Digest {
            algorithm: "sha256".into(),
            value: Sha256::digest(&source.original).to_vec(),
        }
        .into(),
    };
    source.revision.configuration = reference.clone().into();
    let mut value = plan::SessionExecutionPlan::decode_from_slice(&command.execution_plan.plan_bytes).unwrap();
    value.agent_revision = source.revision.clone().into();
    value.effective_configuration = reference.into();
    command.execution_plan = support::stored_bytes(value.encode_to_vec()).into();
}

#[tokio::test]
async fn original_configuration_bytes_and_runtime_registration_cannot_be_replaced() {
    let (mut unreadable, unreadable_command, _unreadable_live) = fixture_material();
    unreadable.configuration_readable = false;
    assert_eq!(
        check(&adapter(unreadable), &unreadable_command).await.err(),
        Some(PlanError::SourceUnavailable)
    );
    let (mut source, command, _live) = fixture_material();
    source.original.push(0);
    assert_eq!(
        check(&adapter(source), &command).await.err(),
        Some(PlanError::OriginalSourceMismatch)
    );
    let (mut oversized, oversized_command, _oversized_live) = fixture_material();
    oversized.original = vec![0; 65537];
    assert_eq!(
        check(&adapter(oversized), &oversized_command).await.err(),
        Some(PlanError::Limit)
    );
    let (source, mut command, _live) = fixture_material();
    let mut value = plan::SessionExecutionPlan::decode_from_slice(&command.execution_plan.plan_bytes).unwrap();
    value
        .runtime_settings
        .get_or_insert_default()
        .contract
        .get_or_insert_default()
        .version = "unknown".into();
    command.execution_plan = support::stored_bytes(value.encode_to_vec()).into();
    assert_eq!(
        check(&adapter(source), &command).await.err(),
        Some(PlanError::UnsupportedRuntime)
    );
    let (mut source, mut command, _live) = fixture_material();
    let mut configuration = agent::AgentConfiguration::decode_from_slice(&source.original).unwrap();
    configuration.settings.get_or_insert_default().value = vec![8, 1].into();
    update_configuration(&mut source, &mut command, configuration);
    let mut value = plan::SessionExecutionPlan::decode_from_slice(&command.execution_plan.plan_bytes).unwrap();
    value.runtime_settings = agent::NativeConfiguration {
        contract: source.settings.clone().into(),
        configuration_bytes: vec![8, 1],
        configuration_digest: agent::Digest {
            algorithm: "sha256".into(),
            value: Sha256::digest([8, 1]).to_vec(),
        }
        .into(),
    }
    .into();
    command.execution_plan = support::stored_bytes(value.encode_to_vec()).into();
    assert_eq!(
        check(&adapter(source), &command).await.err(),
        Some(PlanError::UnsupportedRuntime)
    );
}

#[tokio::test]
async fn source_unavailability_and_withdrawal_fail_without_renewing_authority() {
    let (source, command, live) = fixture_material();
    let deadline = source.deadline;
    let sources = std::collections::BTreeMap::from([(source.revision.agent_id.clone(), source.clone())]);
    let unavailable = PlanAdmission::new(
        RegisteredPlanDecoder::new(support::contract()).unwrap(),
        Arc::new(FixtureOwners {
            sources,
            available: false,
            delegation_allowed: true,
        }),
    );
    assert_eq!(
        check(&unavailable, &command).await.err(),
        Some(PlanError::SourceUnavailable)
    );
    let admission = adapter(source);
    let lease = check(&admission, &command).await.unwrap();
    assert_eq!(lease.deadline(), deadline);
    live.send_replace(false);
    assert_eq!(lease.require_live(), Err(PlanError::AuthorityWithdrawn));
    assert_eq!(
        check(&admission, &command).await.err(),
        Some(PlanError::AuthorityWithdrawn)
    );
    tokio::time::timeout(Duration::from_millis(50), lease.withdrawn())
        .await
        .unwrap();
}

#[tokio::test]
async fn the_raw_command_and_actual_original_declarations_are_mandatory() {
    let (source, mut command, _live) = fixture_material();
    let admission = adapter(source);
    let original = command.encode_to_vec();
    command.checkout_ref.get_or_insert_default().checkout_id = "other-checkout".into();
    let caller = SubjectPrincipal::from_tokens("person", "fixture").unwrap();
    let key = SessionKey::new(NatsToken::new("fixture").unwrap(), "parent-example").unwrap();
    assert_eq!(
        admission
            .authorize_create(CreateAdmission {
                caller: &caller,
                key: &key,
                command: &command,
                original_command: &original
            })
            .await
            .err(),
        Some(PlanError::OriginalSourceMismatch)
    );
    let (mut source, mut command, _live) = fixture_material();
    let mut configuration = agent::AgentConfiguration::decode_from_slice(&source.original).unwrap();
    let tool:agent::ToolDeclaration=serde_json::from_value(serde_json::json!({"bindingName":"unsupported-tool","requirement":"DEPENDENCY_REQUIREMENT_OPTIONAL","exactPin":{"toolId":"tool-example","version":"1","definitionDigest":support::digest(b"tool")}})).unwrap();
    configuration.dependencies = agent::AgentDependencies {
        tools: vec![tool],
        ..Default::default()
    }
    .into();
    update_configuration(&mut source, &mut command, configuration);
    assert_eq!(
        check(&adapter(source), &command).await.err(),
        Some(PlanError::UnsupportedRuntime)
    );
}

#[tokio::test]
async fn variables_use_the_authenticated_schema_without_coercion_or_unknown_names() {
    let (mut source, mut command, _live) = fixture_material();
    let mut configuration = agent::AgentConfiguration::decode_from_slice(&source.original).unwrap();
    configuration.variables_schema=serde_json::from_value::<agent::VariableSchema>(serde_json::json!({"variables":[
        {"name":"count","requirement":"VARIABLE_REQUIREMENT_REQUIRED","valueType":{"integerType":{"minimum":"0","maximum":"10"}}},
        {"name":"options","requirement":"VARIABLE_REQUIREMENT_REQUIRED","valueType":{"objectType":{"fields":[{"name":"enabled","requirement":"VARIABLE_REQUIREMENT_REQUIRED","valueType":{"booleanType":{}}}]}}}
    ]})).unwrap().into();
    update_configuration(&mut source, &mut command, configuration);
    let base = plan::SessionExecutionPlan::decode_from_slice(&command.execution_plan.plan_bytes).unwrap();
    let admission = adapter(source);
    for (values, expected) in [
        (serde_json::json!({"count":3,"options":{"enabled":true}}), None),
        (
            serde_json::json!({"count":"3","options":{"enabled":true}}),
            Some(PlanError::Variables),
        ),
        (
            serde_json::json!({"count":11,"options":{"enabled":true}}),
            Some(PlanError::Variables),
        ),
        (
            serde_json::json!({"count":3,"options":{"enabled":true,"unknown":true}}),
            Some(PlanError::Variables),
        ),
        (
            serde_json::json!({"count":null,"options":{"enabled":true}}),
            Some(PlanError::Variables),
        ),
    ] {
        let mut value = base.clone();
        value.variables = serde_json::from_value::<agent::VariableBindings>(serde_json::json!({"values":values}))
            .unwrap()
            .into();
        command.execution_plan = support::stored_bytes(value.encode_to_vec()).into();
        assert_eq!(check(&admission, &command).await.err(), expected);
    }
}

struct DelegationFixture {
    admission: Arc<PlanAdmission<FixtureOwners>>,
    parent_create: session::CreateSession,
    dispatch: session::DispatchDelegation,
    _live: Vec<tokio::sync::watch::Sender<bool>>,
}

fn delegation_fixture(selector: Option<&str>, child_label: &str, declared: bool, permitted: bool) -> DelegationFixture {
    let (mut child_source, mut child_create, child_live) = fixture_material();
    child_source.revision.agent_id = "child-agent".into();
    let mut child_plan =
        plan::SessionExecutionPlan::decode_from_slice(&child_create.execution_plan.plan_bytes).unwrap();
    child_plan.session_id = Some("child-example".into());
    child_plan.agent_revision = child_source.revision.clone().into();
    child_create.session_id = "child-example".into();
    child_create.execution_plan = support::stored_bytes(child_plan.encode_to_vec()).into();
    let mut child_configuration = agent::AgentConfiguration::decode_from_slice(&child_source.original).unwrap();
    child_configuration
        .selectable_labels
        .insert("role".into(), child_label.into());
    update_configuration(&mut child_source, &mut child_create, child_configuration);
    let pin = agent::ExactDelegateRevisionPin {
        agent_id: child_source.revision.agent_id.clone(),
        revision: child_source.revision.revision_number,
        configuration_digest: child_source.revision.configuration.configuration_digest.clone(),
    };
    let selection = match selector {
        Some(role) => agent::delegate_declaration::Selection::Selector(Box::new(agent::DelegateSelector {
            labels: std::collections::HashMap::from([("role".into(), role.into())])
                .into_iter()
                .collect(),
        })),
        None => agent::delegate_declaration::Selection::ExactPin(Box::new(pin.clone())),
    };
    let declaration = agent::DelegateDeclaration {
        binding_name: "calendar".into(),
        requirement: agent::DependencyRequirement::Required.into(),
        selection: Some(selection),
    };
    let (mut parent_source, mut parent_create, parent_live) = fixture_material();
    let mut parent_configuration = agent::AgentConfiguration::decode_from_slice(&parent_source.original).unwrap();
    if declared {
        parent_configuration.dependencies = agent::AgentDependencies {
            delegates: vec![declaration.clone()],
            ..Default::default()
        }
        .into();
    }
    update_configuration(&mut parent_source, &mut parent_create, parent_configuration);
    let mut parent_plan =
        plan::SessionExecutionPlan::decode_from_slice(&parent_create.execution_plan.plan_bytes).unwrap();
    parent_plan.dependencies = vec![plan::ResolvedDependency {
        declaring_resource: plan::DependencyDeclarer {
            resource: Some(plan::dependency_declarer::Resource::Agent(Box::new(
                parent_source.revision.clone(),
            ))),
        }
        .into(),
        binding_name: Some("calendar".into()),
        declaration: Some(plan::resolved_dependency::Declaration::DelegateDeclaration(Box::new(
            declaration,
        ))),
        resolution: Some(plan::resolved_dependency::Resolution::Delegate(Box::new(pin))),
    }];
    parent_create.execution_plan = support::stored_bytes(parent_plan.encode_to_vec()).into();
    let dispatch = session::DispatchDelegation {
        session_id: parent_create.session_id.clone(),
        operation_id: "operation-example".into(),
        child_session_id: child_create.session_id.clone(),
        cascade_policy: session::CascadePolicy::CascadeOnParentTerminal.into(),
        turn_id: "turn-example".into(),
        delegate_binding: session::DelegationBindingRef {
            binding_name: Some("calendar".into()),
            parent_execution_plan_digest: parent_create.execution_plan.plan_digest.clone(),
        }
        .into(),
        child_creation: session::ChildSessionCreation {
            execution_plan: child_create.execution_plan,
            checkout_ref: child_create.checkout_ref,
            candidate_ref: child_create.candidate_ref,
        }
        .into(),
    };
    let sources = std::collections::BTreeMap::from([
        (parent_source.revision.agent_id.clone(), parent_source),
        (child_source.revision.agent_id.clone(), child_source),
    ]);
    DelegationFixture {
        admission: Arc::new(PlanAdmission::new(
            RegisteredPlanDecoder::new(support::contract()).unwrap(),
            Arc::new(FixtureOwners {
                sources,
                available: true,
                delegation_allowed: permitted,
            }),
        )),
        parent_create,
        dispatch,
        _live: vec![parent_live, child_live],
    }
}

async fn dispatch_check(
    fixture: &DelegationFixture,
    command: &session::DispatchDelegation,
) -> Result<AdmissionLease, PlanError> {
    let caller = SubjectPrincipal::from_tokens("person", "fixture").unwrap();
    let key = SessionKey::new(NatsToken::new("fixture").unwrap(), "parent-example").unwrap();
    let parent = session::SessionStarted {
        session_id: fixture.parent_create.session_id.clone(),
        execution_plan: fixture.parent_create.execution_plan.clone(),
        checkout_ref: fixture.parent_create.checkout_ref.clone(),
        candidate_ref: fixture.parent_create.candidate_ref.clone(),
    };
    fixture
        .admission
        .authorize_dispatch(trogon_session_store::DispatchAdmission {
            caller: &caller,
            key: &key,
            parent: &parent,
            original_parent: &parent.encode_to_vec(),
            command,
            original_command: &command.encode_to_vec(),
            attempt: None,
        })
        .await
}

#[tokio::test]
async fn delegation_requires_the_original_declared_binding_and_current_target_policy() {
    let fixture = delegation_fixture(None, "worker", true, true);
    assert!(dispatch_check(&fixture, &fixture.dispatch).await.is_ok());
    let mut command = fixture.dispatch.clone();
    command.delegate_binding.get_or_insert_default().binding_name = Some("unknown".into());
    assert_eq!(
        dispatch_check(&fixture, &command).await.err(),
        Some(PlanError::OriginalSourceMismatch)
    );
    let mut command = fixture.dispatch.clone();
    command
        .delegate_binding
        .get_or_insert_default()
        .parent_execution_plan_digest
        .get_or_insert_default()
        .value[0] ^= 1;
    assert_eq!(
        dispatch_check(&fixture, &command).await.err(),
        Some(PlanError::OriginalSourceMismatch)
    );
    let invented = delegation_fixture(None, "worker", false, true);
    assert_eq!(
        dispatch_check(&invented, &invented.dispatch).await.err(),
        Some(PlanError::OriginalSourceMismatch)
    );
    let denied = delegation_fixture(None, "worker", true, false);
    assert_eq!(
        dispatch_check(&denied, &denied.dispatch).await.err(),
        Some(PlanError::AuthorityWithdrawn)
    );
}

#[tokio::test]
async fn delegate_selectors_match_the_actual_child_configuration_labels() {
    let matching = delegation_fixture(Some("worker"), "worker", true, true);
    assert!(dispatch_check(&matching, &matching.dispatch).await.is_ok());
    let wrong = delegation_fixture(Some("worker"), "other", true, true);
    assert_eq!(
        dispatch_check(&wrong, &wrong.dispatch).await.err(),
        Some(PlanError::OriginalSourceMismatch)
    );
    let mut repeated = matching.parent_create.clone();
    let mut value = plan::SessionExecutionPlan::decode_from_slice(&repeated.execution_plan.plan_bytes).unwrap();
    value.dependencies.push(value.dependencies[0].clone());
    repeated.execution_plan = support::stored_bytes(value.encode_to_vec()).into();
    assert_eq!(
        check(&matching.admission, &repeated).await.err(),
        Some(PlanError::InvalidPlan)
    );
}

#[derive(Debug, thiserror::Error)]
#[error("fixture Session fact binding mismatch")]
struct FixtureCodecError;

struct FixtureCodec;
impl trogon_session_store::SessionFactCodec for FixtureCodec {
    type Error = FixtureCodecError;
    async fn seal(
        &self,
        key: &SessionKey,
        id: trogon_decider_runtime::EventId,
        kind: &str,
        plain: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        let mut value = format!("{key}\0{id}\0{kind}\0").into_bytes();
        value.extend_from_slice(plain);
        Ok(value)
    }
    async fn open(
        &self,
        key: &SessionKey,
        id: trogon_decider_runtime::EventId,
        kind: &str,
        sealed: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        sealed
            .strip_prefix(format!("{key}\0{id}\0{kind}\0").as_bytes())
            .map(ToOwned::to_owned)
            .ok_or(FixtureCodecError)
    }
}

#[tokio::test]
async fn admitted_original_plans_flow_through_native_acknowledgement_and_child_recovery() {
    let fixture = delegation_fixture(Some("worker"), "worker", true, true);
    let server = match SystemEnv.var("TROGON_SESSION_TEST_NETWORK") {
        Ok(network) => JetStreamTestServer::start_on_network(&network).await,
        Err(_) => JetStreamTestServer::start().await,
    };
    let js = server.jetstream().await;
    let name = "SESSION_ADMISSION_RECOVERY";
    let stream = js
        .create_stream(jetstream::stream::Config {
            name: name.into(),
            subjects: vec!["fixture.session.sessions.>".into()],
            allow_atomic_publish: true,
            storage: jetstream::stream::StorageType::File,
            retention: jetstream::stream::RetentionPolicy::Limits,
            max_messages: 1024,
            max_bytes: 4 * 1024 * 1024,
            max_message_size: 262144,
            discard: jetstream::stream::DiscardPolicy::New,
            deny_delete: true,
            deny_purge: true,
            num_replicas: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    let snapshots = js
        .create_key_value(jetstream::kv::Config {
            bucket: "SESSION_ADMISSION_SNAPSHOTS".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let namespace = NatsToken::new("fixture").unwrap();
    let store = Arc::new(JetStreamStore::builder(js, stream, snapshots).with_subject_resolver(
        SessionSubjectResolver::new(SessionStreamProfile::new(name, namespace.clone(), 1).unwrap()),
    ));
    let parent = SessionKey::new(namespace.clone(), "parent-example").unwrap();
    let child = SessionKey::new(namespace, "child-example").unwrap();
    let caller = SubjectPrincipal::from_tokens("person", "fixture").unwrap();
    let codec = Arc::new(FixtureCodec);
    let runtime = SessionRuntime::new(store.clone(), fixture.admission.clone(), codec.clone());
    runtime
        .create_session(
            &caller,
            &parent,
            CommandId::new(uuid::Uuid::new_v4()),
            &fixture.parent_create.encode_to_vec(),
        )
        .await
        .unwrap();
    runtime
        .dispatch_delegation(
            &caller,
            &parent,
            CommandId::new(uuid::Uuid::new_v4()),
            &fixture.dispatch.encode_to_vec(),
        )
        .await
        .unwrap();
    assert!(
        store
            .read_stream(ReadStreamRequest {
                stream_id: &child,
                from: ReadFrom::Beginning
            })
            .await
            .unwrap()
            .events
            .is_empty()
    );
    drop(runtime);
    let recovered = SessionRuntime::new(store.clone(), fixture.admission.clone(), codec.clone());
    let first = recovered
        .recover_child(&caller, &parent, "operation-example")
        .await
        .unwrap();
    assert!(first.inserted);
    let second = recovered
        .recover_child(&caller, &parent, "operation-example")
        .await
        .unwrap();
    assert!(!second.inserted);
    let history = store
        .read_stream(ReadStreamRequest {
            stream_id: &child,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_eq!(history.events.len(), 2);
    assert_eq!(
        history.events[0].event.r#type,
        "trogonai.session.sessions.v1alpha1.SessionStarted"
    );
    assert_eq!(
        history.events[1].event.r#type,
        "trogonai.session.sessions.v1alpha1.ParentLinked"
    );
    let event = &history.events[0].event;
    let original =
        trogon_session_store::SessionFactCodec::open(codec.as_ref(), &child, event.id, &event.r#type, &event.content)
            .await
            .unwrap();
    let started = session::SessionStarted::decode_from_slice(&original).unwrap();
    assert_eq!(started.execution_plan, fixture.dispatch.child_creation.execution_plan);
    assert_eq!(started.checkout_ref, fixture.dispatch.child_creation.checkout_ref);
    assert_eq!(started.candidate_ref, fixture.dispatch.child_creation.candidate_ref);
}

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use buffa::Message;
use sha2::{Digest as _, Sha256};
use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use trogon_identity_types::SubjectPrincipal;
use trogon_nats::NatsToken;
use trogon_session_plan::{
    DelegationOwnerContext, PlanAdmission, PlanError, PlanOwnerContext, PlanOwners, PlanSourceLease,
    RegisteredPlanDecoder, RuntimePlanInterpreter,
    wire::{agent, plan, session},
};
use trogon_session_store::{CreateAdmission, SessionAdmission, SessionAdmissionLease, SessionKey};

#[derive(Clone)]
struct FixtureLease {
    live: Arc<AtomicBool>,
    deadline: Instant,
    revision: agent::AgentRevisionRef,
    original: Vec<u8>,
    settings: agent::ConfigurationContractRef,
    execution: agent::ConfigurationContractRef,
    effective: agent::NativeConfiguration,
}

impl SessionAdmissionLease for FixtureLease {
    type Error = PlanError;
    fn require_live(&self) -> Result<(), Self::Error> {
        if self.live.load(Ordering::SeqCst) && Instant::now() < self.deadline {
            Ok(())
        } else {
            Err(PlanError::AuthorityWithdrawn)
        }
    }
    fn deadline(&self) -> Instant {
        self.deadline
    }
    async fn withdrawn(&self) {
        pending::<()>().await
    }
}

impl RuntimePlanInterpreter for FixtureLease {
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
    fn original_configuration(&self) -> &[u8] {
        &self.original
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

struct FixtureOwners(FixtureLease);
impl PlanOwners for FixtureOwners {
    type SourceLease = FixtureLease;
    type PlanLease = FixtureLease;
    async fn authorize_source(&self, _caller: &SubjectPrincipal, _key: &SessionKey) -> Result<FixtureLease, PlanError> {
        Ok(self.0.clone())
    }
    async fn admit_plan(&self, _context: PlanOwnerContext<'_>) -> Result<FixtureLease, PlanError> {
        Ok(self.0.clone())
    }
    async fn authorize_delegation(&self, _context: DelegationOwnerContext<'_>) -> Result<FixtureLease, PlanError> {
        Ok(self.0.clone())
    }
}

fn fixture() -> (PlanAdmission<FixtureOwners>, session::CreateSession, Arc<AtomicBool>) {
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
    let live = Arc::new(AtomicBool::new(true));
    let lease = FixtureLease {
        live: live.clone(),
        deadline: Instant::now() + Duration::from_secs(30),
        revision: plan.agent_revision.as_option().unwrap().clone(),
        original,
        settings: plan.runtime_settings.contract.as_option().unwrap().clone(),
        execution: plan.execution_contract.as_option().unwrap().clone(),
        effective: plan.effective_runtime_configuration.as_option().unwrap().clone(),
    };
    let command = session::CreateSession {
        session_id: "parent-example".into(),
        execution_plan: support::stored_bytes(plan.encode_to_vec()).into(),
        checkout_ref: plan.working_directory.checkout.clone(),
        candidate_ref: Default::default(),
    };
    (
        PlanAdmission::new(
            RegisteredPlanDecoder::new(support::contract()).unwrap(),
            Arc::new(FixtureOwners(lease)),
        ),
        command,
        live,
    )
}

#[tokio::test]
async fn authenticated_original_configuration_and_registered_interpreter_admit_the_plan() {
    let (admission, command, _live) = fixture();
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

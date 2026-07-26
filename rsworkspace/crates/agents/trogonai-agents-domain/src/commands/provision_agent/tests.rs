use trogon_decider::{Decider, testing::TestCase};

use super::*;
use crate::commands::domain::{AgentDefinition, AgentName};
use crate::commands::test_support::*;

#[test]
fn provisions_genesis_revision_from_validated_definition() {
    TestCase::<ProvisionAgent>::new()
        .given_no_history()
        .when(provision_command())
        .then([provisioned_event()]);
}

#[test]
fn classifies_an_identical_retry_by_agent_id() {
    TestCase::<ProvisionAgent>::new()
        .given([provisioned_event()])
        .when(provision_command())
        .then_error(ProvisionAgentError::AlreadyProvisionedIdentical { agent_id: agent_id() });
}

#[test]
fn stores_only_retry_identity_in_decider_state() {
    let result = TestCase::<ProvisionAgent>::new()
        .given_no_history()
        .when(provision_command())
        .then([provisioned_event()]);
    let provisioned = result.resulting_state().provisioned.as_option().unwrap();

    assert_eq!(provisioned.agent_id, agent_id().as_str());
    assert_eq!(provisioned.name, "reviewer");
    assert_eq!(provisioned.content_digest, digest('a').as_str());
}

#[test]
fn compares_the_initial_digest_on_retry() {
    let mut command = provision_command();
    command.content_digest = digest('f');

    TestCase::<ProvisionAgent>::new()
        .given([provisioned_event()])
        .when(command)
        .then_error(ProvisionAgentError::AlreadyProvisionedConflict { agent_id: agent_id() });
}

#[test]
fn compares_the_complete_definition_on_retry() {
    let mut command = provision_command();
    command.definition = AgentDefinition::new(
        AgentName::parse("different-name").unwrap(),
        command.definition.parent().clone(),
        command.definition.charter().clone(),
    );

    TestCase::<ProvisionAgent>::new()
        .given([provisioned_event()])
        .when(command)
        .then_error(ProvisionAgentError::AlreadyProvisionedConflict { agent_id: agent_id() });
}

#[test]
fn keeps_the_default_runtime_precondition_so_retries_replay_history() {
    assert_eq!(ProvisionAgent::WRITE_PRECONDITION, None);
}

use trogon_semconv::attribute::DecisionOutcome;
use trogonai_proto::decider::CommandOutcomeCase;
use trogonai_proto::google::rpc::ErrorInfo;

use crate::command_subject::SubjectPrefix;
use crate::constants::{CONTENT_TYPE_HEADER, DEFAULT_SUBJECT_PREFIX, TROGON_COMMAND_ID_HEADER};
use crate::status::{FaultClass, find_detail};

use super::*;

fn router() -> CommandRouter {
    CommandRouter::new(
        CommandSubjects::new(SubjectPrefix::new(DEFAULT_SUBJECT_PREFIX).expect("the default prefix is a token")),
        Arc::new(DeciderRegistryHandle::default()),
    )
}

fn fault_reason(reply: &CommandReply) -> String {
    let Some(CommandOutcomeCase::Faulted(status)) = reply.outcome().outcome.as_ref() else {
        panic!("expected a faulted reply, got {:?}", reply.outcome());
    };
    find_detail::<ErrorInfo>(status)
        .expect("every decider status names its reason")
        .reason
}

#[test]
fn a_command_no_module_claims_is_unroutable() {
    let reply = router()
        .route(
            "decider.trogonai.scheduler.schedules.v1.CreateSchedule",
            Vec::new(),
            None,
        )
        .expect_err("an empty registry claims nothing");

    assert_eq!(reply.decision(), DecisionOutcome::Faulted);
    assert_eq!(
        fault_reason(&reply),
        FaultClass::Unroutable.reason(),
        "a caller told 'internal' would retry forever; told 'unroutable' it goes and activates the module"
    );
}

#[test]
fn a_subject_outside_the_hosts_prefix_is_an_invalid_request() {
    let reply = router()
        .route(
            "elsewhere.trogonai.scheduler.schedules.v1.CreateSchedule",
            Vec::new(),
            None,
        )
        .expect_err("this host does not answer under that prefix");

    assert_eq!(fault_reason(&reply), FaultClass::InvalidRequest.reason());
}

#[test]
fn the_bare_prefix_names_no_command() {
    let reply = router()
        .route(DEFAULT_SUBJECT_PREFIX, Vec::new(), None)
        .expect_err("the subtree root is not a command");

    assert_eq!(fault_reason(&reply), FaultClass::InvalidRequest.reason());
}

#[test]
fn an_encoding_the_host_does_not_speak_is_refused_before_routing() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE_HEADER, "application/json");

    let reply = router()
        .route(
            "decider.trogonai.scheduler.schedules.v1.CreateSchedule",
            Vec::new(),
            Some(&headers),
        )
        .expect_err("json is not a decider command encoding");

    assert_eq!(
        fault_reason(&reply),
        FaultClass::InvalidRequest.reason(),
        "a caller speaking the wrong encoding has a bug in its client, not in the deployment"
    );
}

#[test]
fn an_unparseable_header_outranks_an_unroutable_subject() {
    let mut headers = HeaderMap::new();
    headers.insert(TROGON_COMMAND_ID_HEADER, "not-a-uuid");

    let reply = router()
        .route(
            "decider.trogonai.scheduler.schedules.v1.CreateSchedule",
            Vec::new(),
            Some(&headers),
        )
        .expect_err("a malformed command id is not a command");

    assert_eq!(
        fault_reason(&reply),
        FaultClass::InvalidRequest.reason(),
        "reporting 'unroutable' would send the caller to inspect a deployment that is fine"
    );
}

#[test]
fn no_routing_failure_ever_reports_as_a_decision() {
    let subjects = [
        "decider.trogonai.scheduler.schedules.v1.CreateSchedule",
        "elsewhere.some.Command",
        DEFAULT_SUBJECT_PREFIX,
    ];

    for subject in subjects {
        let reply = router()
            .route(subject, Vec::new(), None)
            .expect_err("nothing routes in an empty registry");

        assert_eq!(
            reply.header_value(),
            DecisionOutcome::Faulted.as_str(),
            "a header that disagreed with the body would meter '{subject}' as something the caller never saw"
        );
    }
}

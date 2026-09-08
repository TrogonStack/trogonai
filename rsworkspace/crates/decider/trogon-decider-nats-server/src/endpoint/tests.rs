use trogon_nats::{MAX_SUBJECT_TOKENS, SubjectViolationError};

use super::{CommandEndpoint, CommandEndpointError, SubjectPrefix};
use crate::constants::DEFAULT_SUBJECT_PREFIX;

fn endpoint(prefix: &str) -> CommandEndpoint {
    CommandEndpoint::new(SubjectPrefix::new(prefix).expect("a token")).expect("a conformant subject")
}

#[test]
fn the_endpoint_answers_on_the_subject_its_service_and_method_name() {
    assert_eq!(
        endpoint(DEFAULT_SUBJECT_PREFIX).subject(),
        "decider.DeciderService.Decide",
        "the subject is what a generated client derives from the descriptor, so a host answering anywhere else is a host no such client can reach"
    );
}

#[test]
fn a_configured_prefix_moves_the_endpoint_without_renaming_it() {
    let moved = endpoint("acme.decider");

    assert_eq!(moved.subject(), "acme.decider.DeciderService.Decide");
    assert_eq!(moved.prefix().as_str(), "acme.decider");
}

#[test]
fn a_prefix_that_is_not_a_nats_token_is_refused() {
    assert!(SubjectPrefix::new("has space").is_err());
    assert!(SubjectPrefix::new("wild*card").is_err());
}

#[test]
fn a_prefix_whose_subject_breaks_the_subject_limits_is_refused_at_startup() {
    // One token under the subject budget, so the prefix is legal on its own and
    // only the two tokens the endpoint appends put the subject over.
    let prefix = SubjectPrefix::new(
        core::iter::repeat_n("seg", MAX_SUBJECT_TOKENS - 1)
            .collect::<Vec<_>>()
            .join("."),
    )
    .expect("a short dotted prefix is a token");

    let error = CommandEndpoint::new(prefix).expect_err("the joined subject is over the limit");

    assert!(
        matches!(
            error,
            CommandEndpointError::Subject {
                source: SubjectViolationError::TooManyTokens { .. },
                ..
            }
        ),
        "a host that cannot legally publish its own endpoint subject can never receive a command, so it has to fail before it claims to be serving: {error}"
    );
}

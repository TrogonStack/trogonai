//! ADR#0055 conformance over the whole ACP subject inventory.
//!
//! Every subject type is constructed here and checked against the shared
//! validator. A new subject type that skips this list is the failure mode this
//! guards against, so [`published`] and [`patterns`] are the single place to
//! register one.

use trogon_nats::subject_conformance::{
    looks_like_request_id, validate_binding_version, validate_published_subject, validate_subject_pattern,
};

use super::{AcpStream, client_ops, commands, global, responses, subscriptions};
use crate::acp_prefix::AcpPrefix;
use crate::ext_method_name::ExtMethodName;
use crate::session_id::AcpSessionId;

const PREFIXES: [&str; 2] = ["acp", "my.multi.part"];

/// Subject types that still carry a per-request id in the subject.
///
/// ADR#0055 keeps correlation in headers, and no ACP subject violates that any
/// more. The assertion below pins this set exactly, so a new violation fails
/// this test. Do not grow it.
const REQ_ID_IN_SUBJECT_ALLOWLIST: [&str; 0] = [];

fn prefix(s: &str) -> AcpPrefix {
    AcpPrefix::new(s).expect("test prefix")
}

fn session() -> AcpSessionId {
    AcpSessionId::new("sess_01").expect("test session id")
}

/// Every concrete subject a binding publishes to, paired with its type name.
fn published(p: &AcpPrefix) -> Vec<(&'static str, String)> {
    let s = session();
    let m = ExtMethodName::new("do_thing").expect("test method name");

    vec![
        ("InitializeSubject", global::InitializeSubject::new(p).to_string()),
        ("AuthenticateSubject", global::AuthenticateSubject::new(p).to_string()),
        ("LogoutSubject", global::LogoutSubject::new(p).to_string()),
        ("SessionNewSubject", global::SessionNewSubject::new(p).to_string()),
        ("SessionListSubject", global::SessionListSubject::new(p).to_string()),
        ("ProvidersListSubject", global::ProvidersListSubject::new(p).to_string()),
        ("ProvidersSetSubject", global::ProvidersSetSubject::new(p).to_string()),
        (
            "ProvidersDisableSubject",
            global::ProvidersDisableSubject::new(p).to_string(),
        ),
        ("ExtSubject", global::ExtSubject::new(p, &m).to_string()),
        ("ExtNotifySubject", global::ExtNotifySubject::new(p, &m).to_string()),
        ("PromptSubject", commands::PromptSubject::new(p, &s).to_string()),
        ("CancelSubject", commands::CancelSubject::new(p, &s).to_string()),
        ("LoadSubject", commands::LoadSubject::new(p, &s).to_string()),
        ("SetModeSubject", commands::SetModeSubject::new(p, &s).to_string()),
        (
            "SetConfigOptionSubject",
            commands::SetConfigOptionSubject::new(p, &s).to_string(),
        ),
        ("ForkSubject", commands::ForkSubject::new(p, &s).to_string()),
        ("ResumeSubject", commands::ResumeSubject::new(p, &s).to_string()),
        ("CloseSubject", commands::CloseSubject::new(p, &s).to_string()),
        ("DeleteSubject", commands::DeleteSubject::new(p, &s).to_string()),
        ("CancelledSubject", responses::CancelledSubject::new(p, &s).to_string()),
        ("ExtReadySubject", responses::ExtReadySubject::new(p, &s).to_string()),
        ("ResponseSubject", responses::ResponseSubject::new(p, &s).to_string()),
        (
            "SessionUpdateSubject",
            client_ops::SessionUpdateSubject::new(p, &s).to_string(),
        ),
        (
            "SessionRequestPermissionSubject",
            client_ops::SessionRequestPermissionSubject::new(p, &s).to_string(),
        ),
        (
            "FsReadTextFileSubject",
            client_ops::FsReadTextFileSubject::new(p, &s).to_string(),
        ),
        (
            "FsWriteTextFileSubject",
            client_ops::FsWriteTextFileSubject::new(p, &s).to_string(),
        ),
        (
            "ElicitationCreateSubject",
            client_ops::ElicitationCreateSubject::new(p, &s).to_string(),
        ),
        (
            "ElicitationCompleteSubject",
            client_ops::ElicitationCompleteSubject::new(p, &s).to_string(),
        ),
        (
            "TerminalCreateSubject",
            client_ops::TerminalCreateSubject::new(p, &s).to_string(),
        ),
        (
            "TerminalKillSubject",
            client_ops::TerminalKillSubject::new(p, &s).to_string(),
        ),
        (
            "TerminalOutputSubject",
            client_ops::TerminalOutputSubject::new(p, &s).to_string(),
        ),
        (
            "TerminalReleaseSubject",
            client_ops::TerminalReleaseSubject::new(p, &s).to_string(),
        ),
        (
            "TerminalWaitForExitSubject",
            client_ops::TerminalWaitForExitSubject::new(p, &s).to_string(),
        ),
    ]
}

/// Every subscription pattern, plus every stream's captured subjects.
fn patterns(p: &AcpPrefix) -> Vec<(&'static str, String)> {
    let s = session();

    let mut out = vec![
        ("AllAgentSubject", subscriptions::AllAgentSubject::new(p).to_string()),
        (
            "AllAgentExtSubject",
            subscriptions::AllAgentExtSubject::new(p).to_string(),
        ),
        ("AllClientSubject", subscriptions::AllClientSubject::new(p).to_string()),
        (
            "AllSessionSubject",
            subscriptions::AllSessionSubject::new(p).to_string(),
        ),
        ("GlobalAllSubject", subscriptions::GlobalAllSubject::new(p).to_string()),
        (
            "PromptWildcardSubject",
            subscriptions::PromptWildcardSubject::new(p).to_string(),
        ),
        (
            "OneAgentSubject",
            subscriptions::OneAgentSubject::new(p, &s).to_string(),
        ),
        (
            "OneClientSubject",
            subscriptions::OneClientSubject::new(p, &s).to_string(),
        ),
        (
            "OneSessionSubject",
            subscriptions::OneSessionSubject::new(p, &s).to_string(),
        ),
    ];

    for stream in AcpStream::ALL {
        out.extend(stream.subject_patterns(p).into_iter().map(|s| ("AcpStream", s)));
    }
    out
}

#[test]
fn every_published_subject_conforms() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for (name, subject) in published(&p) {
            assert_eq!(validate_published_subject(&subject), Ok(()), "{name}: {subject}");
        }
    }
}

#[test]
fn every_pattern_conforms() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for (name, subject) in patterns(&p) {
            assert_eq!(validate_subject_pattern(&subject), Ok(()), "{name}: {subject}");
        }
    }
}

#[test]
fn every_subject_carries_the_binding_version() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for (name, subject) in published(&p).into_iter().chain(patterns(&p)) {
            assert_eq!(
                validate_binding_version(&subject, prefix_str),
                Ok(()),
                "{name}: {subject}"
            );
        }
    }
}

#[test]
fn no_new_subject_type_embeds_a_request_id() {
    let p = prefix("acp");
    let mut offenders: Vec<&str> = published(&p)
        .into_iter()
        .filter(|(_, subject)| subject.split('.').any(looks_like_request_id))
        .map(|(name, _)| name)
        .collect();
    offenders.sort_unstable();
    offenders.dedup();

    let mut expected = REQ_ID_IN_SUBJECT_ALLOWLIST.to_vec();
    expected.sort_unstable();

    assert_eq!(
        offenders, expected,
        "the set of subjects embedding a request id changed; \
         shrink REQ_ID_IN_SUBJECT_ALLOWLIST when one is fixed, and never grow it"
    );
}

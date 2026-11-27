use std::sync::Arc;

use async_nats::Message;
use axum::Router;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::routing::post;
use bytes::Bytes;
use serde_json::{Value, json};
use trogon_sandbox_actor_agent::{
    EXEC_KILL_PATH, EXEC_PATH, EXEC_POLL_PATH, EXEC_WRITE_STDIN_PATH, FILES_READ_PATH, FILES_WRITE_PATH,
    GROUP_MEMBERSHIP_PATH, GROUPS_CREATE_PATH, USERS_CREATE_PATH,
};
use trogon_sandbox_contract::{
    ExecutionId, LeaseId, ProviderKind, PurgeNamedResponse, StdinChunk, StdinEof, StdinSeq, WriteStdinRequest,
};
use trogon_sandbox_protocol::{RedispatchGuard, subjects};

use super::super::{Verb, serve_message};
use super::support::{HELD, MISROUTED, Rig, TestResult, UNHELD, WAIT, body};
use crate::agent::tests::ndjson;

type Subject = fn(ProviderKind, &LeaseId) -> String;

fn json_route(path: &'static str, answer: &'static str) -> impl FnOnce(Router) -> Router {
    move |router| {
        router.route(
            path,
            post(move || async move {
                let mut response = axum::response::Response::new(axum::body::Body::from(answer));
                response
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                response
            }),
        )
    }
}

/// An agent that answers every verb the worker forwards.
fn agent() -> Router {
    let router = Router::new().route(
        EXEC_PATH,
        post(|| async { ndjson("{\"outcome\":\"completed\",\"exit_code\":0,\"stdout\":[111],\"stderr\":[]}\n") }),
    );
    let router = json_route(EXEC_POLL_PATH, r#"{"outcome":"running"}"#)(router);
    let router = json_route(EXEC_KILL_PATH, r#"{"outcome":"signaled"}"#)(router);
    let router = json_route(EXEC_WRITE_STDIN_PATH, r#"{"outcome":"written","next_seq":5}"#)(router);
    let router = json_route(FILES_WRITE_PATH, r#"{"outcome":"written"}"#)(router);
    let router = json_route(FILES_READ_PATH, r#"{"outcome":"content","data":[104,105]}"#)(router);
    let router = json_route(USERS_CREATE_PATH, r#"{"outcome":"created","home_dir":"/home/alice"}"#)(router);
    let router = json_route(
        GROUPS_CREATE_PATH,
        r#"{"outcome":"created","shared_dir":"/shared/dev"}"#,
    )(router);
    json_route(GROUP_MEMBERSHIP_PATH, r#"{"outcome":"applied"}"#)(router)
}

fn subject(verb: Subject, lease: &str) -> String {
    verb(ProviderKind::Substrate, &LeaseId::new(lease))
}

fn is_backend_denial(reply: &Value) -> bool {
    reply["outcome"] == "denied" && reply["kind"] == "backend"
}

/// Checks the four ways a forwarded verb can end: an undecodable request and a lease nobody
/// holds are ignored, a lease whose agent cannot be reached is denied, and a healthy lease is
/// answered by its agent.
async fn exercise(
    rig: &Rig,
    verb: Subject,
    request: fn(&str) -> Result<Value, Box<dyn std::error::Error>>,
    success: &str,
) -> TestResult {
    assert!(rig.unanswered(subject(verb, HELD), Bytes::from_static(b"nope")).await?);
    assert!(rig.unanswered(subject(verb, UNHELD), body(&request(UNHELD)?)?).await?);

    let misrouted: Value = rig.ask(subject(verb, MISROUTED), &request(MISROUTED)?).await?;
    assert!(is_backend_denial(&misrouted), "{misrouted}");

    let answered: Value = rig.ask(subject(verb, HELD), &request(HELD)?).await?;
    assert_eq!(answered["outcome"], success, "{answered}");
    Ok(())
}

#[tokio::test]
async fn run_command_streams_through_the_agent() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_run_command,
        |lease| Ok(json!({"lease_id": lease, "command": ["true"], "timeout_seconds": 5})),
        "completed",
    )
    .await?;

    let streamed: Value = rig
        .ask(
            subject(subjects::provider_run_command, MISROUTED),
            &json!({"lease_id": MISROUTED, "command": ["true"], "timeout_seconds": 5, "stream_output": true}),
        )
        .await?;
    assert!(
        is_backend_denial(&streamed),
        "a stream that never began still ends cleanly: {streamed}"
    );
    rig.finish().await
}

#[tokio::test]
async fn a_request_the_provider_cannot_honor_is_refused_only_for_held_leases() -> TestResult {
    let rig = Rig::start(agent()).await?;
    let request = |lease: &str| json!({"lease_id": lease, "command": ["true"], "require_launch_cancellation": true, "launch_key": "k"});

    let refused: Value = rig
        .ask(subject(subjects::provider_run_command, HELD), &request(HELD))
        .await?;

    assert_eq!(refused["outcome"], "denied");
    assert_eq!(refused["kind"], "invalid_spec");
    assert!(
        rig.unanswered(subject(subjects::provider_run_command, UNHELD), body(&request(UNHELD))?)
            .await?
    );
    rig.finish().await
}

#[tokio::test]
async fn poll_command_is_forwarded() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_poll_command,
        |lease| Ok(json!({"lease_id": lease, "execution_id": "e1"})),
        "running",
    )
    .await?;
    rig.finish().await
}

#[tokio::test]
async fn kill_command_is_forwarded() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_kill_command,
        |lease| Ok(json!({"lease_id": lease, "execution_id": "e1"})),
        "signaled",
    )
    .await?;
    rig.finish().await
}

#[tokio::test]
async fn write_stdin_is_forwarded() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_write_stdin,
        |lease| {
            let request = WriteStdinRequest::new(
                LeaseId::new(lease),
                ExecutionId::new("e1"),
                StdinSeq::new(4),
                StdinChunk::new(b"hi".to_vec())?,
                StdinEof::Keep,
            )?;
            Ok(serde_json::to_value(request)?)
        },
        "written",
    )
    .await?;
    rig.finish().await
}

#[tokio::test]
async fn write_files_is_forwarded() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_write_files,
        |lease| Ok(json!({"lease_id": lease, "files": [{"path": "/workspace/a", "content": [104, 105]}]})),
        "written",
    )
    .await?;
    rig.finish().await
}

#[tokio::test]
async fn read_file_is_forwarded() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_read_file,
        |lease| Ok(json!({"lease_id": lease, "path": "/workspace/a"})),
        "content",
    )
    .await?;
    rig.finish().await
}

#[tokio::test]
async fn create_user_is_forwarded() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_create_user,
        |lease| Ok(json!({"lease_id": lease, "username": "alice"})),
        "created",
    )
    .await?;
    rig.finish().await
}

#[tokio::test]
async fn create_group_is_forwarded() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_create_group,
        |lease| Ok(json!({"lease_id": lease, "groupname": "dev"})),
        "created",
    )
    .await?;
    rig.finish().await
}

#[tokio::test]
async fn modify_group_membership_is_forwarded() -> TestResult {
    let rig = Rig::start(agent()).await?;
    exercise(
        &rig,
        subjects::provider_modify_group_membership,
        |lease| Ok(json!({"lease_id": lease, "username": "alice", "groupname": "dev", "action": "add"})),
        "applied",
    )
    .await?;
    rig.finish().await
}

#[tokio::test]
async fn file_ops_are_answered_as_unsupported_for_held_leases_only() -> TestResult {
    let rig = Rig::start(agent()).await?;
    let request = |lease: &str| json!({"lease_id": lease, "op": {"op": "stat", "path": "/workspace/a"}});

    assert!(
        rig.unanswered(subject(subjects::provider_file_op, HELD), Bytes::from_static(b"nope"))
            .await?
    );
    assert!(
        rig.unanswered(subject(subjects::provider_file_op, UNHELD), body(&request(UNHELD))?)
            .await?
    );
    let answered: Value = rig
        .ask(subject(subjects::provider_file_op, HELD), &request(HELD))
        .await?;

    assert_eq!(answered["outcome"], "unsupported", "{answered}");
    rig.finish().await
}

#[tokio::test]
async fn get_domain_answers_for_held_leases_only() -> TestResult {
    let rig = Rig::start(agent()).await?;
    let request = |lease: &str, port: u16| json!({"lease_id": lease, "port": port});

    assert!(
        rig.unanswered(
            subject(subjects::provider_get_domain, HELD),
            Bytes::from_static(b"nope")
        )
        .await?
    );
    assert!(
        rig.unanswered(
            subject(subjects::provider_get_domain, UNHELD),
            body(&request(UNHELD, 1))?
        )
        .await?
    );
    let unexposed: Value = rig
        .ask(subject(subjects::provider_get_domain, HELD), &request(HELD, 1))
        .await?;
    let exposed: Value = rig
        .ask(
            subject(subjects::provider_get_domain, "01J3"),
            &request("01J3", super::support::EXPOSED_PORT),
        )
        .await?;

    assert_eq!(unexposed["outcome"], "port_not_exposed");
    assert_eq!(exposed["outcome"], "available");
    rig.finish().await
}

#[tokio::test]
async fn unsupported_broadcasts_are_refused_for_held_leases_only() -> TestResult {
    let rig = Rig::start(agent()).await?;
    let checkpoint = |lease: &str| {
        json!({
            "lease_id": lease,
            "target": {"registry": "registry.example", "repository": "snap", "artifact_type": "sandbox_rootfs_v1"},
        })
    };
    let policy = |lease: &str| json!({"lease_id": lease, "network_policy": "deny_all"});

    assert!(
        rig.unanswered(
            subject(subjects::provider_checkpoint_running, HELD),
            Bytes::from_static(b"nope")
        )
        .await?
    );
    assert!(
        rig.unanswered(
            subject(subjects::provider_checkpoint_running, UNHELD),
            body(&checkpoint(UNHELD))?
        )
        .await?
    );
    let snapshot: Value = rig
        .ask(subject(subjects::provider_checkpoint_running, HELD), &checkpoint(HELD))
        .await?;
    assert_eq!(snapshot["outcome"], "err", "{snapshot}");

    assert!(
        rig.unanswered(
            subject(subjects::provider_network_policy_update, UNHELD),
            body(&policy(UNHELD))?
        )
        .await?
    );
    let denied: Value = rig
        .ask(subject(subjects::provider_network_policy_update, HELD), &policy(HELD))
        .await?;
    assert_eq!(denied["outcome"], "denied", "{denied}");
    assert_eq!(denied["kind"], "invalid_network_policy");
    rig.finish().await
}

#[tokio::test]
async fn a_snapshot_export_is_refused_to_whoever_asked() -> TestResult {
    let rig = Rig::start(agent()).await?;
    let export = subjects::worker_snapshot_export(rig.provider.worker_id());

    let refused: Value = rig.ask(export.clone(), &json!({})).await?;
    rig.client.publish(export, Bytes::from_static(b"{}")).await?;
    rig.client.flush().await?;

    assert_eq!(refused["outcome"], "err", "{refused}");
    rig.finish().await
}

#[tokio::test]
async fn purge_named_reports_each_outcome() -> TestResult {
    let rig = Rig::start(agent()).await?;
    let subject = subjects::provider_purge_named(ProviderKind::Substrate);
    let request = |name: &str| json!({"tenant": "tenant-a", "name": name});

    assert!(rig.unanswered(subject.clone(), Bytes::from_static(b"nope")).await?);

    // The control plane has the actor but this worker never indexed it.
    let unowned: PurgeNamedResponse = rig.ask(subject.clone(), &request("ghost")).await?;
    assert!(matches!(unowned, PurgeNamedResponse::Failed { .. }), "{unowned:?}");

    rig.control.fail_get_with(crate::ate_client::AteError::NotFound {
        message: "no such actor".to_string(),
    });
    let unknown: PurgeNamedResponse = rig.ask(subject.clone(), &request("ghost")).await?;
    assert_eq!(unknown, PurgeNamedResponse::NotFound);
    rig.finish().await
}

#[tokio::test]
async fn an_indexed_named_actor_is_purged() -> TestResult {
    let rig = Rig::start(agent()).await?;
    let subject = subjects::provider_purge_named(ProviderKind::Substrate);
    let tenant = trogon_sandbox_contract::TenantId::new("tenant-a")?;
    let atespace = crate::naming::Atespace::for_tenant(&tenant)?;
    let actor = crate::naming::ActorName::for_named_lease(&atespace, "dev");
    let entry = crate::actor_index::ActorEntry {
        atespace: atespace.as_str().into(),
        actor: actor.as_str().into(),
        lease_id: LeaseId::new("01J7"),
        named: true,
        retained: false,
        owner: Some(crate::actor_index::ActorOwner {
            worker_id: rig.provider.worker_id().clone(),
            claim_id: crate::actor_index::ActorClaimId::generate(),
            actor_uid: Some(crate::actor_index::ActorUid::try_from(super::support::UID.to_string())?),
        }),
    };
    crate::actor_index::ActorIndex::put(rig.index.as_ref(), &entry).await?;

    let purged: PurgeNamedResponse = rig.ask(subject, &json!({"tenant": "tenant-a", "name": "dev"})).await?;

    assert_eq!(purged, PurgeNamedResponse::Purged);
    rig.finish().await
}

#[tokio::test]
async fn requests_without_a_reply_subject_are_dropped_by_every_handler() -> TestResult {
    let rig = Rig::start(agent()).await?;
    let request = body(&json!({"lease_id": HELD, "command": ["true"]}))?;
    let verbs = [
        Verb::Lease,
        Verb::Cancel,
        Verb::RunCommand,
        Verb::PollCommand,
        Verb::KillCommand,
        Verb::WriteStdin,
        Verb::WriteFiles,
        Verb::ReadFile,
        Verb::FileOp,
        Verb::GetDomain,
        Verb::CreateUser,
        Verb::CreateGroup,
        Verb::ModifyGroupMembership,
        Verb::CheckpointRunning,
        Verb::NetworkPolicyUpdate,
        Verb::SnapshotExport,
        Verb::PurgeNamed,
    ];
    for verb in verbs {
        let message = Message {
            subject: "sandbox.provider.substrate.any".into(),
            reply: None,
            length: request.len(),
            payload: request.clone(),
            headers: None,
            status: None,
            description: None,
        };
        let grants = Arc::new(RedispatchGuard::new());
        let handled = serve_message(
            verb,
            message,
            rig.client.clone(),
            rig.js.clone(),
            rig.provider.clone(),
            grants,
        );
        tokio::time::timeout(WAIT, handled).await??;
    }
    rig.finish().await
}

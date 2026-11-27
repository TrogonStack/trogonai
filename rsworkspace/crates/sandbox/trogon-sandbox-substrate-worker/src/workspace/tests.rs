use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use tokio::sync::Mutex;
use trogon_sandbox_actor_agent::{EXEC_PATH, ExecFrame};
use trogon_sandbox_contract::{
    CloneAuth, CommandOutput, EmptyObject, ExecutionId, LeaseError, LeaseId, RunCommandRequest, RunCommandResponse,
    RunCommandStartedResponse, SecretRef, TenantId, VolumeMode, WorkspaceSource,
};
use trogon_std::env::InMemoryEnv;

use super::clone_url::authenticated_url;
use super::redact::redact;
use super::*;
use crate::agent::{ActorAgent, ActorTarget};
use crate::naming::{ActorName, Atespace};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const URL: &str = "https://git.example/acme/app.git";

fn literal_secret(value: &str) -> SecretRef {
    SecretRef::Literal {
        value: value.to_string(),
    }
}

fn lease_id() -> LeaseId {
    LeaseId::new("01ARZ")
}

fn target() -> Result<ActorTarget, Box<dyn std::error::Error>> {
    Ok(ActorTarget::new(
        Atespace::for_tenant(&TenantId::new("tenant-a")?)?,
        ActorName::for_lease(&lease_id())?,
        8110,
    ))
}

fn clone_source(auth: CloneAuth) -> WorkspaceSource {
    WorkspaceSource::Clone {
        url: URL.to_string(),
        sha: "0123abc".to_string(),
        base_sha: None,
        auth,
    }
}

fn token_auth() -> CloneAuth {
    CloneAuth::BearerToken {
        token_ref: literal_secret("ghs_secret"),
    }
}

fn frame(response: RunCommandResponse) -> Result<Response, Box<dyn std::error::Error>> {
    let line = serde_json::to_string(&ExecFrame::Response(response))?;
    Ok(Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(Body::from(format!("{line}\n")))?)
}

type Requests = Arc<Mutex<Vec<RunCommandRequest>>>;

/// An agent whose exec endpoint records the request and answers with `response`.
async fn agent_answering(response: RunCommandResponse) -> Result<(ActorAgent, Requests), Box<dyn std::error::Error>> {
    let requests = Requests::default();
    let router = Router::new()
        .route(
            EXEC_PATH,
            post(
                |State((requests, response)): State<(Requests, RunCommandResponse)>,
                 axum::Json(request): axum::Json<RunCommandRequest>| async move {
                    requests.lock().await.push(request);
                    frame(response).map_err(|error| error.to_string())
                },
            ),
        )
        .with_state((requests.clone(), response));
    let agent = crate::agent::tests::serve(target()?, router).await?;
    Ok((agent, requests))
}

fn unroutable_agent() -> Result<ActorAgent, Box<dyn std::error::Error>> {
    Ok(ActorAgent::new("127.0.0.1:1".parse()?, target()?, &InMemoryEnv::new()))
}

fn completed(exit_code: i32, stderr: &str) -> RunCommandResponse {
    RunCommandResponse::Completed(CommandOutput {
        exit_code,
        stderr: stderr.as_bytes().to_vec(),
        ..CommandOutput::default()
    })
}

#[test]
fn an_unauthenticated_url_is_used_as_given() -> TestResult {
    assert_eq!(authenticated_url(URL, &CloneAuth::None(EmptyObject::default()))?, URL);
    Ok(())
}

#[test]
fn a_bearer_token_becomes_a_git_credential_in_the_url() -> TestResult {
    assert_eq!(
        authenticated_url(URL, &token_auth())?,
        "https://oauth2:ghs_secret@git.example/acme/app.git"
    );
    Ok(())
}

#[test]
fn basic_auth_keeps_the_username_and_the_path() -> TestResult {
    let out = authenticated_url(
        URL,
        &CloneAuth::Basic {
            username: "bot".to_string(),
            password_ref: literal_secret("hunter2"),
        },
    )?;
    assert_eq!(out, "https://bot:hunter2@git.example/acme/app.git");
    Ok(())
}

#[test]
fn a_secret_this_worker_cannot_resolve_fails_loudly() -> TestResult {
    let error = authenticated_url(
        URL,
        &CloneAuth::BearerToken {
            token_ref: SecretRef::NatsKv {
                bucket: "secrets".to_string(),
                key: "token".to_string(),
            },
        },
    )
    .err()
    .ok_or("an unresolvable secret must fail")?;
    assert!(matches!(error, WorkspaceError::Unsupported(_)), "got {error:?}");
    Ok(())
}

#[test]
fn a_url_without_a_scheme_is_rejected_rather_than_silently_unauthenticated() -> TestResult {
    for auth in [
        token_auth(),
        CloneAuth::Basic {
            username: "bot".to_string(),
            password_ref: literal_secret("pw"),
        },
    ] {
        let error = authenticated_url("git.example/a.git", &auth)
            .err()
            .ok_or("a schemeless url must fail")?;
        assert!(matches!(error, WorkspaceError::Failed(_)), "got {error:?}");
    }
    Ok(())
}

#[test]
fn a_failing_clone_does_not_echo_the_credential_back() {
    let stderr = "fatal: could not read from 'https://oauth2:ghs_secret@git.example/acme/app.git'\nfatal: exiting";
    let cleaned = redact(stderr, URL);
    assert!(!cleaned.contains("ghs_secret"), "{cleaned}");
    assert!(cleaned.contains(URL), "{cleaned}");
    assert!(cleaned.contains("exiting"), "{cleaned}");
}

#[test]
fn redaction_leaves_unrelated_output_alone() {
    assert_eq!(
        redact("fatal: repository not found", "https://git.example/a.git"),
        "fatal: repository not found"
    );
}

#[test]
fn redaction_leaves_output_alone_for_a_url_without_a_scheme() {
    assert_eq!(redact("fatal: oops", "git.example/a.git"), "fatal: oops");
}

#[test]
fn a_scheme_that_is_not_followed_by_the_tenants_host_is_kept() {
    assert_eq!(
        redact("see https://other.example/x for help", URL),
        "see https://other.example/x for help"
    );
}

#[tokio::test]
async fn an_empty_workspace_needs_no_actor_round_trip() -> TestResult {
    materialize(
        &unroutable_agent()?,
        &lease_id(),
        &WorkspaceSource::empty(),
        WORKSPACE_DIR,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn unsupported_sources_are_named_in_the_error() -> TestResult {
    let agent = unroutable_agent()?;
    for source in [
        WorkspaceSource::HostMount {
            path: "/srv/x".to_string(),
            access: Default::default(),
        },
        WorkspaceSource::S3Volume {
            bucket: "b".to_string(),
            prefix: "p".to_string(),
            mode: VolumeMode::ReadWrite,
            snapshot: None,
        },
    ] {
        let error = materialize(&agent, &lease_id(), &source, WORKSPACE_DIR)
            .await
            .err()
            .ok_or("the source is unsupported")?;
        assert!(matches!(error, WorkspaceError::Unsupported(_)), "got {error:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_clone_runs_inside_the_actor_with_the_credential_only_in_the_environment() -> TestResult {
    let (agent, requests) = agent_answering(completed(0, "")).await?;
    materialize(&agent, &lease_id(), &clone_source(token_auth()), "/workspace/app").await?;

    let requests = requests.lock().await;
    let request = requests.first().ok_or("the agent saw no request")?;
    let env = request.env.as_map();
    assert_eq!(
        env.get("SANDBOX_CLONE_URL").map(String::as_str),
        Some("https://oauth2:ghs_secret@git.example/acme/app.git")
    );
    assert_eq!(env.get("SANDBOX_CLONE_SHA").map(String::as_str), Some("0123abc"));
    assert_eq!(env.get("SANDBOX_CLONE_DIR").map(String::as_str), Some("/workspace/app"));
    assert!(!request.command.iter().any(|part| part.contains("ghs_secret")));
    assert_eq!(request.lease_id, lease_id());
    Ok(())
}

#[tokio::test]
async fn a_failed_clone_reports_its_exit_code_without_the_credential() -> TestResult {
    let stderr = "fatal: unable to access 'https://oauth2:ghs_secret@git.example/acme/app.git'";
    let (agent, _) = agent_answering(completed(128, stderr)).await?;
    let error = materialize(&agent, &lease_id(), &clone_source(token_auth()), WORKSPACE_DIR)
        .await
        .err()
        .ok_or("a nonzero exit must fail")?;
    let WorkspaceError::Failed(message) = error else {
        return Err("expected a failed preparation".into());
    };
    assert!(message.contains("128"), "{message}");
    assert!(!message.contains("ghs_secret"), "{message}");
    Ok(())
}

#[tokio::test]
async fn a_detached_clone_is_refused() -> TestResult {
    let started = RunCommandResponse::Started(RunCommandStartedResponse {
        execution_id: ExecutionId::new("e1"),
        origin: Default::default(),
    });
    let (agent, _) = agent_answering(started).await?;
    let error = materialize(
        &agent,
        &lease_id(),
        &clone_source(CloneAuth::None(EmptyObject::default())),
        WORKSPACE_DIR,
    )
    .await
    .err()
    .ok_or("a detached clone must fail")?;
    assert!(matches!(error, WorkspaceError::Failed(_)), "got {error:?}");
    Ok(())
}

#[tokio::test]
async fn a_denied_clone_is_a_failure() -> TestResult {
    let denied = RunCommandResponse::Denied(LeaseError::NoProviderAvailable);
    let (agent, _) = agent_answering(denied).await?;
    let error = materialize(
        &agent,
        &lease_id(),
        &clone_source(CloneAuth::None(EmptyObject::default())),
        WORKSPACE_DIR,
    )
    .await
    .err()
    .ok_or("a denied clone must fail")?;
    assert!(matches!(error, WorkspaceError::Failed(_)), "got {error:?}");
    Ok(())
}

#[tokio::test]
async fn a_clone_url_that_cannot_be_authenticated_never_reaches_the_actor() -> TestResult {
    let source = WorkspaceSource::Clone {
        url: "git.example/a.git".to_string(),
        sha: "abc".to_string(),
        base_sha: None,
        auth: token_auth(),
    };
    let error = materialize(&unroutable_agent()?, &lease_id(), &source, WORKSPACE_DIR)
        .await
        .err()
        .ok_or("a schemeless url must fail")?;
    assert!(matches!(error, WorkspaceError::Failed(_)), "got {error:?}");
    Ok(())
}

#[tokio::test]
async fn an_unreachable_actor_is_an_agent_error() -> TestResult {
    let error = materialize(
        &unroutable_agent()?,
        &lease_id(),
        &clone_source(CloneAuth::None(EmptyObject::default())),
        WORKSPACE_DIR,
    )
    .await
    .err()
    .ok_or("the actor is unreachable")?;
    assert!(matches!(error, WorkspaceError::Agent(_)), "got {error:?}");
    Ok(())
}

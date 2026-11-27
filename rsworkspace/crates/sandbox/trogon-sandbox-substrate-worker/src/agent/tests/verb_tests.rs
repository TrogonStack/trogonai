//! Each verb posts to its own path and decodes the agent's typed answer.

use axum::Router;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::routing::post;
use trogon_sandbox_actor_agent::{
    EXEC_KILL_PATH, EXEC_POLL_PATH, EXEC_WRITE_STDIN_PATH, FILES_READ_PATH, FILES_WRITE_PATH, GROUP_MEMBERSHIP_PATH,
    GROUPS_CREATE_PATH, USERS_CREATE_PATH,
};
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, ExecutionId, KillCommandResponse,
    ModifyGroupMembershipResponse, PollCommandResponse, ReadFileResponse, StdinChunk, StdinEof, StdinSeq,
    WriteFilesResponse, WriteStdinRequest, WriteStdinResponse,
};

use crate::agent::ActorAgent;
use crate::agent::tests::{parse, serve, target};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn agent_answering(path: &'static str, answer: &'static str) -> Result<ActorAgent, Box<dyn std::error::Error>> {
    serve(
        target()?,
        Router::new().route(
            path,
            post(move || async move {
                let mut response = axum::response::Response::new(axum::body::Body::from(answer));
                response
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                response
            }),
        ),
    )
    .await
}

#[tokio::test]
async fn write_stdin_posts_the_request_and_returns_the_agents_verdict() -> TestResult {
    let agent = serve(
        target()?,
        Router::new().route(
            EXEC_WRITE_STDIN_PATH,
            post(|axum::Json(request): axum::Json<WriteStdinRequest>| async move {
                assert_eq!(request.data().as_slice(), b"hi");
                axum::Json(WriteStdinResponse::Written {
                    next_seq: StdinSeq::new(request.seq().get() + 1),
                })
            }),
        ),
    )
    .await?;

    let request = WriteStdinRequest::new(
        LeaseId::new("01J0"),
        ExecutionId::new("e1"),
        StdinSeq::new(4),
        StdinChunk::new(b"hi".to_vec())?,
        StdinEof::Keep,
    )?;
    let response = agent.write_stdin(&request).await?;
    assert_eq!(
        response,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(5)
        }
    );
    Ok(())
}

#[tokio::test]
async fn poll_returns_the_agents_answer() -> TestResult {
    let agent = agent_answering(EXEC_POLL_PATH, r#"{"outcome":"running"}"#).await?;
    let response = agent
        .poll(&parse(r#"{"lease_id":"01J0","execution_id":"e1"}"#)?)
        .await?;
    assert_eq!(response, PollCommandResponse::Running);
    Ok(())
}

#[tokio::test]
async fn kill_returns_the_agents_answer() -> TestResult {
    let agent = agent_answering(EXEC_KILL_PATH, r#"{"outcome":"signaled"}"#).await?;
    let response = agent
        .kill(&parse(r#"{"lease_id":"01J0","execution_id":"e1"}"#)?)
        .await?;
    assert_eq!(response, KillCommandResponse::Signaled);
    Ok(())
}

#[tokio::test]
async fn write_files_returns_the_agents_answer() -> TestResult {
    let agent = agent_answering(FILES_WRITE_PATH, r#"{"outcome":"written"}"#).await?;
    let response = agent
        .write_files(&parse(
            r#"{"lease_id":"01J0","files":[{"path":"/workspace/a","content":[104,105]}]}"#,
        )?)
        .await?;
    assert_eq!(response, WriteFilesResponse::Written);
    Ok(())
}

#[tokio::test]
async fn read_file_returns_the_agents_answer() -> TestResult {
    let agent = agent_answering(FILES_READ_PATH, r#"{"outcome":"content","data":[104,105]}"#).await?;
    let response = agent
        .read_file(&parse(r#"{"lease_id":"01J0","path":"/workspace/a"}"#)?)
        .await?;
    assert_eq!(response, ReadFileResponse::Content { data: b"hi".to_vec() });
    Ok(())
}

#[tokio::test]
async fn create_user_returns_the_agents_answer() -> TestResult {
    let agent = agent_answering(USERS_CREATE_PATH, r#"{"outcome":"created","home_dir":"/home/alice"}"#).await?;
    let response = agent
        .create_user(&parse(r#"{"lease_id":"01J0","username":"alice"}"#)?)
        .await?;
    assert_eq!(
        response,
        CreateSandboxUserResponse::Created {
            home_dir: "/home/alice".to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn create_group_returns_the_agents_answer() -> TestResult {
    let agent = agent_answering(
        GROUPS_CREATE_PATH,
        r#"{"outcome":"created","shared_dir":"/shared/dev"}"#,
    )
    .await?;
    let response = agent
        .create_group(&parse(r#"{"lease_id":"01J0","groupname":"dev"}"#)?)
        .await?;
    assert_eq!(
        response,
        CreateSandboxGroupResponse::Created {
            shared_dir: "/shared/dev".to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn modify_group_membership_returns_the_agents_answer() -> TestResult {
    let agent = agent_answering(GROUP_MEMBERSHIP_PATH, r#"{"outcome":"applied"}"#).await?;
    let response = agent
        .modify_group_membership(&parse(
            r#"{"lease_id":"01J0","username":"alice","groupname":"dev","action":"add"}"#,
        )?)
        .await?;
    assert_eq!(response, ModifyGroupMembershipResponse::Applied);
    Ok(())
}

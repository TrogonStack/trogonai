use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use super::*;

struct Environment {
    entrypoint: Option<&'static str>,
    cwd: Option<&'static str>,
}

impl EnvironmentVariable for Environment {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        (key == ENTRYPOINT_VARIABLE)
            .then(|| self.entrypoint.map(OsString::from))
            .flatten()
    }
}

impl CurrentDirectory for Environment {
    type Error = &'static str;

    fn current_dir(&self) -> Result<PathBuf, Self::Error> {
        self.cwd.map(PathBuf::from).ok_or("no working directory")
    }
}

struct Stalled;

impl HttpExecute for Stalled {
    fn execute(&self, _request: reqwest::Request) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send {
        std::future::pending()
    }
}

fn client() -> BridgeClient<Stalled, Environment, DefaultGitProbe> {
    BridgeClient::new(
        Stalled,
        Environment {
            entrypoint: Some("sdk-ts"),
            cwd: None,
        },
        DefaultGitProbe,
    )
}

fn fetch(session_id: &str, base_url: &str, token: &str) -> FetchRemoteCredentialsOptions {
    FetchRemoteCredentialsOptions {
        session_id: session_id.to_owned(),
        base_url: base_url.to_owned(),
        access_token: SensitiveString::from(token),
        timeout: Some(Duration::from_secs(1)),
        trusted_device_token: None,
    }
}

fn create(base_url: &str, token: &str, cwd: Option<&str>) -> CreateCodeSessionOptions {
    CreateCodeSessionOptions {
        base_url: base_url.to_owned(),
        access_token: SensitiveString::from(token),
        title: "t".to_owned(),
        timeout: Some(Duration::from_secs(1)),
        tags: Vec::new(),
        git_context: None,
        cwd: cwd.map(str::to_owned),
        model: None,
        session_grouping_id: None,
    }
}

#[test]
fn maps_entrypoints_to_client_platforms() {
    let cases = [
        ("claude-vscode", "claude_code_vscode"),
        ("remote", "claude_code_remote"),
        ("claude-in-teams", "claude_code_remote"),
        ("sdk-py", "claude_code_sdk"),
        ("mcp", "claude_code_mcp"),
        ("claude-code-github-action", "claude_code_github_action"),
        ("local-agent", "claude_code_local_agent"),
        ("claude_in_slack", "claude_in_slack"),
        ("claude-in-slack", "claude-in-slack"),
        ("cli", "claude_code_cli"),
        ("", "claude_code_cli"),
        ("anything", "claude_code_cli"),
    ];
    for (entrypoint, platform) in cases {
        assert_eq!(client_platform(entrypoint), platform, "{entrypoint}");
    }
}

#[tokio::test]
async fn refuses_session_ids_that_are_not_path_safe_without_sending() {
    for id in ["", "a/b", "a b", "a\n", "caf\u{e9}"] {
        let outcome = client()
            .fetch_remote_credentials(&fetch(id, "http://127.0.0.1:1", "t"))
            .await;
        assert_eq!(
            outcome,
            RemoteCredentialsOutcome::Failure(CredentialsFailure::InvalidSessionId),
            "{id:?}"
        );
    }
}

#[tokio::test]
async fn reports_unavailable_when_the_request_cannot_be_built() {
    let bad_url = client()
        .fetch_remote_credentials(&fetch("cse_a", "not a url", "t"))
        .await;
    assert_eq!(bad_url, RemoteCredentialsOutcome::Unavailable);
    let bad_token = client()
        .fetch_remote_credentials(&fetch("cse_a", "http://127.0.0.1:1", "a\nb"))
        .await;
    assert_eq!(bad_token, RemoteCredentialsOutcome::Unavailable);
    let mut options = fetch("cse_a", "http://127.0.0.1:1", "t");
    options.trusted_device_token = Some(SensitiveString::from("a\nb"));
    assert_eq!(
        client().fetch_remote_credentials(&options).await,
        RemoteCredentialsOutcome::Unavailable
    );
    let outcome = client()
        .create_code_session(&create("not a url", "t", Some("/w")), None)
        .await;
    assert_eq!(outcome, Ok(CreateSessionOutcome::Unavailable));
}

#[tokio::test(start_paused = true)]
async fn gives_up_when_the_service_never_answers() {
    let outcome = client()
        .fetch_remote_credentials(&fetch("cse_a", "http://127.0.0.1:1", "t"))
        .await;
    assert_eq!(outcome, RemoteCredentialsOutcome::Unavailable);
}

#[tokio::test]
async fn reports_the_working_directory_failure_when_no_cwd_is_given() {
    let outcome = client()
        .create_code_session(&create("http://127.0.0.1:1", "t", None), None)
        .await;
    assert_eq!(outcome, Err("no working directory"));
}

#[tokio::test(start_paused = true)]
async fn asks_the_environment_for_the_working_directory() {
    let environment = Environment {
        entrypoint: None,
        cwd: Some("/work"),
    };
    let client = BridgeClient::new(Stalled, environment, DefaultGitProbe);
    let outcome = client
        .create_code_session(&create("http://127.0.0.1:1", "t", None), None)
        .await;
    assert_eq!(outcome, Ok(CreateSessionOutcome::Unavailable));
}

#[test]
fn builds_the_native_client() {
    assert!(BridgeClient::native().is_ok());
}

#[tokio::test]
async fn reports_unavailable_when_nothing_listens() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    drop(listener);
    let client = BridgeClient::native()?;
    let mut options = fetch("cse_a", &base, "t");
    options.timeout = None;
    assert_eq!(
        client.fetch_remote_credentials(&options).await,
        RemoteCredentialsOutcome::Unavailable
    );
    Ok(())
}

#[test]
fn describes_sub_reasons_that_are_not_strings() {
    let described = |value: Value| refusal(Some(&value));
    for (value, text) in [
        (json!(1), "(number, not a string)"),
        (json!(true), "(boolean, not a string)"),
        (json!([]), "(object, not a string)"),
        (json!({}), "(object, not a string)"),
    ] {
        assert_eq!(
            described(value),
            DeviceRefusal::Unrecognized {
                sub_reason: text.to_owned()
            }
        );
    }
}

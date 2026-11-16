use std::error::Error;
use std::process::Output;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acp_nats::{AcpPrefix, nats::AcpStream};
use async_nats::jetstream::context::GetStreamErrorKind;
use async_nats::jetstream::stream::{Config, Source, StorageType};
use bytes::Bytes;
use tokio::process::Command;
use trogon_nats::test_support::{CoreTestServer, JetStreamTestServer};
use trogon_std::env::{ReadEnv, SystemEnv};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, thiserror::Error)]
enum CredentialProbeError {
    #[error("the invalid credential probe did not finish")]
    Authentication(#[source] Box<dyn Error + Send + Sync>),
    #[error("the runtime permission probe did not finish")]
    Permission(#[source] Box<dyn Error + Send + Sync>),
    #[error("the operator provisioning probe did not finish")]
    Operator(#[source] Box<dyn Error + Send + Sync>),
}

async fn provision(address: &str, prefix: &str) -> TestResult<Output> {
    provision_as(address, prefix, None).await
}

async fn provision_as(address: &str, prefix: &str, credentials: Option<(&str, &str)>) -> TestResult<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_acp-nats-server"));
    command
        .env_clear()
        .env("NATS_URL", address)
        .env("NATS_TLS_DISABLED", "true")
        .args(["--acp-prefix", prefix, "provision"])
        .kill_on_drop(true);
    if let Some((user, password)) = credentials {
        command
            .env("NATS_USER", user)
            .env("NATS_PASSWORD", password)
            .env("ACP_OPERATION_TIMEOUT_SECS", "2");
    }
    if let Some(profile) = SystemEnv.var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    Ok(tokio::time::timeout(Duration::from_secs(10), command.output()).await??)
}

#[tokio::test]
async fn provisioning_requires_the_operator_role_instead_of_runtime_credentials() -> TestResult {
    let prefix = AcpPrefix::new("cli_authorized")?;
    let password = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos().to_string();
    let permissions = acp_nats::jetstream::streams::all_configs(&prefix)
        .iter()
        .flat_map(|config| {
            ["INFO", "CREATE", "UPDATE"].map(|action| format!("$JS.API.STREAM.{action}.{}", config.name))
        })
        .collect::<Vec<_>>();
    let config = format!(
        r#"jetstream {{ store_dir: "/tmp/cli-provision" }}
authorization {{ users: [
  {{ user: operator, password: "{password}", permissions: {{ publish: {}, subscribe: ["_INBOX.>"] }} }}
  {{ user: runtime, password: "{password}", permissions: {{ publish: ["cli_authorized.v1.>"], subscribe: ["_INBOX.>"] }} }}
] }}"#,
        serde_json::to_string(&permissions)?
    );
    let server = CoreTestServer::start_with_config(&config).await;
    let wrong_password = format!("{password}-wrong");
    let unauthenticated = provision_as(server.address(), prefix.as_str(), Some(("operator", &wrong_password)))
        .await
        .map_err(CredentialProbeError::Authentication)?;
    assert_eq!(unauthenticated.status.code(), Some(1));
    let refused = provision_as(server.address(), prefix.as_str(), Some(("runtime", &password)))
        .await
        .map_err(CredentialProbeError::Permission)?;
    assert_eq!(refused.status.code(), Some(1));
    let allowed = provision_as(server.address(), prefix.as_str(), Some(("operator", &password)))
        .await
        .map_err(CredentialProbeError::Operator)?;
    assert!(allowed.status.success(), "{}", String::from_utf8_lossy(&allowed.stderr));
    let client = async_nats::ConnectOptions::new()
        .user_and_password("operator".to_owned(), password)
        .connect(server.address())
        .await?;
    let js = async_nats::jetstream::new(client);
    for expected in acp_nats::jetstream::streams::all_configs(&prefix) {
        assert_eq!(
            js.get_stream(&expected.name).await?.cached_info().config.subjects,
            expected.subjects
        );
    }
    Ok(())
}

#[tokio::test]
async fn provisioning_creates_a_fresh_installation_and_exits() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let prefix = AcpPrefix::new("cli_fresh")?;
    let result = provision(server.address(), prefix.as_str()).await?;
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let js = server.jetstream().await;
    for config in acp_nats::jetstream::streams::all_configs(&prefix) {
        let stream = js.get_stream(&config.name).await?;
        assert_eq!(stream.cached_info().config.subjects, config.subjects);
    }
    Ok(())
}

#[tokio::test]
async fn provisioning_upgrades_an_existing_stream_without_replacing_operator_settings() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("cli_existing")?;
    let mut old = AcpStream::Commands.config(&prefix);
    old.subjects = vec!["cli_existing.v1.session.*.agent.prompt".to_owned()];
    old.storage = StorageType::Memory;
    old.max_messages = 80;
    old.description = Some("operator-managed command retention".to_owned());
    let stream = js.create_stream(old.clone()).await?;
    let created = stream.cached_info().created;
    let retained = js
        .publish(
            "cli_existing.v1.session.held.agent.prompt",
            Bytes::from_static(b"retained"),
        )
        .await?
        .await?;

    let result = provision(server.address(), prefix.as_str()).await?;
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let upgraded = js.get_stream(&old.name).await?;
    let info = upgraded.cached_info();
    assert_eq!(info.created, created);
    assert_eq!(info.config.storage, old.storage);
    assert_eq!(info.config.max_messages, old.max_messages);
    assert_eq!(info.config.description, old.description);
    assert_eq!(
        upgraded.get_raw_message(retained.sequence).await?.payload,
        b"retained".as_slice()
    );
    assert_eq!(info.config.subjects, AcpStream::Commands.config(&prefix).subjects);

    let repeated = provision(server.address(), prefix.as_str()).await?;
    assert!(repeated.status.success());
    assert_eq!(js.get_stream(&old.name).await?.cached_info().created, created);
    Ok(())
}

#[tokio::test]
async fn provisioning_refuses_an_incompatible_existing_stream() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("cli_sealed")?;
    let mut old = AcpStream::Commands.config(&prefix);
    old.subjects = vec!["cli_sealed.v1.session.*.agent.prompt".to_owned()];
    let stream = js.create_stream(old.clone()).await?;
    let created = stream.cached_info().created;
    let mut sealed = old.clone();
    sealed.sealed = true;
    js.update_stream(sealed).await?;

    let result = provision(server.address(), prefix.as_str()).await?;
    assert_eq!(result.status.code(), Some(1));
    let current = js.get_stream(&old.name).await?;
    assert!(current.cached_info().config.sealed);
    assert_eq!(current.cached_info().created, created);
    assert_eq!(current.cached_info().config.subjects, old.subjects);
    Ok(())
}

#[tokio::test]
async fn a_sealed_later_stream_is_refused_before_updating_any_earlier_stream() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("cli_preflight")?;
    let mut commands = AcpStream::Commands.config(&prefix);
    commands.subjects = vec!["cli_preflight.v1.session.*.agent.prompt".to_owned()];
    commands.storage = StorageType::Memory;
    let commands = js.create_stream(commands).await?;
    let before = commands.cached_info().clone();
    let global_ext = js.create_stream(AcpStream::GlobalExt.config(&prefix)).await?;
    let mut sealed = global_ext.cached_info().config.clone();
    sealed.sealed = true;
    js.update_stream(sealed).await?;
    let sealed = js
        .get_stream(&global_ext.cached_info().config.name)
        .await?
        .cached_info()
        .clone();

    let result = provision(server.address(), prefix.as_str()).await?;
    assert_eq!(result.status.code(), Some(1));
    let after = js.get_stream(&before.config.name).await?;
    assert_eq!(after.cached_info().created, before.created);
    assert_eq!(after.cached_info().config, before.config);
    let after_sealed = js.get_stream(&sealed.config.name).await?;
    assert_eq!(after_sealed.cached_info().created, sealed.created);
    assert_eq!(after_sealed.cached_info().config, sealed.config);
    for stream in [AcpStream::Responses, AcpStream::ClientOps, AcpStream::Global] {
        let error = js.get_stream(stream.stream_name(&prefix)).await.err();
        assert!(error.is_some_and(|error| matches!(error.kind(), GetStreamErrorKind::JetStream(error) if error.error_code() == async_nats::jetstream::ErrorCode::STREAM_NOT_FOUND)));
    }
    Ok(())
}

#[tokio::test]
async fn a_mirrored_later_stream_is_refused_before_updating_any_earlier_stream() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("cli_mirrored")?;
    let mut commands = AcpStream::Commands.config(&prefix);
    commands.subjects = vec!["cli_mirrored.v1.session.*.agent.prompt".to_owned()];
    commands.storage = StorageType::Memory;
    let commands = js.create_stream(commands).await?;
    let before = commands.cached_info().clone();
    let source = js
        .create_stream(Config {
            name: "CLI_MIRROR_SOURCE".to_owned(),
            subjects: vec!["cli_mirror_source.events.>".to_owned()],
            storage: StorageType::Memory,
            ..Default::default()
        })
        .await?;
    let mut mirrored = AcpStream::GlobalExt.config(&prefix);
    mirrored.subjects.clear();
    mirrored.storage = StorageType::Memory;
    mirrored.mirror = Some(Source {
        name: source.cached_info().config.name.clone(),
        ..Default::default()
    });
    let mirrored = js.create_stream(mirrored).await?;
    let before_mirror = mirrored.cached_info().clone();

    let result = provision(server.address(), prefix.as_str()).await?;
    assert_eq!(result.status.code(), Some(1));
    let after = js.get_stream(&before.config.name).await?;
    assert_eq!(after.cached_info().created, before.created);
    assert_eq!(after.cached_info().config, before.config);
    let after_mirror = js.get_stream(&before_mirror.config.name).await?;
    assert_eq!(after_mirror.cached_info().created, before_mirror.created);
    assert_eq!(after_mirror.cached_info().config, before_mirror.config);
    for stream in [AcpStream::Responses, AcpStream::ClientOps, AcpStream::Global] {
        let error = js.get_stream(stream.stream_name(&prefix)).await.err();
        assert!(error.is_some_and(|error| matches!(error.kind(), GetStreamErrorKind::JetStream(error) if error.error_code() == async_nats::jetstream::ErrorCode::STREAM_NOT_FOUND)));
    }
    Ok(())
}

#[tokio::test]
async fn provisioning_preserves_the_broker_error_when_a_filter_belongs_to_another_stream() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("cli_conflicting")?;
    let mut commands = AcpStream::Commands.config(&prefix);
    commands.subjects = vec!["cli_conflicting.v1.session.*.agent.prompt".to_owned()];
    let commands = js.create_stream(commands).await?;
    let before = commands.cached_info().clone();
    let conflicting = js
        .create_stream(Config {
            name: "CLI_CONFLICT_OWNER".to_owned(),
            subjects: vec!["cli_conflicting.v1.session.*.agent.archive".to_owned()],
            ..Default::default()
        })
        .await?;
    let before_conflicting = conflicting.cached_info().clone();

    let result = provision(server.address(), prefix.as_str()).await?;
    assert_eq!(result.status.code(), Some(1));
    let after = js.get_stream(&before.config.name).await?;
    assert_eq!(after.cached_info().created, before.created);
    assert_eq!(after.cached_info().config, before.config);
    let after_conflicting = js.get_stream(&before_conflicting.config.name).await?;
    assert_eq!(after_conflicting.cached_info().created, before_conflicting.created);
    assert_eq!(after_conflicting.cached_info().config, before_conflicting.config);
    Ok(())
}

#[test]
fn provisioning_help_is_available_without_connecting() -> TestResult {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_acp-nats-server"));
    command.env_clear().args(["provision", "--help"]);
    if let Some(profile) = SystemEnv.var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let result = command.output()?;
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout)?;
    assert!(output.contains("provision"));
    assert!(output.contains("--acp-prefix"));
    Ok(())
}

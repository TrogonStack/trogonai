mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{LeaseId, ReadFileRequest, RunCommandRequest, SandboxFileWrite, WriteFilesRequest};
use trogon_sandbox_fly_worker::{
    FlyApi, FlyApiError, FlyApiToken, FlyAppName, FlyEndpoint, FlyProviderError, FlyVolumeId, MachineId,
};
use trogon_sandbox_protocol::{FileNotFoundError, Provider, ReadFileError};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const CHUNK: usize = 24 * 1024;

async fn leased(env: &Env) -> Boxed<LeaseId> {
    let lease = LeaseId::new("lease-1");
    env.provider.lease(&request()?, &lease).await?;
    Ok(lease)
}

fn run_request(lease: &LeaseId, change: impl FnOnce(&mut Value)) -> Boxed<RunCommandRequest> {
    let mut wire = json!({"lease_id": lease.as_str(), "command": ["echo", "hi"], "timeout_seconds": 5});
    change(&mut wire);
    Ok(serde_json::from_value(wire)?)
}

fn write_request(lease: &LeaseId, files: Vec<SandboxFileWrite>) -> WriteFilesRequest {
    WriteFilesRequest {
        lease_id: lease.clone(),
        files,
        as_user: None,
    }
}

fn file(path: &str, content: Vec<u8>, mode: Option<u32>) -> SandboxFileWrite {
    SandboxFileWrite {
        path: path.to_owned(),
        content,
        mode,
    }
}

fn read_request(lease: &LeaseId, path: &str) -> Boxed<ReadFileRequest> {
    Ok(serde_json::from_value(
        json!({"lease_id": lease.as_str(), "path": path}),
    )?)
}

fn machine1() -> Boxed<MachineId> {
    Ok(MachineId::new("machine1")?)
}

/// The commands run after the lease's own workspace preparation.
fn exec_commands(fly: &Fly) -> Vec<Vec<String>> {
    fly.calls()
        .iter()
        .filter(|call| call.path.ends_with("/exec"))
        .skip(1)
        .map(|call| {
            call.body["command"]
                .as_array()
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default()
        })
        .collect()
}

async fn server_replying(template: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any()).respond_with(template).mount(&server).await;
    server
}

fn api_at(url: &str) -> Boxed<FlyApi> {
    Ok(FlyApi::new(
        FlyEndpoint::parse(url)?,
        FlyAppName::new(APP)?,
        FlyApiToken::new(TOKEN)?,
    )?)
}

fn rejected(error: &FlyProviderError) -> bool {
    matches!(error, FlyProviderError::Rejected(_))
}

#[tokio::test]
async fn run_command_parses_the_guest_capture() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    let output = env.provider.run_command(&run_request(&lease, |_| {})?).await?;
    assert_eq!(output.exit_code, 7);
    assert!(!output.stdout.is_empty());
    assert_eq!(exec_commands(&env.fly).len(), 1);
    Ok(())
}

#[tokio::test]
async fn run_command_rejects_unavailable_modes() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    for change in [
        json!({"detached": true}),
        json!({"stream_output": true}),
        json!({"run_as_user": "root"}),
        json!({"command": []}),
        json!({"timeout_seconds": 0}),
        json!({"timeout_seconds": 601}),
    ] {
        let request = run_request(&lease, |wire| {
            if let (Some(target), Some(extra)) = (wire.as_object_mut(), change.as_object()) {
                target.extend(extra.clone());
            }
        })?;
        let error = env
            .provider
            .run_command(&request)
            .await
            .err()
            .ok_or("expected a rejection")?;
        assert!(rejected(&error), "{error}");
    }
    assert!(exec_commands(&env.fly).is_empty());
    Ok(())
}

#[tokio::test]
async fn run_command_accepts_the_timeout_bounds() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    for seconds in [1, 600] {
        let request = run_request(&lease, |wire| wire["timeout_seconds"] = json!(seconds))?;
        env.provider.run_command(&request).await?;
    }
    Ok(())
}

#[tokio::test]
async fn run_command_resolves_the_working_directory() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.provider
        .run_command(&run_request(&lease, |wire| wire["cwd"] = json!("sub/dir"))?)
        .await?;
    env.provider
        .run_command(&run_request(&lease, |wire| wire["cwd"] = json!("/abs/dir"))?)
        .await?;
    env.provider.run_command(&run_request(&lease, |_| {})?).await?;
    let commands = exec_commands(&env.fly);
    assert_eq!(commands.len(), 3);
    assert!(commands[0][5].ends_with("/sub/dir"), "{}", commands[0][5]);
    assert_eq!(commands[1][5], "/abs/dir");
    assert!(!commands[2][5].is_empty());
    Ok(())
}

#[tokio::test]
async fn run_command_merges_lease_request_and_proxy_env() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.egress_proxy = Some("http://127.0.0.1:3128".to_owned());
        Ok(())
    })
    .await?;
    let lease = LeaseId::new("lease-1");
    let lease_request = request_with(|wire| wire["spec"]["env"] = json!({"KEPT": "lease", "SHADOWED": "lease"}))?;
    env.provider.lease(&lease_request, &lease).await?;
    let request = run_request(&lease, |wire| {
        wire["env"] = json!({"SHADOWED": "request", "EXTRA": "x"})
    })?;
    env.provider.run_command(&request).await?;
    let commands = exec_commands(&env.fly);
    let argv = commands.last().ok_or("no exec call")?;
    for expected in [
        "KEPT=lease",
        "SHADOWED=request",
        "EXTRA=x",
        "HTTP_PROXY=http://127.0.0.1:3128",
        "https_proxy=http://127.0.0.1:3128",
        "5s",
    ] {
        assert!(argv.iter().any(|part| part == expected), "missing {expected}");
    }
    assert!(!argv.iter().any(|part| part == "SHADOWED=lease"));
    assert_eq!(&argv[argv.len() - 2..], ["echo", "hi"]);
    Ok(())
}

#[tokio::test]
async fn run_command_reports_a_failed_guest_capture() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.fly
        .with(|state| state.exec_exit.push(("trogon-sandbox-capture".to_owned(), 9)));
    let error = env
        .provider
        .run_command(&run_request(&lease, |_| {})?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, FlyProviderError::Backend("guest capture failed")));
    Ok(())
}

#[tokio::test]
async fn run_command_rejects_a_malformed_capture() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.fly.with(|state| state.capture_reply = Some("garbage".to_owned()));
    let error = env
        .provider
        .run_command(&run_request(&lease, |_| {})?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, FlyProviderError::Capture(_)), "{error}");
    Ok(())
}

#[tokio::test]
async fn run_command_surfaces_an_api_failure() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.fly
        .with(|state| state.exec_http_fail.push("trogon-sandbox-capture".to_owned()));
    let error = env
        .provider
        .run_command(&run_request(&lease, |_| {})?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(
        matches!(error, FlyProviderError::Api(FlyApiError::Status(_))),
        "{error}"
    );
    Ok(())
}

#[tokio::test]
async fn operations_on_an_unheld_lease_fail() -> TestResult {
    let env = Env::start().await?;
    let lease = LeaseId::new("nobody");
    let error = env
        .provider
        .run_command(&run_request(&lease, |_| {})?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, FlyProviderError::NotHeld));
    let error = env
        .provider
        .write_files(&write_request(&lease, vec![]))
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, FlyProviderError::NotHeld));
    let error = env
        .provider
        .read_file(&read_request(&lease, "a")?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, ReadFileError::Other(FlyProviderError::NotHeld)));
    Ok(())
}

#[tokio::test]
async fn an_exec_that_outlasts_its_timeout_fails() -> TestResult {
    let env = Env::start().await?;
    env.fly.with(|state| state.exec_sleep = Some(Duration::from_secs(7)));
    let error = env
        .fly
        .api()?
        .exec(&machine1()?, vec!["true".to_owned()], Duration::ZERO)
        .await
        .err()
        .ok_or("expected a timeout")?;
    assert!(matches!(error, FlyApiError::Transport(_)), "{error}");
    Ok(())
}

#[tokio::test]
async fn write_files_writes_several_files() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    let files = vec![
        file("a.txt", b"alpha".to_vec(), None),
        file("/abs/b.txt", b"beta".to_vec(), Some(0o600)),
        file("empty.txt", Vec::new(), Some(0o7777)),
    ];
    env.provider.write_files(&write_request(&lease, files)).await?;
    let commands = exec_commands(&env.fly);
    let finishes: Vec<&Vec<String>> = commands
        .iter()
        .filter(|argv| argv.iter().any(|part| part.contains("chmod")))
        .collect();
    assert_eq!(finishes.len(), 3);
    let modes: Vec<&str> = finishes.iter().map(|argv| argv[argv.len() - 1].as_str()).collect();
    assert_eq!(modes, ["644", "600", "7777"]);
    assert!(finishes[1].iter().any(|part| part == "/abs/b.txt"));
    Ok(())
}

#[tokio::test]
async fn write_files_splits_large_content_into_chunks() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    let content = vec![5u8; CHUNK * 2 + 1];
    env.provider
        .write_files(&write_request(&lease, vec![file("big.bin", content, None)]))
        .await?;
    let appends = exec_commands(&env.fly)
        .iter()
        .filter(|argv| argv.iter().any(|part| part.contains("base64 -d")))
        .count();
    assert_eq!(appends, 3);
    Ok(())
}

#[tokio::test]
async fn write_files_rejects_invalid_requests() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    let mut as_user = write_request(&lease, vec![file("a", vec![1], None)]);
    as_user.as_user = Some(serde_json::from_value(json!("root"))?);
    let too_many: Vec<SandboxFileWrite> = (0..257)
        .map(|index| file(&format!("f{index}"), Vec::new(), None))
        .collect();
    let too_big = vec![file("huge", vec![0u8; 16 * 1024 * 1024 + 1], None)];
    let requests = [
        as_user,
        write_request(&lease, too_many),
        write_request(&lease, too_big),
        write_request(&lease, vec![file("", vec![1], None)]),
        write_request(&lease, vec![file("a\0b", vec![1], None)]),
        write_request(&lease, vec![file("a", vec![1], Some(0o10000))]),
    ];
    for request in &requests {
        let error = env
            .provider
            .write_files(request)
            .await
            .err()
            .ok_or("expected a rejection")?;
        assert!(rejected(&error), "{error}");
    }
    assert!(exec_commands(&env.fly).is_empty());
    Ok(())
}

#[tokio::test]
async fn write_files_accepts_the_file_count_limit() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    let files: Vec<SandboxFileWrite> = (0..256)
        .map(|index| file(&format!("f{index}"), Vec::new(), None))
        .collect();
    env.provider.write_files(&write_request(&lease, files)).await?;
    Ok(())
}

#[tokio::test]
async fn write_files_discards_the_temporary_file_after_a_failed_step() -> TestResult {
    for needle in ["base64 -d", "chmod"] {
        let env = Env::start().await?;
        let lease = leased(&env).await?;
        env.fly.with(|state| state.exec_exit.push((needle.to_owned(), 1)));
        let error = env
            .provider
            .write_files(&write_request(&lease, vec![file("a", b"data".to_vec(), None)]))
            .await
            .err()
            .ok_or("expected a failure")?;
        assert!(matches!(
            error,
            FlyProviderError::Backend("guest file operation failed")
        ));
        let discards = exec_commands(&env.fly)
            .iter()
            .filter(|argv| argv.iter().any(|part| part == "rm -f -- \"$1\""))
            .count();
        assert_eq!(discards, 1, "{needle}");
    }
    Ok(())
}

#[tokio::test]
async fn write_files_tolerates_a_failed_discard() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.fly.with(|state| {
        state.exec_exit.push(("chmod".to_owned(), 1));
        state.exec_http_fail.push("rm -f".to_owned());
    });
    let error = env
        .provider
        .write_files(&write_request(&lease, vec![file("a", b"x".to_vec(), None)]))
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, FlyProviderError::Backend(_)));
    Ok(())
}

#[tokio::test]
async fn write_files_fails_when_the_temporary_file_cannot_be_made() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.fly.with(|state| state.exec_exit.push(("mktemp".to_owned(), 1)));
    let error = env
        .provider
        .write_files(&write_request(&lease, vec![file("a", vec![1], None)]))
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, FlyProviderError::Backend(_)));
    Ok(())
}

#[tokio::test]
async fn read_file_returns_the_content() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    assert_eq!(env.provider.read_file(&read_request(&lease, "rel.txt")?).await?, [1, 2]);
    assert_eq!(
        env.provider.read_file(&read_request(&lease, "/abs.txt")?).await?,
        [1, 2]
    );
    let commands = exec_commands(&env.fly);
    assert!(commands[0][4].ends_with("/rel.txt"), "{}", commands[0][4]);
    assert_eq!(commands[1][4], "/abs.txt");
    Ok(())
}

#[tokio::test]
async fn read_file_reports_a_missing_file() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.fly.with(|state| state.read_exit_code = 3);
    let error = env
        .provider
        .read_file(&read_request(&lease, "/nope")?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, ReadFileError::NotFound(FileNotFoundError { ref path }) if path == "/nope"));
    Ok(())
}

#[tokio::test]
async fn read_file_reports_other_guest_failures() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.fly.with(|state| state.read_exit_code = 1);
    let error = env
        .provider
        .read_file(&read_request(&lease, "/dir")?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(
        error,
        ReadFileError::Other(FlyProviderError::Api(FlyApiError::GuestRead(1)))
    ));
    Ok(())
}

#[tokio::test]
async fn read_file_enforces_the_size_cap() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.fly.with(|state| state.read_always_full = true);
    let error = env
        .provider
        .read_file(&read_request(&lease, "/endless")?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, ReadFileError::Other(FlyProviderError::Rejected(_))));
    assert!(exec_commands(&env.fly).len() > 600);
    Ok(())
}

#[tokio::test]
async fn read_file_rejects_a_per_user_read() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    let request: ReadFileRequest =
        serde_json::from_value(json!({"lease_id": lease.as_str(), "path": "a", "as_user": "root"}))?;
    let error = env
        .provider
        .read_file(&request)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, ReadFileError::Other(FlyProviderError::Rejected(_))));
    Ok(())
}

#[tokio::test]
async fn read_file_chunk_validates_its_position() -> TestResult {
    let env = Env::start().await?;
    let api = env.fly.api()?;
    let id = machine1()?;
    for (path, offset) in [("", 0), ("a\0b", 0), ("a", 1), ("a", CHUNK + 1)] {
        let error = api
            .read_file_chunk(&id, path, offset)
            .await
            .err()
            .ok_or("expected a failure")?;
        assert!(matches!(error, FlyApiError::InvalidReadPosition));
    }
    let (chunk, full) = api.read_file_chunk(&id, "a", CHUNK).await?;
    assert_eq!(chunk, [1, 2]);
    assert!(!full);
    assert_eq!(env.fly.count("POST", "/exec"), 1);
    Ok(())
}

#[tokio::test]
async fn read_file_chunk_rejects_bad_base64() -> TestResult {
    let env = Env::start().await?;
    env.fly
        .with(|state| state.read_reply = Some("!!not base64!!".to_owned()));
    let error = env
        .fly
        .api()?
        .read_file_chunk(&machine1()?, "a", 0)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Encoding(_)));
    Ok(())
}

#[tokio::test]
async fn the_api_covers_machines_and_volumes() -> TestResult {
    let env = Env::start().await?;
    let api = env.fly.api()?;
    assert_eq!(api.app().as_str(), APP);
    let created = api
        .create(&json!({"name": "m", "config": {"metadata": {"k": "v"}}, "skip_launch": true}))
        .await?;
    assert_eq!(created.id.as_str(), "machine1");
    assert_eq!(created.metadata("k"), Some("v"));
    assert_eq!(api.list().await?.len(), 1);
    let id = machine1()?;
    assert!(api.get(&id).await?.is_some());
    assert!(api.get(&MachineId::new("missing")?).await?.is_none());
    api.start(&id).await?;
    api.suspend(&id).await?;
    let output = api.exec(&id, vec!["true".to_owned()], Duration::from_secs(3)).await?;
    assert_eq!(output.exit_code, 0);
    api.destroy(&id).await?;
    api.destroy(&id).await?;
    assert!(api.get(&id).await?.is_none());
    let query = env
        .fly
        .calls()
        .iter()
        .find(|call| call.method == "DELETE")
        .and_then(|call| call.query.clone());
    assert_eq!(query.as_deref(), Some("force=true"));

    let volume = FlyVolumeId::new("vol_test")?;
    assert_eq!(api.volume(&volume).await?.id.as_str(), "vol_test");
    let made = api
        .create_volume(&json!({"name": "v", "region": "iad", "size_gb": 1}))
        .await?;
    assert_eq!(api.list_volumes().await?.len(), 1);
    api.delete_volume(&made.id).await?;
    api.delete_volume(&made.id).await?;
    assert!(api.list_volumes().await?.is_empty());
    let missing = api
        .volume(&FlyVolumeId::new("vol_missing")?)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(missing, FlyApiError::NotFound("the volume")));
    Ok(())
}

#[tokio::test]
async fn the_api_creates_and_lists_snapshots() -> TestResult {
    let env = Env::start().await?;
    let api = env.fly.api()?;
    let volume = FlyVolumeId::new("vol_test")?;
    assert_eq!(api.create_snapshot(&volume).await?, "vs_snap1");
    assert_eq!(api.list_snapshots(&volume).await?.len(), 1);
    env.fly.with(|state| state.snapshot_without_id = true);
    let error = api.create_snapshot(&volume).await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::SnapshotWithoutId));
    Ok(())
}

#[tokio::test]
async fn a_snapshot_falls_back_to_the_backup_id() -> TestResult {
    let server =
        server_replying(ResponseTemplate::new(200).set_body_json(json!({"Msg": {"backup": {"id": "01ONLY"}}}))).await;
    let id = api_at(&server.uri())?
        .create_snapshot(&FlyVolumeId::new("vol_test")?)
        .await?;
    assert_eq!(id, "01ONLY");
    Ok(())
}

#[tokio::test]
async fn the_api_maps_http_failures() -> TestResult {
    let env = Env::start().await?;
    let api = env.fly.api()?;
    env.fly.with(|state| state.list_fails = true);
    let error = api.list().await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Status(code) if code.as_u16() == 503));
    env.fly.with(|state| state.create_status = Some(404));
    let error = api.create(&json!({})).await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Status(code) if code.as_u16() == 404));
    let wrong_token = FlyApi::new(
        FlyEndpoint::parse(&env.fly.url())?,
        FlyAppName::new(APP)?,
        FlyApiToken::new("other")?,
    )?;
    let error = wrong_token.list().await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Status(code) if code.as_u16() == 401));
    Ok(())
}

#[tokio::test]
async fn a_missing_list_endpoint_is_not_found() -> TestResult {
    let server = server_replying(ResponseTemplate::new(404)).await;
    let error = api_at(&server.uri())?.list().await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::NotFound("the app")));
    Ok(())
}

#[tokio::test]
async fn an_empty_or_no_content_body_is_null() -> TestResult {
    for template in [ResponseTemplate::new(204), ResponseTemplate::new(200)] {
        let server = server_replying(template).await;
        let api = api_at(&server.uri())?;
        api.start(&machine1()?).await?;
        let error = api.list().await.err().ok_or("expected a failure")?;
        assert!(matches!(error, FlyApiError::Decode(_)));
    }
    Ok(())
}

#[tokio::test]
async fn a_malformed_body_is_a_decode_error() -> TestResult {
    let server = server_replying(ResponseTemplate::new(200).set_body_string("{not json")).await;
    let api = api_at(&server.uri())?;
    let error = api.list().await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Decode(_)));
    let error = api.get(&machine1()?).await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Decode(_)));
    Ok(())
}

#[tokio::test]
async fn a_body_of_the_wrong_shape_is_a_decode_error() -> TestResult {
    let server = server_replying(ResponseTemplate::new(200).set_body_json(json!({"unexpected": true}))).await;
    let error = api_at(&server.uri())?.list().await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Decode(_)));
    Ok(())
}

#[tokio::test]
async fn redirects_are_not_followed() -> TestResult {
    let target = MockServer::start().await;
    let server =
        server_replying(ResponseTemplate::new(302).insert_header("location", format!("{}/v1/stolen", target.uri())))
            .await;
    let error = api_at(&server.uri())?.list().await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Status(code) if code.as_u16() == 302));
    assert!(target.received_requests().await.unwrap_or_default().is_empty());
    Ok(())
}

async fn refused_url() -> Boxed<String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(format!("http://127.0.0.1:{port}"))
}

#[tokio::test]
async fn a_refused_connection_is_a_transport_error() -> TestResult {
    let api = api_at(&refused_url().await?)?;
    let error = api.list().await.err().ok_or("expected a failure")?;
    assert!(matches!(error, FlyApiError::Transport(_)));
    Ok(())
}

#[tokio::test]
async fn every_api_error_has_a_message() -> TestResult {
    let transport = reqwest::Client::new()
        .get(refused_url().await?)
        .send()
        .await
        .err()
        .ok_or("expected a failure")?;
    let decode = serde_json::from_str::<Value>("{").err().ok_or("expected a failure")?;
    let encoding = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, "!")
        .err()
        .ok_or("expected a failure")?;
    let errors = vec![
        FlyApiError::Client(transport),
        FlyApiError::Url(url::ParseError::EmptyHost),
        FlyApiError::Status(reqwest::StatusCode::BAD_GATEWAY),
        FlyApiError::Decode(decode),
        FlyApiError::NotFound("the app"),
        FlyApiError::SnapshotWithoutId,
        FlyApiError::InvalidReadPosition,
        FlyApiError::GuestRead(4),
        FlyApiError::Encoding(encoding),
        FlyApiError::FileNotFound(FileNotFoundError { path: "/x".into() }),
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
        assert!(!format!("{error:?}").is_empty());
    }
    let transport = reqwest::Client::new()
        .get(refused_url().await?)
        .send()
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(!FlyApiError::from(transport).to_string().is_empty());
    Ok(())
}

#[test]
fn tokens_are_validated_and_hidden() -> TestResult {
    assert!(FlyApiToken::new("").is_err());
    assert!(FlyApiToken::new("bad\nvalue").is_err());
    let token = FlyApiToken::new("secret-value")?;
    assert_eq!(format!("{token:?}"), "FlyApiToken(..)");
    assert_eq!(format!("{:?}", token.clone()), "FlyApiToken(..)");
    Ok(())
}

#[test]
fn endpoints_must_be_secure_and_plain() -> TestResult {
    assert!(FlyEndpoint::parse("https://api.machines.dev").is_ok());
    for url in ["http://127.0.0.1:1", "http://localhost:1"] {
        assert!(FlyEndpoint::parse(url).is_ok());
    }
    for url in [
        "http://example.com",
        "ftp://127.0.0.1",
        "https://user@example.com",
        "https://user:pass@example.com",
        "https://example.com/?q=1",
        "https://example.com/#frag",
        "not a url",
    ] {
        assert!(FlyEndpoint::parse(url).is_err(), "{url}");
    }
    let endpoint = FlyEndpoint::parse("https://api.machines.dev")?;
    assert_eq!(endpoint.clone(), endpoint);
    assert!(!format!("{endpoint:?}").is_empty());
    Ok(())
}

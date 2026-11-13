#![cfg(feature = "test-support")]

use serde_json::json;
use std::{
    error::Error as StdError,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, Semaphore},
    time::{sleep, timeout},
};
use trogon_claude_agent_sdk::*;

type TestResult = Result<(), Box<dyn StdError>>;

fn options(mode: &str) -> Result<ClaudeAgentOptions, Box<dyn StdError>> {
    let mut options = ClaudeAgentOptions {
        cli_path: Some(std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?),
        model: Some(format!("fixture:{mode}").into()),
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(2)));
    options.limits.shutdown = trogon_claude_agent_sdk::Shutdown::Strict(Timeout::new(Duration::from_millis(200)));
    // The runtime force-kills the fixture on shutdown, possibly while it is writing its coverage
    // profile at exit, which leaves a corrupt profile. The fixture runs no crate code, so its
    // profile is discarded.
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    options.limits.run_end_timeout = Timeout::new(Duration::from_secs(2));
    Ok(options)
}

async fn receive_result(client: &mut ClaudeSdkClient) -> Result<String, Box<dyn StdError>> {
    timeout(Duration::from_secs(4), async {
        while let Some(message) = client.next_message().await? {
            if let Message::Result(result) = message {
                return Ok(result.result.unwrap_or_default());
            }
        }
        Err(std::io::Error::other("missing result").into())
    })
    .await?
}

#[tokio::test]
async fn coalesced_input_count_ends_the_run_after_one_result() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("queued-coalesce")?).await?;
    let handle = client.handle();
    let input = [
        UserInput::from(Prompt::from("one")),
        UserInput::from(Prompt::from("two")),
    ];
    let writer = tokio::spawn(async move { handle.stream_input(futures_util::stream::iter(input)).await });
    assert_eq!(receive_result(&mut client).await?, "coalesced");
    assert!(
        timeout(Duration::from_secs(3), async {
            while let Some(message) = client.next_message().await? {
                if matches!(message, Message::Result(_)) {
                    return Err(std::io::Error::other("a second result followed the coalesced one").into());
                }
            }
            Ok::<_, Box<dyn StdError>>(())
        })
        .await?
        .is_ok()
    );
    writer.await??;
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn explicit_turn_helper_keeps_the_single_result_contract() -> TestResult {
    let mut response = query_turn(Prompt::from("background"), options("full-run")?).await?;
    let mut results = 0;
    while let Some(message) = response.next_message().await? {
        if matches!(message, Message::Result(_)) {
            results += 1;
        }
    }
    assert_eq!(results, 1);
    Ok(())
}

struct GatedStore {
    inner: InMemorySessionStore,
    entered: AtomicUsize,
    gates: [Semaphore; 2],
    observed: Notify,
}
impl GatedStore {
    fn new() -> Self {
        Self {
            inner: InMemorySessionStore::default(),
            entered: AtomicUsize::new(0),
            gates: [Semaphore::new(0), Semaphore::new(0)],
            observed: Notify::new(),
        }
    }
}
impl SessionStore for GatedStore {
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            let index = self.entered.fetch_add(1, Ordering::SeqCst);
            self.observed.notify_waiters();
            if let Some(gate) = self.gates.get(index) {
                gate.acquire().await.map_err(SessionStoreError::backend)?.forget();
            }
            self.inner.append(key, entries).await
        })
    }
    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        self.inner.load(key)
    }
}

async fn wait_appends(store: &GatedStore, count: usize) -> TestResult {
    timeout(Duration::from_secs(3), async {
        while store.entered.load(Ordering::SeqCst) < count {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

fn store_options(
    store: Arc<dyn SessionStore>,
    directory: &std::path::Path,
) -> Result<SessionStoreOptions, Box<dyn StdError>> {
    let mut options = SessionStoreOptions::from(store);
    options.config_directory = Some(directory.to_path_buf());
    options.project_key = Some(SessionProjectKey::new("fixture-project")?);
    options.limits.append_timeout = Timeout::new(Duration::from_secs(3));
    options.limits.flush_timeout = Some(Timeout::new(Duration::from_secs(3)));
    Ok(options)
}

#[tokio::test]
async fn each_result_waits_for_its_own_mirror_fence_and_controls_progress() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Arc::new(GatedStore::new());
    let mut options = options("mirror-two")?;
    options.session_store = Some(store_options(store.clone(), directory.path())?);
    let permission = Arc::new(Notify::new());
    let permission_observed = permission.clone();
    options.permission_handler = Some(Arc::new(move |_: ToolPermissionRequest| {
        let observed = permission_observed.clone();
        async move {
            observed.notify_one();
            PermissionDecision::allow()
        }
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    let _ = client.next_message().await?;
    client.send_prompt(Prompt::from("mirror")).await?;
    wait_appends(&store, 1).await?;
    timeout(Duration::from_secs(2), permission.notified()).await?;
    let Some(Message::System(marker)) = client.next_message().await? else {
        return Err(std::io::Error::other("missing enqueue marker").into());
    };
    assert_eq!(marker.subtype, "mirror_enqueued");
    // The permission request came after the result, so the driver has already held the result
    // behind the fence: a single poll is enough to show it was not delivered.
    assert!(timeout(Duration::ZERO, client.next_message()).await.is_err());
    store.gates[0].add_permits(1);
    assert_eq!(receive_result(&mut client).await?, "mirror-one");
    wait_appends(&store, 2).await?;
    loop {
        match timeout(Duration::from_millis(30), client.next_message()).await {
            Err(_) => break,
            Ok(Ok(Some(Message::Result(_)))) => {
                return Err(std::io::Error::other("second result escaped its mirror fence").into());
            }
            Ok(Ok(Some(_))) => {}
            Ok(outcome) => {
                outcome?;
                return Err(std::io::Error::other("session closed before second mirror fence").into());
            }
        }
    }
    store.gates[1].add_permits(1);
    assert_eq!(receive_result(&mut client).await?, "mirror-two");
    let key = SessionKey::new(
        SessionProjectKey::new("fixture-project")?,
        SessionId::from("fixture-session"),
    )?;
    assert_eq!(store.load(&key).await?.map(|entries| entries.len()), Some(2));
    client.disconnect().await?;
    Ok(())
}

struct FailedStore;
impl SessionStore for FailedStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async {
            Err(SessionStoreError::backend(std::io::Error::other(
                "fixture backend failure",
            )))
        })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test]
async fn s20_a_final_mirror_failure_does_not_fail_disconnect() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut options = options("mirror-close")?;
    options.session_store = Some(store_options(Arc::new(FailedStore), directory.path())?);
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("mirror")).await?;
    timeout(Duration::from_secs(4), async {
        loop {
            match client.next_message().await? {
                Some(Message::System(message)) if message.subtype == "mirror_enqueued" => return Ok(()),
                Some(_) => {}
                None => return Err(std::io::Error::other("session closed before mirror_enqueued").into()),
            }
        }
    })
    .await?
    .map_err(|error: Box<dyn StdError>| error)?;
    client.disconnect().await?;
    Ok(())
}

fn echo_registry(counter: Arc<AtomicUsize>) -> Result<Arc<dyn SdkMcpServer>, Box<dyn StdError>> {
    let schema = McpToolInputSchema::new(serde_json::from_value(json!({"type":"object"}))?);
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("echo"),
        description: "Echo".into(),
        input_schema: schema,
        handler: Arc::new(move |request: McpToolRequest| {
            let calls = counter.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(McpToolOutput::text(
                    request.arguments["value"].as_str().unwrap_or_default(),
                ))
            }
        }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    Ok(Arc::new(LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("local"),
        version: Some("1".into()),
        tools: Some(tool.into()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    })))
}

#[tokio::test]
async fn rejected_mcp_replacement_still_disconnects_removed_in_process_servers() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let rejected_calls = Arc::new(AtomicUsize::new(0));
    let reconnected_calls = Arc::new(AtomicUsize::new(0));
    let mut client = ClaudeSdkClient::connect(options("mcp")?).await?;
    let handle = client.handle();
    let servers = SdkMcpServers::from([(McpServerName::from("local"), echo_registry(calls.clone())?)]);
    let installed = handle.set_mcp_servers(McpServers::new(), servers).await?;
    assert_eq!(installed.added, ["local"]);
    let rejected = SdkMcpServers::from([(McpServerName::from("reject"), echo_registry(rejected_calls.clone())?)]);
    assert!(matches!(
        handle.set_mcp_servers(McpServers::new(), rejected).await,
        Err(Error::ControlRejected {
            operation: ControlOperation::McpSetServers,
            ..
        })
    ));
    let reconnected = SdkMcpServers::from([(McpServerName::from("local"), echo_registry(reconnected_calls.clone())?)]);
    handle.set_mcp_servers(McpServers::new(), reconnected).await?;
    client.send_prompt(Prompt::from("mcp")).await?;
    assert_eq!(receive_result(&mut client).await?, "from-fixture");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(rejected_calls.load(Ordering::SeqCst), 0);
    assert_eq!(reconnected_calls.load(Ordering::SeqCst), 1);
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn retained_mcp_server_keeps_the_existing_connection_session() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let replacement_calls = Arc::new(AtomicUsize::new(0));
    let mut options = options("mcp-retain")?;
    options
        .sdk_mcp_servers
        .insert(McpServerName::from("local"), echo_registry(calls.clone())?);
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("first")).await?;
    assert_eq!(receive_result(&mut client).await?, "from-fixture");
    client
        .handle()
        .set_mcp_servers(
            McpServers::new(),
            SdkMcpServers::from([(McpServerName::from("local"), echo_registry(replacement_calls.clone())?)]),
        )
        .await?;
    client.send_prompt(Prompt::from("second")).await?;
    assert_eq!(receive_result(&mut client).await?, "from-fixture");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(replacement_calls.load(Ordering::SeqCst), 0);
    client.disconnect().await?;
    Ok(())
}

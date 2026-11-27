mod support;

use futures::StreamExt;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use support::{FakeGuest, GuestEnd, Server, TestResult, json, start_nats};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::{
    LeaseId, TerminalArgument, TerminalDimension, TerminalError, TerminalId, TerminalInput, TerminalOp, TerminalOutput,
    TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::subjects::{terminal_output, worker_lease_activity};
use trogon_sandbox_protocol::terminal_output_bytes;
use trogon_sandbox_ssh_gateway::{HubSshHost, SshTerminalHost, TerminalHub, run_terminal_handler};

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn terminal(name: &str) -> TestResult<TerminalId> {
    Ok(TerminalId::new(name)?)
}

fn dim(value: u16) -> TestResult<TerminalDimension> {
    Ok(TerminalDimension::new(value)?)
}

fn request(lease: &LeaseId, id: &str, op: TerminalOp) -> TestResult<TerminalRequest> {
    Ok(TerminalRequest {
        lease_id: lease.clone(),
        terminal_id: terminal(id)?,
        op,
    })
}

fn open_op(cmd: &[&str], env: &[(&str, &str)], cwd: Option<&str>) -> TestResult<TerminalOp> {
    Ok(TerminalOp::Open {
        cols: dim(80)?,
        rows: dim(24)?,
        cmd: cmd
            .iter()
            .map(|word| TerminalArgument::new(*word))
            .collect::<Result<_, _>>()?,
        env: env
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>(),
        cwd: cwd.map(str::to_string),
        resume_from: None,
    })
}

fn input(bytes: &[u8]) -> TestResult<TerminalOp> {
    Ok(TerminalOp::Input {
        data: TerminalInput::new(STANDARD.encode(bytes))?,
    })
}

struct Setup {
    _server: Server,
    client: async_nats::Client,
    guest: Arc<FakeGuest>,
    hub: Arc<TerminalHub>,
}

async fn setup() -> TestResult<Setup> {
    let (server, client) = start_nats().await?;
    let guest = FakeGuest::holding(&["lease-1", "lease-2"]);
    let hub = Arc::new(TerminalHub::new(guest.clone(), client.clone()));
    Ok(Setup {
        _server: server,
        client,
        guest,
        hub,
    })
}

async fn open(setup: &Setup, id: &str) -> TestResult<GuestEnd> {
    let response = setup
        .hub
        .handle(&request(&lease(), id, open_op(&[], &[], None)?)?)
        .await;
    assert!(matches!(response, TerminalResponse::Opened { .. }), "{response:?}");
    setup.guest.take_end().await
}

async fn next_output(subscription: &mut async_nats::Subscriber) -> TestResult<TerminalOutput> {
    let message = tokio::time::timeout(Duration::from_secs(10), subscription.next())
        .await?
        .ok_or("subscription ended")?;
    Ok(serde_json::from_slice(&message.payload)?)
}

#[tokio::test]
async fn opening_a_terminal_starts_a_default_shell() -> TestResult {
    let setup = setup().await?;
    // The guest end stays alive: dropping it hangs up the terminal.
    let _end = open(&setup, "t1").await?;
    assert_eq!(setup.hub.open_count(&lease()), 1);
    assert_eq!(
        setup.guest.opened.lock().map_err(|error| error.to_string())?[0],
        ("xterm-256color".to_string(), None, (80, 24))
    );
    // Opening the same terminal again for the same lease is a no-op.
    let again = setup
        .hub
        .handle(&request(&lease(), "t1", open_op(&[], &[], None)?)?)
        .await;
    assert!(matches!(again, TerminalResponse::Opened { .. }));
    assert_eq!(setup.hub.open_count(&lease()), 1);
    Ok(())
}

#[tokio::test]
async fn the_command_line_carries_cwd_env_and_arguments() -> TestResult {
    let setup = setup().await?;
    let op = open_op(&["ls", "it's"], &[("TERM", "vt100"), ("A_B", "x y")], Some("/tmp"))?;
    setup.hub.handle(&request(&lease(), "t1", op)?).await;
    let opened = setup.guest.opened.lock().map_err(|error| error.to_string())?[0].clone();
    assert_eq!(opened.0, "vt100");
    assert_eq!(
        opened.1.as_deref(),
        Some("cd '/tmp' || exit 1; export A_B='x y'; export TERM='vt100'; exec 'ls' 'it'\\''s'")
    );
    let shell = open_op(&[], &[("A", "1")], None)?;
    setup.hub.handle(&request(&lease(), "t2", shell)?).await;
    let second = setup.guest.opened.lock().map_err(|error| error.to_string())?[1].clone();
    assert_eq!(second.1.as_deref(), Some("export A='1'; exec \"${SHELL:-/bin/sh}\""));
    Ok(())
}

#[tokio::test]
async fn an_invalid_environment_name_is_refused() -> TestResult {
    let setup = setup().await?;
    let response = setup
        .hub
        .handle(&request(&lease(), "t1", open_op(&[], &[("1bad", "x")], None)?)?)
        .await;
    assert!(matches!(
        response,
        TerminalResponse::Failed(TerminalError::InvalidRequest { .. })
    ));
    let blank = setup
        .hub
        .handle(&request(&lease(), "t2", open_op(&[], &[("", "x")], None)?)?)
        .await;
    assert!(matches!(
        blank,
        TerminalResponse::Failed(TerminalError::InvalidRequest { .. })
    ));
    assert_eq!(setup.hub.open_count(&lease()), 0);
    Ok(())
}

#[tokio::test]
async fn ids_belong_to_one_lease_and_leases_are_capped() -> TestResult {
    let setup = setup().await?;
    // Every guest end stays alive: dropping one hangs up its terminal and frees a slot.
    let mut ends = vec![open(&setup, "t1").await?];
    let other = LeaseId::new("lease-2");
    let clash = setup
        .hub
        .handle(&request(&other, "t1", open_op(&[], &[], None)?)?)
        .await;
    assert_eq!(clash, TerminalResponse::Failed(TerminalError::IdInUse));
    let input_clash = setup.hub.handle(&request(&other, "t1", input(b"x")?)?).await;
    assert_eq!(input_clash, TerminalResponse::Failed(TerminalError::IdInUse));
    let close_clash = setup.hub.handle(&request(&other, "t1", TerminalOp::Close)?).await;
    assert_eq!(close_clash, TerminalResponse::Failed(TerminalError::IdInUse));
    for index in 2..=8 {
        ends.push(open(&setup, &format!("t{index}")).await?);
    }
    let ninth = setup
        .hub
        .handle(&request(&lease(), "t9", open_op(&[], &[], None)?)?)
        .await;
    assert_eq!(
        ninth,
        TerminalResponse::Failed(TerminalError::TooManyTerminals { max: 8 })
    );
    assert_eq!(ends.len(), 8);
    Ok(())
}

#[tokio::test]
async fn a_lease_the_guest_does_not_hold_cannot_open() -> TestResult {
    let setup = setup().await?;
    let stranger = LeaseId::new("stranger");
    let response = setup
        .hub
        .handle(&request(&stranger, "t1", open_op(&[], &[], None)?)?)
        .await;
    assert_eq!(
        response,
        TerminalResponse::Failed(TerminalError::SpawnFailed {
            reason: "the worker holds no sandbox for this lease".to_string()
        })
    );
    assert!(!setup.hub.holds(&stranger).await);
    assert!(setup.hub.holds(&lease()).await);
    Ok(())
}

#[tokio::test]
async fn a_guest_that_cannot_open_fails_the_spawn() -> TestResult {
    let setup = setup().await?;
    *setup.guest.open_error.lock().map_err(|error| error.to_string())? = Some("boom".to_string());
    let response = setup
        .hub
        .handle(&request(&lease(), "t1", open_op(&[], &[], None)?)?)
        .await;
    assert_eq!(
        response,
        TerminalResponse::Failed(TerminalError::SpawnFailed {
            reason: "sandbox unavailable: boom".to_string()
        })
    );
    Ok(())
}

#[tokio::test]
async fn input_reaches_the_terminal_and_resizes_reach_the_channel() -> TestResult {
    let setup = setup().await?;
    let mut end = open(&setup, "t1").await?;
    let done = setup.hub.handle(&request(&lease(), "t1", input(b"hello")?)?).await;
    assert_eq!(done, TerminalResponse::Done);
    let mut buf = [0u8; 5];
    end.stdin.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"hello");
    let resize = TerminalOp::Resize {
        cols: dim(100)?,
        rows: dim(30)?,
    };
    let resized = setup.hub.handle(&request(&lease(), "t1", resize)?).await;
    assert_eq!(resized, TerminalResponse::Done);
    let size = end.resize.recv().await.ok_or("no resize")?;
    assert_eq!((size.0.get(), size.1.get()), (100, 30));
    Ok(())
}

#[tokio::test]
async fn requests_for_a_missing_terminal_fail() -> TestResult {
    let setup = setup().await?;
    let missing = TerminalResponse::Failed(TerminalError::NotFound);
    assert_eq!(
        setup.hub.handle(&request(&lease(), "nope", input(b"x")?)?).await,
        missing
    );
    let resize = TerminalOp::Resize {
        cols: dim(1)?,
        rows: dim(1)?,
    };
    assert_eq!(setup.hub.handle(&request(&lease(), "nope", resize)?).await, missing);
    assert_eq!(
        setup.hub.handle(&request(&lease(), "nope", TerminalOp::Close)?).await,
        TerminalResponse::Done
    );
    Ok(())
}

#[tokio::test]
async fn a_terminal_whose_shell_is_gone_reports_exited() -> TestResult {
    let setup = setup().await?;
    let end = open(&setup, "t1").await?;
    let GuestEnd {
        stdin, resize, stdout, ..
    } = end;
    drop(stdin);
    drop(resize);
    let write = setup.hub.handle(&request(&lease(), "t1", input(b"x")?)?).await;
    assert_eq!(write, TerminalResponse::Failed(TerminalError::Exited));
    let resize = TerminalOp::Resize {
        cols: dim(1)?,
        rows: dim(1)?,
    };
    let size = setup.hub.handle(&request(&lease(), "t1", resize)?).await;
    assert_eq!(size, TerminalResponse::Failed(TerminalError::Exited));
    drop(stdout);
    Ok(())
}

#[tokio::test]
async fn a_shell_that_does_not_read_times_the_write_out() -> TestResult {
    let (_server, client) = start_nats().await?;
    let guest = FakeGuest::with_stdin_capacity(&["lease-1"], 4);
    let hub = Arc::new(TerminalHub::new(guest.clone(), client));
    hub.handle(&request(&lease(), "t1", open_op(&[], &[], None)?)?).await;
    let _end = guest.take_end().await?;
    let write = hub.handle(&request(&lease(), "t1", input(&[b'x'; 64])?)?).await;
    assert_eq!(write, TerminalResponse::Failed(TerminalError::Exited));
    Ok(())
}

#[tokio::test]
async fn output_is_published_with_sequence_numbers_and_an_activity_signal() -> TestResult {
    let setup = setup().await?;
    let mut output = setup.client.subscribe(terminal_output(&terminal("t1")?)).await?;
    let mut activity = setup.client.subscribe(worker_lease_activity(&lease())).await?;
    setup.client.flush().await?;
    let mut end = open(&setup, "t1").await?;
    end.stdout.write_all(b"first").await?;
    let first = next_output(&mut output).await?;
    assert!(matches!(&first, TerminalOutput::Data { seq: 0, .. }));
    assert_eq!(terminal_output_bytes(&first), Some(b"first".to_vec()));
    tokio::time::timeout(Duration::from_secs(10), futures::StreamExt::next(&mut activity))
        .await?
        .ok_or("no activity")?;
    end.stdout.write_all(b"second").await?;
    assert!(matches!(
        next_output(&mut output).await?,
        TerminalOutput::Data { seq: 1, .. }
    ));
    Ok(())
}

#[tokio::test]
async fn a_shell_that_exits_reports_its_status() -> TestResult {
    let setup = setup().await?;
    let mut output = setup.client.subscribe(terminal_output(&terminal("t1")?)).await?;
    setup.client.flush().await?;
    let end = open(&setup, "t1").await?;
    let GuestEnd { stdout, done, .. } = end;
    drop(stdout);
    let _ = done.send(5);
    assert_eq!(
        next_output(&mut output).await?,
        TerminalOutput::Exit {
            seq: 0,
            exit_code: Some(5)
        }
    );
    assert_eq!(setup.hub.open_count(&lease()), 0);
    Ok(())
}

#[tokio::test]
async fn a_shell_that_exits_without_a_status_reports_none() -> TestResult {
    let setup = setup().await?;
    let mut output = setup.client.subscribe(terminal_output(&terminal("t1")?)).await?;
    setup.client.flush().await?;
    let end = open(&setup, "t1").await?;
    let GuestEnd { stdout, done, .. } = end;
    drop(stdout);
    drop(done);
    assert_eq!(
        next_output(&mut output).await?,
        TerminalOutput::Exit {
            seq: 0,
            exit_code: None
        }
    );
    Ok(())
}

#[tokio::test]
async fn closing_a_terminal_hangs_up_its_channel_and_reports_exit() -> TestResult {
    let setup = setup().await?;
    let mut output = setup.client.subscribe(terminal_output(&terminal("t1")?)).await?;
    setup.client.flush().await?;
    let end = open(&setup, "t1").await?;
    let closed = setup.hub.handle(&request(&lease(), "t1", TerminalOp::Close)?).await;
    assert_eq!(closed, TerminalResponse::Done);
    assert!(end.close.is_cancelled());
    assert_eq!(
        next_output(&mut output).await?,
        TerminalOutput::Exit {
            seq: 0,
            exit_code: None
        }
    );
    assert_eq!(setup.hub.open_count(&lease()), 0);
    Ok(())
}

#[tokio::test]
async fn an_idle_terminal_is_closed() -> TestResult {
    let (_server, client) = start_nats().await?;
    let guest = FakeGuest::holding(&["lease-1"]);
    let hub = Arc::new(TerminalHub::with_idle_timeout(
        guest.clone(),
        client.clone(),
        Duration::from_millis(400),
    ));
    let mut output = client.subscribe(terminal_output(&terminal("t1")?)).await?;
    client.flush().await?;
    hub.handle(&request(&lease(), "t1", open_op(&[], &[], None)?)?).await;
    let _end = guest.take_end().await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    hub.handle(&request(&lease(), "t1", input(b"x")?)?).await;
    assert_eq!(
        next_output(&mut output).await?,
        TerminalOutput::Exit {
            seq: 0,
            exit_code: None
        }
    );
    assert_eq!(hub.open_count(&lease()), 0);
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_closes_its_terminals() -> TestResult {
    let setup = setup().await?;
    let _first = open(&setup, "t1").await?;
    let _second = open(&setup, "t2").await?;
    setup.hub.close_lease(&lease()).await;
    assert_eq!(setup.hub.open_count(&lease()), 0);
    assert_eq!(
        *setup.guest.released.lock().map_err(|error| error.to_string())?,
        [lease()]
    );
    Ok(())
}

#[tokio::test]
async fn output_that_cannot_be_published_is_dropped() -> TestResult {
    let server = Server::start_with_config("max_payload: 1024\n").await;
    let client = server.client().await?;
    let guest = FakeGuest::holding(&["lease-1"]);
    let hub = Arc::new(TerminalHub::new(guest.clone(), client.clone()));
    hub.handle(&request(&lease(), "t1", open_op(&[], &[], None)?)?).await;
    let mut end = guest.take_end().await?;
    let mut output = client.subscribe(terminal_output(&terminal("t1")?)).await?;
    client.flush().await?;
    end.stdout.write_all(&[b'x'; 4000]).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    hub.handle(&request(&lease(), "t1", TerminalOp::Close)?).await;
    assert!(matches!(next_output(&mut output).await?, TerminalOutput::Exit { .. }));
    Ok(())
}

#[tokio::test]
async fn the_ssh_host_forwards_to_the_hub() -> TestResult {
    let setup = setup().await?;
    let host = HubSshHost(Arc::clone(&setup.hub));
    assert!(host.holds(&lease()).await);
    assert!(!host.holds(&LeaseId::new("stranger")).await);
    let opened = host.terminal(&request(&lease(), "t1", open_op(&[], &[], None)?)?).await;
    assert!(matches!(opened, TerminalResponse::Opened { .. }));
    Ok(())
}

#[tokio::test]
async fn the_handler_answers_only_requests_for_held_leases() -> TestResult {
    let setup = setup().await?;
    let handler = tokio::spawn(run_terminal_handler(
        setup.client.clone(),
        Arc::clone(&setup.hub),
        "test.terminals",
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let reply = |response: Bytes| -> TestResult<TerminalResponse> { Ok(serde_json::from_slice(&response)?) };
    let held = json(&request(&lease(), "t1", open_op(&[], &[], None)?)?)?;
    let answer = setup.client.request("test.terminals", held).await?;
    assert!(matches!(reply(answer.payload)?, TerminalResponse::Opened { .. }));

    // Requests no worker answers: the lease is not held, or the body is garbage.
    let stranger = json(&request(&LeaseId::new("stranger"), "t1", open_op(&[], &[], None)?)?)?;
    let silent = setup.client.request("test.terminals", stranger);
    assert!(tokio::time::timeout(Duration::from_millis(300), silent).await.is_err());
    let garbage = setup.client.request("test.terminals", Bytes::from_static(b"garbage"));
    assert!(tokio::time::timeout(Duration::from_millis(300), garbage).await.is_err());
    // A request without a reply subject is dropped.
    setup
        .client
        .publish(
            "test.terminals",
            json(&request(&lease(), "t2", open_op(&[], &[], None)?)?)?,
        )
        .await?;
    setup.client.flush().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(setup.hub.open_count(&lease()), 1);

    setup.client.drain().await?;
    tokio::time::timeout(Duration::from_secs(10), handler).await???;
    Ok(())
}

#[tokio::test]
async fn the_handler_fails_on_a_closed_connection() -> TestResult {
    let setup = setup().await?;
    setup.client.drain().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let result = run_terminal_handler(setup.client.clone(), Arc::clone(&setup.hub), "test.terminals").await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn a_reply_that_cannot_be_published_is_logged() -> TestResult {
    let server = Server::start_with_config("max_payload: 1024\n").await;
    let client = server.client().await?;
    let guest = FakeGuest::holding(&["lease-1"]);
    *guest.open_error.lock().map_err(|error| error.to_string())? = Some("x".repeat(3000));
    let hub = Arc::new(TerminalHub::new(guest, client.clone()));
    tokio::spawn(run_terminal_handler(client.clone(), Arc::clone(&hub), "test.terminals"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let ask = request(&lease(), "t1", open_op(&[], &[], None)?)?;
    let pending = client.request("test.terminals", json(&ask)?);
    assert!(tokio::time::timeout(Duration::from_millis(500), pending).await.is_err());
    Ok(())
}

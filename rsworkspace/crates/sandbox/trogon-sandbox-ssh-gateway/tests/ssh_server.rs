mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use russh::{ChannelMsg, Disconnect};
use support::{API_KEY, TestResult, connect, eventually, login, start_gateway};
use trogon_sandbox_ssh_gateway::{SshChannelKind, SshStreamControl};

type Channel = russh::Channel<russh::client::Msg>;

struct Outcome {
    output: Vec<u8>,
    status: Option<u32>,
}

async fn drain(channel: &mut Channel) -> Outcome {
    let mut outcome = Outcome {
        output: Vec::new(),
        status: None,
    };
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => outcome.output.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => outcome.status = Some(exit_status),
            _ => {}
        }
    }
    outcome
}

async fn run_exec(command: &str) -> TestResult<(Outcome, support::Gateway)> {
    let gateway = start_gateway(4).await?;
    let session = login(gateway.addr, "lease-1").await?;
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, command).await?;
    let outcome = drain(&mut channel).await;
    Ok((outcome, gateway))
}

#[tokio::test]
async fn exec_streams_output_and_exit_status() -> TestResult {
    let (outcome, gateway) = run_exec("echo hi").await?;
    assert_eq!(String::from_utf8(outcome.output)?, "ran: echo hi");
    assert_eq!(outcome.status, Some(3));
    assert_eq!(gateway.tunnel.attach_count(), 1);
    eventually(|| gateway.tunnel.detach_reasons() == ["channel_close"]).await?;
    Ok(())
}

#[tokio::test]
async fn worker_error_is_shown_and_fails_the_channel() -> TestResult {
    let (outcome, _gateway) = run_exec("boom").await?;
    assert_eq!(
        String::from_utf8(outcome.output)?,
        "trogon-sandbox-ssh: no such thing\r\n"
    );
    assert_eq!(outcome.status, Some(255));
    Ok(())
}

#[tokio::test]
async fn exit_without_status_reports_failure() -> TestResult {
    let (outcome, _gateway) = run_exec("killed").await?;
    assert_eq!(outcome.status, Some(255));
    Ok(())
}

#[tokio::test]
async fn frames_without_payload_are_skipped() -> TestResult {
    let (outcome, _gateway) = run_exec("ignored-controls").await?;
    assert!(outcome.output.is_empty());
    assert_eq!(outcome.status, Some(0));
    Ok(())
}

#[tokio::test]
async fn exec_exit_closes_without_the_worker() -> TestResult {
    let (outcome, gateway) = run_exec("exit").await?;
    assert_eq!(outcome.status, Some(0));
    assert!(gateway.tunnel.published().is_empty());
    Ok(())
}

#[tokio::test]
async fn shell_with_pty_relays_input_and_resizes() -> TestResult {
    let gateway = start_gateway(4).await?;
    let session = login(gateway.addr, "lease-1").await?;
    let mut channel = session.channel_open_session().await?;
    channel.request_pty(true, "xterm", 100, 40, 0, 0, &[]).await?;
    channel.request_shell(true).await?;
    channel.window_change(120, 50, 0, 0).await?;

    let big = vec![b'x'; 20_000];
    channel.data(&big[..]).await?;
    channel.eof().await?;
    let outcome = drain(&mut channel).await;

    let text = outcome.output;
    assert!(text.starts_with(b"shell ready\r\n"));
    assert_eq!(text.iter().filter(|byte| **byte == b'x').count(), 20_000);
    assert_eq!(outcome.status, Some(0));

    let frames = gateway.tunnel.published();
    assert!(frames.iter().any(|frame| matches!(
        &frame.ctl,
        Some(SshStreamControl::Open { kind: SshChannelKind::Shell, term: Some(term), cols, rows })
            if term == "xterm" && cols.get() == 100 && rows.get() == 40
    )));
    assert!(frames.iter().any(|frame| matches!(
        &frame.ctl,
        Some(SshStreamControl::Resize { cols, rows }) if cols.get() == 120 && rows.get() == 50
    )));
    let chunks: Vec<usize> = frames
        .iter()
        .filter(|frame| frame.ctl.is_none() && !frame.data.is_empty())
        .map(|frame| frame.data.len())
        .collect();
    assert_eq!(chunks, [8192, 8192, 3616]);
    Ok(())
}

#[tokio::test]
async fn pty_sides_default_and_cap() -> TestResult {
    let gateway = start_gateway(4).await?;
    let session = login(gateway.addr, "lease-1").await?;
    let mut channel = session.channel_open_session().await?;
    channel.request_pty(true, "vt100", 0, 5000, 0, 0, &[]).await?;
    channel.exec(true, "go").await?;
    drain(&mut channel).await;
    let frames = gateway.tunnel.published();
    assert!(frames.iter().any(|frame| matches!(
        &frame.ctl,
        Some(SshStreamControl::Open { cols, rows, .. }) if cols.get() == 80 && rows.get() == 1000
    )));
    Ok(())
}

#[tokio::test]
async fn exec_without_pty_opens_with_default_window() -> TestResult {
    let (_outcome, gateway) = run_exec("go").await?;
    let frames = gateway.tunnel.published();
    assert!(frames.iter().any(|frame| matches!(
        &frame.ctl,
        Some(SshStreamControl::Open { term: None, cols, rows, .. }) if cols.get() == 80 && rows.get() == 24
    )));
    Ok(())
}

#[tokio::test]
async fn dropped_connection_detaches() -> TestResult {
    let gateway = start_gateway(4).await?;
    let session = login(gateway.addr, "lease-1").await?;
    let channel = session.channel_open_session().await?;
    channel.exec(true, "hang").await?;
    session.disconnect(Disconnect::ByApplication, "bye", "en").await?;
    drop(channel);
    eventually(|| gateway.tunnel.detach_count() == 1).await?;
    Ok(())
}

#[tokio::test]
async fn failed_detach_does_not_break_the_session() -> TestResult {
    let gateway = start_gateway(4).await?;
    gateway.tunnel.refuse_detach.store(true, Ordering::SeqCst);
    let session = login(gateway.addr, "lease-1").await?;
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, "echo").await?;
    let outcome = drain(&mut channel).await;
    assert_eq!(outcome.status, Some(3));
    eventually(|| gateway.tunnel.detach_count() == 1).await?;
    Ok(())
}

#[tokio::test]
async fn failed_input_publish_is_tolerated() -> TestResult {
    let gateway = start_gateway(4).await?;
    let session = login(gateway.addr, "lease-1").await?;
    let channel = session.channel_open_session().await?;
    channel.request_pty(true, "xterm", 80, 24, 0, 0, &[]).await?;
    channel.request_shell(true).await?;
    gateway.tunnel.refuse_publish.store(true, Ordering::SeqCst);
    channel.window_change(90, 30, 0, 0).await?;
    channel.data(&b"lost"[..]).await?;
    channel.eof().await?;
    session.disconnect(Disconnect::ByApplication, "bye", "en").await?;
    eventually(|| gateway.tunnel.detach_count() == 1).await?;
    Ok(())
}

#[tokio::test]
async fn wrong_password_is_rejected() -> TestResult {
    let gateway = start_gateway(4).await?;
    let mut session = connect(gateway.addr).await?;
    let auth = session.authenticate_password("lease-1@gw", "wrong").await?;
    assert!(!auth.success());
    assert_eq!(gateway.tunnel.attach_count(), 0);
    Ok(())
}

#[tokio::test]
async fn invalid_lease_user_is_refused() -> TestResult {
    let gateway = start_gateway(4).await?;
    for user in ["a.b@gw", "a*@gw", "@gw", "a b@gw"] {
        let mut session = connect(gateway.addr).await?;
        let accepted = session
            .authenticate_password(user, API_KEY)
            .await
            .is_ok_and(|auth| auth.success());
        assert!(!accepted, "{user} was accepted");
    }
    assert_eq!(gateway.tunnel.attach_count(), 0);
    Ok(())
}

#[tokio::test]
async fn refused_attach_fails_login() -> TestResult {
    let gateway = start_gateway(4).await?;
    gateway.tunnel.refuse_attach.store(true, Ordering::SeqCst);
    let mut session = connect(gateway.addr).await?;
    let accepted = session
        .authenticate_password("lease-1@gw", API_KEY)
        .await
        .is_ok_and(|auth| auth.success());
    assert!(!accepted);
    Ok(())
}

#[tokio::test]
async fn failed_subscription_drops_the_connection() -> TestResult {
    let gateway = start_gateway(4).await?;
    gateway.tunnel.refuse_subscribe.store(true, Ordering::SeqCst);
    let session = login(gateway.addr, "lease-1").await?;
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, "echo").await?;
    let outcome = drain(&mut channel).await;
    assert_eq!(outcome.status, None);
    eventually(|| gateway.tunnel.detach_count() == 1).await?;
    Ok(())
}

#[tokio::test]
async fn connection_limit_queues_clients() -> TestResult {
    let gateway = start_gateway(1).await?;
    let first = login(gateway.addr, "lease-1").await?;
    let addr = gateway.addr;
    let queued = tokio::spawn(async move { login(addr, "lease-2").await.map_err(|error| error.to_string()) });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!queued.is_finished());
    first.disconnect(Disconnect::ByApplication, "bye", "en").await?;
    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(10), queued)
        .await??
        .map_err(Box::<dyn std::error::Error>::from)?;
    second.disconnect(Disconnect::ByApplication, "bye", "en").await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_stops_the_server() -> TestResult {
    let gateway = start_gateway(4).await?;
    gateway.cancel.cancel();
    gateway.server.await??;
    Ok(())
}

#[tokio::test]
async fn the_session_ends_when_its_last_channel_closes() -> TestResult {
    let gateway = start_gateway(4).await?;
    let session = login(gateway.addr, "lease-1").await?;
    let first = session.channel_open_session().await?;
    first.exec(true, "hang").await?;
    let mut second = session.channel_open_session().await?;
    second.exec(true, "echo hi").await?;
    let outcome = drain(&mut second).await;
    assert_eq!(outcome.status, Some(3));
    assert_eq!(gateway.tunnel.attach_count(), 1);
    assert_eq!(gateway.tunnel.detach_count(), 0);

    first.close().await?;
    eventually(|| gateway.tunnel.detach_reasons() == ["channel_close"]).await?;
    Ok(())
}

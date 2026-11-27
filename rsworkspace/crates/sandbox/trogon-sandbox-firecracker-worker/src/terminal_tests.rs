use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_actor_agent::{ActorIdentitySource, AgentState, BindLeaseRequest, router};
use trogon_sandbox_contract::{TerminalArgument, TerminalDimension, TerminalInput};
use trogon_sandbox_protocol::{terminal_output_bytes, terminal_output_seq};

use super::*;
use crate::agent::tests::serve;

/// Room for every frame a test produces.
const AMPLE_REPLAY_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct Collect {
    frames: Mutex<Vec<(String, Bytes)>>,
}

#[async_trait]
impl FrameSink for Collect {
    async fn publish(&self, subject: String, payload: Bytes) {
        self.frames().push((subject, payload));
    }
}

impl Collect {
    fn frames(&self) -> MutexGuard<'_, Vec<(String, Bytes)>> {
        self.frames.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn outputs(&self, id: &str) -> Vec<TerminalOutput> {
        let subject = format!("sandbox.terminal.out.{id}");
        self.frames()
            .iter()
            .filter(|(s, _)| *s == subject)
            .filter_map(|(_, p)| serde_json::from_slice(p).ok())
            .collect()
    }

    fn text(&self, id: &str) -> String {
        self.outputs(id)
            .iter()
            .filter_map(terminal_output_bytes)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .collect()
    }

    async fn until(&self, what: &str, done: impl Fn(&Collect) -> bool) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !done(self) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for {what}"))
    }
}

fn terminal(id: &str) -> anyhow::Result<TerminalId> {
    Ok(TerminalId::new(id)?)
}

async fn guest() -> anyhow::Result<(GuestAgent, tempfile::TempDir)> {
    let state = Arc::new(AgentState::new(ActorIdentitySource::new("/nonexistent/actor-id")));
    let (agent, tmp) = serve(router(state)).await?;
    agent
        .bind(&BindLeaseRequest {
            lease_id: LeaseId::new("l1"),
            entropy_seed: None,
            env: BTreeMap::new(),
            working_dir: None,
            exposed_port: None,
        })
        .await?;
    Ok((agent, tmp))
}

fn request(id: &str, op: TerminalOp) -> anyhow::Result<TerminalRequest> {
    Ok(TerminalRequest {
        lease_id: LeaseId::new("l1"),
        terminal_id: terminal(id)?,
        op,
    })
}

fn open_with(cmd: &[&str], resume_from: Option<u64>) -> anyhow::Result<TerminalOp> {
    Ok(TerminalOp::Open {
        cols: TerminalDimension::new(80)?,
        rows: TerminalDimension::new(24)?,
        cmd: cmd
            .iter()
            .map(|arg| TerminalArgument::new(*arg))
            .collect::<Result<_, _>>()?,
        env: BTreeMap::new(),
        cwd: None,
        resume_from,
    })
}

fn open(cmd: &[&str]) -> anyhow::Result<TerminalOp> {
    open_with(cmd, None)
}

fn input(text: &str) -> anyhow::Result<TerminalOp> {
    Ok(TerminalOp::Input {
        data: TerminalInput::new(STANDARD.encode(text))?,
    })
}

fn setup() -> (Arc<FirecrackerTerminals>, Arc<Collect>) {
    let sink = Arc::new(Collect::default());
    (
        FirecrackerTerminals::with_replay_bytes(sink.clone(), AMPLE_REPLAY_BYTES),
        sink,
    )
}

fn exit_of(sink: &Collect, id: &str) -> Option<(u64, Option<i32>)> {
    sink.outputs(id).into_iter().find_map(|f| match f {
        TerminalOutput::Exit { seq, exit_code } => Some((seq, exit_code)),
        _ => None,
    })
}

#[tokio::test]
async fn a_shell_runs_through_the_guest_pty_with_resize_and_close() -> anyhow::Result<()> {
    let (agent, _tmp) = guest().await?;
    let (terminals, sink) = setup();

    let opened = terminals.handle(&agent, &request("t1", open(&[])?)?).await;
    assert_eq!(
        opened,
        TerminalResponse::Opened {
            terminal_id: terminal("t1")?
        }
    );
    // Opening an id that is already open answers `Opened` again. This reader
    // claims to have seen everything, so nothing is replayed.
    let reopen = open_with(&[], Some(u64::MAX))?;
    assert_eq!(terminals.handle(&agent, &request("t1", reopen)?).await, opened);
    assert_eq!(
        terminals
            .handle(&agent, &request("t1", input("echo pty-$((40+2))\r")?)?)
            .await,
        TerminalResponse::Done
    );
    sink.until("echoed output", |s| s.text("t1").contains("pty-42")).await?;

    terminals
        .handle(
            &agent,
            &request(
                "t1",
                TerminalOp::Resize {
                    cols: TerminalDimension::new(132)?,
                    rows: TerminalDimension::new(50)?,
                },
            )?,
        )
        .await;
    terminals.handle(&agent, &request("t1", input("stty size\r")?)?).await;
    sink.until("the new size", |s| s.text("t1").contains("50 132")).await?;

    assert_eq!(
        terminals.handle(&agent, &request("t1", TerminalOp::Close)?).await,
        TerminalResponse::Done
    );
    sink.until("the exit frame", |s| exit_of(s, "t1").is_some()).await?;
    // Frames are gapless and the single exit frame is last.
    let frames = sink.outputs("t1");
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(terminal_output_seq(frame), i as u64);
    }
    assert_eq!(
        frames
            .iter()
            .filter(|f| matches!(f, TerminalOutput::Exit { .. }))
            .count(),
        1
    );
    assert!(matches!(frames.last(), Some(TerminalOutput::Exit { .. })));
    sink.until("the terminal to be forgotten", |_| terminals.open_count() == 0)
        .await
}

#[tokio::test]
async fn the_exit_status_is_published() -> anyhow::Result<()> {
    let (agent, _tmp) = guest().await?;
    let (terminals, sink) = setup();
    terminals
        .handle(&agent, &request("t2", open(&["/bin/sh", "-c", "echo bye; exit 5"])?)?)
        .await;
    sink.until("the exit frame", |s| exit_of(s, "t2").is_some()).await?;
    assert_eq!(exit_of(&sink, "t2").context("no exit frame")?.1, Some(5));
    assert!(sink.text("t2").contains("bye"));
    sink.until("the terminal to be forgotten", |_| terminals.open_count() == 0)
        .await?;
    assert_eq!(
        terminals.handle(&agent, &request("t2", input("x")?)?).await,
        TerminalResponse::Failed(TerminalError::NotFound)
    );
    Ok(())
}

#[tokio::test]
async fn a_lost_guest_ends_the_terminal_with_one_exit_frame() -> anyhow::Result<()> {
    let (agent, _tmp) = guest().await?;
    let (terminals, sink) = setup();
    terminals.handle(&agent, &request("t3", open(&[])?)?).await;
    // The worker dropping the stream is what a lost VM looks like to the pump.
    terminals.owned(&LeaseId::new("l1"), &terminal("t3")?)?.stop.cancel();
    sink.until("the synthesized exit", |s| exit_of(s, "t3").is_some())
        .await?;
    assert_eq!(exit_of(&sink, "t3").context("no exit frame")?.1, None);
    Ok(())
}

#[tokio::test]
async fn another_leases_terminal_is_not_addressable() -> anyhow::Result<()> {
    let (agent, _tmp) = guest().await?;
    let (terminals, _sink) = setup();
    terminals.handle(&agent, &request("t4", open(&[])?)?).await;
    let mut other = request("t4", input("x")?)?;
    other.lease_id = LeaseId::new("other");
    assert_eq!(
        terminals.handle(&agent, &other).await,
        TerminalResponse::Failed(TerminalError::IdInUse)
    );
    terminals.handle(&agent, &request("t4", TerminalOp::Close)?).await;
    Ok(())
}

#[tokio::test]
async fn an_agent_without_terminal_routes_is_unsupported() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(axum::Router::new()).await?;
    let (terminals, _sink) = setup();
    assert_eq!(
        terminals.handle(&agent, &request("t5", open(&[])?)?).await,
        TerminalResponse::Unsupported
    );
    assert_eq!(terminals.open_count(), 0);
    Ok(())
}

const PRINT_THREE: &str = "printf aaaa; sleep 0.4; printf bbbb; sleep 0.4; printf cccc; sleep 30";

fn data_seqs(sink: &Collect, id: &str) -> Vec<u64> {
    sink.outputs(id)
        .iter()
        .filter(|f| matches!(f, TerminalOutput::Data { .. }))
        .map(terminal_output_seq)
        .collect()
}

#[tokio::test]
async fn a_reattach_resumes_after_the_offset_the_reader_saw() -> anyhow::Result<()> {
    let (agent, _tmp) = guest().await?;
    let (terminals, sink) = setup();
    let cmd = open(&["/bin/sh", "-c", PRINT_THREE])?;
    terminals.handle(&agent, &request("t1", cmd)?).await;
    sink.until("all the output", |s| s.text("t1").contains("cccc")).await?;
    let live = sink.outputs("t1").len();
    let seqs = data_seqs(&sink, "t1");
    let [first, second, third] = seqs[..] else {
        bail!("expected three data frames: {seqs:?}");
    };

    let seen_one = request("t1", open_with(&[], Some(first))?)?;
    terminals.handle(&agent, &seen_one).await;
    let replayed = sink.outputs("t1")[live..].to_vec();
    assert_eq!(
        replayed.iter().map(terminal_output_seq).collect::<Vec<_>>(),
        vec![second, third]
    );

    let seen_all = request("t1", open_with(&[], Some(third))?)?;
    terminals.handle(&agent, &seen_all).await;
    assert_eq!(sink.outputs("t1").len(), live + 2);
    terminals.handle(&agent, &request("t1", TerminalOp::Close)?).await;
    Ok(())
}

#[tokio::test]
async fn a_reattach_older_than_the_ring_gets_a_resync_first() -> anyhow::Result<()> {
    let (agent, _tmp) = guest().await?;
    let sink = Arc::new(Collect::default());
    // Room for one 4-byte frame only.
    let terminals = FirecrackerTerminals::with_replay_bytes(sink.clone(), 4);
    let cmd = open(&["/bin/sh", "-c", PRINT_THREE])?;
    terminals.handle(&agent, &request("t1", cmd)?).await;
    sink.until("all the output", |s| s.text("t1").contains("cccc")).await?;
    let live = sink.outputs("t1").len();
    let seqs = data_seqs(&sink, "t1");
    let [first, _, third] = seqs[..] else {
        bail!("expected three data frames: {seqs:?}");
    };

    let behind = request("t1", open_with(&[], Some(first))?)?;
    terminals.handle(&agent, &behind).await;
    let replayed = sink.outputs("t1")[live..].to_vec();
    assert_eq!(replayed.len(), 2, "{replayed:?}");
    assert_eq!(replayed[0], TerminalOutput::Resync { from_seq: third });
    assert_eq!(terminal_output_bytes(&replayed[1]).context("no bytes")?, b"cccc");
    terminals.handle(&agent, &request("t1", TerminalOp::Close)?).await;
    Ok(())
}

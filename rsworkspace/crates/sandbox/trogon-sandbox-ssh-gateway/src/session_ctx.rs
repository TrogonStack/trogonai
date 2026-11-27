use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_nats::Client;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use trogon_sandbox_contract::{
    LeaseId, TerminalArgument, TerminalDimension, TerminalId, TerminalInput, TerminalOp, TerminalOutput,
    TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::subjects::terminal_output;
use trogon_sandbox_protocol::terminal_output_bytes;

use crate::bridge_channel::BridgeChannel;
use crate::channel_error::ChannelError;
use crate::constants::{EOT, MAX_FRAME_DATA_BYTES, MAX_TERM_LEN};
use crate::subjects::ssh_stream_from_worker;
use crate::{SshChannelKind, SshSessionId, SshStreamControl, SshStreamFrame, SshTerminalHost, TunnelError};

/// One bridged session: the terminals its channels opened and the activity clock.
pub(crate) struct SessionCtx {
    pub(crate) session_id: SshSessionId,
    pub(crate) lease: LeaseId,
    pub(crate) client: Client,
    pub(crate) host: Arc<dyn SshTerminalHost>,
    pub(crate) channels: Mutex<HashMap<u32, BridgeChannel>>,
    pub(crate) activity: Mutex<Instant>,
    pub(crate) max_channels: NonZeroUsize,
}

fn valid_term(term: &str) -> bool {
    !term.is_empty()
        && term.len() <= MAX_TERM_LEN
        && term
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

impl SessionCtx {
    pub(crate) fn touch(&self) {
        *self.activity.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
    }

    pub(crate) fn last_activity(&self) -> Instant {
        *self.activity.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Publishes `frame` toward the gateway. A frame that cannot be sent is logged: the gateway's side of the session
    /// ends on its own.
    async fn send(&self, frame: &SshStreamFrame) {
        if let Err(error) = self.try_send(frame).await {
            warn!(%error, "ssh stream publish failed");
        }
    }

    async fn try_send(&self, frame: &SshStreamFrame) -> Result<(), TunnelError> {
        let payload = serde_json::to_vec(frame).map_err(TunnelError::Encode)?;
        self.client
            .publish(ssh_stream_from_worker(&self.session_id), Bytes::from(payload))
            .await?;
        Ok(())
    }

    async fn fail_channel(&self, channel_id: u32, message: String) {
        self.send(&SshStreamFrame::control(
            &self.session_id,
            channel_id,
            SshStreamControl::Error { message },
        ))
        .await;
        self.send(&SshStreamFrame::eof(&self.session_id, channel_id)).await;
    }

    async fn terminal(&self, terminal_id: &TerminalId, op: TerminalOp) -> TerminalResponse {
        self.host
            .terminal(&TerminalRequest {
                lease_id: self.lease.clone(),
                terminal_id: terminal_id.clone(),
                op,
            })
            .await
    }

    /// Runs `op` on a terminal and holds the host to it: anything but `done` is an error.
    async fn terminal_done(&self, terminal_id: &TerminalId, op: TerminalOp) -> Result<(), ChannelError> {
        match self.terminal(terminal_id, op).await {
            TerminalResponse::Done => Ok(()),
            other => Err(ChannelError::Refused(other)),
        }
    }

    pub(crate) async fn open_channel(
        self: &Arc<Self>,
        channel_id: u32,
        kind: SshChannelKind,
        cols: TerminalDimension,
        rows: TerminalDimension,
        term: Option<String>,
    ) {
        if let Err(error) = self.try_open_channel(channel_id, kind, cols, rows, term).await {
            self.fail_channel(channel_id, error.to_string()).await;
        }
    }

    async fn try_open_channel(
        self: &Arc<Self>,
        channel_id: u32,
        kind: SshChannelKind,
        cols: TerminalDimension,
        rows: TerminalDimension,
        term: Option<String>,
    ) -> Result<(), ChannelError> {
        let (already_open, open_count) = {
            let channels = self.channels.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            (channels.contains_key(&channel_id), channels.len())
        };
        if already_open {
            return Ok(());
        }
        if open_count >= self.max_channels.get() {
            return Err(ChannelError::AtLimit { max: self.max_channels });
        }
        let terminal_id = TerminalId::new(format!("ssh-{}-{channel_id}", self.session_id))?;
        let cmd = match kind {
            SshChannelKind::Shell => Vec::new(),
            SshChannelKind::Exec { command } => vec![
                TerminalArgument::new("/bin/sh")?,
                TerminalArgument::new("-c")?,
                TerminalArgument::new(command)?,
            ],
        };
        let mut env = BTreeMap::new();
        if let Some(term) = term.filter(|term| valid_term(term)) {
            env.insert("TERM".to_string(), term);
        }

        // Subscribe before opening so the first bytes cannot be missed.
        let output = self.client.subscribe(terminal_output(&terminal_id)).await?;
        let response = self
            .terminal(
                &terminal_id,
                TerminalOp::Open {
                    cols,
                    rows,
                    cmd,
                    env,
                    cwd: None,
                    resume_from: None,
                },
            )
            .await;
        if !matches!(response, TerminalResponse::Opened { .. }) {
            return Err(ChannelError::NotOpened(response));
        }
        let stop_pump = CancellationToken::new();
        self.channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                channel_id,
                BridgeChannel {
                    terminal_id,
                    stop_pump: stop_pump.clone(),
                },
            );
        tokio::spawn(Arc::clone(self).pump(channel_id, output, stop_pump));
        Ok(())
    }

    /// Forwards a terminal's output to the gateway until it exits.
    async fn pump(self: Arc<Self>, channel_id: u32, mut output: async_nats::Subscriber, stop: CancellationToken) {
        loop {
            let message = tokio::select! {
                _ = stop.cancelled() => return,
                message = output.next() => message,
            };
            let Some(message) = message else { return };
            let Ok(frame) = serde_json::from_slice::<TerminalOutput>(&message.payload) else {
                continue;
            };
            self.touch();
            match &frame {
                TerminalOutput::Data { .. } => {
                    // Bytes travel as a JSON number array, so keep each frame well under the NATS payload limit.
                    let bytes = terminal_output_bytes(&frame).unwrap_or_default();
                    for chunk in bytes.chunks(MAX_FRAME_DATA_BYTES) {
                        self.send(&SshStreamFrame::data(&self.session_id, channel_id, chunk.to_vec()))
                            .await;
                    }
                }
                // The bridge never reattaches, so it never asks for a resync.
                TerminalOutput::Resync { .. } => {}
                TerminalOutput::Exit { exit_code, .. } => {
                    self.channels
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&channel_id);
                    let status = (*exit_code).and_then(|code| u32::try_from(code).ok());
                    self.send(&SshStreamFrame::control(
                        &self.session_id,
                        channel_id,
                        SshStreamControl::Exit { status },
                    ))
                    .await;
                    self.send(&SshStreamFrame::eof(&self.session_id, channel_id)).await;
                    return;
                }
            }
        }
    }

    /// Applies one frame from the gateway.
    pub(crate) async fn handle(self: &Arc<Self>, frame: SshStreamFrame) {
        self.touch();
        let channel_id = frame.channel_id;
        if let Err(error) = self.try_handle(frame).await {
            debug!(%error, channel_id, "ssh stream frame not applied");
        }
    }

    async fn try_handle(self: &Arc<Self>, frame: SshStreamFrame) -> Result<(), ChannelError> {
        let channel_id = frame.channel_id;
        if let Some(control) = frame.ctl {
            match control {
                SshStreamControl::Open { kind, cols, rows, term } => {
                    self.open_channel(channel_id, kind, cols, rows, term).await;
                }
                SshStreamControl::Resize { cols, rows } => {
                    if let Some(id) = self.terminal_of(channel_id) {
                        self.terminal_done(&id, TerminalOp::Resize { cols, rows }).await?;
                    }
                }
                // Worker-bound sessions never receive these; ignore echoes.
                SshStreamControl::Exit { .. } | SshStreamControl::Error { .. } => {}
            }
            return Ok(());
        }
        let Some(id) = self.terminal_of(channel_id) else {
            return Ok(());
        };
        for chunk in frame.data.chunks(TerminalInput::MAX_BYTES) {
            let data = TerminalInput::new(STANDARD.encode(chunk))?;
            self.terminal_done(&id, TerminalOp::Input { data }).await?;
        }
        if frame.eof {
            // On a PTY the way to end input is the EOT character.
            let data = TerminalInput::new(STANDARD.encode([EOT]))?;
            self.terminal_done(&id, TerminalOp::Input { data }).await?;
        }
        Ok(())
    }

    fn terminal_of(&self, channel_id: u32) -> Option<TerminalId> {
        self.channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&channel_id)
            .map(|channel| channel.terminal_id.clone())
    }

    /// Closes every terminal the session still has open. With `reason`, the gateway is told each channel failed.
    pub(crate) async fn close_all(&self, reason: Option<&str>) {
        let open: Vec<(u32, BridgeChannel)> = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain()
            .collect();
        for (channel_id, channel) in open {
            channel.stop_pump.cancel();
            self.terminal(&channel.terminal_id, TerminalOp::Close).await;
            if let Some(reason) = reason {
                self.fail_channel(channel_id, reason.to_string()).await;
            }
        }
    }
}

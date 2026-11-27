use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use russh::server::{Auth, Handler, Msg, Session};
use russh::{Channel, ChannelId};
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{LeaseId, TenantId, TerminalDimension, TerminalDimensionError};

pub use crate::constants::DEFAULT_SESSION_TTL_SECONDS;
use crate::constants::{DEFAULT_COLS, DEFAULT_ROWS, FAILURE_EXIT_STATUS, MAX_FRAME_BYTES};
use crate::lease_user::parse_lease_user;
use crate::{
    EphemeralSessionKey, GatewayError, GatewayState, SessionCredential, SshAttachRequest, SshChannelKind,
    SshDetachRequest, SshSessionId, SshStreamControl, SshStreamFrame, validate_bearer,
};

type ChannelMap = Arc<Mutex<HashMap<u32, ChannelId>>>;

struct PtyRequest {
    term: String,
    cols: TerminalDimension,
    rows: TerminalDimension,
}

/// A session the worker accepted and the gateway must end.
struct AttachedSession {
    id: SshSessionId,
    lease_id: LeaseId,
    tenant: TenantId,
    credential: Option<SessionCredential>,
}

/// One SSH connection: authenticates the client, attaches a session on the worker and relays channel bytes.
pub(crate) struct SshGatewayHandler {
    state: Arc<GatewayState>,
    attached: Option<AttachedSession>,
    /// Open SSH channels by wire id; the worker's frames name channels by number and russh cannot build a
    /// `ChannelId` from one.
    channels: ChannelMap,
    ptys: HashMap<u32, PtyRequest>,
    /// Stops the task that relays the worker's frames.
    pump_stop: CancellationToken,
    frames_started: bool,
}

impl SshGatewayHandler {
    pub(crate) fn new(state: Arc<GatewayState>) -> Self {
        Self {
            state,
            attached: None,
            channels: Arc::new(Mutex::new(HashMap::new())),
            ptys: HashMap::new(),
            pump_stop: CancellationToken::new(),
            frames_started: false,
        }
    }

    fn session_id(&self) -> Result<&SshSessionId, GatewayError> {
        self.attached
            .as_ref()
            .map(|session| &session.id)
            .ok_or(GatewayError::NotAttached)
    }

    /// Tells the worker to start `kind` on `channel`, and makes sure the worker's frames reach the client.
    async fn open_channel(
        &mut self,
        channel: ChannelId,
        kind: SshChannelKind,
        session: &mut Session,
    ) -> Result<(), GatewayError> {
        let session_id = self.session_id()?.clone();
        let tunnel = Arc::clone(&self.state.config.tunnel);

        // Subscribe before opening so the worker's first bytes are not lost.
        if !self.frames_started {
            let frames = tunnel.subscribe_frames(&session_id).await?;
            self.frames_started = true;
            tokio::spawn(relay_frames(
                frames,
                session.handle(),
                Arc::clone(&self.channels),
                self.pump_stop.clone(),
            ));
        }

        let (term, cols, rows) = match self.ptys.get(&u32::from(channel)) {
            Some(pty) => (Some(pty.term.clone()), pty.cols, pty.rows),
            None => (None, dimension(0, DEFAULT_COLS)?, dimension(0, DEFAULT_ROWS)?),
        };
        session.channel_success(channel)?;
        let open = SshStreamFrame::control(
            &session_id,
            u32::from(channel),
            SshStreamControl::Open { kind, cols, rows, term },
        );
        tunnel.publish_frame(&open).await?;
        Ok(())
    }

    async fn end_session(&mut self, reason: &str) {
        let Some(attached) = self.attached.take() else { return };
        self.pump_stop.cancel();
        end_session(&self.state, attached, reason).await;
    }
}

/// Revokes the session's broker token and tells the worker it ended. Failures are logged: the session is over either
/// way.
async fn end_session(state: &GatewayState, attached: AttachedSession, reason: &str) {
    if let (Some(credential), Some(mint)) = (&attached.credential, &state.config.key_mint)
        && let Err(error) = mint.revoke_session(credential, reason).await
    {
        warn!(%error, "session credential revoke failed");
    }
    let request = SshDetachRequest {
        session_id: attached.id,
        lease_id: attached.lease_id,
        reason: reason.to_string(),
    };
    if let Err(error) = state.config.tunnel.detach(&attached.tenant, &request).await {
        warn!(%error, lease_id = %request.lease_id, "detach failed");
    }
}

/// Forwards the worker's frames to the client until the stream ends, the client goes away, or `stop` fires.
async fn relay_frames(
    mut frames: tokio::sync::mpsc::Receiver<SshStreamFrame>,
    handle: russh::server::Handle,
    channels: ChannelMap,
    stop: CancellationToken,
) {
    loop {
        let frame = tokio::select! {
            _ = stop.cancelled() => return,
            frame = frames.recv() => frame,
        };
        let Some(frame) = frame else { return };
        let id = channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&frame.channel_id)
            .copied();
        let Some(id) = id else { continue };
        match frame.ctl {
            Some(SshStreamControl::Exit { status }) => {
                let _ = handle
                    .exit_status_request(id, status.unwrap_or(FAILURE_EXIT_STATUS))
                    .await;
            }
            Some(SshStreamControl::Error { message }) => {
                let text = format!("trogon-sandbox-ssh: {message}\r\n");
                let _ = handle.data(id, Bytes::from(text)).await;
                let _ = handle.exit_status_request(id, FAILURE_EXIT_STATUS).await;
            }
            Some(_) => {}
            None => {
                if !frame.data.is_empty() {
                    let _ = handle.data(id, Bytes::from(frame.data)).await;
                }
            }
        }
        if frame.eof {
            let _ = handle.eof(id).await;
            let _ = handle.close(id).await;
        }
    }
}

/// A terminal side from an SSH request: zero means "not given" and takes `default`, and a side beyond what a
/// terminal supports is capped.
fn dimension(value: u32, default: u16) -> Result<TerminalDimension, TerminalDimensionError> {
    let wanted = u16::try_from(value)
        .ok()
        .filter(|side| *side > 0)
        .unwrap_or(default)
        .min(TerminalDimension::MAX);
    TerminalDimension::new(wanted)
}

impl Drop for SshGatewayHandler {
    fn drop(&mut self) {
        self.pump_stop.cancel();
        if let Some(attached) = self.attached.take() {
            let state = Arc::clone(&self.state);
            tokio::spawn(async move { end_session(&state, attached, "connection_drop").await });
        }
    }
}

impl Handler for SshGatewayHandler {
    type Error = GatewayError;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let lease_id = parse_lease_user(user).ok_or(GatewayError::InvalidUser)?;
        let Some((tenant, service_account)) = validate_bearer(self.state.config.api_keys.as_ref(), password).await?
        else {
            return Ok(Auth::reject());
        };

        let session_id = SshSessionId::generate();
        let session_key = EphemeralSessionKey::generate()?;
        let credential = match &self.state.config.key_mint {
            Some(mint) => Some(
                mint.register_session(&session_id, &lease_id, DEFAULT_SESSION_TTL_SECONDS, service_account)
                    .await?,
            ),
            None => None,
        };

        let attach = SshAttachRequest {
            session_id: session_id.clone(),
            lease_id: lease_id.clone(),
            public_key: session_key.public_key_line,
            ttl_seconds: DEFAULT_SESSION_TTL_SECONDS,
        };
        self.state
            .config
            .tunnel
            .attach(&tenant, &attach)
            .await
            .map_err(GatewayError::Attach)?;

        self.attached = Some(AttachedSession {
            id: session_id,
            lease_id,
            tenant,
            credential,
        });
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(u32::from(channel.id()), channel.id());
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ptys.insert(
            u32::from(channel),
            PtyRequest {
                term: term.to_string(),
                cols: dimension(col_width, DEFAULT_COLS)?,
                rows: dimension(row_height, DEFAULT_ROWS)?,
            },
        );
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let session_id = self.session_id()?.clone();
        let cols = dimension(col_width, DEFAULT_COLS)?;
        let rows = dimension(row_height, DEFAULT_ROWS)?;
        if let Some(pty) = self.ptys.get_mut(&u32::from(channel)) {
            pty.cols = cols;
            pty.rows = rows;
        }
        let frame = SshStreamFrame::control(&session_id, u32::from(channel), SshStreamControl::Resize { cols, rows });
        let _ = self.state.config.tunnel.publish_frame(&frame).await;
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        self.open_channel(channel, SshChannelKind::Shell, session).await
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).trim().to_string();
        if command == "exit" {
            session.channel_success(channel)?;
            session.exit_status_request(channel, 0)?;
            session.close(channel)?;
            return Ok(());
        }
        self.open_channel(channel, SshChannelKind::Exec { command }, session)
            .await
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _session: &mut Session) -> Result<(), Self::Error> {
        let session_id = self.session_id()?;
        for chunk in data.chunks(MAX_FRAME_BYTES) {
            let frame = SshStreamFrame::data(session_id, u32::from(channel), chunk.to_vec());
            let _ = self.state.config.tunnel.publish_frame(&frame).await;
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        let frame = SshStreamFrame::eof(self.session_id()?, u32::from(channel));
        let _ = self.state.config.tunnel.publish_frame(&frame).await;
        Ok(())
    }

    async fn channel_close(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        let last = {
            let mut channels = self.channels.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            channels.remove(&u32::from(channel));
            channels.is_empty()
        };
        self.ptys.remove(&u32::from(channel));
        if last {
            self.end_session("channel_close").await;
        }
        Ok(())
    }
}

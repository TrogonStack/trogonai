//! The worker's side of the control protocol for one ordinary command: it numbers and sends control frames through the
//! session input, waits for the bridge to acknowledge each, and reads the bridge's state and capture files.

use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{CommandOutput, CommandTimeoutStatus};
use trogon_sandbox_protocol::{StdinWrite, cap_buffered};

use crate::constants::{ACK_POLL, STDIN_PIECE_BYTES};
use crate::installed_runtime::InstalledRuntime;
use crate::ordinary_acknowledgement::OrdinaryAcknowledgement;
use crate::ordinary_completion::OrdinaryCompletion;
use crate::ordinary_configuration::OrdinaryConfiguration;
use crate::ordinary_control::OrdinaryControl;
use crate::ordinary_identity::OrdinaryIdentity;
use crate::ordinary_operation::OrdinaryOperation;
use crate::ordinary_phase::OrdinaryPhase;
use crate::ordinary_protocol::{CONTROL_TIMEOUT, MAX_FRAME_BYTES, MAX_STATE_BYTES, VERSION};
use crate::ordinary_request::OrdinaryRequest;
use crate::ordinary_signal_outcome::OrdinarySignalOutcome;
use crate::ordinary_state::OrdinaryState;
use crate::sha256_pin::Sha256Pin;
use crate::{DaytonaApi, DaytonaApiError, DaytonaExecError};

/// One ordinary command: the sandbox and session it runs in, the runtime installed for it and the state of its
/// control channel.
#[derive(Debug)]
pub(crate) struct OrdinaryExecution {
    pub(crate) execution_id: String,
    pub(crate) nonce: String,
    pub(crate) prefix: String,
    pub(crate) sandbox_id: String,
    pub(crate) session_id: String,
    pub(crate) command_id: String,
    pub(crate) user: Option<String>,
    pub(crate) installed: InstalledRuntime,
    control: Mutex<OrdinaryControl>,
    /// The supervisor and root identities, which stay fixed from the moment they are first seen.
    binding: Mutex<Option<(OrdinaryIdentity, OrdinaryIdentity)>>,
}

impl OrdinaryExecution {
    pub(crate) fn new(
        execution_id: String,
        prefix: String,
        sandbox_id: String,
        session_id: String,
        command_id: String,
        user: Option<String>,
        installed: InstalledRuntime,
    ) -> Self {
        Self {
            nonce: installed.nonce.clone(),
            execution_id,
            prefix,
            sandbox_id,
            session_id,
            command_id,
            user,
            installed,
            control: Mutex::new(OrdinaryControl::default()),
            binding: Mutex::new(None),
        }
    }

    fn state_path(&self) -> String {
        format!("{}.state", self.prefix)
    }

    fn ack_path(&self) -> String {
        format!("{}.ack", self.prefix)
    }

    /// Creates the capture files and the stdin pipe and leaves the command prepared but not started.
    pub(crate) async fn prepare(
        &self,
        api: &DaytonaApi,
        configuration: OrdinaryConfiguration,
    ) -> Result<(), DaytonaExecError> {
        let interactive = configuration.stdin == trogon_sandbox_contract::StdinMode::Interactive;
        self.send(api, OrdinaryOperation::Init { config: configuration })
            .await?;
        if interactive && let Some(user) = &self.user {
            // The pipe is created by the bridge, which may not be the user the command runs as.
            let pipe = format!("{}.stdin", self.prefix);
            api.set_permissions_owned(&self.sandbox_id, &pipe, 0o600, user, None)
                .await?;
        }
        let state = self.read_state(api).await?;
        if state.phase == OrdinaryPhase::Prepared {
            Ok(())
        } else {
            Err(DaytonaExecError::OrdinaryUnconfirmed)
        }
    }

    /// Starts the held command and returns once the supervisor reports it running.
    pub(crate) async fn launch(&self, api: &DaytonaApi) -> Result<(), DaytonaExecError> {
        self.send(api, OrdinaryOperation::Launch).await?;
        let state = self.read_state(api).await?;
        match state.phase {
            OrdinaryPhase::Running | OrdinaryPhase::Draining | OrdinaryPhase::Absent => Ok(()),
            _ => Err(DaytonaExecError::OrdinaryUnconfirmed),
        }
    }

    /// Sends one control frame and waits for the acknowledgement of exactly that frame. Any failure ends control for
    /// good, since whether the bridge applied the frame is unknown.
    async fn send(
        &self,
        api: &DaytonaApi,
        operation: OrdinaryOperation,
    ) -> Result<OrdinaryAcknowledgement, DaytonaExecError> {
        let mut control = self.control.lock().await;
        if control.broken {
            return Err(DaytonaExecError::OrdinaryUnconfirmed);
        }
        control.sequence += 1;
        let sequence = control.sequence;
        let result = self.exchange(api, &operation, sequence).await;
        if result.is_err() {
            control.broken = true;
        }
        result
    }

    async fn exchange(
        &self,
        api: &DaytonaApi,
        operation: &OrdinaryOperation,
        sequence: u64,
    ) -> Result<OrdinaryAcknowledgement, DaytonaExecError> {
        let frame = serde_json::to_string(&OrdinaryRequest {
            version: VERSION,
            execution_id: &self.execution_id,
            nonce: &self.nonce,
            request_id: sequence,
            operation,
        })
        .map_err(|_| DaytonaExecError::OrdinaryUnconfirmed)?;
        if frame.len() > MAX_FRAME_BYTES {
            return Err(DaytonaExecError::OrdinaryUnconfirmed);
        }
        api.send_session_input(&self.sandbox_id, &self.session_id, &self.command_id, &frame)
            .await?;
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        loop {
            match api.download(&self.sandbox_id, &self.ack_path()).await {
                Ok(bytes) => {
                    if bytes.len() > MAX_STATE_BYTES {
                        return Err(DaytonaExecError::OrdinaryUnconfirmed);
                    }
                    let ack: OrdinaryAcknowledgement =
                        serde_json::from_slice(&bytes).map_err(|_| DaytonaExecError::OrdinaryUnconfirmed)?;
                    if !ack.is_bound(&self.execution_id, &self.nonce, sequence) {
                        return Err(DaytonaExecError::OrdinaryUnconfirmed);
                    }
                    if ack.request_id == sequence {
                        let matches = ack.operation == operation.name()
                            && ack.signal == operation.signal()
                            && match operation {
                                OrdinaryOperation::Signal { .. } => ack.delivered.is_some() && ack.bytes_written == 0,
                                OrdinaryOperation::Stdin { .. } => ack.delivered.is_none(),
                                _ => ack.is_plain(),
                            };
                        return if matches {
                            Ok(ack)
                        } else {
                            Err(DaytonaExecError::OrdinaryUnconfirmed)
                        };
                    }
                }
                Err(DaytonaApiError::NotFound(_)) => {}
                Err(error) => return Err(error.into()),
            }
            if Instant::now() >= deadline {
                return Err(DaytonaExecError::OrdinaryUnconfirmed);
            }
            tokio::time::sleep(ACK_POLL).await;
        }
    }

    /// Reads the bridge's state file and checks it belongs to this command.
    pub(crate) async fn read_state(&self, api: &DaytonaApi) -> Result<OrdinaryState, DaytonaExecError> {
        let bytes = api.download(&self.sandbox_id, &self.state_path()).await?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(DaytonaExecError::OrdinaryUnconfirmed);
        }
        let state: OrdinaryState = serde_json::from_slice(&bytes).map_err(|_| DaytonaExecError::OrdinaryUnconfirmed)?;
        if let Some(seen) = state.validate(&self.execution_id, &self.nonce)? {
            let mut binding = self.binding.lock().await;
            match *binding {
                None => *binding = Some(seen),
                Some(bound) if bound == seen => {}
                Some(_) => return Err(DaytonaExecError::OrdinaryUnconfirmed),
            }
        } else if self.binding.lock().await.is_some() {
            return Err(DaytonaExecError::OrdinaryUnconfirmed);
        }
        Ok(state)
    }

    /// The finished command's exit code, once the bridge reports its process family gone. A command the bridge gave
    /// up on is an error.
    pub(crate) async fn finished(&self, api: &DaytonaApi) -> Result<Option<OrdinaryCompletion>, DaytonaExecError> {
        let state = self.read_state(api).await?;
        match state.phase {
            OrdinaryPhase::Absent => Ok(state.completion),
            OrdinaryPhase::Failed => Err(DaytonaExecError::OrdinaryUnconfirmed),
            _ => Ok(None),
        }
    }

    /// Asks the supervisor to deliver `signal` to the command's process family.
    pub(crate) async fn signal(
        &self,
        api: &DaytonaApi,
        signal: u32,
    ) -> Result<OrdinarySignalOutcome, DaytonaExecError> {
        if self.finished(api).await?.is_some() {
            return Ok(OrdinarySignalOutcome::AlreadyCompleted);
        }
        let ack = self.send(api, OrdinaryOperation::Signal { signal }).await?;
        Ok(if ack.delivered == Some(true) {
            OrdinarySignalOutcome::Delivered
        } else {
            OrdinarySignalOutcome::AlreadyCompleted
        })
    }

    /// Writes the bytes of each write to the command's stdin pipe, in order, closing it after one that ends input.
    pub(crate) async fn write_stdin(&self, api: &DaytonaApi, writes: &[StdinWrite]) -> Result<(), DaytonaExecError> {
        for write in writes {
            let data = write.data.as_slice();
            let closes = write.eof.closes();
            let mut pieces: Vec<&[u8]> = data.chunks(STDIN_PIECE_BYTES).collect();
            if pieces.is_empty() && closes {
                pieces.push(&[]);
            }
            let last = pieces.len().saturating_sub(1);
            for (index, piece) in pieces.into_iter().enumerate() {
                let eof = closes && index == last;
                let ack = self
                    .send(
                        api,
                        OrdinaryOperation::Stdin {
                            data: STANDARD.encode(piece),
                            eof,
                        },
                    )
                    .await?;
                if ack.bytes_written != piece.len() || ack.eof != eof {
                    self.control.lock().await.broken = true;
                    return Err(DaytonaExecError::OrdinaryUnconfirmed);
                }
            }
        }
        Ok(())
    }

    /// The output of the finished command, with each stream checked against the length and digest the bridge
    /// recorded.
    pub(crate) async fn output(
        &self,
        api: &DaytonaApi,
        completion: &OrdinaryCompletion,
        cap: usize,
    ) -> Result<CommandOutput, DaytonaExecError> {
        let stdout = self
            .stream(api, "out", completion.stdout_len, &completion.stdout_sha256, "stdout")
            .await?;
        let stderr = self
            .stream(api, "err", completion.stderr_len, &completion.stderr_sha256, "stderr")
            .await?;
        let (stdout, out_truncated) = cap_buffered(stdout, cap);
        let (stderr, err_truncated) = cap_buffered(stderr, cap);
        let timed_out = if completion.timed_out {
            CommandTimeoutStatus::TimedOut
        } else {
            CommandTimeoutStatus::WithinDeadline
        };
        Ok(CommandOutput {
            exit_code: if completion.timed_out {
                CommandOutput::TIMED_OUT_EXIT_CODE
            } else {
                completion.root_exit().code()
            },
            stdout,
            stderr,
            timed_out,
            truncated: (out_truncated.is_truncated()
                || err_truncated.is_truncated()
                || completion.stdout_truncated
                || completion.stderr_truncated)
                .into(),
        })
    }

    async fn stream(
        &self,
        api: &DaytonaApi,
        extension: &str,
        length: u64,
        digest: &str,
        name: &'static str,
    ) -> Result<Vec<u8>, DaytonaExecError> {
        let bytes = api
            .download(&self.sandbox_id, &format!("{}.{extension}", self.prefix))
            .await
            .map_err(|_| DaytonaExecError::StreamUnreadable { stream: name })?;
        if bytes.len() as u64 != length || Sha256Pin::of(&bytes).as_str() != digest {
            return Err(DaytonaExecError::StreamUnreadable { stream: name });
        }
        Ok(bytes.to_vec())
    }

    /// Removes what the command left in the sandbox: its files and the installed runtime. Best effort.
    pub(crate) async fn cleanup(&self, api: &DaytonaApi) {
        for extension in ["out", "err", "exit", "status", "state", "ack", "stdin"] {
            let _ = api
                .delete_file(&self.sandbox_id, &format!("{}.{extension}", self.prefix))
                .await;
        }
        let _ = self.installed.cleanup(api, &self.sandbox_id).await;
    }
}

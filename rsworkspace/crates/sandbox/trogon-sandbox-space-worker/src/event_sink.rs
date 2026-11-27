use base64::Engine;
use serde_json::Value;
use trogon_sandbox_contract::{CommandOutput, CommandTimeoutStatus, OutputTruncation};
use trogon_sandbox_protocol::CappedOutput;

use crate::{ExecFailureError, SseFrame};

/// The result of one command, accumulated from its event stream.
pub(crate) struct EventSink {
    stdout: CappedOutput,
    stderr: CappedOutput,
    pub(crate) last_id: Option<String>,
    pub(crate) exit: Option<(i32, String)>,
}

impl EventSink {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            stdout: CappedOutput::new(cap),
            stderr: CappedOutput::new(cap),
            last_id: None,
            exit: None,
        }
    }

    pub(crate) fn apply(&mut self, frame: SseFrame) -> Result<(), ExecFailureError> {
        if let Some(id) = frame.id {
            self.last_id = Some(id);
        }
        match frame.data.get("type").and_then(Value::as_str) {
            Some(kind @ ("stdout" | "stderr")) => {
                let data = frame
                    .data
                    .get("data_base64")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| ExecFailureError::Unknown("output frame was not base64".into()))?;
                if kind == "stdout" {
                    self.stdout.push(&bytes);
                } else {
                    self.stderr.push(&bytes);
                }
            }
            Some("process_exited") => {
                let code = frame
                    .data
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| ExecFailureError::Unknown("exit event had no exit code".into()))?;
                let status = frame.data.get("status").and_then(Value::as_str).unwrap_or("unknown");
                self.exit = Some((i32::try_from(code).unwrap_or(-1), status.to_owned()));
            }
            Some(kind @ ("resync_required" | "error")) => {
                return Err(ExecFailureError::Unknown(format!(
                    "the event stream reported {kind}, so output was lost"
                )));
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn finish(self, exit: (i32, String)) -> CommandOutput {
        let (code, status) = exit;
        let timed_out = status == "timed_out";
        CommandOutput {
            exit_code: if timed_out {
                CommandOutput::TIMED_OUT_EXIT_CODE
            } else {
                code
            },
            timed_out: CommandTimeoutStatus::from(timed_out),
            truncated: OutputTruncation::from(
                self.stdout.truncation().is_truncated() || self.stderr.truncation().is_truncated(),
            ),
            stdout: self.stdout.into_bytes(),
            stderr: self.stderr.into_bytes(),
        }
    }
}

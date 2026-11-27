use std::sync::atomic::Ordering;

use trogon_sandbox_contract::{WriteStdinRequest, WriteStdinResponse};
use trogon_sandbox_protocol::StdinAdmit;

use crate::scripts::stdin_lines;
use crate::{DaytonaExec, LeaseSandbox};

impl DaytonaExec {
    /// Delivers one stdin write to an interactive command, at most once and in `seq` order. An input call that fails
    /// leaves stdin closed for good, since whether the toolbox applied it is unknown.
    pub async fn handle_write_stdin(&self, sandbox: &LeaseSandbox, request: &WriteStdinRequest) -> WriteStdinResponse {
        let Some(execution) = self
            .owned_execution(sandbox, request.lease_id().as_str(), request.execution_id().as_str())
            .await
        else {
            return WriteStdinResponse::NotFound;
        };
        match self.exit_status(&execution).await {
            Ok(Some(_)) => return WriteStdinResponse::AlreadyCompleted,
            Ok(None) => {}
            Err(error) => return WriteStdinResponse::Denied(error.into_lease_error()),
        }
        let Some(stdin) = execution.stdin.clone() else {
            return WriteStdinResponse::StdinNotInteractive;
        };
        let mut sequencer = stdin.sequencer.lock().await;
        if stdin.broken.load(Ordering::Acquire) {
            return WriteStdinResponse::StdinClosed;
        }
        let writes = match sequencer.admit(request.seq(), request.data().clone(), request.eof()) {
            StdinAdmit::Apply(writes) => writes,
            StdinAdmit::Duplicate => {
                return WriteStdinResponse::Duplicate {
                    next_seq: sequencer.next_seq(),
                };
            }
            StdinAdmit::Buffered => {
                return WriteStdinResponse::Buffered {
                    next_seq: sequencer.next_seq(),
                };
            }
            StdinAdmit::OutOfWindow => {
                return WriteStdinResponse::OutOfWindow {
                    next_seq: sequencer.next_seq(),
                };
            }
            StdinAdmit::Closed => return WriteStdinResponse::StdinClosed,
        };
        let mut applied = 0;
        if let Some(ordinary) = &execution.ordinary {
            for write in &writes {
                if ordinary
                    .write_stdin(&self.api, std::slice::from_ref(write))
                    .await
                    .is_err()
                {
                    sequencer.commit(applied);
                    stdin.broken.store(true, Ordering::Release);
                    return match self.exit_status(&execution).await {
                        Ok(Some(_)) => WriteStdinResponse::AlreadyCompleted,
                        _ => WriteStdinResponse::StdinClosed,
                    };
                }
                applied += 1;
            }
            sequencer.commit(applied);
            return WriteStdinResponse::Written {
                next_seq: sequencer.next_seq(),
            };
        }
        for write in &writes {
            for line in stdin_lines(write) {
                let sent = self
                    .api
                    .send_session_input(
                        &execution.sandbox_id,
                        &execution.session_id,
                        &execution.command_id,
                        &line,
                    )
                    .await;
                if sent.is_err() {
                    sequencer.commit(applied);
                    stdin.broken.store(true, Ordering::Release);
                    return match self.exit_status(&execution).await {
                        Ok(Some(_)) => WriteStdinResponse::AlreadyCompleted,
                        _ => WriteStdinResponse::StdinClosed,
                    };
                }
            }
            applied += 1;
        }
        sequencer.commit(applied);
        WriteStdinResponse::Written {
            next_seq: sequencer.next_seq(),
        }
    }
}

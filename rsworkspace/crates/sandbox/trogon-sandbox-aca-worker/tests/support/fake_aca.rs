use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use trogon_sandbox_aca_worker::{
    AcaError, AcaSessionApi, ExecuteResult, SessionDeleteOutcome, SessionIdentifier, SessionInfo, SessionStatus,
};
use trogon_sandbox_contract::ShortLivedToken;

#[derive(Default)]
struct State {
    gets: VecDeque<Result<SessionStatus, u16>>,
    deletes: VecDeque<Result<SessionDeleteOutcome, u16>>,
    executed: Vec<String>,
    uploads: Vec<(String, Vec<u8>)>,
    bearers: Vec<String>,
    sessions: Vec<String>,
    fail_upload: bool,
    fail_execute: bool,
}

/// An in-process stand-in for the Dynamic Sessions data plane that records what it was asked.
#[derive(Default)]
pub struct FakeAca {
    state: Mutex<State>,
}

impl FakeAca {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next `get_session` answers with this status code.
    pub fn script_get(&self, answer: Result<SessionStatus, u16>) {
        self.state().gets.push_back(answer);
    }

    /// The next `delete_session` answers with this outcome or status code.
    pub fn script_delete(&self, answer: Result<SessionDeleteOutcome, u16>) {
        self.state().deletes.push_back(answer);
    }

    pub fn fail_uploads(&self) {
        self.state().fail_upload = true;
    }

    pub fn fail_executions(&self) {
        self.state().fail_execute = true;
    }

    pub fn executed(&self) -> Vec<String> {
        self.state().executed.clone()
    }

    pub fn uploads(&self) -> Vec<(String, Vec<u8>)> {
        self.state().uploads.clone()
    }

    pub fn bearers(&self) -> Vec<String> {
        self.state().bearers.clone()
    }

    pub fn sessions(&self) -> Vec<String> {
        self.state().sessions.clone()
    }

    fn record(&self, bearer: &ShortLivedToken, identifier: &SessionIdentifier) {
        let mut state = self.state();
        state.bearers.push(bearer.expose_secret().to_owned());
        state.sessions.push(identifier.as_str().to_owned());
    }
}

#[async_trait]
impl AcaSessionApi for FakeAca {
    async fn get_session(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
    ) -> Result<SessionInfo, AcaError> {
        self.record(bearer, identifier);
        let answer = self.state().gets.pop_front().unwrap_or(Ok(SessionStatus::Ready));
        match answer {
            Ok(status) => Ok(SessionInfo {
                identifier: identifier.clone(),
                status,
            }),
            Err(status) => Err(AcaError::aca_api(status, "scripted")),
        }
    }

    async fn execute_code(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
        code: &str,
        _language: &str,
    ) -> Result<ExecuteResult, AcaError> {
        self.record(bearer, identifier);
        let mut state = self.state();
        if state.fail_execute {
            return Err(AcaError::aca_api(500, "execute failed"));
        }
        state.executed.push(code.to_owned());
        Ok(ExecuteResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    async fn upload_file(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
        remote_path: &str,
        data: &[u8],
    ) -> Result<(), AcaError> {
        self.record(bearer, identifier);
        let mut state = self.state();
        if state.fail_upload {
            return Err(AcaError::aca_api(500, "upload failed"));
        }
        state.uploads.push((remote_path.to_owned(), data.to_vec()));
        Ok(())
    }

    async fn delete_session(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
    ) -> Result<SessionDeleteOutcome, AcaError> {
        self.record(bearer, identifier);
        let answer = self
            .state()
            .deletes
            .pop_front()
            .unwrap_or(Ok(SessionDeleteOutcome::Deleted));
        answer.map_err(|status| AcaError::aca_api(status, "scripted"))
    }
}

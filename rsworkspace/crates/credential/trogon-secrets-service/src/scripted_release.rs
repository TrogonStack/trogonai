use std::sync::Mutex;

use tokio::sync::Notify;

use super::{CredentialReleaseError, ReleaseCredential, ReleaseRequest, SealedRelease};

/// A [`ReleaseCredential`] that answers every call with one scripted outcome,
/// or never answers, and records what it was asked.
pub(crate) struct ScriptedRelease {
    outcome: Option<Result<SealedRelease, CredentialReleaseError>>,
    calls: Mutex<Vec<(ReleaseRequest, String)>>,
    called: Notify,
}

impl ScriptedRelease {
    pub(crate) fn answering(outcome: Result<SealedRelease, CredentialReleaseError>) -> Self {
        Self::scripted(Some(outcome))
    }

    pub(crate) fn hanging() -> Self {
        Self::scripted(None)
    }

    fn scripted(outcome: Option<Result<SealedRelease, CredentialReleaseError>>) -> Self {
        Self {
            outcome,
            calls: Mutex::new(Vec::new()),
            called: Notify::new(),
        }
    }

    pub(crate) fn calls(&self) -> Vec<(ReleaseRequest, String)> {
        self.calls.lock().unwrap().clone()
    }

    /// Waits for a call made since the last wait.
    pub(crate) async fn called(&self) {
        self.called.notified().await;
    }
}

impl ReleaseCredential for ScriptedRelease {
    async fn release(
        &self,
        request: &ReleaseRequest,
        requester_xkey: &str,
    ) -> Result<SealedRelease, CredentialReleaseError> {
        self.calls
            .lock()
            .unwrap()
            .push((request.clone(), requester_xkey.to_owned()));
        self.called.notify_one();
        match &self.outcome {
            Some(outcome) => outcome.clone(),
            None => std::future::pending().await,
        }
    }
}

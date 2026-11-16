use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_secret_store::OpenBaoLoginError;

/// How the service retries its startup login to OpenBao. An instance that
/// cannot authenticate keeps out of the release queue group and retries with
/// backoff (ADR#0023 Decision 3): the delay starts at `initial` and doubles
/// after each refusal, up to `max`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LoginRetry {
    initial: Duration,
    max: Duration,
}

impl LoginRetry {
    pub(crate) const fn new(initial: Duration, max: Duration) -> Self {
        Self { initial, max }
    }

    /// Attempts `login` until OpenBao accepts it, waiting the backoff delay
    /// after each failed attempt and logging why it failed with its attempt
    /// number and the delay before the next one. An attempt fails when OpenBao
    /// refuses it or when the secret id file cannot be read for it, which a
    /// platform rotating the file in place can cause for a moment.
    ///
    /// Returns `Ok(None)` when `shutdown` comes first, whether during an
    /// attempt or the wait after it, so the caller stops without serving.
    ///
    /// # Errors
    ///
    /// Returns [`OpenBaoLoginError::Config`] at once, without a retry: an
    /// address or client the login cannot be built with stays that way.
    pub(crate) async fn log_in<Login, Attempt, Store>(
        self,
        shutdown: &CancellationToken,
        mut login: Login,
    ) -> Result<Option<Store>, OpenBaoLoginError>
    where
        Login: FnMut() -> Attempt,
        Attempt: Future<Output = Result<Store, OpenBaoLoginError>>,
    {
        let mut attempt: u32 = 1;
        let mut delay = self.initial;
        loop {
            if shutdown.is_cancelled() {
                return Ok(None);
            }
            let outcome = tokio::select! {
                biased;
                () = shutdown.cancelled() => return Ok(None),
                outcome = login() => outcome,
            };
            match outcome {
                Ok(store) => return Ok(Some(store)),
                Err(error @ OpenBaoLoginError::Config(_)) => return Err(error),
                Err(error @ OpenBaoLoginError::Refused { .. }) => {
                    warn!(attempt, retry_in = ?delay, error = %error, "OpenBao refused the AppRole login; retrying");
                }
                Err(error @ OpenBaoLoginError::SecretId(_)) => {
                    warn!(attempt, retry_in = ?delay, error = %error, "could not read the OpenBao AppRole secret id; retrying");
                }
            }
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return Ok(None),
                () = tokio::time::sleep(delay) => {}
            }
            attempt = attempt.saturating_add(1);
            delay = delay.saturating_mul(2).min(self.max);
        }
    }
}

#[cfg(test)]
mod tests;

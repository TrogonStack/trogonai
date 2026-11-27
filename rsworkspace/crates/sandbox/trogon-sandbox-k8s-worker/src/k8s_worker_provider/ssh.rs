use trogon_sandbox_contract::LeaseId;

use crate::{K8sWorkerError, K8sWorkerProvider};

impl K8sWorkerProvider {
    /// Installs `public_key` (a full `authorized_keys` line) into
    /// `lease_id`'s pod under `~/.ssh/authorized_keys`, tracking
    /// `session_id -> public_key` so [`Self::ssh_detach_for_lease`] can strip
    /// precisely this line later without disturbing other concurrent SSH
    /// sessions on the same pod. Returns `None` if `lease_id` has no active
    /// lease on this worker.
    pub async fn ssh_attach_for_lease(
        &self,
        lease_id: &LeaseId,
        session_id: &str,
        public_key: &str,
    ) -> Option<Result<(), K8sWorkerError>> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        let escaped_key = public_key.replace('\'', "'\\''");
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!(
                "mkdir -p ~/.ssh && chmod 700 ~/.ssh && echo '{escaped_key}' >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"
            ),
        ];
        let result = match self.inner.exec_in_pod(&sandbox_id, command).await {
            Ok(output) if output.exit_code == 0 => Ok(()),
            Ok(output) => Err(K8sWorkerError::CommandFailed {
                action: "ssh attach",
                exit_code: output.exit_code,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
            Err(e) => Err(e.into()),
        };
        if result.is_ok() {
            self.ssh_sessions
                .lock()
                .await
                .insert(session_id.to_string(), (lease_id.clone(), public_key.to_string()));
        }
        Some(result)
    }

    /// Removes the `authorized_keys` line installed by
    /// [`Self::ssh_attach_for_lease`] for `session_id`, leaving other
    /// sessions' keys intact. A no-op success (not an error) when the
    /// session is already gone. Returns `None` if `lease_id` has no active
    /// lease on this worker.
    ///
    /// Filters via `grep -vF` fed the key over stdin rather than
    /// interpolating it into a shell command, so arbitrary `authorized_keys`
    /// line content never needs shell-metacharacter escaping.
    pub async fn ssh_detach_for_lease(
        &self,
        lease_id: &LeaseId,
        session_id: &str,
    ) -> Option<Result<(), K8sWorkerError>> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        let Some((_, public_key)) = self.ssh_sessions.lock().await.remove(session_id) else {
            return Some(Ok(()));
        };
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "f=~/.ssh/authorized_keys; [ -f \"$f\" ] || exit 0; \
                key=$(cat); tmp=\"$f.tmp.$$\"; grep -vF \"$key\" \"$f\" > \"$tmp\" || true; \
                mv \"$tmp\" \"$f\""
                .to_string(),
        ];
        let result = match self
            .inner
            .exec_in_pod_with_stdin(&sandbox_id, command, public_key.into_bytes())
            .await
        {
            Ok(output) if output.exit_code == 0 => Ok(()),
            Ok(output) => Err(K8sWorkerError::CommandFailed {
                action: "ssh detach",
                exit_code: output.exit_code,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
            Err(e) => Err(e.into()),
        };
        Some(result)
    }
}

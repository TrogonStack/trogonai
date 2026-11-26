use std::time::Duration;

use crate::acl_outcome::AclOutcome;
use crate::acl_release_error::AclReleaseError;
use crate::acl_revoke::AclRevoke;
use crate::acl_revoke::flatten_restore;
use crate::acl_revoke::judge_revoke;
use crate::acl_revoke::path_is_gone;
use crate::fs_access_set::FsAccessSet;
use crate::windows_fence::WindowsFence;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

const GRANT_TIMEOUT: Duration = Duration::from_secs(600);

const STAMP_TIMEOUT: Duration = Duration::from_secs(60);

const RELEASE_TIMEOUT: Duration = Duration::from_secs(60);

const PARTIAL: i32 = 2;

fn holder_args(verb: &str, holder_pid: u32, sandbox_user_sid: &str) -> Vec<String> {
    vec![
        "acl".to_string(),
        verb.to_string(),
        "--holder-pid".to_string(),
        holder_pid.to_string(),
        "--sandbox-user-sid".to_string(),
        sandbox_user_sid.to_string(),
    ]
}

fn grant_args(
    sandbox_user_sid: &str,
    holder_pid: u32,
    persistent: bool,
    allow_profile_exclusions: &[String],
) -> Vec<String> {
    let mut args = holder_args("grant", holder_pid, sandbox_user_sid);
    if persistent {
        args.push("--persistent".to_string());
    }
    for path in allow_profile_exclusions {
        args.push("--allow-profile-exclusion".to_string());
        args.push(path.clone());
    }
    args
}

fn refused(label: &str, status: Option<i32>, stdout: &str, stderr: &str) -> String {
    let status_text = status.map_or_else(|| "null".to_string(), |code| code.to_string());
    let how = if status == Some(PARTIAL) {
        "(partial - some inputs skipped)"
    } else {
        "(failed)"
    };
    let detail = if stderr.is_empty() { stdout } else { stderr };
    format!("fence-helper {label} exited {status_text} {how}: {detail}")
}

impl WindowsFence {
    /// Session start under the write-restricted token: one `fence-helper acl grant
    /// --persistent`. Grants on the write roots are kept at the end of the
    /// session. A failure leaves whatever was granted for
    /// [`WindowsFence::acl_revoke`] to release.
    pub fn acl_grant_persistent(
        &self,
        sandbox_user_sid: &str,
        holder_pid: u32,
        access: &FsAccessSet,
        allow_profile_exclusions: &[String],
    ) -> Result<(), WindowsSandboxError> {
        let args = grant_args(sandbox_user_sid, holder_pid, true, allow_profile_exclusions);
        self.acl_apply(
            "acl grant --persistent",
            Code::AclGrantFailed,
            &args,
            access,
            GRANT_TIMEOUT,
        )
    }

    /// The grants of one session, released with the
    /// holder. A failure leaves whatever was granted for
    /// [`WindowsFence::acl_revoke`] to release.
    pub fn acl_grant(
        &self,
        sandbox_user_sid: &str,
        holder_pid: u32,
        read: &[String],
        write: &[String],
        allow_profile_exclusions: &[String],
    ) -> Result<(), WindowsSandboxError> {
        let args = grant_args(sandbox_user_sid, holder_pid, false, allow_profile_exclusions);
        let access = FsAccessSet {
            grant_read: read.to_vec(),
            grant_write: write.to_vec(),
            ..FsAccessSet::default()
        };
        self.acl_apply("acl grant", Code::AclGrantFailed, &args, &access, STAMP_TIMEOUT)
    }

    /// The file denies of one session. Inputs that
    /// resolve are stamped even when others are skipped, so a failure leaves
    /// something for [`WindowsFence::acl_restore`] to release.
    pub fn acl_stamp(
        &self,
        sandbox_user_sid: &str,
        holder_pid: u32,
        deny_read: &[String],
        deny_write: &[String],
    ) -> Result<(), WindowsSandboxError> {
        let args = holder_args("stamp", holder_pid, sandbox_user_sid);
        let access = FsAccessSet {
            deny_read: deny_read.to_vec(),
            deny_write: deny_write.to_vec(),
            ..FsAccessSet::default()
        };
        self.acl_apply("acl stamp", Code::AclStampFailed, &args, &access, STAMP_TIMEOUT)
    }

    fn acl_apply(
        &self,
        label: &str,
        code: Code,
        args: &[String],
        access: &FsAccessSet,
        timeout: Duration,
    ) -> Result<(), WindowsSandboxError> {
        let stdin = serde_json::to_string(access)
            .map_err(|error| WindowsSandboxError::new(code, format!("{label} input: {error}")))?;
        let out = self.fence_helper().run_with_stdin(args, &stdin, timeout)?;
        if out.status != Some(0) {
            return Err(WindowsSandboxError::new(
                code,
                refused(label, out.status, &out.stdout, &out.stderr),
            ));
        }
        Ok(())
    }

    /// Releases this holder's grants and says whether it completed.
    pub fn acl_revoke(&self, sandbox_user_sid: &str, holder_pid: u32) -> AclRevoke {
        let mut args = holder_args("revoke", holder_pid, sandbox_user_sid);
        args.push("--json".to_string());
        match self.fence_helper().run(&args, RELEASE_TIMEOUT) {
            Ok(out) => judge_revoke(out.status == Some(0), &out.stdout, &out.stderr, &path_is_gone),
            Err(error) => AclRevoke {
                complete: false,
                outcomes: None,
                pending: Vec::new(),
                detail: Some(error.message().to_string()),
            },
        }
    }

    /// Revoke the holder's grants and,
    /// only when that completed, restore its denies. The error says what
    /// stayed behind.
    pub fn acl_release(&self, sandbox_user_sid: &str, holder_pid: u32) -> Result<(), AclReleaseError> {
        let revoked = self.acl_revoke(sandbox_user_sid, holder_pid);
        if !revoked.complete {
            return Err(AclReleaseError::GrantsNotReleased {
                pending: revoked.pending,
                detail: revoked.detail.unwrap_or_else(|| "no detail".to_string()),
            });
        }
        self.acl_restore(sandbox_user_sid, holder_pid)
            .map(drop)
            .ok_or(AclReleaseError::DeniesNotRestored)
    }

    /// Releases this holder's deny entries. Best effort: `None` when
    /// `fence-helper` failed. `fence-helper` refuses while the holder still has a
    /// grant recorded, so revoke first and restore only when it completed.
    pub fn acl_restore(&self, sandbox_user_sid: &str, holder_pid: u32) -> Option<Vec<AclOutcome>> {
        let mut args = holder_args("restore", holder_pid, sandbox_user_sid);
        args.push("--json".to_string());
        let out = self.fence_helper().run(&args, RELEASE_TIMEOUT).ok()?;
        flatten_restore(&out.stdout)
    }
}

#[cfg(test)]
mod tests;

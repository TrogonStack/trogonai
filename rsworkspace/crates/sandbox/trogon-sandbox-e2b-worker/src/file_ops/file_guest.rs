use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileStat};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::guest_bodies::{ListDirBody, PathBody};
use super::guest_entry::GuestEntry;
use crate::api::ApiError;
use crate::envd::{Envd, EnvdTarget, ProcessRequest, ProcessResult};

/// One sandbox's envd, addressed as one user, for the length of a file operation.
#[derive(Clone, Copy)]
pub(super) struct FileGuest<'a> {
    pub(super) envd: &'a Envd,
    pub(super) target: &'a EnvdTarget,
    pub(super) user: &'a str,
}

impl<'a> FileGuest<'a> {
    pub(super) fn new(envd: &'a Envd, target: &'a EnvdTarget, user: &'a str) -> Self {
        Self { envd, target, user }
    }

    /// One unary call of envd's filesystem service.
    pub(super) async fn rpc<B: Serialize>(&self, method: &str, body: B) -> Result<Value, ApiError> {
        self.envd.filesystem_unary(self.target, self.user, method, body).await
    }

    pub(super) async fn stat(&self, path: &str) -> Result<FileStat, FileOpError> {
        if path.contains('\0') {
            return Err(FileOpError::invalid_path("path contains a NUL"));
        }
        let reply = self
            .rpc("Stat", PathBody { path })
            .await
            .map_err(|error| api_failure(error, path))?;
        Ok(GuestEntry::parse(reply.get("entry").cloned().unwrap_or(Value::Null))?)
    }

    /// The direct children of a directory, sorted by name, each checked to sit directly under it.
    pub(super) async fn directory(&self, path: &str) -> Result<Vec<FileStat>, FileOpError> {
        let reply = self
            .rpc("ListDir", ListDirBody { path, depth: 1 })
            .await
            .map_err(|error| api_failure(error, path))?;
        let values = match reply.get("entries") {
            None => return Ok(vec![]),
            Some(Value::Array(values)) => values,
            _ => {
                return Err(FileOpError::failed(
                    FileOpErrorKind::Io,
                    "invalid envd directory entries",
                ));
            }
        };
        let prefix = format!("{}/", path.trim_end_matches('/'));
        let mut entries = Vec::with_capacity(values.len().min(FileOp::MAX_ENTRIES as usize));
        for value in values {
            let entry = GuestEntry::parse(value.clone())?;
            let relative = entry.path.strip_prefix(&prefix).ok_or_else(|| {
                FileOpError::failed(FileOpErrorKind::Io, "envd returned an entry outside the directory")
            })?;
            if relative.is_empty()
                || relative.contains('/')
                || relative == "."
                || relative == ".."
                || relative.contains('\0')
                || relative != entry.name
            {
                return Err(FileOpError::failed(
                    FileOpErrorKind::Io,
                    "envd returned a malformed directory entry",
                ));
            }
            entries.push(entry);
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// Runs `script` with `args` through `sh` under the C locale, so tool messages stay in English.
    /// `subject` names what a missing sandbox is reported against.
    pub(super) async fn script(
        &self,
        script: &str,
        args: Vec<String>,
        timeout: Duration,
        subject: &str,
    ) -> Result<ProcessResult, FileOpError> {
        let mut argv: Vec<String> = ["env", "LC_ALL=C", "sh", "-c", script, "sh"]
            .into_iter()
            .map(String::from)
            .collect();
        argv.extend(args);
        let output = self
            .envd
            .run(
                self.target,
                &ProcessRequest {
                    argv,
                    env: BTreeMap::new(),
                    cwd: None,
                    user: self.user.into(),
                    timeout,
                },
            )
            .await
            .map_err(|error| api_failure(error, subject))?;
        Ok(output)
    }

    /// Removes a staged guest file. A file that is already gone counts as removed.
    pub(super) async fn discard(&self, path: &str) -> Result<(), ApiError> {
        match self.rpc("Remove", PathBody { path }).await {
            Ok(_) | Err(ApiError::NotFound(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Removes a staged guest file and only logs a failure, because the operation already has its result.
    pub(super) async fn discard_logged(&self, path: &str) {
        self.discard(path)
            .await
            .inspect_err(|error| tracing::warn!(%error, staged = path, "guest staging cleanup failed"))
            .ok();
    }
}

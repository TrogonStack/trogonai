use super::*;

/// Turns the result of `cat <path>` into file bytes. `cat` prints this errno
/// text (exit 1) for an absent path in every libc/busybox flavour, so a
/// missing file stays distinct from other read failures.
pub(super) fn cat_output_into_file_bytes(
    path: &str,
    out: CommandOutput,
) -> Result<Vec<u8>, ReadFileError<anyhow::Error>> {
    if out.exit_code != 0 {
        if String::from_utf8_lossy(&out.stderr).contains("No such file or directory") {
            return Err(ReadFileError::NotFound(FileNotFoundError { path: path.into() }));
        }
        return Err(ReadFileError::Other(anyhow::anyhow!(
            "reading {path} failed (exit {}): {}",
            out.exit_code,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out.stdout)
}

impl DockerWorkerProvider {
    /// Writes `files` into `lease_id`'s running container by shelling out to
    /// `sh -c 'mkdir -p ... && cat > ...'` per file and piping the content
    /// through the exec's stdin, then `chmod`-ing when a mode is given.
    /// Avoids building a tar archive since bollard's `upload_to_container`
    /// takes a raw tar body, which would be a heavier dependency surface for
    /// no real benefit at this file count/size.
    pub async fn write_files_for_lease(
        &self,
        lease_id: &LeaseId,
        files: &[SandboxFileWrite],
        as_user: Option<&SandboxUserName>,
    ) -> anyhow::Result<()> {
        let container_id = self.active_container_id(lease_id).await?;
        let cwd = as_user.map(home_dir_for_user);
        let user = as_user.map(SandboxUserName::as_str);
        for file in files {
            let write_cmd = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!(
                    "mkdir -p \"$(dirname '{}')\" && cat > '{}'",
                    file.path.replace('\'', "'\\''"),
                    file.path.replace('\'', "'\\''")
                ),
            ];
            let out = Self::exec_collect(
                &self.docker,
                &container_id,
                write_cmd,
                cwd.clone(),
                &std::collections::BTreeMap::new(),
                ExecStdin::Once(file.content.clone()),
                user,
                None,
                None,
                OutputRetention::Buffer,
            )
            .await?;
            if out.exit_code != 0 {
                anyhow::bail!(
                    "writing {} failed (exit {}): {}",
                    file.path,
                    out.exit_code,
                    String::from_utf8_lossy(&out.stderr)
                );
            }

            if let Some(mode) = file.mode {
                let chmod_cmd = vec!["chmod".to_string(), format!("{mode:o}"), file.path.clone()];
                let out = Self::exec_collect(
                    &self.docker,
                    &container_id,
                    chmod_cmd,
                    cwd.clone(),
                    &std::collections::BTreeMap::new(),
                    ExecStdin::None,
                    user,
                    None,
                    None,
                    OutputRetention::Buffer,
                )
                .await?;
                if out.exit_code != 0 {
                    anyhow::bail!(
                        "chmod {} on {} failed (exit {}): {}",
                        mode,
                        file.path,
                        out.exit_code,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
            }
        }
        Ok(())
    }

    /// Reads a single file out of `lease_id`'s running container via `cat`.
    pub async fn read_file_for_lease(
        &self,
        lease_id: &LeaseId,
        path: &str,
        as_user: Option<&SandboxUserName>,
    ) -> Result<Vec<u8>, ReadFileError<anyhow::Error>> {
        let container_id = self.active_container_id(lease_id).await.map_err(ReadFileError::Other)?;
        let cwd = as_user.map(home_dir_for_user);
        let cmd = vec!["cat".to_string(), path.to_string()];
        let out = Self::exec_collect(
            &self.docker,
            &container_id,
            cmd,
            cwd,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            as_user.map(SandboxUserName::as_str),
            None,
            None,
            OutputRetention::Buffer,
        )
        .await
        .map_err(ReadFileError::Other)?;
        cat_output_into_file_bytes(path, out)
    }

    /// Scans the new container's workspace so `changes` without a checkpoint is exact.
    pub(super) async fn record_start_baseline(&self, lease_id: &LeaseId, container_id: &str) {
        let container = crate::file_ops::DockerContainer {
            docker: &self.docker,
            container_id: container_id.to_string(),
        };
        let source = crate::file_ops::ContainerSource {
            container: &container,
            inotify_missing: None,
            feeds: None,
        };
        crate::file_ops::record_start_baseline(&self.tracking, lease_id.as_str(), &source).await;
    }

    /// Runs a structured file operation inside `lease_id`'s running container. `None` means
    /// this worker holds no container for the lease, so the caller stays silent.
    pub async fn file_op_for_lease(
        &self,
        request: &trogon_sandbox_contract::FileOpRequest,
    ) -> Option<trogon_sandbox_contract::FileOpResponse> {
        let container_id = self.active_container_id(&request.lease_id).await.ok()?;
        let container = crate::file_ops::DockerContainer {
            docker: &self.docker,
            container_id: container_id.clone(),
        };
        let class = FileOpClass::of(&request.op);
        if class.is_tracked() {
            if request.as_user.is_some() {
                return Some(trogon_sandbox_contract::FileOpResponse::Unsupported {
                    message: "the docker provider cannot run a file operation as another user".into(),
                });
            }
            let source = crate::file_ops::ContainerSource {
                container: &container,
                inotify_missing: Some((&self.inotify_missing, request.lease_id.as_str())),
                feeds: Some(&self.watch_feeds),
            };
            let response = self
                .tracking
                .serve(request.lease_id.as_str(), &request.op, &source)
                .await;
            if class.is_watch() {
                self.sync_watch_feeds(&request.lease_id, &container_id, &request.op, &response);
            }
            self.publish_watch_overflows(&request.lease_id).await;
            return response;
        }
        Some(crate::file_ops::execute(&container, &request.op, request.as_user.is_some()).await)
    }

    /// Starts the feed of a watch that just started, and stops the feeds of watches that
    /// stopped or expired. An image known to lack `inotifywait` gets no feed.
    pub(super) fn sync_watch_feeds(
        &self,
        lease_id: &LeaseId,
        container_id: &str,
        op: &trogon_sandbox_contract::FileOp,
        response: &Option<trogon_sandbox_contract::FileOpResponse>,
    ) {
        self.watch_feeds.retain(
            &self.docker,
            container_id,
            lease_id.as_str(),
            &self.tracking.watch_ids(lease_id.as_str()),
        );
        if let (
            trogon_sandbox_contract::FileOp::WatchStart { path, recursive },
            Some(trogon_sandbox_contract::FileOpResponse::WatchStarted { watch_id }),
        ) = (op, response)
            && !self.inotify_missing.contains(lease_id.as_str())
            && let Ok(path) = crate::file_ops::absolute_path(path)
        {
            self.watch_feeds.start(
                self.docker.clone(),
                container_id.to_string(),
                lease_id.as_str(),
                watch_id,
                path,
                *recursive,
                Arc::clone(&self.inotify_missing),
            );
        }
    }

    /// Puts watch overflows on the lease's evidence timeline. Best effort: a
    /// failed publish is logged and never fails the file operation.
    pub(super) async fn publish_watch_overflows(&self, lease_id: &LeaseId) {
        for (subject, evidence) in self.tracking.take_overflow_evidence(lease_id.as_str(), lease_id) {
            match serde_json::to_vec(&evidence) {
                Ok(payload) => {
                    if let Err(e) = self.js.publish(subject.clone(), payload.into()).await {
                        warn!(%subject, error = %e, "failed to publish file watch overflow");
                    }
                }
                Err(e) => warn!(error = %e, "failed to serialize file watch overflow"),
            }
        }
    }

    /// Runs `command` in the container for the file operation scripts, with optional stdin.
    pub(crate) async fn exec_for_file_op(
        docker: &Docker,
        container_id: &str,
        command: Vec<String>,
        stdin: Option<Vec<u8>>,
    ) -> anyhow::Result<CommandOutput> {
        Self::exec_collect(
            docker,
            container_id,
            command,
            None,
            &std::collections::BTreeMap::new(),
            stdin.map_or(ExecStdin::None, ExecStdin::Once),
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await
    }
}

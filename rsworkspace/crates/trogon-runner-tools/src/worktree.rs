use tokio::process::Command;

pub struct TempWorktree {
    pub path: String,
    parent_cwd: String,
    cleaned: bool,
}

impl TempWorktree {
    /// Normal async cleanup — call when in async context.
    pub async fn cleanup(mut self) {
        self.cleaned = true;
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force", &self.path])
            .current_dir(&self.parent_cwd)
            .status()
            .await;
    }
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        // best-effort sync cleanup — use std::thread::spawn to avoid blocking tokio thread
        let path = self.path.clone();
        let parent_cwd = self.parent_cwd.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("git")
                .args(["worktree", "remove", "--force", &path])
                .current_dir(&parent_cwd)
                .status();
        });
    }
}

pub async fn create_worktree(parent_cwd: &str) -> Option<TempWorktree> {
    let is_git = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(parent_cwd)
        .output()
        .await
        .ok()?
        .status
        .success();

    if !is_git {
        return None;
    }

    let uuid = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/trogon-sub-{}", &uuid[..8]);

    let ok = Command::new("git")
        .args(["worktree", "add", &path, "HEAD"])
        .current_dir(parent_cwd)
        .status()
        .await
        .ok()?
        .success();

    if ok {
        Some(TempWorktree { path, parent_cwd: parent_cwd.to_string(), cleaned: false })
    } else {
        None
    }
}

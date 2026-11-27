#![cfg(target_os = "macos")]

//! A write grant named `/tmp/sandbox` must keep meaning that name when the name is later turned into a link to
//! the root or an ancestor, so the link cannot widen what a command may write.

use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{FsPathEntry, FsPolicy};
use trogon_sandbox_policy_exec::{
    CommandSpec, FilesystemView, HostPaths, NetworkPolicy, PolicyExec, PolicyExecConfig, SandboxSettings,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const LINK_NAMES: [&str; 2] = ["/tmp/sandbox", "/private/tmp/sandbox"];

fn clear_tmp_link() {
    for name in LINK_NAMES {
        let path = Path::new(name);
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                let _ = std::fs::remove_dir_all(path);
            }
            Ok(_) => {
                let _ = std::fs::remove_file(path);
            }
            Err(_) => {}
        }
    }
}

struct Scene {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    base: PathBuf,
    workspace: PathBuf,
    outside: PathBuf,
}

impl Scene {
    async fn new(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let guard = SERIAL.lock().await;
        clear_tmp_link();
        let id = std::process::id();
        let base = PathBuf::from(format!("/private/tmp/symlink-boundary-{label}-{id}"));
        let _ = std::fs::remove_dir_all(&base);
        let workspace = base.join("workspace");
        std::fs::create_dir_all(&workspace)?;
        let outside = PathBuf::from(format!("/private/tmp/outside-allowed-{label}-{id}.txt"));
        let _ = std::fs::remove_file(&outside);
        Ok(Self {
            _guard: guard,
            base,
            workspace,
            outside,
        })
    }

    fn allow_write(&self) -> Vec<String> {
        vec![
            self.workspace.display().to_string(),
            "/tmp/sandbox".to_string(),
            "/private/tmp/sandbox".to_string(),
        ]
    }

    async fn run(&self, command: &str) -> Result<bool, Box<dyn std::error::Error>> {
        let policy = policy_for(&self.workspace, &self.allow_write())?;
        let config = PolicyExecConfig::new(&self.workspace)
            .view(FilesystemView::Host)
            .network(NetworkPolicy::DenyAll)
            .fs_policy(policy);
        let runtime = PolicyExec::new(config)?;
        Ok(runtime.run(CommandSpec::shell(command)).await?.success())
    }

    fn write_outside(&self) -> String {
        format!("echo TEST_CONTENT > {}", self.outside.display())
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        clear_tmp_link();
        let _ = std::fs::remove_dir_all(&self.base);
        let _ = std::fs::remove_file(&self.outside);
    }
}

fn policy_for(workspace: &Path, allow_write: &[String]) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    let allow = allow_write
        .iter()
        .map(|path| format!("{path:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let json = format!(
        r#"{{"network": {{"allowedDomains": [], "deniedDomains": []}},
            "filesystem": {{"denyRead": [], "allowWrite": [{allow}], "denyWrite": []}}}}"#
    );
    SandboxSettings::from_json_for_host(&json, &HostPaths::new(workspace))?
        .policy()?
        .fs
        .ok_or_else(|| "settings carry no filesystem policy".into())
}

fn is_link(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

#[tokio::test]
async fn a_name_turned_into_a_link_to_the_root_does_not_widen_the_grant() -> TestResult {
    if !PolicyExec::is_supported_platform() {
        return Ok(());
    }
    let scene = Scene::new("root").await?;

    assert!(!scene.run(&scene.write_outside()).await?);
    assert!(!scene.outside.exists());

    assert!(scene.run("ln -s / /tmp/sandbox").await?);
    assert!(is_link("/tmp/sandbox"));

    assert!(!scene.run(&scene.write_outside()).await?);
    assert!(!scene.outside.exists());
    Ok(())
}

#[tokio::test]
async fn writes_outside_stay_blocked_when_the_name_is_absent() -> TestResult {
    if !PolicyExec::is_supported_platform() {
        return Ok(());
    }
    let scene = Scene::new("absent").await?;
    assert!(!Path::new("/tmp/sandbox").exists());

    assert!(!scene.run(&scene.write_outside()).await?);
    assert!(!scene.outside.exists());
    Ok(())
}

#[tokio::test]
async fn writes_outside_stay_blocked_when_the_name_is_a_directory() -> TestResult {
    if !PolicyExec::is_supported_platform() {
        return Ok(());
    }
    let scene = Scene::new("dir").await?;
    std::fs::create_dir_all("/tmp/sandbox")?;

    assert!(!scene.run(&scene.write_outside()).await?);
    assert!(!scene.outside.exists());
    Ok(())
}

#[tokio::test]
async fn a_write_through_the_link_to_the_root_is_blocked() -> TestResult {
    if !PolicyExec::is_supported_platform() {
        return Ok(());
    }
    let scene = Scene::new("traversal").await?;
    std::os::unix::fs::symlink("/", "/tmp/sandbox")?;
    let id = std::process::id();
    let through = format!("/tmp/sandbox/tmp/traversal-write-{id}.txt");
    let real = PathBuf::from(format!("/tmp/traversal-write-{id}.txt"));

    assert!(!scene.run(&format!("echo test > {through}")).await?);
    assert!(!real.exists());
    Ok(())
}

#[tokio::test]
async fn a_link_that_would_broaden_the_scope_is_rejected() -> TestResult {
    if !PolicyExec::is_supported_platform() {
        return Ok(());
    }
    let scene = Scene::new("broaden").await?;
    std::os::unix::fs::symlink("/", "/tmp/sandbox")?;

    assert!(!scene.run(&scene.write_outside()).await?);
    assert!(!scene.outside.exists());
    Ok(())
}

// A trailing `/**` is a request for the directory and everything below it, so
// the name that survives is the directory, never what the link points at.
fn globs(workspace: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(policy_for(workspace, &["/tmp/sandbox/**".to_string()])?
        .allow_write()
        .iter()
        .map(|entry| match entry {
            FsPathEntry::Literal { path, .. } => path.to_string(),
            FsPathEntry::Glob { pattern } => pattern.to_string(),
        })
        .collect())
}

#[tokio::test]
async fn a_glob_whose_base_links_to_the_root_keeps_its_name() -> TestResult {
    let scene = Scene::new("glob-root").await?;
    std::os::unix::fs::symlink("/", "/tmp/sandbox")?;

    assert_eq!(globs(&scene.workspace)?, ["/tmp/sandbox"]);
    Ok(())
}

#[tokio::test]
async fn a_glob_whose_base_links_to_the_parent_keeps_its_name() -> TestResult {
    let scene = Scene::new("glob-parent").await?;
    std::os::unix::fs::symlink("/tmp", "/tmp/sandbox")?;

    assert_eq!(globs(&scene.workspace)?, ["/tmp/sandbox"]);
    Ok(())
}

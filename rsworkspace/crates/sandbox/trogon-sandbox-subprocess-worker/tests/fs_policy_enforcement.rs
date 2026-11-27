#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{DirectoryMarker, GlobPattern, LiteralPath};
use trogon_sandbox_contract::{FsPathEntry, FsPolicy, MandatoryDenySearchDepth, PlatformOptions};
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::default(),
    })
}

fn glob(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

const PROBE: &str = r#"
import errno, os, sys

report = []

def attempt(name, fn):
    try:
        report.append((name, fn()))
    except OSError as e:
        report.append((name, errno.errorcode[e.errno]))

def read(path):
    def inner():
        with open(path) as f:
            return "ok:" + f.read().strip()
    return inner

def write(path):
    def inner():
        with open(path, "w") as f:
            f.write("x")
        return "ok"
    return inner

attempt("read_secret", read("/workspace/secret.txt"))
attempt("read_glob_match", read("/workspace/a.key"))
attempt("read_glob_miss", read("/workspace/a.txt"))
attempt("read_link_to_secret", read("/workspace/link-to-secret"))
attempt("list_hidden_dir", lambda: "[" + ",".join(sorted(os.listdir("/workspace/hidden"))) + "]")
attempt("read_hidden_inner", read("/workspace/hidden/inner.txt"))
attempt("read_vault_inner", read("/workspace/vault/inner.txt"))
attempt("read_vault_open", read("/workspace/vault/open/ok.txt"))
attempt("write_outside_grant", write("/workspace/newfile"))
attempt("write_granted", write("/workspace/out/new.txt"))
attempt("write_denied_inside_grant", write("/workspace/out/locked/new.txt"))
attempt("write_tmp", write("/tmp/probe"))
attempt("rename_ancestor", lambda: os.rename("/workspace/deep/a", "/workspace/deep/z") or "ok")
attempt("rename_inner_ancestor", lambda: os.rename("/workspace/deep/a/b", "/workspace/deep/a/c") or "ok")
attempt("rmdir_ancestor", lambda: os.rmdir("/workspace/deep/a/b") or "ok")
attempt("replace_stub", lambda: os.rename("/workspace/out/new.txt", "/workspace/deep/a/b/denied.txt") or "ok")
attempt("create_guarded", write("/workspace/deep/.env"))
attempt("read_denied_deep", read("/workspace/deep/a/b/denied.txt"))

def extra_fds():
    found = []
    for name in os.listdir("/proc/self/fd"):
        try:
            target = os.readlink("/proc/self/fd/" + name)
        except OSError:
            continue
        if int(name) > 2 and not target.endswith("/fd") and not target.startswith("pipe:"):
            found.append(target)
    return ",".join(sorted(found)) or "none"

attempt("extra_fds", extra_fds)

with open("/workspace/out/probe.txt", "w") as out:
    for name, value in report:
        out.write(name + " " + value + "\n")
"#;

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

fn policy() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    policy_with_tmp(true)
}

fn policy_with_tmp(tmp_writable: bool) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    let mut allow_write = vec![literal("/workspace/out")?, literal("/workspace/deep")?];
    if tmp_writable {
        allow_write.push(literal("/tmp")?);
    }
    Ok(FsPolicy::new(
        vec![
            literal("/workspace/secret.txt")?,
            literal("/workspace/hidden")?,
            literal("/workspace/vault")?,
            literal("/workspace/deep/a/b/denied.txt")?,
            literal("/workspace/deep/.env")?,
            glob("/workspace/*.key")?,
        ],
        Vec::new(),
        vec![literal("/workspace/vault/open")?],
        Vec::new(),
        allow_write,
        vec![literal("/workspace/out/locked")?],
    )?)
}

fn write_under(root: &Path, rel: &str, body: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().ok_or("a parent directory")?)?;
    std::fs::write(path, body)?;
    Ok(())
}

fn populate(ws: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let write = |rel: &str, body: &str| write_under(ws, rel, body);
    write("secret.txt", "TOPSECRET")?;
    write("a.key", "KEYMATERIAL")?;
    write("a.txt", "PLAIN")?;
    write("hidden/inner.txt", "HIDDEN")?;
    write("vault/inner.txt", "VAULT")?;
    write("vault/open/ok.txt", "OPEN")?;
    write("out/locked/keep.txt", "KEEP")?;
    write("deep/a/b/denied.txt", "DEEPSECRET")?;
    std::os::unix::fs::symlink("secret.txt", ws.join("link-to-secret"))?;
    Ok(())
}

async fn run_probe(
    ws: &Path,
    fs_policy: FsPolicy,
) -> Result<(BTreeMap<String, String>, Vec<PathBuf>), Box<dyn std::error::Error>> {
    run_script(ws, fs_policy, PROBE, "out/probe.txt", Default::default()).await
}

async fn run_script(
    ws: &Path,
    fs_policy: FsPolicy,
    script: &str,
    report_path: &str,
    platform: PlatformOptions,
) -> Result<(BTreeMap<String, String>, Vec<PathBuf>), Box<dyn std::error::Error>> {
    let backend = BubblewrapBackend::new(which::which("bwrap")?)
        .with_confiner(PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker")));
    let spec = LaunchSpec {
        binary: which::which("python3")?,
        args: vec!["-c".to_string(), script.to_string()],
        env: vec![],
        workspace_host_path: ws.to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: Some(fs_policy),
        platform,
        fs_denials: None,
    };

    let launched = backend.launch(&spec).await?;
    let placeholders = launched.placeholders.clone();
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    let report = std::fs::read_to_string(ws.join(report_path)).unwrap_or_default();
    assert!(
        output.status.success(),
        "probe exited with {}: {report}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let results = report
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Ok((results, placeholders))
}

#[tokio::test]
async fn fs_policy_is_enforced_inside_the_sandbox() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    populate(ws.path())?;

    let (r, placeholders) = run_probe(ws.path(), policy()?).await?;
    let got = |key: &str| r.get(key).map(String::as_str).unwrap_or("<missing>");

    assert_eq!(got("read_secret"), "EACCES", "denied file must be unreadable: {r:?}");
    assert_eq!(got("read_glob_match"), "EACCES", "a pattern match must be denied");
    assert_eq!(got("read_glob_miss"), "ok:PLAIN");
    assert_eq!(
        got("read_link_to_secret"),
        "EACCES",
        "a symlink to a denied path must be denied"
    );
    assert_eq!(got("list_hidden_dir"), "[]", "denied directory must look empty");
    assert_eq!(got("read_hidden_inner"), "ENOENT");
    assert_eq!(got("read_vault_inner"), "ENOENT");
    assert_eq!(
        got("read_vault_open"),
        "ok:OPEN",
        "allow_read_within_deny must re-open a path inside a deny"
    );

    assert_eq!(got("write_outside_grant"), "EROFS");
    assert_eq!(got("write_granted"), "ok");
    assert_eq!(
        got("write_denied_inside_grant"),
        "EROFS",
        "deny_write must win inside an allow_write"
    );
    assert_eq!(got("write_tmp"), "ok");

    assert_eq!(
        got("rename_ancestor"),
        "EBUSY",
        "renaming an ancestor of a denied path must fail"
    );
    assert_eq!(got("rename_inner_ancestor"), "EBUSY");
    assert_eq!(got("rmdir_ancestor"), "EBUSY");
    assert_ne!(got("replace_stub"), "ok", "a stub must not be replaced");
    assert_ne!(
        got("create_guarded"),
        "ok",
        "a denied path that did not exist must not become writable"
    );
    assert_eq!(got("read_denied_deep"), "EACCES");

    assert_eq!(
        got("extra_fds"),
        "none",
        "no bind or argument descriptor may reach the workload (the seccomp pipes are the only inherited ones)"
    );

    assert!(
        placeholders.contains(&ws.path().join("deep/.env")),
        "the guard mount point is reported for cleanup: {placeholders:?}"
    );
    assert_eq!(std::fs::read_to_string(ws.path().join("secret.txt"))?, "TOPSECRET");
    assert!(ws.path().join("deep/a/b/denied.txt").exists());
    Ok(())
}

#[tokio::test]
async fn fs_policy_makes_tmp_read_only_unless_granted() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    populate(ws.path())?;

    let (r, _) = run_probe(ws.path(), policy_with_tmp(false)?).await?;
    assert_eq!(r.get("write_tmp").map(String::as_str), Some("EROFS"), "{r:?}");
    Ok(())
}

const MANDATORY_PROBE: &str = r#"
import errno, os

report = []

def attempt(name, fn):
    try:
        report.append((name, fn()))
    except OSError as e:
        report.append((name, errno.errorcode[e.errno]))

def write(path):
    def inner():
        with open(path, "w") as f:
            f.write("x")
        return "ok"
    return inner

attempt("new_hook", write("/workspace/.git/hooks/new-hook"))
attempt("overwrite_hook", write("/workspace/.git/hooks/pre-commit"))
attempt("git_config", write("/workspace/.git/config"))
attempt("git_index", write("/workspace/.git/index"))
attempt("new_bashrc", write("/workspace/.bashrc"))
attempt("new_mcp_json", write("/workspace/.mcp.json"))
attempt("new_agent_command", write("/workspace/.agent/commands/evil.md"))
attempt("new_agent_settings", write("/workspace/.agent/settings.json"))
attempt("nested_hook", write("/workspace/pkg/inner/.git/hooks/post-checkout"))
attempt("nested_vscode", write("/workspace/pkg/.vscode/tasks.json"))
attempt("rename_hooks", lambda: os.rename("/workspace/.git/hooks", "/workspace/.git/hooks-old") or "ok")
attempt("rename_git", lambda: os.rename("/workspace/.git", "/workspace/.git-old") or "ok")
attempt("worktree_hook", write("/workspace/wt/shared.git/hooks/pre-push"))
attempt("gitfile", write("/workspace/wt/work/.git"))
attempt("ordinary_file", write("/workspace/src/new.txt"))

with open("/workspace/mandatory-probe.txt", "w") as out:
    for name, value in report:
        out.write(name + " " + value + "\n")
"#;

#[tokio::test]
async fn mandatory_paths_stay_read_only_inside_a_writable_workspace() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let write = |rel: &str, body: &str| write_under(ws.path(), rel, body);
    write(".git/hooks/pre-commit", "#!/bin/sh\n")?;
    write(".git/config", "[core]\n")?;
    write(".git/index", "idx")?;
    write("pkg/inner/.git/hooks/pre-commit", "#!/bin/sh\n")?;
    write("pkg/.vscode/settings.json", "{}")?;
    write("src/keep.txt", "keep")?;
    write("wt/shared.git/hooks/pre-commit", "#!/bin/sh\n")?;
    write("wt/shared.git/config", "[core]\n")?;
    write("wt/shared.git/worktrees/work/commondir", "../..\n")?;
    write("wt/work/.git", "gitdir: /workspace/wt/shared.git/worktrees/work\n")?;
    std::fs::create_dir_all(ws.path().join(".agent/commands"))?;

    let policy = FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        Vec::new(),
    )?;
    let mut platform = PlatformOptions::default();
    platform.filesystem.mandatory_deny_search_depth = Some(MandatoryDenySearchDepth::new(10)?);
    let (r, placeholders) = run_script(ws.path(), policy, MANDATORY_PROBE, "mandatory-probe.txt", platform).await?;
    let got = |key: &str| r.get(key).map(String::as_str).unwrap_or("<missing>");

    assert_eq!(got("new_hook"), "EROFS", "{r:?}");
    assert_eq!(got("overwrite_hook"), "EROFS", "{r:?}");
    assert_eq!(got("git_config"), "EROFS", "{r:?}");
    assert_eq!(
        got("git_index"),
        "ok",
        "git state outside hooks and config stays writable"
    );
    assert_ne!(got("new_bashrc"), "ok", "{r:?}");
    assert_ne!(got("new_mcp_json"), "ok", "{r:?}");
    assert_eq!(got("new_agent_command"), "EROFS", "{r:?}");
    assert_eq!(got("new_agent_settings"), "ok", "the rest of .agent stays writable");
    assert_eq!(got("nested_hook"), "EROFS", "{r:?}");
    assert_eq!(got("nested_vscode"), "EROFS", "{r:?}");
    assert_eq!(got("rename_hooks"), "EBUSY", "{r:?}");
    assert_ne!(got("rename_git"), "ok", "{r:?}");
    assert_eq!(
        got("worktree_hook"),
        "ok",
        "a gitfile below the working directory is left to git"
    );
    assert_eq!(got("gitfile"), "ok", "{r:?}");
    assert_eq!(got("ordinary_file"), "ok");

    assert!(placeholders.contains(&ws.path().join(".bashrc")), "{placeholders:?}");
    assert_eq!(
        std::fs::read_to_string(ws.path().join(".git/hooks/pre-commit"))?,
        "#!/bin/sh\n"
    );
    Ok(())
}

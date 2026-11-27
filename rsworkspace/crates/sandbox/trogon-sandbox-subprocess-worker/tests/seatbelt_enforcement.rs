#![cfg(target_os = "macos")]

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, GlobPattern, LiteralPath};
use trogon_sandbox_subprocess_worker::fs_violation::FsDenialSink;
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{IsolationBackend, SeatbeltBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

#[derive(Default)]
struct Lists {
    deny_read: Vec<FsPathEntry>,
    credential_deny_read: Vec<FsPathEntry>,
    allow_read_within_deny: Vec<FsPathEntry>,
    allow_write: Vec<FsPathEntry>,
    deny_write: Vec<FsPathEntry>,
}

impl Lists {
    fn build(self) -> Result<FsPolicy, Box<dyn std::error::Error>> {
        Ok(FsPolicy::new(
            self.deny_read,
            self.credential_deny_read,
            self.allow_read_within_deny,
            vec![],
            self.allow_write,
            self.deny_write,
        )?)
    }
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        for sub in ["open", "vault/open", "out/locked/deep", "proj/.git/hooks"] {
            fs::create_dir_all(root.join(sub))?;
        }
        for file in [
            "secret.txt",
            "open/ok.txt",
            "vault/inner.txt",
            "vault/open/ok.txt",
            "out/locked/deep/keep.txt",
            "proj/.git/config",
        ] {
            fs::write(root.join(file), "data")?;
        }
        Ok(Self { _dir: dir, root })
    }

    fn path(&self, relative: &str) -> String {
        self.root.join(relative).to_string_lossy().into_owned()
    }

    async fn sh(&self, script: &str) -> Result<i32, Box<dyn std::error::Error>> {
        self.sh_with(EgressPolicy::DenyAll, script).await
    }

    async fn sh_with(&self, egress: EgressPolicy, script: &str) -> Result<i32, Box<dyn std::error::Error>> {
        self.sh_under(egress, policy()?, script).await
    }

    async fn sh_under(
        &self,
        egress: EgressPolicy,
        fs_policy: FsPolicy,
        script: &str,
    ) -> Result<i32, Box<dyn std::error::Error>> {
        let spec = LaunchSpec {
            binary: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_string(), script.to_string()],
            env: vec![],
            workspace_host_path: self.root.clone(),
            workspace_sandbox_path: PathBuf::from("/workspace"),
            workspace_readonly: false,
            egress_policy: egress,
            extra_binds: vec![],
            user: None,
            limits: ResourceLimits::default(),
            fs_policy: Some(fs_policy),
            platform: Default::default(),
            fs_denials: None,
        };
        let launched = SeatbeltBackend::new().launch(&spec).await?;
        let mut child = launched.child.lock().await.take().ok_or("the launched child")?;
        Ok(child.wait().await?.code().unwrap_or(-1))
    }
}

fn entry(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::Unmarked,
    })
}

fn glob(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

fn policy() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Lists {
        deny_read: vec![
            entry("/workspace/secret.txt")?,
            entry("/workspace/vault")?,
            glob("/workspace/**/*.key")?,
        ],
        allow_read_within_deny: vec![entry("/workspace/vault/open")?],
        allow_write: vec![entry("/workspace/out")?, entry("/workspace/proj")?],
        deny_write: vec![entry("/workspace/out/locked")?],
        ..Lists::default()
    }
    .build()
}

fn allowed(code: i32) -> bool {
    code == 0
}

#[tokio::test]
async fn an_allowed_read_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let code = fixture
        .sh(&format!("cat {} >/dev/null", fixture.path("open/ok.txt")))
        .await?;
    assert!(allowed(code), "exit {code}");
    Ok(())
}

#[tokio::test]
async fn a_denied_file_cannot_be_read() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let code = fixture
        .sh(&format!("cat {} >/dev/null 2>&1", fixture.path("secret.txt")))
        .await?;
    assert!(!allowed(code));
    Ok(())
}

#[tokio::test]
async fn a_denied_directory_hides_its_files_except_the_carve_out() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let inner = fixture
        .sh(&format!("cat {} >/dev/null 2>&1", fixture.path("vault/inner.txt")))
        .await?;
    assert!(!allowed(inner));
    let carved = fixture
        .sh(&format!("cat {} >/dev/null 2>&1", fixture.path("vault/open/ok.txt")))
        .await?;
    assert!(allowed(carved), "exit {carved}");
    Ok(())
}

#[tokio::test]
async fn an_allow_read_re_opens_a_plain_deny_but_not_a_credential_deny() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let read = |relative: &str| format!("cat {} >/dev/null 2>&1", fixture.path(relative));
    let plain = Lists {
        deny_read: vec![entry("/workspace/vault/inner.txt")?],
        allow_read_within_deny: vec![entry("/workspace/vault/inner.txt")?],
        ..Lists::default()
    }
    .build()?;
    let credential = Lists {
        credential_deny_read: vec![entry("/workspace/vault/inner.txt")?],
        allow_read_within_deny: vec![entry("/workspace/vault")?],
        ..Lists::default()
    }
    .build()?;
    let lifted = fixture
        .sh_under(EgressPolicy::DenyAll, plain, &read("vault/inner.txt"))
        .await?;
    assert!(allowed(lifted), "exit {lifted}");
    let held = fixture
        .sh_under(EgressPolicy::DenyAll, credential.clone(), &read("vault/inner.txt"))
        .await?;
    assert!(!allowed(held));
    let sibling = fixture
        .sh_under(EgressPolicy::DenyAll, credential, &read("vault/open/ok.txt"))
        .await?;
    assert!(allowed(sibling), "exit {sibling}");
    Ok(())
}

#[tokio::test]
async fn a_credential_deny_holds_under_an_allow_read_of_the_file_itself() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let policy = Lists {
        credential_deny_read: vec![entry("/workspace/secret.txt")?],
        allow_read_within_deny: vec![entry("/workspace/secret.txt")?],
        ..Lists::default()
    }
    .build()?;
    let code = fixture
        .sh_under(
            EgressPolicy::DenyAll,
            policy,
            &format!("cat {} >/dev/null 2>&1", fixture.path("secret.txt")),
        )
        .await?;
    assert!(!allowed(code));
    Ok(())
}

#[tokio::test]
async fn a_glob_deny_blocks_matching_files_only() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fs::write(fixture.root.join("open/a.key"), "k")?;
    let key = fixture
        .sh(&format!("cat {} >/dev/null 2>&1", fixture.path("open/a.key")))
        .await?;
    assert!(!allowed(key));
    let other = fixture
        .sh(&format!("cat {} >/dev/null", fixture.path("open/ok.txt")))
        .await?;
    assert!(allowed(other), "exit {other}");
    Ok(())
}

#[tokio::test]
async fn writes_land_only_where_the_policy_grants_them() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let granted = fixture.sh(&format!("echo x > {}", fixture.path("out/new.txt"))).await?;
    assert!(allowed(granted), "exit {granted}");
    let outside = fixture
        .sh(&format!("echo x > {} 2>/dev/null", fixture.path("new.txt")))
        .await?;
    assert!(!allowed(outside));
    assert!(!fixture.root.join("new.txt").exists());
    Ok(())
}

#[tokio::test]
async fn a_write_deny_inside_a_granted_directory_holds() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let code = fixture
        .sh(&format!("echo x > {} 2>/dev/null", fixture.path("out/locked/new.txt")))
        .await?;
    assert!(!allowed(code));
    assert!(!fixture.root.join("out/locked/new.txt").exists());
    Ok(())
}

#[tokio::test]
async fn the_scratch_directory_is_writable() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let code = fixture.sh("echo x > \"$TMPDIR/probe\"").await?;
    assert!(allowed(code), "exit {code}");
    Ok(())
}

#[tokio::test]
async fn a_protected_directory_cannot_be_renamed_away() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let moved = fixture
        .sh(&format!(
            "mv {} {} 2>/dev/null",
            fixture.path("out/locked"),
            fixture.path("out/moved")
        ))
        .await?;
    assert!(!allowed(moved));
    assert!(fixture.root.join("out/locked/deep/keep.txt").exists());
    assert!(!fixture.root.join("out/moved").exists());
    Ok(())
}

#[tokio::test]
async fn an_unprotected_path_can_be_renamed() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let code = fixture
        .sh(&format!(
            "echo x > {a} && mv {a} {b}",
            a = fixture.path("out/new.txt"),
            b = fixture.path("out/renamed.txt")
        ))
        .await?;
    assert!(allowed(code), "exit {code}");
    Ok(())
}

#[tokio::test]
async fn a_protected_file_cannot_be_replaced_by_a_rename() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let code = fixture
        .sh(&format!(
            "echo x > {a} && mv {a} {b} 2>/dev/null",
            a = fixture.path("out/new.txt"),
            b = fixture.path("out/locked/deep/keep.txt")
        ))
        .await?;
    assert!(!allowed(code));
    assert_eq!(
        fs::read_to_string(fixture.root.join("out/locked/deep/keep.txt"))?,
        "data"
    );
    Ok(())
}

#[tokio::test]
async fn a_protected_file_cannot_be_deleted() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let code = fixture
        .sh(&format!("rm {} 2>/dev/null", fixture.path("out/locked/deep/keep.txt")))
        .await?;
    assert!(!allowed(code));
    assert!(fixture.root.join("out/locked/deep/keep.txt").exists());
    Ok(())
}

#[tokio::test]
async fn git_hooks_and_config_are_protected_in_a_writable_root() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let hook = fixture
        .sh(&format!(
            "echo x > {} 2>/dev/null",
            fixture.path("proj/.git/hooks/pre-commit")
        ))
        .await?;
    assert!(!allowed(hook));
    let config = fixture
        .sh(&format!("echo x >> {} 2>/dev/null", fixture.path("proj/.git/config")))
        .await?;
    assert!(!allowed(config));
    let moved = fixture
        .sh(&format!(
            "mv {} {} 2>/dev/null",
            fixture.path("proj/.git"),
            fixture.path("proj/.gone")
        ))
        .await?;
    assert!(!allowed(moved));
    let ordinary = fixture
        .sh(&format!("echo x > {}", fixture.path("proj/file.txt")))
        .await?;
    assert!(allowed(ordinary), "exit {ordinary}");
    Ok(())
}

fn reaches(port: u16) -> String {
    format!("/usr/bin/nc -z -w 2 127.0.0.1 {port} >/dev/null 2>&1")
}

fn listener() -> Result<(TcpListener, u16), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

fn loopback(port: u16) -> Result<EgressPolicy, Box<dyn std::error::Error>> {
    Ok(EgressPolicy::Loopback(format!("127.0.0.1:{port}").parse()?))
}

#[tokio::test]
async fn deny_all_blocks_the_network() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (_listener, port) = listener()?;
    let code = fixture.sh_with(EgressPolicy::DenyAll, &reaches(port)).await?;
    assert!(!allowed(code));
    Ok(())
}

#[tokio::test]
async fn allow_all_reaches_the_network() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (_listener, port) = listener()?;
    let code = fixture.sh_with(EgressPolicy::AllowAll, &reaches(port)).await?;
    assert!(allowed(code), "exit {code}");
    Ok(())
}

#[tokio::test]
async fn a_loopback_proxy_port_is_the_only_destination() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (_proxy, proxy_port) = listener()?;
    let (_other, other_port) = listener()?;
    let proxy = fixture.sh_with(loopback(proxy_port)?, &reaches(proxy_port)).await?;
    assert!(allowed(proxy), "exit {proxy}");
    let other = fixture.sh_with(loopback(proxy_port)?, &reaches(other_port)).await?;
    assert!(!allowed(other));
    Ok(())
}

#[tokio::test]
async fn a_denied_write_is_reported_as_an_fs_denial() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let denied = fixture.path("out/locked/new.txt");
    let (tx, mut rx) = FsDenialSink::channel();
    let spec = LaunchSpec {
        binary: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".to_string(),
            format!("for i in 1 2 3 4 5 6 7 8 9 10; do echo x > {denied} 2>/dev/null; sleep 1; done"),
        ],
        env: vec![],
        workspace_host_path: fixture.root.clone(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: Some(policy()?),
        platform: Default::default(),
        fs_denials: Some(tx),
    };
    let launched = SeatbeltBackend::new().launch(&spec).await?;
    drop(spec);
    let denial = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let denial = rx.recv().await?;
            if denial.path.as_str() == denied {
                break Some(denial);
            }
        }
    })
    .await?
    .ok_or("a denial of the locked path before the channel closed")?;
    assert_eq!(denial.operation, trogon_sandbox_contract::FsOperation::Create);
    if let Some(mut child) = launched.child.lock().await.take() {
        let _ = child.kill().await;
    }
    Ok(())
}

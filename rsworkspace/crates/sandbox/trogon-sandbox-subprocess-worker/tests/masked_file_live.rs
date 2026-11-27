//! Live tests of credential file masks. A masked file is a host copy with
//! sentinels bound read-only over the real path, and a credential deny is a
//! stub no `allow_read_within_deny` entry can lift.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, LiteralPath};
use trogon_sandbox_subprocess_worker::isolation::resource_limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, ExtraBind, LaunchSpec};

type TestResult = Result<(), Box<dyn Error>>;
type Results = BTreeMap<String, String>;

const REAL: &str = "real-secret-value";
const FAKE: &str = "sentinel-value";

const PROBE: &str = r#"
import os, sys
sys.stdout = open('/workspace/probe.txt', 'w')
target = sys.argv[1]
sibling = os.path.join(os.path.dirname(target), 'sibling.txt')

def attempt(name, fn):
    try:
        print(name, fn())
    except OSError as e:
        print(name, 'ERR' + str(e.errno))

def write(path):
    with open(path, 'a') as f:
        f.write('x')
    return 'ok'

def read(path):
    with open(path) as f:
        return f.read().strip() or 'EMPTY'

attempt('read', lambda: read(target))
attempt('write', lambda: write(target))
attempt('unlink', lambda: os.unlink(target))
attempt('rename_away', lambda: os.rename(target, target + '.moved'))
attempt('rename_over', lambda: (open(sibling + '.new', 'w').write('y'), os.rename(sibling + '.new', target))[1])
attempt('read_after', lambda: read(target))
attempt('sibling_read', lambda: read(sibling))
attempt('sibling_write', lambda: write(sibling))
"#;

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::Unmarked,
    })
}

fn literals(raw: &[&str]) -> Result<Vec<FsPathEntry>, Box<dyn Error>> {
    raw.iter().map(|path| literal(path)).collect()
}

/// The lists of a policy, each given as literal guest paths.
#[derive(Default)]
struct Lists<'a> {
    deny_read: &'a [&'a str],
    credential_deny_read: &'a [&'a str],
    allow_read_within_deny: &'a [&'a str],
    allow_write: &'a [&'a str],
    deny_write: &'a [&'a str],
}

fn policy(lists: &Lists<'_>) -> Result<FsPolicy, Box<dyn Error>> {
    Ok(FsPolicy::new(
        literals(lists.deny_read)?,
        literals(lists.credential_deny_read)?,
        literals(lists.allow_read_within_deny)?,
        vec![],
        literals(lists.allow_write)?,
        literals(lists.deny_write)?,
    )?)
}

struct Case {
    ws: tempfile::TempDir,
    fake: PathBuf,
    holder: String,
}

impl Case {
    fn new(holder: &str) -> Result<Self, Box<dyn Error>> {
        let ws = tempfile::tempdir()?;
        let root = ws.path().canonicalize()?;
        let dir = root.join(holder);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("token"), REAL)?;
        std::fs::write(dir.join("sibling.txt"), "sibling")?;
        let fake = root.join("fake-token");
        std::fs::write(&fake, FAKE)?;
        Ok(Self {
            ws,
            fake,
            holder: holder.to_string(),
        })
    }

    fn target(&self) -> String {
        format!("/workspace/{}/token", self.holder)
    }

    async fn run(&self, policy: FsPolicy, masked: bool) -> Result<Results, Box<dyn Error>> {
        let helper = PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker"));
        let backend = BubblewrapBackend::new(which::which("bwrap")?)
            .with_confiner(helper.clone())
            .with_guest_init(helper);
        let extra_binds = if masked {
            vec![ExtraBind {
                host_path: self.fake.clone(),
                sandbox_path: self.target(),
                readonly: true,
                credential_mask: true,
            }]
        } else {
            vec![]
        };
        let spec = LaunchSpec {
            binary: which::which("python3")?,
            args: vec!["-c".to_string(), PROBE.to_string(), self.target()],
            env: vec![],
            workspace_host_path: self.ws.path().to_path_buf(),
            workspace_sandbox_path: PathBuf::from("/workspace"),
            workspace_readonly: false,
            egress_policy: EgressPolicy::DenyAll,
            extra_binds,
            user: None,
            limits: ResourceLimits::default(),
            fs_policy: Some(policy),
            platform: Default::default(),
            fs_denials: None,
        };
        let launched = backend.launch(&spec).await?;
        let child = launched
            .child
            .lock()
            .await
            .take()
            .ok_or("the launch holds a child handle")?;
        let output = child.wait_with_output().await?;
        assert!(output.status.success(), "{output:?}");
        Ok(read_probe(self.ws.path()))
    }
}

fn read_probe(ws: &Path) -> Results {
    std::fs::read_to_string(ws.join("probe.txt"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// What the probe reported for one attempt.
fn outcome<'a>(results: &'a Results, attempt: &str) -> Result<&'a str, Box<dyn Error>> {
    Ok(results
        .get(attempt)
        .ok_or_else(|| format!("no result for {attempt}: {results:?}"))?
        .as_str())
}

fn granted() -> Lists<'static> {
    Lists {
        allow_write: &["/workspace"],
        ..Lists::default()
    }
}

fn assert_mask_held(results: &Results) -> TestResult {
    assert_eq!(outcome(results, "read")?, FAKE, "{results:?}");
    for attempt in ["write", "unlink", "rename_away", "rename_over"] {
        assert!(outcome(results, attempt)?.starts_with("ERR"), "{attempt}: {results:?}");
    }
    assert_eq!(outcome(results, "read_after")?, FAKE, "{results:?}");
    assert_eq!(outcome(results, "sibling_read")?, "sibling", "{results:?}");
    assert_eq!(outcome(results, "sibling_write")?, "ok", "{results:?}");
    Ok(())
}

#[tokio::test]
async fn a_masked_file_reads_as_the_fake_and_cannot_be_written_moved_or_replaced() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let case = Case::new("d")?;
    let results = case.run(policy(&granted())?, true).await?;
    assert_mask_held(&results)
}

#[tokio::test]
async fn a_mask_in_a_directory_named_with_brackets_is_applied() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let case = Case::new("[WIP] project")?;
    let results = case.run(policy(&granted())?, true).await?;
    assert_mask_held(&results)
}

#[tokio::test]
async fn a_write_grant_that_is_the_masked_file_does_not_make_it_writable() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let case = Case::new("d")?;
    let target = case.target();
    let lists = Lists {
        allow_write: &["/workspace", &target],
        ..Lists::default()
    };
    let results = case.run(policy(&lists)?, true).await?;
    assert_eq!(outcome(&results, "read")?, FAKE, "{results:?}");
    assert!(outcome(&results, "write")?.starts_with("ERR"), "{results:?}");
    Ok(())
}

#[tokio::test]
async fn a_mask_survives_a_write_deny_of_the_directory_that_holds_it() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let case = Case::new("d")?;
    let lists = Lists {
        allow_write: &["/workspace"],
        deny_write: &["/workspace/d"],
        ..Lists::default()
    };
    let results = case.run(policy(&lists)?, true).await?;
    assert_eq!(outcome(&results, "read")?, FAKE, "{results:?}");
    assert!(outcome(&results, "write")?.starts_with("ERR"), "{results:?}");
    assert!(outcome(&results, "sibling_write")?.starts_with("ERR"), "{results:?}");
    Ok(())
}

#[tokio::test]
async fn a_credential_deny_holds_under_an_allow_read_and_a_plain_deny_does_not() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let case = Case::new("d")?;
    let target = case.target();
    let credential = Lists {
        credential_deny_read: &[&target],
        allow_read_within_deny: &[&target],
        ..granted()
    };
    let results = case.run(policy(&credential)?, false).await?;
    assert_ne!(outcome(&results, "read")?, REAL, "{results:?}");
    assert_eq!(outcome(&results, "sibling_read")?, "sibling", "{results:?}");

    let plain = Lists {
        deny_read: &[&target],
        allow_read_within_deny: &[&target],
        ..granted()
    };
    let results = case.run(policy(&plain)?, false).await?;
    assert_eq!(outcome(&results, "read")?, REAL, "{results:?}");
    Ok(())
}

#[tokio::test]
async fn a_credential_deny_holds_under_an_allow_read_of_the_workspace() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let case = Case::new("d")?;
    let target = case.target();
    let lists = Lists {
        credential_deny_read: &[&target],
        allow_read_within_deny: &["/workspace"],
        ..granted()
    };
    let results = case.run(policy(&lists)?, false).await?;
    assert_ne!(outcome(&results, "read")?, REAL, "{results:?}");
    assert_eq!(outcome(&results, "sibling_read")?, "sibling", "{results:?}");
    Ok(())
}

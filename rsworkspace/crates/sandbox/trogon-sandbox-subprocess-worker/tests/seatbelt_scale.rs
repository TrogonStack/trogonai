#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, GlobPattern, LiteralPath};
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{ChildStdio, IsolationBackend, SeatbeltBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

struct Lab {
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

impl Lab {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        fs::create_dir_all(root.join("allowed"))?;
        Ok(Self { _dir: dir, root })
    }

    fn allowed(&self) -> PathBuf {
        self.root.join("allowed")
    }

    async fn run(&self, fs_policy: FsPolicy, script: &str) -> Result<Outcome, Box<dyn std::error::Error>> {
        let spec = LaunchSpec {
            binary: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_string(), script.to_string()],
            env: vec![],
            workspace_host_path: self.root.clone(),
            workspace_sandbox_path: PathBuf::from("/workspace"),
            workspace_readonly: false,
            egress_policy: EgressPolicy::DenyAll,
            extra_binds: vec![],
            user: None,
            limits: ResourceLimits::default(),
            fs_policy: Some(fs_policy),
            platform: Default::default(),
            fs_denials: None,
        };
        let launched = SeatbeltBackend::new()
            .with_stdio(ChildStdio::captured())
            .launch(&spec)
            .await?;
        let child = launched.child.lock().await.take().ok_or("the launched child")?;
        let output = child.wait_with_output().await?;
        Ok(Outcome {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn lit(path: &Path) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(path.to_str().ok_or("a utf-8 path")?)?,
        directory: DirectoryMarker::Unmarked,
    })
}

fn glob(path: &Path) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(path.to_str().ok_or("a utf-8 path")?)?,
    })
}

fn group_dir(allowed: &Path, index: usize) -> PathBuf {
    allowed.join("nested").join("groups").join(format!("g{index:04}"))
}

const GROUPS: usize = 200;

#[tokio::test]
async fn hundreds_of_folded_write_denies_still_spawn_and_enforce() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let allowed = lab.allowed();
    let probed = group_dir(&allowed, 150);
    fs::create_dir_all(&probed)?;
    let mut deny_write = Vec::new();
    for index in 0..GROUPS {
        let dir = group_dir(&allowed, index);
        deny_write.extend([
            lit(&dir.join("a.conf"))?,
            lit(&dir.join("a.conf.lock"))?,
            lit(&dir.join("b"))?,
        ]);
    }
    deny_write.push(glob(&allowed.join("**").join("*.key"))?);
    let policy = Lists {
        allow_write: vec![lit(&allowed)?],
        deny_write,
        ..Lists::default()
    }
    .build()?;
    let moved = format!("{}.moved", probed.display());
    let script = format!(
        "touch '{p}/other'; touch '{p}/a.conf'; touch '{p}/x.key'; mv '{p}' '{moved}'",
        p = probed.display()
    );
    let outcome = lab.run(policy, &script).await?;
    assert!(probed.join("other").exists());
    assert!(!probed.join("a.conf").exists());
    assert!(!probed.join("x.key").exists());
    assert!(!Path::new(&moved).exists());
    for denied in [
        format!("{}/a.conf: Operation not permitted", probed.display()),
        format!("{}/x.key: Operation not permitted", probed.display()),
        format!("{moved}: Operation not permitted"),
    ] {
        assert!(outcome.stderr.contains(&denied), "{}", outcome.stderr);
    }
    Ok(())
}

#[tokio::test]
async fn hundreds_of_glob_read_denies_still_compile_and_enforce() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let allowed = lab.allowed();
    let probed = group_dir(&allowed, 150);
    fs::create_dir_all(&probed)?;
    fs::write(probed.join("x.secret"), "SIZE_TEST_SECRET")?;
    fs::write(probed.join("x.plain"), "SIZE_TEST_PLAIN")?;
    let mut deny_read = Vec::new();
    for index in 0..GROUPS {
        let dir = group_dir(&allowed, index);
        deny_read.push(glob(&dir.join("**").join("*.secret"))?);
        deny_read.push(glob(&dir.join("**").join(".env"))?);
    }
    for index in 0..100 {
        deny_read.push(glob(
            &allowed.join(format!("area{index}")).join("**").join("credentials"),
        )?);
    }
    let policy = Lists {
        deny_read,
        allow_read_within_deny: vec![lit(&allowed)?],
        allow_write: vec![lit(&allowed)?],
        ..Lists::default()
    }
    .build()?;
    let secret = probed.join("x.secret");
    let script = format!(
        "cat '{p}/x.plain'; cat '{s}'; mv '{s}' '{s}.moved'; true",
        p = probed.display(),
        s = secret.display()
    );
    let outcome = lab.run(policy, &script).await?;
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert!(outcome.stdout.contains("SIZE_TEST_PLAIN"));
    assert!(!outcome.stdout.contains("SIZE_TEST_SECRET"));
    assert!(secret.exists());
    assert!(!probed.join("x.secret.moved").exists());
    Ok(())
}

const LEAVES: [&str; 3] = ["commondir", "config.worktree", "config.worktree.lock"];
const NAMES: usize = 300;
const ODD_NAME: &str = "odd.name+(1){2}^$|\\\"q";
const UNICODE_NAME: &str = "funci\u{f3}n-\u{540d}\u{524d}";

fn grouped_denies(allowed: &Path) -> Result<(Vec<FsPathEntry>, PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let registry = allowed.join("repo").join(".git").join("worktrees");
    let mut denies = Vec::new();
    for name in [ODD_NAME, UNICODE_NAME] {
        for leaf in LEAVES {
            denies.push(lit(&registry.join(name).join(leaf))?);
        }
    }
    for index in 0..NAMES {
        for leaf in LEAVES {
            denies.push(lit(&registry.join(format!("feature-branch-{index}")).join(leaf))?);
        }
    }
    for index in 0..5 {
        denies.push(lit(&registry.join(format!("partial-{index}")).join("commondir"))?);
    }
    let lone = allowed.join("other").join("single").join("file");
    denies.push(lit(&lone)?);
    let deep = allowed
        .join("d".repeat(200))
        .join("d".repeat(200))
        .join("d".repeat(200))
        .join("reg");
    for name in ["a".repeat(240), "b".repeat(240), "c".repeat(240), "\u{e9}".repeat(120)] {
        for leaf in LEAVES {
            denies.push(lit(&deep.join(&name).join(leaf))?);
        }
    }
    Ok((denies, registry, lone))
}

fn grouped_lab() -> Result<(Lab, FsPolicy, PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let allowed = lab.allowed();
    let (deny_write, registry, lone) = grouped_denies(&allowed)?;
    for name in ["feature-branch-7", ODD_NAME, UNICODE_NAME, "partial-2"] {
        fs::create_dir_all(registry.join(name))?;
        fs::write(registry.join(name).join("commondir"), "../..\n")?;
    }
    fs::create_dir_all(lone.parent().ok_or("a parent")?)?;
    let policy = Lists {
        allow_write: vec![lit(&allowed)?],
        deny_write,
        ..Lists::default()
    }
    .build()?;
    Ok((lab, policy, registry, lone))
}

#[tokio::test]
async fn a_profile_of_thousands_of_folded_paths_compiles_quickly() -> Result<(), Box<dyn std::error::Error>> {
    let (lab, policy, ..) = grouped_lab()?;
    let start = Instant::now();
    let outcome = lab.run(policy, "true").await?;
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    Ok(())
}

#[tokio::test]
async fn the_folded_deny_keeps_the_per_path_semantics() -> Result<(), Box<dyn std::error::Error>> {
    let (lab, policy, registry, lone) = grouped_lab()?;
    let entry = registry.join("feature-branch-7");
    let odd = registry.join(ODD_NAME);
    let uni = registry.join(UNICODE_NAME);
    let partial = registry.join("partial-2");
    let fresh = registry.join("brand-new");
    let q = |path: &Path| format!("'{}'", path.display());
    let script = [
        format!("echo x > {} && echo WROTE-LEAF", q(&entry.join("commondir"))),
        format!("echo x > {} && echo WROTE-LEAF-2", q(&entry.join("config.worktree"))),
        format!("echo x > {} && echo WROTE-SIBLING", q(&entry.join("other"))),
        format!("mv {} {} && echo MOVED-ENTRY", q(&entry), q(&registry.join("moved"))),
        format!("mkdir {f} && echo x > {f}/commondir && echo CREATED-NEW", f = q(&fresh)),
        format!("echo x > {} && echo WROTE-ODD", q(&odd.join("commondir"))),
        format!("echo x > {} && echo WROTE-ODD-SIBLING", q(&odd.join("other"))),
        format!("mv {} {} && echo MOVED-ODD", q(&odd), q(&registry.join("moved-odd"))),
        format!("echo x > {} && echo WROTE-UNICODE", q(&uni.join("commondir"))),
        format!("echo x > {} && echo WROTE-UNICODE-SIBLING", q(&uni.join("other"))),
        format!(
            "mv {} {} && echo MOVED-UNICODE",
            q(&uni),
            q(&registry.join("moved-uni"))
        ),
        format!("echo x > {} && echo WROTE-PARTIAL", q(&partial.join("commondir"))),
        format!(
            "echo x > {} && echo WROTE-PARTIAL-OTHER-LEAF",
            q(&partial.join("config.worktree"))
        ),
        format!("echo x > {} && echo WROTE-LONE", q(&lone)),
    ]
    .join("; ");
    let outcome = lab.run(policy, &script).await?;
    let out = outcome.stdout;
    assert!(!out.contains("WROTE-LEAF"), "{out}");
    assert!(out.contains("WROTE-SIBLING"), "{out}");
    assert!(!out.contains("MOVED-ENTRY"), "{out}");
    assert!(out.contains("CREATED-NEW"), "{out}");
    assert!(!out.contains("WROTE-ODD\n"), "{out}");
    assert!(out.contains("WROTE-ODD-SIBLING"), "{out}");
    assert!(!out.contains("MOVED-ODD"), "{out}");
    assert!(!out.contains("WROTE-UNICODE\n"), "{out}");
    assert!(out.contains("WROTE-UNICODE-SIBLING"), "{out}");
    assert!(!out.contains("MOVED-UNICODE"), "{out}");
    assert!(!out.contains("WROTE-PARTIAL\n"), "{out}");
    assert!(out.contains("WROTE-PARTIAL-OTHER-LEAF"), "{out}");
    assert!(!out.contains("WROTE-LONE"), "{out}");
    for dir in [&entry, &odd, &uni] {
        assert_eq!(fs::read_to_string(dir.join("commondir"))?, "../..\n");
    }
    assert!(fresh.join("commondir").exists());
    Ok(())
}

const MATRIX_FILES: [(char, &str); 10] = [
    ('S', "[user-id]/secret"),
    ('E', "[user-id]/a.env"),
    ('D', "deep/[user-id]/a.env"),
    ('U', "u/a.env"),
    ('P', "plain"),
    ('L', "list[].js"),
    ('C', "x]y"),
    ('O', "o[p[q1"),
    ('W', "a*c"),
    ('B', "abc"),
];

const MATRIX_ENTRIES: [(&str, &str); 8] = [
    ("[z-a]", ""),
    ("[user-id]", "SE"),
    ("[user-id]/secret", "S"),
    ("**/[user-id]/*.env", "ED"),
    ("**/list[].js", "L"),
    ("**/x]y", "C"),
    ("**/o[p[q*", "O"),
    ("**/a[*]c", "W"),
];

#[derive(Clone, Copy, Debug)]
enum MatrixList {
    DenyRead,
    CredentialDenyRead,
    DenyWrite,
    AllowRead,
    AllowWrite,
}

fn bracket_entry(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    match GlobPattern::new(raw) {
        Ok(_) => glob(Path::new(raw)),
        Err(_) => lit(Path::new(raw)),
    }
}

#[tokio::test]
async fn brackets_that_spell_no_set_cover_exactly_their_own_names_in_every_list()
-> Result<(), Box<dyn std::error::Error>> {
    for list in [
        MatrixList::DenyRead,
        MatrixList::CredentialDenyRead,
        MatrixList::DenyWrite,
        MatrixList::AllowRead,
        MatrixList::AllowWrite,
    ] {
        for (entry, covered) in MATRIX_ENTRIES {
            let lab = Lab::new()?;
            let tree = lab.root.join("tree");
            for (letter, relative) in MATRIX_FILES {
                let path = tree.join(relative);
                fs::create_dir_all(path.parent().ok_or("a parent")?)?;
                fs::write(&path, letter.to_string())?;
            }
            let raw = format!("{}/{entry}", tree.display());
            let list_entry = bracket_entry(&raw)?;
            let tree_entry = lit(&tree)?;
            let (policy, writes) = match list {
                MatrixList::DenyRead => (
                    Lists {
                        deny_read: vec![list_entry],
                        ..Lists::default()
                    }
                    .build()?,
                    false,
                ),
                MatrixList::CredentialDenyRead => (
                    Lists {
                        credential_deny_read: vec![list_entry],
                        ..Lists::default()
                    }
                    .build()?,
                    false,
                ),
                MatrixList::DenyWrite => (
                    Lists {
                        allow_write: vec![tree_entry],
                        deny_write: vec![list_entry],
                        ..Lists::default()
                    }
                    .build()?,
                    true,
                ),
                MatrixList::AllowRead => (
                    Lists {
                        deny_read: vec![tree_entry],
                        allow_read_within_deny: vec![list_entry],
                        ..Lists::default()
                    }
                    .build()?,
                    false,
                ),
                MatrixList::AllowWrite => (
                    Lists {
                        allow_write: vec![list_entry],
                        ..Lists::default()
                    }
                    .build()?,
                    true,
                ),
            };
            let inverted = matches!(list, MatrixList::AllowRead | MatrixList::AllowWrite);
            let mut script = String::new();
            for (letter, relative) in MATRIX_FILES {
                let path = format!("{}/{relative}", tree.display());
                if writes {
                    script.push_str(&format!("if ( : >> '{path}' ) 2>/dev/null; then echo {letter}; fi; "));
                } else {
                    script.push_str(&format!("cat '{path}' 2>/dev/null; "));
                }
            }
            let outcome = lab.run(policy, &script).await?;
            let reached: String = outcome.stdout.chars().filter(|c| c.is_ascii_uppercase()).collect();
            for (letter, _) in MATRIX_FILES {
                let in_set = covered.contains(letter);
                let expect_reached = in_set == inverted;
                assert_eq!(
                    reached.contains(letter),
                    expect_reached,
                    "{list:?} {entry} {letter}: reached={reached} {}",
                    outcome.stderr
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn what_a_command_moves_out_from_under_a_credential_deny_does_not_arrive_readable()
-> Result<(), Box<dyn std::error::Error>> {
    const CONTENT: &str = "MOVE-TEST-CONTENT";
    let rows: [(&str, &str, &str, &str, bool); 8] = [
        ("token", "", "plain", "", true),
        ("token", "", "token", "", false),
        ("token", "token", "token", "", false),
        ("dir", "dir", "dir/token", "", false),
        ("dir", "dir", "dir", "token", false),
        ("dir", "", "", "dir/token", false),
        ("**/.netrc", ".netrc", ".netrc", "", false),
        ("token", "token", "token", "", false),
    ];
    for (entry, allow_write, moved, read, arrives) in rows {
        let lab = Lab::new()?;
        let boxed = lab.root.join("box");
        let out = lab.root.join("out");
        fs::create_dir_all(boxed.join("dir"))?;
        fs::create_dir_all(&out)?;
        for file in ["dir/token", "token", "plain", ".netrc"] {
            fs::write(boxed.join(file), CONTENT)?;
        }
        let to = out.join("moved");
        let join = |relative: &str| {
            if relative.is_empty() {
                boxed.clone()
            } else {
                boxed.join(relative)
            }
        };
        let deny = if entry.starts_with("**") {
            glob(&boxed.join(entry))?
        } else {
            lit(&boxed.join(entry))?
        };
        let policy = Lists {
            credential_deny_read: vec![deny],
            allow_write: vec![lit(&join(allow_write))?, lit(&out)?],
            ..Lists::default()
        }
        .build()?;
        let moved_from = if moved.is_empty() {
            boxed.clone()
        } else {
            boxed.join(moved)
        };
        let read_from = if read.is_empty() { to.clone() } else { to.join(read) };
        let script = format!(
            "echo ran; mv '{}' '{}'; cat '{}'",
            moved_from.display(),
            to.display(),
            read_from.display()
        );
        let outcome = lab.run(policy, &script).await?;
        assert!(outcome.stdout.starts_with("ran\n"), "{}", outcome.stderr);
        assert_eq!(
            outcome.stdout.contains(CONTENT),
            arrives,
            "{entry} {allow_write} {moved} {read}: {}",
            outcome.stdout
        );
    }
    Ok(())
}

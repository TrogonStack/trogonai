#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{
    DirectoryMarker, FilesystemOptions, FsPathEntry, FsPolicy, GlobPattern, LiteralPath, MacosOptions, PlatformOptions,
    ProtectedNamesAnchor,
};
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{IsolationBackend, SeatbeltBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

const SECRET: &str = "GLOB_DENY_SECRET";
const PLAIN: &str = "PLAIN_CONTENT_OK";

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

struct Lab {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Lab {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::named("")
    }

    fn named(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?.join(name);
        fs::create_dir_all(&root)?;
        let lab = Self { _dir: dir, root };
        lab.reset()?;
        Ok(lab)
    }

    fn reset(&self) -> Result<(), Box<dyn std::error::Error>> {
        let project = self.root.join("project");
        let _ = fs::remove_dir_all(&project);
        for (relative, content) in [
            (".env", SECRET),
            ("sub/.env", SECRET),
            ("secrets/key", SECRET),
            ("secrets/deep/nested.txt", SECRET),
            ("certs/a.key", SECRET),
            ("conf/ok.pem", PLAIN),
            ("conf/bad.pem", SECRET),
            ("plain.txt", PLAIN),
            ("deleteme.txt", "x"),
            ("other/.keep", "x"),
        ] {
            let path = project.join(relative);
            fs::create_dir_all(path.parent().ok_or("a parent")?)?;
            fs::write(path, content)?;
        }
        fs::write(self.root.join("outside.txt"), SECRET)?;
        Ok(())
    }

    fn host(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn at(&self, relative: &str) -> String {
        self.host(relative).to_string_lossy().into_owned()
    }

    async fn run(&self, fs_policy: FsPolicy, script: &str) -> Result<i32, Box<dyn std::error::Error>> {
        self.run_on(PlatformOptions::default(), fs_policy, script).await
    }

    async fn run_on(
        &self,
        platform: PlatformOptions,
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
            egress_policy: EgressPolicy::DenyAll,
            extra_binds: vec![],
            user: None,
            limits: ResourceLimits::default(),
            fs_policy: Some(fs_policy),
            platform,
            fs_denials: None,
        };
        let launched = SeatbeltBackend::new().launch(&spec).await?;
        let mut child = launched.child.lock().await.take().ok_or("the launched child")?;
        Ok(child.wait().await?.code().unwrap_or(-1))
    }

    async fn reads(&self, fs_policy: FsPolicy, relative: &str) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(self
            .run(fs_policy, &format!("cat '{}' >/dev/null 2>&1", self.at(relative)))
            .await?
            == 0)
    }
}

fn lit(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
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

fn project_read() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    project_read_lists()?.build()
}

fn project_read_lists() -> Result<Lists, Box<dyn std::error::Error>> {
    Ok(Lists {
        deny_read: vec![
            glob("/workspace/project/**/.env")?,
            glob("/workspace/project/**/secrets")?,
            glob("/**/*.key")?,
        ],
        allow_read_within_deny: vec![lit("/workspace/project")?],
        ..Lists::default()
    })
}

fn project_write() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Lists {
        allow_write: vec![lit("/workspace/project")?],
        ..project_read_lists()?
    }
    .build()
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[tokio::test]
async fn a_glob_deny_blocks_each_match_inside_an_allowed_directory() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    for relative in [
        "project/.env",
        "project/sub/.env",
        "project/secrets/key",
        "project/secrets/deep/nested.txt",
        "project/certs/a.key",
    ] {
        assert!(!lab.reads(project_read()?, relative).await?, "{relative}");
    }
    Ok(())
}

#[tokio::test]
async fn stat_of_a_glob_denied_file_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let code = lab
        .run(
            project_read()?,
            &format!("/usr/bin/stat '{}' >/dev/null 2>&1", lab.at("project/.env")),
        )
        .await?;
    assert_ne!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_denied_directory_can_be_statted_but_not_listed_or_read_into() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        deny_read: vec![lit("/workspace/project/secrets")?],
        ..Lists::default()
    }
    .build()?;
    let secrets = lab.at("project/secrets");
    for (script, allowed) in [
        (format!("/usr/bin/stat '{secrets}'"), true),
        (format!("ls '{secrets}'"), false),
        (format!("cat '{secrets}/key'"), false),
        (format!("/usr/bin/stat '{secrets}/key'"), false),
    ] {
        let code = lab.run(policy.clone(), &format!("{script} >/dev/null 2>&1")).await?;
        assert_eq!(code == 0, allowed, "{script}");
    }
    Ok(())
}

#[tokio::test]
async fn a_file_that_matches_no_glob_stays_readable() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    assert!(lab.reads(project_read()?, "project/plain.txt").await?);
    Ok(())
}

#[tokio::test]
async fn a_pem_glob_denies_the_siblings_of_an_allowed_file() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        deny_read: vec![glob("/workspace/project/conf/*.pem")?],
        allow_read_within_deny: vec![lit("/workspace/project")?, lit("/workspace/project/conf/ok.pem")?],
        ..Lists::default()
    }
    .build()?;
    assert!(lab.reads(policy.clone(), "project/conf/ok.pem").await?);
    assert!(!lab.reads(policy, "project/conf/bad.pem").await?);
    Ok(())
}

#[tokio::test]
async fn a_trust_bundle_stays_readable_under_a_crt_deny() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    fs::create_dir_all(lab.host("ca"))?;
    fs::write(lab.host("ca/trust-bundle.crt"), PLAIN)?;
    fs::write(lab.host("project/conf/user.crt"), SECRET)?;
    let policy = Lists {
        deny_read: vec![glob("/**/*.crt")?],
        allow_read_within_deny: vec![lit("/workspace/project")?, lit("/workspace/ca/trust-bundle.crt")?],
        ..Lists::default()
    }
    .build()?;
    assert!(lab.reads(policy.clone(), "ca/trust-bundle.crt").await?);
    assert!(!lab.reads(policy, "project/conf/user.crt").await?);
    Ok(())
}

#[tokio::test]
async fn a_glob_denied_path_cannot_be_renamed_to_an_unmatched_name() -> Result<(), Box<dyn std::error::Error>> {
    for (source, target, probe) in [
        ("project/.env", "project/moved.txt", "project/moved.txt"),
        ("project/secrets", "project/s2", "project/s2/key"),
    ] {
        let lab = Lab::new()?;
        let code = lab
            .run(
                project_write()?,
                &format!(
                    "mv '{}' '{}' 2>/dev/null; cat '{}' 2>/dev/null; true",
                    lab.at(source),
                    lab.at(target),
                    lab.at(probe)
                ),
            )
            .await?;
        assert_eq!(code, 0);
        assert!(exists(&lab.host(source)), "{source} moved");
        assert!(!exists(&lab.host(target)), "{target} appeared");
    }
    Ok(())
}

#[tokio::test]
async fn a_literal_deny_nested_under_an_allow_cannot_be_renamed() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        deny_read: vec![lit("/workspace/project/secrets")?],
        allow_read_within_deny: vec![lit("/workspace/project")?],
        allow_write: vec![lit("/workspace/project")?],
        ..Lists::default()
    }
    .build()?;
    lab.run(
        policy,
        &format!(
            "mv '{}' '{}' 2>/dev/null; true",
            lab.at("project/secrets"),
            lab.at("project/s3")
        ),
    )
    .await?;
    assert!(exists(&lab.host("project/secrets")));
    assert!(!exists(&lab.host("project/s3")));
    Ok(())
}

#[tokio::test]
async fn a_literal_deny_without_any_allow_read_cannot_be_renamed() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        deny_read: vec![lit("/workspace/project/secrets")?],
        allow_write: vec![lit("/workspace/project")?],
        ..Lists::default()
    }
    .build()?;
    lab.run(
        policy,
        &format!(
            "mv '{}' '{}' 2>/dev/null; true",
            lab.at("project/secrets"),
            lab.at("project/s4")
        ),
    )
    .await?;
    assert!(exists(&lab.host("project/secrets")));
    assert!(!exists(&lab.host("project/s4")));
    Ok(())
}

#[tokio::test]
async fn the_directory_holding_a_deeper_deny_cannot_be_renamed() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        deny_read: vec![lit("/workspace/project/secrets/deep/nested.txt")?],
        allow_read_within_deny: vec![lit("/workspace/project")?],
        allow_write: vec![lit("/workspace/project")?],
        ..Lists::default()
    }
    .build()?;
    lab.run(
        policy,
        &format!(
            "mv '{}' '{}' 2>/dev/null; true",
            lab.at("project/secrets"),
            lab.at("project/s5")
        ),
    )
    .await?;
    assert!(exists(&lab.host("project/secrets/deep/nested.txt")));
    Ok(())
}

#[tokio::test]
async fn a_non_denied_file_in_the_write_root_can_be_removed() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let code = lab
        .run(project_write()?, &format!("rm '{}'", lab.at("project/deleteme.txt")))
        .await?;
    assert_eq!(code, 0);
    assert!(!exists(&lab.host("project/deleteme.txt")));
    Ok(())
}

#[tokio::test]
async fn a_new_file_can_take_a_read_denied_name() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let code = lab
        .run(
            project_write()?,
            &format!("echo fresh > '{}'", lab.at("project/other/.env")),
        )
        .await?;
    assert_eq!(code, 0);
    assert_eq!(fs::read_to_string(lab.host("project/other/.env"))?, "fresh\n");
    Ok(())
}

#[tokio::test]
async fn rm_works_inside_a_carve_out_nested_in_a_literal_deny() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        deny_read: vec![lit("/workspace/project/secrets")?],
        allow_read_within_deny: vec![lit("/workspace/project/secrets/deep")?],
        allow_write: vec![lit("/workspace/project")?],
        ..Lists::default()
    }
    .build()?;
    let code = lab
        .run(policy, &format!("rm '{}'", lab.at("project/secrets/deep/nested.txt")))
        .await?;
    assert_eq!(code, 0);
    assert!(!exists(&lab.host("project/secrets/deep/nested.txt")));
    Ok(())
}

fn region_lists(allow: FsPathEntry) -> Result<Lists, Box<dyn std::error::Error>> {
    Ok(Lists {
        deny_read: vec![glob("/workspace/**/*")?],
        allow_read_within_deny: vec![allow],
        ..Lists::default()
    })
}

fn region_policy(allow: FsPathEntry) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    region_lists(allow)?.build()
}

#[tokio::test]
async fn a_region_glob_deny_leaves_a_literal_carve_out_readable() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = region_policy(lit("/workspace/project")?)?;
    assert!(lab.reads(policy.clone(), "project/plain.txt").await?);
    assert!(!lab.reads(policy, "outside.txt").await?);
    Ok(())
}

#[tokio::test]
async fn a_region_glob_deny_leaves_a_glob_carve_out_readable() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = region_policy(glob("/workspace/project/*.txt")?)?;
    assert!(lab.reads(policy.clone(), "project/plain.txt").await?);
    assert!(!lab.reads(policy, "project/secrets/key").await?);
    Ok(())
}

#[tokio::test]
async fn rm_works_inside_the_carve_out_of_a_region_glob_in_a_write_root() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        allow_write: vec![lit("/workspace/project")?],
        ..region_lists(lit("/workspace/project")?)?
    }
    .build()?;
    let code = lab
        .run(policy, &format!("rm '{}'", lab.at("project/deleteme.txt")))
        .await?;
    assert_eq!(code, 0);
    assert!(!exists(&lab.host("project/deleteme.txt")));
    Ok(())
}

#[tokio::test]
async fn a_hard_link_to_a_denied_file_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    lab.run(
        project_write()?,
        &format!(
            "ln '{}' '{}' 2>/dev/null; true",
            lab.at("project/.env"),
            lab.at("project/hardlink.txt")
        ),
    )
    .await?;
    assert!(!exists(&lab.host("project/hardlink.txt")));
    Ok(())
}

#[tokio::test]
async fn a_clone_of_a_denied_file_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    lab.run(
        project_write()?,
        &format!(
            "cp -c '{}' '{}' 2>/dev/null; true",
            lab.at("project/.env"),
            lab.at("project/clone.txt")
        ),
    )
    .await?;
    assert!(!exists(&lab.host("project/clone.txt")));
    Ok(())
}

#[tokio::test]
async fn the_data_volume_spelling_of_a_denied_file_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let alias = format!("/System/Volumes/Data{}", lab.at("project/.env"));
    if !Path::new(&alias).exists() {
        return Ok(());
    }
    let code = lab
        .run(project_read()?, &format!("cat '{alias}' >/dev/null 2>&1"))
        .await?;
    assert_ne!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_case_folded_spelling_of_a_denied_file_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let folded = lab.host("project/.ENV");
    if !folded.exists() {
        return Ok(());
    }
    let code = lab
        .run(project_read()?, &format!("cat '{}' >/dev/null 2>&1", folded.display()))
        .await?;
    assert_ne!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_hard_link_deny_rule_compiles_and_blocks_ln() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let profile = format!(
        "(version 1) (allow default) (deny file-link (subpath \"{}\"))",
        lab.at("project")
    );
    let output = std::process::Command::new("/usr/bin/sandbox-exec")
        .args(["-p", &profile, "/bin/sh", "-c"])
        .arg(format!(
            "ln '{}' '{}'",
            lab.at("project/plain.txt"),
            lab.at("project/hardlink2.txt")
        ))
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ln:"), "{stderr}");
    assert!(stderr.contains("Operation not permitted"), "{stderr}");
    assert!(!exists(&lab.host("project/hardlink2.txt")));
    Ok(())
}

const EXEC_DEPS: [&str; 6] = ["/bin", "/usr", "/System", "/private", "/dev", "/etc"];

fn root_deny(extra_deny: Option<FsPathEntry>) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    let mut allow = vec![lit("/workspace/project")?];
    for dep in EXEC_DEPS {
        allow.push(lit(dep)?);
    }
    let mut deny = vec![lit("/")?];
    deny.extend(extra_deny);
    Lists {
        deny_read: deny,
        allow_read_within_deny: allow,
        ..Lists::default()
    }
    .build()
}

#[tokio::test]
async fn a_root_deny_reads_the_carve_out_and_denies_the_rest() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let outside = std::env::current_dir()?.join("Cargo.toml");
    assert!(lab.reads(root_deny(None)?, "project/plain.txt").await?);
    let code = lab
        .run(
            root_deny(None)?,
            &format!("cat '{}' >/dev/null 2>&1", outside.display()),
        )
        .await?;
    assert_ne!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_root_deny_alongside_a_top_level_glob_still_execs() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let outside = std::env::current_dir()?.join("Cargo.toml");
    let policy = root_deny(Some(glob("/*")?))?;
    assert!(lab.reads(policy.clone(), "project/plain.txt").await?);
    let code = lab
        .run(policy, &format!("cat '{}' >/dev/null 2>&1", outside.display()))
        .await?;
    assert_ne!(code, 0);
    Ok(())
}

fn rename(lab: &Lab, from: &str, to: &str) -> String {
    format!("mv '{}' '{}'", lab.at(from), lab.at(to))
}

#[tokio::test]
async fn a_nested_write_root_under_a_literal_deny_can_rename() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    fs::create_dir_all(lab.host("project/secrets/scratch"))?;
    fs::write(lab.host("project/secrets/scratch/tmp.txt"), "x")?;
    let policy = Lists {
        deny_read: vec![lit("/workspace/project/secrets")?],
        allow_write: vec![lit("/workspace/project")?, lit("/workspace/project/secrets/scratch")?],
        ..Lists::default()
    }
    .build()?;
    let code = lab
        .run(
            policy,
            &rename(
                &lab,
                "project/secrets/scratch/tmp.txt",
                "project/secrets/scratch/tmp2.txt",
            ),
        )
        .await?;
    assert_eq!(code, 0);
    assert!(exists(&lab.host("project/secrets/scratch/tmp2.txt")));
    Ok(())
}

#[tokio::test]
async fn an_uncarved_write_root_inside_a_region_glob_can_rename() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    fs::create_dir_all(lab.host("project/cache"))?;
    fs::write(lab.host("project/cache/tmp.txt"), "x")?;
    let policy = Lists {
        deny_read: vec![glob("/workspace/project/**/*")?],
        allow_write: vec![lit("/workspace/project/cache")?],
        ..Lists::default()
    }
    .build()?;
    let code = lab
        .run(policy, &rename(&lab, "project/cache/tmp.txt", "project/cache/tmp2.txt"))
        .await?;
    assert_eq!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_leaf_glob_deny_still_blocks_renaming_its_match() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        deny_read: vec![glob("/workspace/**/.env")?],
        allow_write: vec![lit("/workspace/project")?],
        ..Lists::default()
    }
    .build()?;
    let code = lab
        .run(policy, &rename(&lab, "project/.env", "project/env.txt"))
        .await?;
    assert_ne!(code, 0);
    assert!(exists(&lab.host("project/.env")));
    Ok(())
}

#[tokio::test]
async fn the_directory_holding_a_glob_denied_file_cannot_be_moved() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = Lists {
        deny_read: vec![glob("/workspace/project/conf/*.pem")?],
        allow_write: vec![lit("/workspace/project")?],
        ..Lists::default()
    }
    .build()?;
    let code = lab.run(policy, &rename(&lab, "project/conf", "project/conf2")).await?;
    assert_ne!(code, 0);
    assert!(exists(&lab.host("project/conf/bad.pem")));
    Ok(())
}

fn write_policy(deny_write: Vec<FsPathEntry>) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Lists {
        allow_write: vec![lit("/workspace/project")?],
        deny_write,
        ..Lists::default()
    }
    .build()
}

async fn moves_are_refused(
    lab: &Lab,
    policy: FsPolicy,
    moves: &[(&str, &str)],
    kept: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    for (from, to) in moves {
        let code = lab.run(policy.clone(), &rename(lab, from, to)).await?;
        assert_ne!(code, 0, "{from}");
        lab.reset()?;
    }
    for path in kept {
        assert!(exists(&lab.host(path)), "{path}");
    }
    Ok(())
}

#[tokio::test]
async fn a_write_denied_file_cannot_be_moved_with_its_ancestors() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = write_policy(vec![lit("/workspace/project/secrets/deep/nested.txt")?])?;
    moves_are_refused(
        &lab,
        policy,
        &[
            ("project/secrets/deep/nested.txt", "project/secrets/deep/moved.txt"),
            ("project/secrets/deep", "project/secrets/deep2"),
            ("project/secrets", "project/secrets2"),
        ],
        &["project/secrets/deep/nested.txt"],
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_write_denied_glob_match_cannot_be_moved_or_written_or_carried_away() -> Result<(), Box<dyn std::error::Error>>
{
    let lab = Lab::new()?;
    let policy = write_policy(vec![glob("/workspace/project/conf/*.pem")?])?;
    moves_are_refused(
        &lab,
        policy.clone(),
        &[
            ("project/conf/ok.pem", "project/conf/ok.txt"),
            ("project/conf", "project/conf2"),
        ],
        &["project/conf/ok.pem"],
    )
    .await?;
    let code = lab
        .run(
            policy,
            &format!("echo x > '{}' 2>/dev/null", lab.at("project/conf/ok.pem")),
        )
        .await?;
    assert_ne!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_recursive_write_deny_glob_blocks_moving_nested_matches() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    fs::create_dir_all(lab.host("project/a/b"))?;
    fs::write(lab.host("project/a/b/note.txt"), "x")?;
    let policy = write_policy(vec![glob("/workspace/project/**/*.txt")?])?;
    moves_are_refused(
        &lab,
        policy,
        &[("project/plain.txt", "project/plain.md")],
        &["project/plain.txt"],
    )
    .await?;
    let code = lab
        .run(
            write_policy(vec![glob("/workspace/project/**/*.txt")?])?,
            &rename(&lab, "project/a/b/note.txt", "project/a/b/note.md"),
        )
        .await?;
    assert_ne!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_symlink_cannot_stand_in_for_an_absent_protected_ancestor() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    fs::create_dir_all(lab.host("decoy"))?;
    let policy = write_policy(vec![lit("/workspace/project/absent/dir/file.txt")?])?;
    let code = lab
        .run(
            policy,
            &format!(
                "ln -s '{}' '{}' 2>/dev/null",
                lab.at("decoy"),
                lab.at("project/absent/dir")
            ),
        )
        .await?;
    assert_ne!(code, 0);
    assert!(!lab.host("project/absent/dir").exists());
    Ok(())
}

#[tokio::test]
async fn an_ordinary_file_can_still_be_created_beside_write_denies() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = write_policy(vec![lit("/workspace/project/absent/dir/file.txt")?])?;
    let code = lab
        .run(policy, &format!("echo hi > '{}'", lab.at("project/ordinary.txt")))
        .await?;
    assert_eq!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_stripped_recursive_write_deny_blocks_existing_and_new_files() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = write_policy(vec![glob("/workspace/project/**/secrets")?])?;
    let existing = lab
        .run(
            policy.clone(),
            &format!("echo x > '{}' 2>/dev/null", lab.at("project/secrets/key")),
        )
        .await?;
    assert_ne!(existing, 0);
    let created = lab
        .run(
            policy.clone(),
            &format!("echo x > '{}' 2>/dev/null", lab.at("project/secrets/new")),
        )
        .await?;
    assert_ne!(created, 0);
    let elsewhere = lab
        .run(policy, &format!("echo x > '{}'", lab.at("project/other/new")))
        .await?;
    assert_eq!(elsewhere, 0);
    Ok(())
}

#[tokio::test]
async fn hooks_of_a_nested_repository_are_protected_and_other_files_are_not() -> Result<(), Box<dyn std::error::Error>>
{
    let lab = Lab::new()?;
    fs::create_dir_all(lab.host("project/vendor/dep/.git/hooks"))?;
    fs::write(lab.host("project/vendor/dep/.git/hooks/pre-commit"), "x")?;
    let policy = write_policy(vec![])?;
    for target in [
        "project/vendor/dep/.git/hooks/pre-commit",
        "project/vendor/dep/.git/hooks/post-merge",
    ] {
        let code = lab
            .run(policy.clone(), &format!("echo x > '{}' 2>/dev/null", lab.at(target)))
            .await?;
        assert_ne!(code, 0, "{target}");
    }
    let ordinary = lab
        .run(policy, &format!("echo x > '{}'", lab.at("project/vendor/dep/code.rs")))
        .await?;
    assert_eq!(ordinary, 0);
    Ok(())
}

fn git_config_allowed() -> PlatformOptions {
    PlatformOptions {
        filesystem: FilesystemOptions {
            allow_git_config: true.into(),
            ..FilesystemOptions::default()
        },
        ..PlatformOptions::default()
    }
}

#[tokio::test]
async fn allowing_git_config_frees_the_config_but_not_the_hooks() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    fs::create_dir_all(lab.host("project/.git/hooks"))?;
    fs::write(lab.host("project/.git/config"), "[core]\n")?;
    let write = |target: &str| format!("echo x >> '{}' 2>/dev/null", lab.at(target));
    let policy = write_policy(vec![])?;
    let config = lab
        .run_on(git_config_allowed(), policy.clone(), &write("project/.git/config"))
        .await?;
    assert_eq!(config, 0);
    let hook = lab
        .run_on(
            git_config_allowed(),
            policy.clone(),
            &write("project/.git/hooks/pre-commit"),
        )
        .await?;
    assert_ne!(hook, 0);
    let refused = lab.run(policy, &write("project/.git/config")).await?;
    assert_ne!(refused, 0);
    Ok(())
}

fn anchored_at_the_working_directory() -> PlatformOptions {
    PlatformOptions {
        macos: MacosOptions {
            protected_names_anchor: ProtectedNamesAnchor::WorkingDirectory,
            ..MacosOptions::default()
        },
        ..PlatformOptions::default()
    }
}

#[tokio::test]
async fn anchored_at_the_working_directory_a_root_below_it_gets_the_default_protections()
-> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let policy = write_policy(vec![])?;
    let make_git_dir = format!("mkdir '{}' 2>/dev/null", lab.at("project/.git"));
    assert_ne!(lab.run(policy.clone(), &make_git_dir).await?, 0);
    let made = lab
        .run_on(anchored_at_the_working_directory(), policy.clone(), &make_git_dir)
        .await?;
    assert_eq!(made, 0);
    assert!(lab.host("project/.git").is_dir());
    for target in [
        "project/.git/hooks/pre-commit",
        "project/.git/config",
        "project/.gitconfig",
        "project/sub/.bashrc",
        "project/.vscode/settings.json",
        "project/.agent/agents/a.md",
    ] {
        let script = format!(
            "mkdir -p \"$(dirname '{path}')\" 2>/dev/null; {{ echo x > '{path}'; }} 2>/dev/null",
            path = lab.at(target)
        );
        let code = lab
            .run_on(anchored_at_the_working_directory(), policy.clone(), &script)
            .await?;
        assert_ne!(code, 0, "{target}");
        assert!(!lab.host(target).exists(), "{target}");
    }
    let plain = format!("echo x > '{}'", lab.at("project/.agent/settings.json"));
    fs::create_dir_all(lab.host("project/.agent"))?;
    let code = lab.run_on(anchored_at_the_working_directory(), policy, &plain).await?;
    assert_eq!(code, 0);
    Ok(())
}

#[tokio::test]
async fn a_bracketed_working_directory_keeps_every_protection_literal() -> Result<(), Box<dyn std::error::Error>> {
    for name in ["[WIP]", "build*", "notes (draft?)", "[user-id]"] {
        let lab = Lab::named(name)?;
        fs::create_dir_all(lab.host("project/.git/hooks"))?;
        fs::create_dir_all(lab.host("project/.vscode"))?;
        fs::write(lab.host("project/.gitconfig"), "x")?;
        let policy = write_policy(vec![])?;
        for target in [
            "project/.git/hooks/pre-commit",
            "project/.git/config",
            "project/.vscode/settings.json",
            "project/.gitconfig",
        ] {
            let code = lab
                .run(policy.clone(), &format!("echo x > '{}' 2>/dev/null", lab.at(target)))
                .await?;
            assert_ne!(code, 0, "{name}: {target}");
        }
        let moved = lab
            .run(policy.clone(), &rename(&lab, "project/.git", "project/.gone"))
            .await?;
        assert_ne!(moved, 0, "{name}: move");
        let created = lab
            .run(
                policy.clone(),
                &format!("mkdir '{}' 2>/dev/null", lab.at("project/.agent")),
            )
            .await?;
        assert_ne!(created, 0, "{name}: create");
        let ordinary = lab
            .run(policy, &format!("echo x > '{}'", lab.at("project/ok.txt")))
            .await?;
        assert_eq!(ordinary, 0, "{name}: ordinary");
    }
    Ok(())
}

#[tokio::test]
async fn a_bracketed_directory_is_denied_by_name_and_its_sibling_is_not() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::named("[WIP]")?;
    fs::create_dir_all(lab.host("project/[user-id]"))?;
    fs::write(lab.host("project/[user-id]/secret"), SECRET)?;
    fs::create_dir_all(lab.host("project/u"))?;
    fs::write(lab.host("project/u/secret"), PLAIN)?;
    let policy = Lists {
        deny_read: vec![lit("/workspace/project/[user-id]")?],
        allow_write: vec![lit("/workspace/project")?],
        ..Lists::default()
    }
    .build()?;
    assert!(!lab.reads(policy.clone(), "project/[user-id]/secret").await?);
    assert!(lab.reads(policy.clone(), "project/u/secret").await?);
    let code = lab
        .run(policy, &format!("echo x > '{}'", lab.at("project/u/new.txt")))
        .await?;
    assert_eq!(code, 0);
    Ok(())
}

const GROUPED_NAMES: [&str; 8] = [
    "plain.key",
    "with space.key",
    "caf\u{e9}.key",
    "[bracket].key",
    "star*.key",
    "dollar$.key",
    "paren(1).key",
    "plus+.key",
];

fn grouped_lab() -> Result<Lab, Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    fs::create_dir_all(lab.host("project/grp"))?;
    for name in GROUPED_NAMES {
        fs::write(lab.host(&format!("project/grp/{name}")), "x")?;
    }
    fs::write(lab.host("project/grp/sibling.txt"), "x")?;
    Ok(lab)
}

fn grouped_policy(names: &[&str]) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    write_policy(
        names
            .iter()
            .map(|name| lit(&format!("/workspace/project/grp/{name}")))
            .collect::<Result<Vec<_>, _>>()?,
    )
}

#[tokio::test]
async fn a_grouped_write_deny_blocks_each_leaf_including_odd_names() -> Result<(), Box<dyn std::error::Error>> {
    let lab = grouped_lab()?;
    for name in GROUPED_NAMES {
        let code = lab
            .run(
                grouped_policy(&GROUPED_NAMES)?,
                &format!("echo x > '{}' 2>/dev/null", lab.at(&format!("project/grp/{name}"))),
            )
            .await?;
        assert_ne!(code, 0, "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn a_grouped_write_deny_leaves_siblings_writable_and_creatable() -> Result<(), Box<dyn std::error::Error>> {
    let lab = grouped_lab()?;
    let policy = grouped_policy(&GROUPED_NAMES)?;
    let sibling = lab
        .run(
            policy.clone(),
            &format!("echo y > '{}'", lab.at("project/grp/sibling.txt")),
        )
        .await?;
    assert_eq!(sibling, 0);
    let created = lab
        .run(
            policy.clone(),
            &format!("echo y > '{}'", lab.at("project/grp/fresh.txt")),
        )
        .await?;
    assert_eq!(created, 0);
    let prefix_name = lab
        .run(policy, &format!("echo y > '{}'", lab.at("project/grp/plain.key.bak")))
        .await?;
    assert_eq!(prefix_name, 0);
    Ok(())
}

#[tokio::test]
async fn a_grouped_write_deny_cannot_be_renamed_away() -> Result<(), Box<dyn std::error::Error>> {
    let lab = grouped_lab()?;
    for name in ["plain.key", "caf\u{e9}.key", "[bracket].key"] {
        let code = lab
            .run(
                grouped_policy(&GROUPED_NAMES)?,
                &rename(&lab, &format!("project/grp/{name}"), "project/grp/moved.txt"),
            )
            .await?;
        assert_ne!(code, 0, "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn a_partial_group_and_a_lone_path_still_hold() -> Result<(), Box<dyn std::error::Error>> {
    let lab = grouped_lab()?;
    for names in [&GROUPED_NAMES[..2], &GROUPED_NAMES[..1]] {
        let policy = grouped_policy(names)?;
        for name in names {
            let code = lab
                .run(
                    policy.clone(),
                    &format!("echo x > '{}' 2>/dev/null", lab.at(&format!("project/grp/{name}"))),
                )
                .await?;
            assert_ne!(code, 0, "{name}");
        }
        let other = lab
            .run(
                policy,
                &format!("echo x > '{}'", lab.at(&format!("project/grp/{}", GROUPED_NAMES[7]))),
            )
            .await?;
        assert_eq!(other, 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_folded_set_of_six_hundred_denied_paths_runs_and_enforces() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    for i in 0..600 {
        fs::create_dir_all(lab.host(&format!("project/pkg{i}")))?;
        fs::write(lab.host(&format!("project/pkg{i}/lock.json")), "x")?;
    }
    let policy = write_policy(
        (0..600)
            .map(|i| lit(&format!("/workspace/project/pkg{i}/lock.json")))
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let touched = lab
        .run(
            policy.clone(),
            &format!("echo x > '{}' 2>/dev/null", lab.at("project/pkg599/lock.json")),
        )
        .await?;
    assert_ne!(touched, 0);
    let moved = lab
        .run(policy.clone(), &rename(&lab, "project/pkg300", "project/pkg300b"))
        .await?;
    assert_ne!(moved, 0);
    let sibling = lab
        .run(policy, &format!("echo x > '{}'", lab.at("project/pkg10/other.txt")))
        .await?;
    assert_eq!(sibling, 0);
    Ok(())
}

#[tokio::test]
async fn five_hundred_glob_read_denies_compile_and_enforce() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    for i in 0..500 {
        fs::create_dir_all(lab.host(&format!("project/g{i}")))?;
        fs::write(lab.host(&format!("project/g{i}/data.pem")), SECRET)?;
    }
    let policy = Lists {
        deny_read: (0..500)
            .map(|i| glob(&format!("/workspace/project/g{i}/*.pem")))
            .collect::<Result<Vec<_>, _>>()?,
        allow_read_within_deny: vec![lit("/workspace/project")?],
        ..Lists::default()
    }
    .build()?;
    assert!(!lab.reads(policy.clone(), "project/g499/data.pem").await?);
    assert!(lab.reads(policy, "project/plain.txt").await?);
    Ok(())
}

#[tokio::test]
async fn processes_can_still_be_enumerated() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    let code = lab
        .run(FsPolicy::default(), "test -n \"$(/usr/bin/pgrep -f . | head -1)\"")
        .await?;
    assert_eq!(code, 0);
    Ok(())
}

const SPELLED_FILES: [(char, &str); 10] = [
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

const SPELLED_ENTRIES: [(&str, &str); 8] = [
    ("[z-a]", ""),
    ("[user-id]", "SE"),
    ("[user-id]/secret", "S"),
    ("**/[user-id]/*.env", "ED"),
    ("**/list[].js", "L"),
    ("**/x]y", "C"),
    ("**/o[p[q*", "O"),
    ("**/a[*]c", "W"),
];

fn spelled_entry(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    match glob(raw) {
        Ok(entry) => Ok(entry),
        Err(_) => lit(raw),
    }
}

fn spelled_policy(list: &str, entry: &str) -> Result<(FsPolicy, bool), Box<dyn std::error::Error>> {
    let path = spelled_entry(&format!("/workspace/tree/{entry}"))?;
    let tree = lit("/workspace/tree")?;
    Ok(match list {
        "denyRead" => (
            Lists {
                deny_read: vec![path],
                ..Lists::default()
            }
            .build()?,
            false,
        ),
        "credential deny" => (
            Lists {
                credential_deny_read: vec![path],
                ..Lists::default()
            }
            .build()?,
            false,
        ),
        "allowRead" => (
            Lists {
                deny_read: vec![tree],
                allow_read_within_deny: vec![path],
                ..Lists::default()
            }
            .build()?,
            true,
        ),
        "denyWrite" => (
            Lists {
                allow_write: vec![tree],
                deny_write: vec![path],
                ..Lists::default()
            }
            .build()?,
            false,
        ),
        _ => (
            Lists {
                allow_write: vec![path],
                ..Lists::default()
            }
            .build()?,
            true,
        ),
    })
}

#[tokio::test]
async fn brackets_that_spell_no_set_are_text_in_every_list() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::new()?;
    for (_, file) in SPELLED_FILES {
        let path = lab.host(&format!("tree/{file}"));
        fs::create_dir_all(path.parent().ok_or("a parent")?)?;
        fs::write(path, "content")?;
    }
    for list in ["denyRead", "credential deny", "allowRead", "denyWrite", "allowWrite"] {
        let writes = list.ends_with("Write");
        for (entry, covered) in SPELLED_ENTRIES {
            let (policy, grants) = spelled_policy(list, entry)?;
            for (letter, file) in SPELLED_FILES {
                let target = lab.at(&format!("tree/{file}"));
                let script = if writes {
                    format!(": >> '{target}' 2>/dev/null")
                } else {
                    format!("cat '{target}' >/dev/null 2>&1")
                };
                let reached = lab.run(policy.clone(), &script).await? == 0;
                let expected = covered.contains(letter) == grants;
                assert_eq!(reached, expected, "{list} {entry} {letter}");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn an_ordinary_relative_deny_works_from_a_bracketed_directory() -> Result<(), Box<dyn std::error::Error>> {
    let lab = Lab::named("[user-id]")?;
    fs::write(lab.host("a.env"), SECRET)?;
    fs::write(lab.host("secret"), PLAIN)?;
    let policy = Lists {
        deny_read: vec![glob("/workspace/*.env")?],
        ..Lists::default()
    }
    .build()?;
    assert!(!lab.reads(policy.clone(), "a.env").await?);
    assert!(lab.reads(policy, "secret").await?);
    Ok(())
}

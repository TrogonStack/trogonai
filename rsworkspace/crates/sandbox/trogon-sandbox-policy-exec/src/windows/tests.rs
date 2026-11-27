use super::*;
use crate::FilesystemOverrides;
use crate::restrictions::literal_entry;
use trogon_sandbox_contract::{DomainPattern, GlobPattern, InjectHosts, LiteralPath, SecretBindingParts, SecretName};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Lists {
    deny_read: Vec<FsPathEntry>,
    allow_read_within_deny: Vec<FsPathEntry>,
    allow_write: Vec<FsPathEntry>,
    deny_write: Vec<FsPathEntry>,
}

impl Lists {
    fn build(self) -> Result<FsPolicy, Box<dyn std::error::Error>> {
        Ok(FsPolicy::new(
            self.deny_read,
            Vec::new(),
            self.allow_read_within_deny,
            Vec::new(),
            self.allow_write,
            self.deny_write,
        )?)
    }
}

struct AnyPath;

impl Disk for AnyPath {
    fn exists(&self, _: &str) -> bool {
        true
    }

    fn list(&self, _: &str) -> Vec<(String, trogon_sandbox_subprocess_worker::windows_fs::Kind)> {
        Vec::new()
    }

    fn resolve(&self, path: &str) -> String {
        path.to_string()
    }
}

fn bin_shell() -> Result<BinShell, Box<dyn std::error::Error>> {
    Ok(BinShell::parse(None, r"C:\Windows")?)
}

fn launch() -> Result<WindowsLaunch, Box<dyn std::error::Error>> {
    Ok(WindowsLaunch {
        command: "echo hi".to_string(),
        bin_shell: bin_shell()?,
        held_by: Some(7),
        base_env: vec![("PATH".to_string(), r"C:\Windows".to_string())],
        mask_env: Vec::new(),
        proxy_env: Vec::new(),
        allow_loopback: false,
        cwd: r"C:\work".to_string(),
        ca_cert_path: None,
    })
}

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(literal_entry(raw)?)
}

fn config() -> PolicyExecConfig {
    PolicyExecConfig::new(std::env::temp_dir())
}

#[test]
fn write_roots_and_denies_become_flags() -> TestResult {
    let policy = Lists {
        allow_write: vec![literal("/C:/work")?],
        deny_read: vec![literal("/C:/work/secret")?],
        deny_write: vec![literal("/C:/work/.git")?],
        ..Lists::default()
    }
    .build()?;
    let request = request_for(&config(), &policy, launch()?, &AnyPath)?;
    let args = request.args()?;
    let pairs: Vec<(&str, &str)> = args.windows(2).map(|w| (w[0].as_str(), w[1].as_str())).collect();
    assert!(pairs.contains(&("--write-root", r"C:\work")));
    assert!(pairs.contains(&("--deny-read", r"C:\work\secret")));
    assert!(pairs.contains(&("--deny-write", r"C:\work\.git")));
    assert!(pairs.contains(&("--held-by", "7")));
    Ok(())
}

#[test]
fn a_grant_beneath_a_read_deny_is_kept_open() -> TestResult {
    let policy = Lists {
        allow_write: vec![literal("/C:/work/secret/out")?, literal("/C:/other")?],
        deny_read: vec![literal("/C:/work/secret")?],
        allow_read_within_deny: vec![literal("/C:/work/secret/in")?],
        ..Lists::default()
    }
    .build()?;
    let request = request_for(&config(), &policy, launch()?, &AnyPath)?;
    assert_eq!(
        request.deny_read_except,
        vec![r"C:\work\secret\in", r"C:\work\secret\out"]
    );
    Ok(())
}

#[test]
fn a_pattern_that_matches_nothing_adds_nothing() -> TestResult {
    let policy = Lists {
        deny_read: vec![FsPathEntry::Glob {
            pattern: GlobPattern::new("/C:/work/*.key")?,
        }],
        ..Lists::default()
    }
    .build()?;
    let request = request_for(&config(), &policy, launch()?, &AnyPath)?;
    assert!(request.deny_read.is_empty());
    Ok(())
}

#[test]
fn a_posix_path_is_refused() -> TestResult {
    let policy = Lists {
        allow_write: vec![literal("/tmp/work")?],
        ..Lists::default()
    }
    .build()?;
    assert!(matches!(
        request_for(&config(), &policy, launch()?, &AnyPath),
        Err(RuntimeError::Unsupported(_))
    ));
    Ok(())
}

#[test]
fn the_session_stamps_the_ranked_set_and_the_command_carries_only_write_roots() -> TestResult {
    let policy = Lists {
        allow_write: vec![literal("/C:/work")?, literal("/C:/work/frozen")?],
        allow_read_within_deny: vec![literal("/C:/lib")?],
        deny_read: vec![literal("/C:/work/secret")?],
        deny_write: vec![literal("/C:/work/frozen")?],
    }
    .build()?;
    let access = session_access(&policy, &[], false, &AnyPath)?;
    assert_eq!(access.grant_write, vec![r"C:\work"]);
    assert_eq!(access.grant_read, vec![r"C:\lib", r"C:\work\frozen"]);
    assert_eq!(access.deny_read, vec![r"C:\work\secret"]);
    assert_eq!(access.deny_write, vec![r"C:\work\frozen"]);
    let session = SessionStamp {
        access: &access,
        masked: &[],
        credential_deny: &[],
        verified_loopback: false,
    };
    let request = session_request(&config(), launch()?, &session, &CommandOverrides::default(), &AnyPath)?;
    assert_eq!(request.write_roots, vec![r"C:\work"]);
    assert!(request.deny_read.is_empty() && request.deny_write.is_empty());
    Ok(())
}

#[test]
fn git_trusts_the_working_tree_and_the_write_roots() -> TestResult {
    let env = git_env(&[r"C:\work".to_string(), r"C:\work".to_string()], false, true, &[]);
    let get = |name: &str| env.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    assert_eq!(get("GIT_CONFIG_VALUE_0"), Some("C:/work"));
    assert_eq!(get("GIT_CONFIG_VALUE_1"), Some("C:/work/*"));
    assert_eq!(get("GIT_CONFIG_VALUE_2"), Some("openssl"));
    assert_eq!(get("GIT_CONFIG_COUNT"), Some("3"));
    Ok(())
}

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn the_openssl_backend_is_added_only_when_asked_for() -> TestResult {
    let value_of = |env: &[(String, String)], name: &str| env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    let without = git_env(&[r"C:\work".to_string()], false, false, &[]);
    assert!(!without.iter().any(|(_, v)| v == "openssl"));
    assert_eq!(value_of(&without, "GIT_CONFIG_COUNT").as_deref(), Some("2"));
    let with = git_env(&[r"C:\work".to_string()], false, true, &[]);
    assert_eq!(value_of(&with, "GIT_CONFIG_COUNT").as_deref(), Some("3"));
    Ok(())
}

#[test]
fn an_existing_config_count_is_continued() -> TestResult {
    let base = pairs(&[("GIT_CONFIG_COUNT", "2")]);
    let env = git_env(&[], false, true, &base);
    assert_eq!(
        env,
        pairs(&[
            ("GIT_CONFIG_KEY_2", "http.sslBackend"),
            ("GIT_CONFIG_VALUE_2", "openssl"),
            ("GIT_CONFIG_COUNT", "3"),
        ])
    );
    let junk = pairs(&[("GIT_CONFIG_COUNT", "many")]);
    let env = git_env(&[], false, true, &junk);
    assert!(env.contains(&("GIT_CONFIG_KEY_0".to_string(), "http.sslBackend".to_string())));
    Ok(())
}

#[test]
fn a_config_count_of_zero_opts_out_and_nothing_to_set_gives_nothing() -> TestResult {
    let off = pairs(&[("GIT_CONFIG_COUNT", "0")]);
    assert!(git_env(&[r"C:\work".to_string()], true, true, &off).is_empty());
    assert!(git_env(&[], false, false, &[]).is_empty());
    Ok(())
}

#[test]
fn a_bare_drive_keeps_its_slash_and_many_directories_become_a_wildcard() -> TestResult {
    let env = git_env(&["C:".to_string()], true, true, &[]);
    assert!(env.contains(&("GIT_CONFIG_VALUE_0".to_string(), "C:/".to_string())));
    assert!(env.contains(&("GIT_CONFIG_VALUE_1".to_string(), "C:/*".to_string())));
    let many: Vec<String> = (0..9).map(|n| format!(r"C:\d{n}")).collect();
    let env = git_env(&many, false, true, &[]);
    assert!(env.contains(&("GIT_CONFIG_VALUE_0".to_string(), "*".to_string())));
    Ok(())
}

#[test]
fn loopback_and_proxy_reach_the_request() -> TestResult {
    let policy = Lists::default().build()?;
    let mut launch = launch()?;
    launch.allow_loopback = true;
    launch.proxy_env = vec![("http_proxy".to_string(), "http://127.0.0.1:60080".to_string())];
    let request = request_for(&config(), &policy, launch, &AnyPath)?;
    let args = request.args()?;
    assert!(args.contains(&"--allow-loopback".to_string()));
    assert!(args.contains(&"no_proxy=localhost,127.0.0.1,::1".to_string()));
    assert!(args.contains(&"NODE_USE_ENV_PROXY=1".to_string()));
    Ok(())
}

#[test]
fn masked_sentinels_precede_the_proxy_set_and_feed_the_git_count() -> TestResult {
    let policy = Lists::default().build()?;
    let mut launch = launch()?;
    launch.mask_env = vec![
        ("HTTPS_PROXY".to_string(), "sentinel".to_string()),
        ("GIT_CONFIG_COUNT".to_string(), "0".to_string()),
        ("API_TOKEN".to_string(), "fake_value_1".to_string()),
    ];
    launch.proxy_env = vec![("https_proxy".to_string(), "http://127.0.0.1:60080".to_string())];
    let request = request_for(&config(), &policy, launch, &AnyPath)?;
    let args = request.args()?;
    assert!(args.contains(&"API_TOKEN=fake_value_1".to_string()));
    assert!(args.contains(&"https_proxy=http://127.0.0.1:60080".to_string()));
    assert!(!args.iter().any(|arg| arg == "HTTPS_PROXY=sentinel"));
    assert!(!args.iter().any(|arg| arg.starts_with("GIT_CONFIG_KEY_")));
    Ok(())
}

fn file_binding(path: &str, mode: SecretMode) -> Result<SecretBinding, Box<dyn std::error::Error>> {
    let name = SecretName::new("file")?;
    Ok(SecretBinding::new(SecretBindingParts {
        source: crate::host_source::host_source_request(&name),
        name,
        target: SecretTarget::File(LiteralPath::new(path)?),
        mode,
        extract: None,
        mask_duplicates: Default::default(),
        on_extract_no_match: ExtractNoMatch::default(),
        strip_response_headers: Default::default(),
        inject_hosts: InjectHosts::new(vec![DomainPattern::new("api.example.com")?])?,
        inject_scope: Default::default(),
    })?)
}

#[test]
fn a_masked_file_with_a_value_is_denied_and_a_missing_or_empty_one_is_not() -> TestResult {
    let read = |path: &str| match path {
        "/C:/home/.npmrc" => Ok("token\n".to_string()),
        "/C:/home/empty" => Ok(String::new()),
        _ => Err(std::io::ErrorKind::NotFound.into()),
    };
    let bindings = [
        file_binding("/C:/home/.npmrc", SecretMode::Mask)?,
        file_binding("/C:/home/empty", SecretMode::Mask)?,
        file_binding("/C:/home/missing", SecretMode::Mask)?,
        file_binding("/C:/home/denied", SecretMode::Deny)?,
    ];
    assert_eq!(
        masked_file_denies(&bindings, &read)?,
        vec![literal("/C:/home/.npmrc")?, literal("/C:/home/denied")?]
    );
    Ok(())
}

#[test]
fn a_grant_at_a_credential_deny_is_dropped() -> TestResult {
    let policy = Lists {
        allow_write: vec![literal("/C:/home/.npmrc")?, literal("/C:/work")?],
        ..Lists::default()
    }
    .build()?;
    let access = session_access(&policy, &[literal("/C:/home/.npmrc")?], false, &AnyPath)?;
    assert_eq!(access.grant_write, vec![r"C:\work".to_string()]);
    assert_eq!(access.deny_read, vec![r"C:\home\.npmrc".to_string()]);
    Ok(())
}

#[test]
fn a_disabled_filesystem_stamps_only_the_masked_files() -> TestResult {
    let policy = Lists {
        allow_write: vec![literal("/C:/work")?],
        deny_read: vec![literal("/C:/work/secret")?],
        deny_write: vec![literal("/C:/work/.git")?],
        ..Lists::default()
    }
    .build()?;
    let access = session_access(&policy, &[literal("/C:/home/.npmrc")?], true, &AnyPath)?;
    assert!(access.grant_read.is_empty() && access.grant_write.is_empty());
    assert!(access.deny_write.is_empty());
    assert_eq!(access.deny_read, vec![r"C:\home\.npmrc".to_string()]);
    Ok(())
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn stamped() -> FsAccessSet {
    FsAccessSet {
        grant_read: strings(&[r"C:\work\secret\in"]),
        grant_write: strings(&[r"C:\work"]),
        deny_read: strings(&[r"C:\work\secret"]),
        deny_write: strings(&[r"C:\work\.git"]),
    }
}

fn denies_of(overrides: &CommandOverrides) -> Result<CommandDenies, RuntimeError> {
    let access = stamped();
    let session = SessionStamp {
        access: &access,
        masked: &[],
        credential_deny: &[],
        verified_loopback: false,
    };
    per_command_denies(overrides, &session, &AnyPath)
}

fn fs_overrides(own: FilesystemOverrides) -> CommandOverrides {
    CommandOverrides::default().filesystem(own)
}

#[test]
fn a_command_deny_the_session_stamped_already_is_not_stamped_again() -> TestResult {
    let own = FilesystemOverrides::default()
        .deny_read(vec![literal("/C:/work/secret")?, literal("/C:/work/extra")?])
        .deny_write(vec![literal("/C:/work/.git")?, literal("/C:/work/dist")?]);
    let denies = denies_of(&fs_overrides(own))?;
    assert_eq!(denies.deny_read, strings(&[r"C:\work\extra"]));
    assert_eq!(denies.deny_write, strings(&[r"C:\work\dist"]));
    Ok(())
}

#[test]
fn a_session_grant_beneath_a_command_deny_keeps_its_rank() -> TestResult {
    let own = FilesystemOverrides::default().deny_read(vec![literal("/C:/work")?]);
    let denies = denies_of(&fs_overrides(own))?;
    assert_eq!(denies.deny_read, strings(&[r"C:\work"]));
    assert_eq!(denies.deny_read_except, strings(&[r"C:\work\secret\in"]));
    Ok(())
}

#[test]
fn a_command_grant_is_refused_as_unsupported() -> TestResult {
    for own in [
        FilesystemOverrides::default().allow_write(vec![literal("/C:/other")?]),
        FilesystemOverrides::default().allow_read(vec![literal("/C:/other")?]),
    ] {
        let error = denies_of(&fs_overrides(own)).err().ok_or("expected an error")?;
        assert!(matches!(error, RuntimeError::Unsupported(_)), "{error}");
    }
    Ok(())
}

#[test]
fn a_disabled_command_filesystem_adds_no_denies_or_refusals() -> TestResult {
    let own = FilesystemOverrides::default()
        .disabled(true)
        .deny_read(vec![literal("/C:/work/extra")?])
        .allow_write(vec![literal("/C:/other")?]);
    assert_eq!(denies_of(&fs_overrides(own))?, CommandDenies::default());
    Ok(())
}

#[test]
fn a_masked_file_the_session_stamped_adds_nothing() -> TestResult {
    let access = FsAccessSet {
        deny_read: strings(&[r"C:\home\.npmrc"]),
        ..FsAccessSet::default()
    };
    let masked = [literal("/C:/home/.npmrc")?];
    let session = SessionStamp {
        access: &access,
        masked: &masked,
        credential_deny: &[],
        verified_loopback: false,
    };
    let denies = per_command_denies(&CommandOverrides::default(), &session, &AnyPath)?;
    assert_eq!(denies, CommandDenies::default());
    Ok(())
}

#[test]
fn the_credential_files_a_command_denies_are_denied_for_it_alone() -> TestResult {
    let access = stamped();
    let credential = [literal("/C:/home/.aws/credentials")?];
    let session = SessionStamp {
        access: &access,
        masked: &[],
        credential_deny: &credential,
        verified_loopback: false,
    };
    let denies = per_command_denies(&CommandOverrides::default(), &session, &AnyPath)?;
    assert_eq!(denies.deny_read, strings(&[r"C:\home\.aws\credentials"]));

    let off = CommandOverrides::default().filesystem(FilesystemOverrides::default().disabled(true));
    assert_eq!(per_command_denies(&off, &session, &AnyPath)?, CommandDenies::default());
    Ok(())
}

#[test]
fn a_loopback_mode_other_than_the_verified_one_is_refused() -> TestResult {
    let asks_open = CommandOverrides::default().allow_local_binding(true);
    let error = check_loopback(&asks_open, false).err().ok_or("expected an error")?;
    let RuntimeError::Launch(cause) = error else {
        return Err("not a launch error".into());
    };
    let cause = cause.downcast::<WindowsSandboxError>()?;
    assert_eq!(cause.code(), WindowsSandboxErrorCode::WfpLoopbackMismatch);
    assert!(check_loopback(&asks_open, true).is_ok());
    assert!(check_loopback(&CommandOverrides::default(), true).is_ok());
    Ok(())
}

#[test]
fn the_request_carries_the_command_denies_and_the_session_write_roots() -> TestResult {
    let access = stamped();
    let session = SessionStamp {
        access: &access,
        masked: &[],
        credential_deny: &[],
        verified_loopback: false,
    };
    let own = FilesystemOverrides::default().deny_write(vec![literal("/C:/work/dist")?]);
    let request = session_request(&config(), launch()?, &session, &fs_overrides(own), &AnyPath)?;
    assert_eq!(request.write_roots, strings(&[r"C:\work"]));
    assert_eq!(request.deny_write, strings(&[r"C:\work\dist"]));
    Ok(())
}

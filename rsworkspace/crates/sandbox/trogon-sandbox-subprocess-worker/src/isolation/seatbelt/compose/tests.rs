use super::*;
use crate::isolation::seatbelt::test_support::{Lists, entry, glob};

fn compose_with(policy: &FsPolicy, egress: &EgressPolicy) -> Result<String, Box<dyn std::error::Error>> {
    Ok(compose(policy, &PathMapper::new(vec![]), egress, LogTag::new("TAG")?)?
        .as_str()
        .to_string())
}

fn lines(profile: &str) -> Vec<&str> {
    profile.lines().collect()
}

fn last_allow(profile: &str) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(lines(profile)
        .iter()
        .rposition(|line| line.starts_with("(allow"))
        .ok_or("an allow rule")?)
}

fn first_deny_rule(profile: &str) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(lines(profile)
        .iter()
        .position(|line| line.starts_with("(deny") && !line.starts_with("(deny default"))
        .ok_or("a deny rule")?)
}

fn full_policy() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Lists {
        deny_read: vec![entry("/home/u")?, glob("/data/**/*.pem")?],
        allow_read_within_deny: vec![entry("/home/u/project")?],
        allow_write: vec![entry("/home/u/project")?],
        deny_write: vec![entry("/home/u/project/locked")?],
        ..Lists::default()
    }
    .build()
}

#[test]
fn every_allow_comes_before_every_deny() -> Result<(), Box<dyn std::error::Error>> {
    for egress in [
        EgressPolicy::DenyAll,
        EgressPolicy::AllowAll,
        EgressPolicy::Loopback("127.0.0.1:4000".parse()?),
    ] {
        let profile = compose_with(&full_policy()?, &egress)?;
        assert!(last_allow(&profile)? < first_deny_rule(&profile)?, "{profile}");
    }
    Ok(())
}

#[test]
fn the_profile_starts_denied_by_default_with_the_tag() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&FsPolicy::default(), &EgressPolicy::DenyAll)?;
    let head = lines(&profile);
    assert_eq!(head[0], "(version 1)");
    assert_eq!(head[1], "(deny default (with message \"TAG\"))");
    Ok(())
}

#[test]
fn deny_all_grants_no_network() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&FsPolicy::default(), &EgressPolicy::DenyAll)?;
    assert!(!profile.contains("network"));
    Ok(())
}

#[test]
fn allow_all_grants_the_whole_network() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&FsPolicy::default(), &EgressPolicy::AllowAll)?;
    assert!(profile.contains("(allow network*)"));
    Ok(())
}

#[test]
fn a_loopback_proxy_grants_only_its_port() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&FsPolicy::default(), &EgressPolicy::Loopback("127.0.0.1:4000".parse()?))?;
    assert!(profile.contains("(allow network-outbound (remote ip \"localhost:4000\"))"));
    assert!(!profile.contains("network*"));
    Ok(())
}

#[test]
fn a_proxy_off_loopback_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let result = compose(
        &FsPolicy::default(),
        &PathMapper::new(vec![]),
        &EgressPolicy::Loopback("10.0.0.1:4000".parse()?),
        LogTag::new("TAG")?,
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn writes_are_granted_only_where_the_policy_allows() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&full_policy()?, &EgressPolicy::DenyAll)?;
    assert!(profile.contains(
        "(allow file-write* (subpath \"/System/Volumes/Data/home/u/project\") (subpath \"/home/u/project\"))"
    ));
    let none = compose_with(&FsPolicy::default(), &EgressPolicy::DenyAll)?;
    assert!(
        !none
            .lines()
            .any(|line| line.starts_with("(allow file-write*") && line.contains("subpath"))
    );
    assert!(none.contains("(allow file-write* (literal \"/dev/stdout\")"));
    Ok(())
}

#[test]
fn a_write_deny_covers_the_paths_the_policy_names() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&full_policy()?, &EgressPolicy::DenyAll)?;
    assert!(profile.contains("(deny file-write* (subpath \"/System/Volumes/Data/home/u/project/locked\") (subpath \"/home/u/project/locked\")"));
    Ok(())
}

#[test]
fn mandatory_denies_guard_every_writable_root() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&full_policy()?, &EgressPolicy::DenyAll)?;
    assert!(profile.contains("^/home/u/project/(.*/)?"));
    assert!(profile.contains("[gG][iI][tT]"));
    Ok(())
}

#[test]
fn a_read_deny_without_a_carve_out_leaves_directories_statable() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![entry("/home/u")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(profile.contains(
        "(deny file-read* (require-all (require-any (subpath \"/System/Volumes/Data/home/u\") (subpath \"/home/u\")) (require-not (vnode-type DIRECTORY)))"
    ));
    assert!(profile.contains("(deny file-read-data file-read-xattr"));
    Ok(())
}

#[test]
fn a_credential_deny_is_not_carved_out_by_any_allow_read() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        credential_deny_read: vec![entry("/home/u/.netrc")?],
        allow_read_within_deny: vec![entry("/home/u")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(profile.contains(
        "(deny file-read* (require-all (require-any (subpath \"/System/Volumes/Data/home/u/.netrc\") (subpath \"/home/u/.netrc\")"
    ));
    assert!(!profile.contains("(require-not (require-any"));
    Ok(())
}

#[test]
fn an_own_allow_is_carved_out_of_a_credential_deny_but_an_allow_read_is_not() -> Result<(), Box<dyn std::error::Error>>
{
    let policy = Lists {
        credential_deny_read: vec![entry("/home/u")?],
        allow_read_within_deny: vec![entry("/home/u/.netrc")?],
        own_allow_within_deny: vec![entry("/home/u/ca.pem")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(profile.contains("/home/u/ca.pem"));
    assert!(!profile.contains("/home/u/.netrc"));
    Ok(())
}

#[test]
fn a_credential_deny_with_brackets_in_its_path_is_a_literal_subpath() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        credential_deny_read: vec![entry("/home/u/[WIP] project/.netrc")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(profile.contains(
        "(deny file-read* (require-all (require-any (subpath \"/System/Volumes/Data/home/u/[WIP] project/.netrc\") (subpath \"/home/u/[WIP] project/.netrc\")"
    ));
    assert!(!profile.contains("regex"));
    Ok(())
}

#[test]
fn a_credential_deny_is_denied_after_every_allow() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        credential_deny_read: vec![entry("/home/u/.netrc")?],
        allow_read_within_deny: vec![entry("/home/u/.netrc")?],
        allow_write: vec![entry("/home/u")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    let last_allow = profile.rfind("(allow ").ok_or("a value")?;
    let credential = profile
        .find("(deny file-read* (require-all (require-any (subpath \"/System/Volumes/Data/home/u/.netrc\")")
        .ok_or("a value")?;
    assert!(last_allow < credential);
    Ok(())
}

#[test]
fn a_carve_out_is_subtracted_from_the_deny_not_allowed_after_it() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&full_policy()?, &EgressPolicy::DenyAll)?;
    let non_directory = profile
        .lines()
        .find(|line| line.starts_with("(deny file-read* (require-all") && line.contains("/home/u/project"))
        .ok_or("a value")?;
    assert!(non_directory.contains(
        "(require-not (require-any (subpath \"/System/Volumes/Data/home/u/project\") (subpath \"/home/u/project\")))"
    ));
    assert!(non_directory.contains("(require-not (vnode-type DIRECTORY))"));
    let directory = profile
        .lines()
        .find(|line| line.starts_with("(deny file-read-data file-read-xattr"))
        .ok_or("a value")?;
    assert!(directory.contains("(vnode-type DIRECTORY)"));
    assert!(directory.contains("(require-not (literal \"/\"))"));
    Ok(())
}

#[test]
fn a_leaf_glob_deny_is_not_carved_out_of_a_directory_allow() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![glob("/**/.env")?],
        allow_read_within_deny: vec![entry("/ws")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(profile.contains("\\.env"), "{profile}");
    assert!(!profile.contains("require-not (require-any (subpath"), "{profile}");
    Ok(())
}

#[test]
fn a_literal_deny_inside_a_literal_allow_is_not_carved_out() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![entry("/ws/secrets")?],
        allow_read_within_deny: vec![entry("/ws")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(
        profile.contains("(deny file-read* (require-all (require-any (subpath \"/ws/secrets\")"),
        "{profile}"
    );
    assert!(!profile.contains("require-not (require-any (subpath"), "{profile}");
    Ok(())
}

#[test]
fn a_region_glob_deny_is_carved_out_by_the_allows_it_covers() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![glob("/home/**/*")?],
        allow_read_within_deny: vec![entry("/home/u/project")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(
        profile.contains("(require-not (require-any (subpath \"/System/Volumes/Data/home/u/project\") (subpath \"/home/u/project\")))"),
        "{profile}"
    );
    Ok(())
}

#[test]
fn the_top_level_protected_directories_cannot_be_created_in_a_writable_root() -> Result<(), Box<dyn std::error::Error>>
{
    let profile = compose_with(&full_policy()?, &EgressPolicy::DenyAll)?;
    let create: Vec<&str> = profile
        .lines()
        .filter(|line| line.starts_with("(deny file-write-create"))
        .collect();
    assert!(
        create
            .iter()
            .any(|l| l.contains("^/home/u/project/(\\\\.[gG][iI][tT]|")),
        "{profile}"
    );
    Ok(())
}

#[test]
fn the_ancestors_of_a_write_denied_path_cannot_be_created_as_links() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        allow_write: vec![entry("/ws")?],
        deny_write: vec![entry("/ws/a/b/secret")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    let pinned = profile
        .lines()
        .find(|line| line.starts_with("(deny file-write-create") && line.contains("(literal \"/ws/a/b\")"))
        .ok_or("a value")?;
    assert!(pinned.contains("(literal \"/ws/a\")"));
    Ok(())
}

#[test]
fn denied_paths_cannot_be_moved_away() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_with(&full_policy()?, &EgressPolicy::DenyAll)?;
    let unlink: Vec<&str> = profile
        .lines()
        .filter(|line| line.starts_with("(deny file-write-unlink"))
        .collect();
    assert!(unlink.iter().any(|l| l.contains("/home/u/project/locked")));
    assert!(unlink.iter().any(|l| l.contains(
        "(require-not (require-any (subpath \"/System/Volumes/Data/home/u/project\") (subpath \"/home/u/project\")))"
    )));
    assert!(
        unlink
            .iter()
            .any(|l| l.contains("\\.git") || l.contains("[gG][iI][tT]"))
    );
    Ok(())
}

#[test]
fn the_ancestors_between_a_root_and_a_protected_path_are_pinned() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        allow_write: vec![entry("/ws")?],
        deny_write: vec![entry("/ws/a/b/secret")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    let pinned = profile
        .lines()
        .find(|line| line.contains("(literal \"/ws/a/b\")"))
        .ok_or("a value")?;
    assert!(pinned.contains("(literal \"/ws/a\")"));
    assert!(!pinned.contains("(literal \"/ws\")"));
    Ok(())
}

#[test]
fn a_glob_deny_is_a_regex_in_the_read_and_move_rules() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![glob("/data/**/*.pem")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    let count = profile.matches("(regex \"^/data/(.*/)?[^/]*\\\\.pem(/.*)?$\")").count();
    assert_eq!(count, 3, "{profile}");
    Ok(())
}

#[test]
fn a_write_root_inside_a_read_deny_is_left_out_of_the_unlink_deny() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![entry("/ws/secrets")?],
        allow_write: vec![entry("/ws")?, entry("/ws/secrets/scratch")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(
        profile.contains("(require-not (require-any (subpath \"/ws/secrets/scratch\")))"),
        "{profile}"
    );
    Ok(())
}

#[test]
fn a_credential_deny_keeps_its_unlink_guard_over_a_write_root() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        credential_deny_read: vec![entry("/ws/vault")?],
        allow_write: vec![entry("/ws")?, entry("/ws/vault/scratch")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(
        profile.contains("(deny file-write-unlink (require-any (subpath \"/ws/vault\"))"),
        "{profile}"
    );
    Ok(())
}

#[test]
fn a_write_root_above_a_credential_deny_is_kept_in_place() -> Result<(), Box<dyn std::error::Error>> {
    let credential = Lists {
        credential_deny_read: vec![entry("/ws/box/dir")?],
        allow_write: vec![entry("/ws/box")?, entry("/ws/out")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&credential, &EgressPolicy::DenyAll)?;
    assert!(profile.contains("(literal \"/ws/box\")"), "{profile}");
    let plain = Lists {
        deny_read: credential.credential_deny_read().to_vec(),
        allow_write: credential.allow_write().to_vec(),
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&plain, &EgressPolicy::DenyAll)?;
    assert!(!profile.contains("(literal \"/ws/box\")"), "{profile}");
    Ok(())
}

#[test]
fn a_deny_named_as_a_root_inside_another_root_stays_guarded() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![entry("/ws/secrets")?],
        allow_write: vec![entry("/ws")?, entry("/ws/secrets")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(!profile.contains("require-not (require-any"), "{profile}");
    Ok(())
}

#[test]
fn the_directory_of_a_glob_deny_below_a_root_cannot_be_moved() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![glob("/ws/conf/*.pem")?],
        allow_write: vec![entry("/ws")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(
        profile
            .lines()
            .any(|l| l.starts_with("(deny file-write-unlink") && l.contains("(literal \"/ws/conf\")")),
        "{profile}"
    );
    Ok(())
}

#[test]
fn brackets_in_a_literal_path_are_escaped() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        deny_read: vec![
            entry("/a[1]/b")?,
            entry("/a[1]/c")?,
            entry("/a[1]/d")?,
            entry("/a[1]/e")?,
        ],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(profile.contains("\\\\[1\\\\]"), "{profile}");
    Ok(())
}

#[test]
fn paths_are_rewritten_to_the_host() -> Result<(), Box<dyn std::error::Error>> {
    let mapper = PathMapper::new(vec![(LiteralPath::new("/workspace")?, LiteralPath::new("/host/ws")?)]);
    let policy = Lists {
        allow_write: vec![entry("/workspace")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose(&policy, &mapper, &EgressPolicy::DenyAll, LogTag::new("TAG")?)?;
    assert!(profile.as_str().contains("(subpath \"/host/ws\")"));
    assert!(!profile.as_str().contains("\"/workspace"));
    Ok(())
}

fn compose_filesystem(policy: &FsPolicy, filesystem: &FilesystemOptions) -> Result<String, Box<dyn std::error::Error>> {
    Ok(compose_with_opt_ins(
        policy,
        &PathMapper::new(vec![]),
        &LiteralPath::new("/work")?,
        &EgressPolicy::DenyAll,
        &MacosOptions::default(),
        filesystem,
        LogTag::new("TAG")?,
    )?
    .as_str()
    .to_string())
}

fn writable_workspace() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Lists {
        allow_write: vec![entry("/work")?],
        ..Lists::default()
    }
    .build()
}

#[test]
fn turning_the_filesystem_rules_off_allows_everything_and_denies_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let profile = compose_filesystem(
        &writable_workspace()?,
        &FilesystemOptions {
            disabled: true.into(),
            ..FilesystemOptions::default()
        },
    )?;
    assert!(profile.contains("(allow file-read*)"), "{profile}");
    assert!(profile.contains("(allow file-write*)"), "{profile}");
    assert!(!profile.contains("(deny file-write*"), "{profile}");
    assert!(!profile.contains(".git/hooks"), "{profile}");
    Ok(())
}

#[test]
fn allow_git_config_frees_the_config_and_keeps_the_hooks() -> Result<(), Box<dyn std::error::Error>> {
    const CONFIG: &str = "[cC][oO][nN][fF][iI][gG]";
    const HOOKS: &str = "[hH][oO][oO][kK][sS]";
    let policy = writable_workspace()?;
    let protected = compose_filesystem(&policy, &FilesystemOptions::default())?;
    let freed = compose_filesystem(
        &policy,
        &FilesystemOptions {
            allow_git_config: true.into(),
            ..FilesystemOptions::default()
        },
    )?;
    assert!(protected.matches(HOOKS).count() > 0, "{protected}");
    assert_eq!(freed.matches(HOOKS).count(), protected.matches(HOOKS).count());
    assert!(
        freed.matches(CONFIG).count() < protected.matches(CONFIG).count(),
        "{freed}"
    );
    Ok(())
}

#[test]
fn an_oversized_policy_is_refused_before_it_reaches_the_composer() -> Result<(), Box<dyn std::error::Error>> {
    let entries = (0..40000)
        .map(|i| entry(&format!("/very/long/directory/{i}/{}{i}", "x".repeat(200))))
        .collect::<Result<Vec<FsPathEntry>, _>>()?;
    let policy = Lists {
        deny_read: entries,
        ..Lists::default()
    }
    .build();
    assert!(policy.is_err());
    Ok(())
}

fn deny_lines(profile: &str) -> Vec<&str> {
    profile
        .lines()
        .filter(|line| line.starts_with("(deny") && !line.starts_with("(deny default"))
        .collect()
}

#[test]
fn a_credential_deny_comes_after_the_allow_beside_it_by_its_kind() -> Result<(), Box<dyn std::error::Error>> {
    let deny = "(deny file-read* (require-all (require-any (subpath \"/vault/v1/token\")) (require-not (vnode-type DIRECTORY))";
    let carved = "(require-not (require-any";
    for allow in [
        entry("/vault/v1/token")?,
        glob("/vault/v1/*")?,
        entry("/vault/v1/token/part")?,
    ] {
        let plain = compose_with(
            &Lists {
                deny_read: vec![entry("/vault/v1/token")?],
                allow_read_within_deny: vec![allow.clone()],
                ..Lists::default()
            }
            .build()?,
            &EgressPolicy::DenyAll,
        )?;
        assert!(!plain.contains(deny), "{allow:?}\n{plain}");
        assert!(plain.contains(carved), "{allow:?}\n{plain}");
        let credential = compose_with(
            &Lists {
                credential_deny_read: vec![entry("/vault/v1/token")?],
                allow_read_within_deny: vec![allow.clone()],
                ..Lists::default()
            }
            .build()?,
            &EgressPolicy::DenyAll,
        )?;
        assert!(credential.contains(deny), "{allow:?}\n{credential}");
        assert!(!deny_lines(&credential).iter().any(|l| l.contains(carved)));
        assert!(credential.rfind("(allow ").ok_or("a value")? < credential.find(deny).ok_or("a value")?);
    }
    Ok(())
}

#[test]
fn a_credential_deny_on_a_name_with_brackets_comes_after_the_allows_as_that_name()
-> Result<(), Box<dyn std::error::Error>> {
    let policy = Lists {
        credential_deny_read: vec![entry("/vault/[v1.2]/token")?],
        allow_read_within_deny: vec![glob("/vault/\\[v1.2]/*")?],
        ..Lists::default()
    }
    .build()?;
    let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
    assert!(
        profile.contains("(deny file-read* (require-all (require-any (subpath \"/vault/[v1.2]/token\")"),
        "{profile}"
    );
    assert!(!deny_lines(&profile).iter().any(|l| l.contains("regex")));
    Ok(())
}

#[test]
fn a_deny_on_a_link_to_a_device_is_one_on_the_link_alone() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let root = root.to_str().ok_or("a value")?;
    std::os::unix::fs::symlink("/dev/null", format!("{root}/link-to-null"))?;
    std::os::unix::fs::symlink("/dev", format!("{root}/link-to-dev"))?;
    let entries = [
        entry(&format!("{root}/link-to-null"))?,
        entry(&format!("{root}/link-to-dev"))?,
        glob(&format!("{root}/link-to-dev/*"))?,
        entry(&format!("{root}/link-to-dev/null"))?,
    ];
    for entry in entries {
        for credential in [false, true] {
            let mut lists = Lists::default();
            if credential {
                lists.credential_deny_read = vec![entry.clone()];
            } else {
                lists.deny_read = vec![entry.clone()];
            }
            let policy = lists.build()?;
            let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
            let denies = deny_lines(&profile).join("\n");
            assert!(denies.contains("link-to-"), "{entry:?}\n{profile}");
            assert!(!denies.contains("\"/dev"), "{entry:?}\n{profile}");
            assert!(!denies.contains("^/dev"), "{entry:?}\n{profile}");
        }
    }
    Ok(())
}

struct Vault {
    _dir: tempfile::TempDir,
    root: String,
}

impl Vault {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?.to_str().ok_or("a utf-8 path")?.to_string();
        for file in [
            "vault/v1/token",
            "vault/v1/sibling",
            "vault/[v1.2]/token",
            "vault/dir/deep/token",
            "vault/sub/.netrc",
        ] {
            let path = std::path::PathBuf::from(format!("{root}/{file}"));
            std::fs::create_dir_all(path.parent().ok_or("a parent")?)?;
            std::fs::write(path, "file-content")?;
        }
        let link = |target: &str, name: &str| std::os::unix::fs::symlink(target, format!("{root}/{name}"));
        link(&format!("{root}/vault/v1/token"), "vault/link-to-token")?;
        link(&format!("{root}/vault/dir"), "vault/link-to-dir")?;
        link(&format!("{root}/vault/[v1.2]"), "vault/link-to-brackets")?;
        link(&format!("{root}/vault"), "link-to-vault")?;
        link("/", "link-to-root")?;
        Ok(Self { _dir: dir, root })
    }

    fn at(&self, tail: &str) -> String {
        format!("{}/{tail}", self.root)
    }

    fn lit(&self, tail: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
        entry(&self.at(tail))
    }

    fn glob(&self, tail: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
        glob(&self.at(tail))
    }
}

fn filter_covers(filter: &Filter, file: &str) -> bool {
    match filter {
        Filter::Literal(s) => s.as_str() == file,
        Filter::Subpath(s) => file == s.as_str() || file.starts_with(&format!("{}/", s.as_str())),
        Filter::Regex(s) => regex::Regex::new(s.as_str()).is_ok_and(|re| re.is_match(file)),
        _ => false,
    }
}

fn region_covers(region: &Region, file: &str) -> bool {
    region.filters.iter().any(|f| filter_covers(f, file))
}

#[test]
fn a_deny_is_emitted_under_both_spellings_of_its_path() -> Result<(), Box<dyn std::error::Error>> {
    let vault = Vault::new()?;
    let denies = [
        vault.lit("vault/link-to-token")?,
        vault.lit("link-to-vault/dir")?,
        vault.glob("link-to-vault/**/.netrc")?,
        vault.glob("vault/link-to-brackets/*")?,
        vault.lit("link-to-root")?,
    ];
    let region = Region::translate(&denies, &PathMapper::new(vec![]))?;
    for file in [
        "vault/link-to-token",
        "vault/v1/token",
        "vault/dir/deep/token",
        "vault/sub/.netrc",
        "vault/[v1.2]/token",
    ] {
        assert!(region_covers(&region, &vault.at(file)), "{file}");
    }
    assert!(!region_covers(&region, &vault.at("vault/v1/sibling")));
    Ok(())
}

#[test]
fn an_allow_is_emitted_under_both_spellings_of_its_name() -> Result<(), Box<dyn std::error::Error>> {
    let vault = Vault::new()?;
    let rows = [
        (vault.lit("link-to-vault/dir/deep")?, "vault/dir/deep/token", true),
        (vault.lit("link-to-vault/dir/deep")?, "vault/dir/deep", true),
        (vault.glob("link-to-vault/dir/deep/*")?, "vault/dir/deep/token", true),
        (vault.glob("vault/link-to-dir/*")?, "vault/dir/deep", true),
        (vault.glob("vault/link-to-brackets/*")?, "vault/[v1.2]/token", true),
        (vault.lit("vault/link-to-dir")?, "vault/dir/deep/token", false),
        (vault.lit("vault/link-to-token")?, "vault/v1/token", false),
        (vault.glob("link-to-root/*")?, "", false),
    ];
    for (allow, file, matches) in rows {
        let region = Region::translate_allows(std::slice::from_ref(&allow), &PathMapper::new(vec![]))?;
        assert_eq!(region_covers(&region, &vault.at(file)), matches, "{allow:?} {file}");
    }
    Ok(())
}

#[test]
fn both_spellings_of_an_allow_count_against_the_denies_around_it() -> Result<(), Box<dyn std::error::Error>> {
    let vault = Vault::new()?;
    let compose = |policy: FsPolicy| compose_with(&policy, &EgressPolicy::DenyAll);
    let nested = compose(
        Lists {
            deny_read: vec![vault.lit("vault/dir/deep")?],
            allow_read_within_deny: vec![vault.lit("link-to-vault/dir")?],
            ..Lists::default()
        }
        .build()?,
    )?;
    assert!(
        nested.contains(&format!(
            "(deny file-read* (require-all (require-any (subpath \"{}\")",
            vault.at("vault/dir/deep")
        )),
        "{nested}"
    );
    let token = vault.at("vault/dir/deep/token");
    let carved = format!("(subpath \"{token}\")");
    let covered = compose(
        Lists {
            deny_read: vec![vault.glob("vault/**/token")?],
            allow_read_within_deny: vec![vault.lit("link-to-vault/dir/deep/token")?],
            allow_write: vec![vault.lit("vault")?],
            ..Lists::default()
        }
        .build()?,
    )?;
    assert!(
        deny_lines(&covered)
            .iter()
            .filter(|l| l.contains("(require-not") && l.contains(&carved))
            .count()
            >= 2,
        "{covered}"
    );
    let sibling = vault.at("vault/v1/sibling");
    let own = compose(
        Lists {
            credential_deny_read: vec![vault.lit("vault")?],
            allow_read_within_deny: vec![vault.lit("link-to-vault/v1/sibling")?],
            own_allow_within_deny: vec![vault.lit("link-to-vault/v1/sibling")?],
            ..Lists::default()
        }
        .build()?,
    )?;
    assert!(
        deny_lines(&own)
            .iter()
            .any(|l| l.contains("(require-not") && l.contains(&format!("(subpath \"{sibling}\")"))),
        "{own}"
    );
    Ok(())
}

fn bracket_entry(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(match trogon_sandbox_contract::GlobPattern::new(raw) {
        Ok(_) => glob(raw)?,
        Err(_) => entry(raw)?,
    })
}

fn regex_filters(profile: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = profile;
    while let Some(at) = rest.find("(regex \"") {
        let body = &rest[at + 8..];
        let mut source = String::new();
        let mut chars = body.char_indices();
        let mut end = body.len();
        while let Some((index, c)) = chars.next() {
            match c {
                '"' => {
                    end = index;
                    break;
                }
                '\\' => source.extend(chars.next().map(|(_, escaped)| escaped)),
                other => source.push(other),
            }
        }
        found.push(source);
        rest = &body[end..];
    }
    found
}

#[test]
fn bracket_entries_compile_to_the_text_they_are_in_every_list() -> Result<(), Box<dyn std::error::Error>> {
    const TREE: &str = "/work/tree";
    let entries: [(&str, Option<&str>); 8] = [
        ("[z-a]", None),
        ("[user-id]", None),
        ("[user-id]/secret", None),
        ("**/[user-id]/*.env", Some(r"(.*/)?\[user-id\]/[^/]*\.env")),
        ("**/list[].js", Some(r"(.*/)?list\[\]\.js")),
        ("**/x]y", Some(r"(.*/)?x\]y")),
        ("**/o[p[q*", Some(r"(.*/)?o\[p\[q[^/]*")),
        ("**/a[*]c", Some(r"(.*/)?a(\*)c")),
    ];
    for (name, source) in entries {
        let spelled = format!("{TREE}/{name}");
        let e = bracket_entry(&spelled)?;
        let lists = [
            (
                "denyRead",
                "(/.*)?$",
                Lists {
                    deny_read: vec![e.clone()],
                    ..Lists::default()
                }
                .build()?,
            ),
            (
                "credential deny",
                "(/.*)?$",
                Lists {
                    credential_deny_read: vec![e.clone()],
                    ..Lists::default()
                }
                .build()?,
            ),
            (
                "allowRead",
                "$",
                Lists {
                    deny_read: vec![entry(TREE)?],
                    allow_read_within_deny: vec![e.clone()],
                    ..Lists::default()
                }
                .build()?,
            ),
            (
                "denyWrite",
                "(/.*)?$",
                Lists {
                    allow_write: vec![entry(TREE)?],
                    deny_write: vec![e.clone()],
                    ..Lists::default()
                }
                .build()?,
            ),
            (
                "allowWrite",
                "$",
                Lists {
                    allow_write: vec![e.clone()],
                    ..Lists::default()
                }
                .build()?,
            ),
        ];
        for (list, tail, policy) in lists {
            let profile = compose_with(&policy, &EgressPolicy::DenyAll)?;
            let regexes = regex_filters(&profile);
            for regex in &regexes {
                assert!(regex::Regex::new(regex).is_ok(), "{list} {name}: {regex}");
            }
            match source {
                Some(source) => {
                    let expected = format!("^{TREE}/{source}{tail}");
                    assert!(regexes.contains(&expected), "{list} {name}: {expected}\n{regexes:?}");
                }
                None => assert!(
                    profile.contains(&format!("(subpath \"{spelled}\")")),
                    "{list} {name}\n{profile}"
                ),
            }
        }
    }
    Ok(())
}

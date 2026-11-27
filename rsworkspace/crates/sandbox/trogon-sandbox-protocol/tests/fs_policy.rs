use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, GlobPattern, LiteralPath};
use trogon_sandbox_protocol::{CompiledFsEntry, FsAccess, FsPolicyEvaluator};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn lit(raw: &str) -> Result<LiteralPath, Box<dyn std::error::Error>> {
    Ok(LiteralPath::new(raw)?)
}

fn entry_lit(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: lit(raw)?,
        directory: DirectoryMarker::Unmarked,
    })
}

fn entry_glob(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

#[derive(Default)]
struct Lists {
    deny_read: Vec<FsPathEntry>,
    credential_deny_read: Vec<FsPathEntry>,
    allow_read_within_deny: Vec<FsPathEntry>,
    own_allow_within_deny: Vec<FsPathEntry>,
    allow_write: Vec<FsPathEntry>,
    deny_write: Vec<FsPathEntry>,
}

fn evaluator(lists: Lists) -> Result<FsPolicyEvaluator, Box<dyn std::error::Error>> {
    let policy = FsPolicy::new(
        lists.deny_read,
        lists.credential_deny_read,
        lists.allow_read_within_deny,
        lists.own_allow_within_deny,
        lists.allow_write,
        lists.deny_write,
    )?;
    Ok(FsPolicyEvaluator::new(&policy))
}

fn read(evaluator: &FsPolicyEvaluator, path: &str) -> Result<FsAccess, Box<dyn std::error::Error>> {
    Ok(evaluator.read_access(&lit(path)?))
}

fn write(evaluator: &FsPolicyEvaluator, path: &str) -> Result<FsAccess, Box<dyn std::error::Error>> {
    Ok(evaluator.write_access(&lit(path)?))
}

#[test]
fn literal_and_glob_cover_differently() -> TestResult {
    let literal = CompiledFsEntry::new(&entry_lit("/data/*")?);
    let glob = CompiledFsEntry::new(&entry_glob("/data/*")?);
    assert!(literal.covers(&lit("/data/*")?));
    assert!(!literal.covers(&lit("/data/a")?));
    assert!(glob.covers(&lit("/data/a")?));
    assert!(glob.covers(&lit("/data/a/deep/file")?));
    assert!(!glob.covers(&lit("/other/a")?));
    Ok(())
}

#[test]
fn backslash_is_a_name_character_to_the_policy() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_lit(r"/w/a\b")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, r"/w/a\b")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/w/a/b")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn an_own_allow_lifts_a_credential_deny_for_the_library_file_only() -> TestResult {
    let policy = evaluator(Lists {
        credential_deny_read: vec![entry_lit("/home/u")?],
        own_allow_within_deny: vec![entry_lit("/home/u/ca.pem")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/home/u/ca.pem")?, FsAccess::Allowed);
    assert_eq!(read(&policy, "/home/u/.netrc")?, FsAccess::Denied);
    Ok(())
}

#[test]
fn a_credential_deny_is_not_lifted_by_an_allow_read() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_lit("/home/a")?],
        credential_deny_read: vec![entry_lit("/home/.netrc")?, entry_glob("/home/*.pem")?],
        allow_read_within_deny: vec![
            entry_lit("/home/a")?,
            entry_lit("/home/.netrc")?,
            entry_glob("/home/*.pem")?,
        ],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/home/a")?, FsAccess::Allowed);
    assert_eq!(read(&policy, "/home/.netrc")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/home/k.pem")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/home/b")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn empty_policy_allows_reads_and_denies_writes() -> TestResult {
    let policy = evaluator(Lists::default())?;
    assert_eq!(read(&policy, "/etc/passwd")?, FsAccess::Allowed);
    assert_eq!(write(&policy, "/work/x")?, FsAccess::Denied);
    Ok(())
}

#[test]
fn allow_read_within_deny_beats_deny_read() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_lit("/home")?],
        allow_read_within_deny: vec![entry_lit("/home/user/project")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/home/other")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/home/user/.ssh")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/home/user/project/src/main.rs")?, FsAccess::Allowed);
    assert_eq!(read(&policy, "/srv/data")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn a_literal_deny_inside_a_literal_allow_stays_denied() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_lit("/home/user/project/secrets")?],
        allow_read_within_deny: vec![entry_lit("/home/user")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/home/user/project/secrets/key")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/home/user/project/main.rs")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn a_literal_allow_inside_a_literal_deny_still_reopens_it() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_lit("/home")?],
        allow_read_within_deny: vec![entry_lit("/home/user")?, entry_lit("/home")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/home/x")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn a_leaf_glob_deny_survives_a_directory_allow() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_glob("/**/.env")?],
        allow_read_within_deny: vec![entry_lit("/work")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/work/.env")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/work/a/b/.env")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/work/a/main.rs")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn a_region_glob_deny_is_carved_out_by_the_allows_it_covers() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_glob("/home/**/*")?],
        allow_read_within_deny: vec![entry_lit("/home/u/project")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/home/u/project/a")?, FsAccess::Allowed);
    assert_eq!(read(&policy, "/home/u/other")?, FsAccess::Denied);
    Ok(())
}

#[test]
fn a_glob_allow_re_opens_a_glob_deny_only_when_the_deny_covers_its_sample() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_glob("/home/*/project")?],
        allow_read_within_deny: vec![entry_glob("/home/**")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/home/u/project/x")?, FsAccess::Denied);
    let covering = evaluator(Lists {
        deny_read: vec![entry_glob("/home/**")?],
        allow_read_within_deny: vec![entry_glob("/home/*/project")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&covering, "/home/u/project/x")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn a_glob_deny_is_opened_by_the_allows_it_covers_at_the_sample_path() -> TestResult {
    let deny = entry_glob("/h/*/f?")?;
    let policy = evaluator(Lists {
        deny_read: vec![deny.clone()],
        credential_deny_read: vec![deny.clone()],
        allow_read_within_deny: vec![
            entry_glob("/h/u/f?*")?,
            entry_glob("/h/u/g?*")?,
            entry_lit("/h/v/fa")?,
            entry_lit("/h/v/fab")?,
        ],
        own_allow_within_deny: vec![entry_lit("/h/v/fa")?, entry_lit("/elsewhere")?],
        ..Lists::default()
    })?;
    assert_eq!(policy.read_exceptions(&deny), vec![0, 2]);
    assert_eq!(policy.credential_read_exceptions(&deny), vec![0]);
    Ok(())
}

#[test]
fn a_literal_deny_is_reopened_by_every_allow_unless_nested_in_an_allowed_literal() -> TestResult {
    let policy = evaluator(Lists {
        allow_read_within_deny: vec![entry_lit("/a")?, entry_glob("/b/*")?],
        ..Lists::default()
    })?;
    assert_eq!(policy.read_exceptions(&entry_lit("/c")?), vec![0, 1]);
    assert_eq!(policy.read_exceptions(&entry_lit("/a/inner")?), Vec::<usize>::new());
    Ok(())
}

#[test]
fn allow_read_outside_every_deny_changes_nothing() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_lit("/secret")?],
        allow_read_within_deny: vec![entry_lit("/elsewhere")?],
        ..Lists::default()
    })?;
    assert_eq!(read(&policy, "/secret/x")?, FsAccess::Denied);
    assert_eq!(read(&policy, "/elsewhere/x")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn deny_write_beats_allow_write() -> TestResult {
    let policy = evaluator(Lists {
        allow_write: vec![entry_lit("/work")?],
        deny_write: vec![entry_lit("/work/.git/hooks")?, entry_glob("/work/**/*.pem")?],
        ..Lists::default()
    })?;
    assert_eq!(write(&policy, "/work/src/a.rs")?, FsAccess::Allowed);
    assert_eq!(write(&policy, "/work/.git/hooks/pre-commit")?, FsAccess::Denied);
    assert_eq!(write(&policy, "/work/a/b/key.pem")?, FsAccess::Denied);
    assert_eq!(write(&policy, "/other")?, FsAccess::Denied);
    Ok(())
}

#[test]
fn read_and_write_lists_are_independent() -> TestResult {
    let policy = evaluator(Lists {
        deny_read: vec![entry_lit("/work/private")?],
        allow_write: vec![entry_lit("/work")?],
        deny_write: vec![entry_lit("/work/frozen")?],
        ..Lists::default()
    })?;
    assert_eq!(write(&policy, "/work/private/x")?, FsAccess::Allowed);
    assert_eq!(read(&policy, "/work/frozen/x")?, FsAccess::Allowed);
    Ok(())
}

#[test]
fn an_entry_exposes_the_sample_that_stands_for_it() -> TestResult {
    assert_eq!(
        CompiledFsEntry::new(&entry_lit("/a/b")?).sample_path(),
        Some(&lit("/a/b")?)
    );
    assert_eq!(
        CompiledFsEntry::new(&entry_glob("/a/*/b")?).sample_path(),
        Some(&lit("/a/x/b")?)
    );
    Ok(())
}

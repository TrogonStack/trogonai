use trogon_sandbox_windows_fence::{
    CallerReads, GrantEntry, ProfileGrantCounts, ProfileGrantPath, ProfileGrantPathKind, ProfileReadState,
    ProfileReadVar, WindowsSandboxErrorCode, parse_profile_read_status,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CURRENT: &str = r#"{
  "state": "current", "record_version": 1, "profile": "C:\\Users\\me",
  "capabilities": ["a", "b"],
  "exclusions": ["C:\\Users\\me\\.ssh"], "ungranted_only": [],
  "not_granted": [{"path": "C:\\x", "reason": "owned"}],
  "stale_allow": [{"path": "C:\\y", "reason": "gone", "entry": "inherited"}],
  "replaced_files": ["C:\\z"], "extra_exclusions": [".cfg"],
  "containers": 2, "granted": 3, "files": 4, "denied": 5,
  "env": {"NPM_CONFIG_USERCONFIG": "C:\\n", "CARGO_HOME": "", "OTHER": "x",
          "GIT_CONFIG_GLOBAL": "C:\\g"},
  "paths": [{"path": "C:\\p", "kind": "granted"}]
}"#;

#[test]
fn a_current_status_with_unreadable_lists_is_outdated() -> TestResult {
    let status = parse_profile_read_status(r#"{"state":"current","record_version":2}"#)?;
    assert_eq!(status.state, ProfileReadState::Outdated);
    let status = parse_profile_read_status(
        r#"{"state":"current","record_version":1,"exclusions":["C:\\a"],"ungranted_only":[]}"#,
    )?;
    assert_eq!(status.state, ProfileReadState::Current);
    assert_eq!(status.exclusions.exclusions, vec![r"C:\a".to_string()]);
    assert!(status.unusable().is_none());
    Ok(())
}

#[test]
fn a_state_not_known_is_unusable() -> TestResult {
    let status = parse_profile_read_status(r#"{"state":"weird"}"#)?;
    assert_eq!(status.state, ProfileReadState::Unknown);
    let error = status.unusable().ok_or("expected an error")?;
    assert_eq!(error.code(), WindowsSandboxErrorCode::ProfileGrantOutdated);
    let none = parse_profile_read_status(r#"{"state":"none"}"#)?;
    assert!(none.unusable().is_none());
    Ok(())
}

#[test]
fn the_status_carries_what_the_last_grant_left() -> TestResult {
    let status = parse_profile_read_status(CURRENT)?;
    assert_eq!(status.state, ProfileReadState::Current);
    assert_eq!(status.profile.as_deref(), Some(r"C:\Users\me"));
    assert_eq!(status.capabilities, ["a", "b"]);
    assert_eq!(status.not_granted.len(), 1);
    assert_eq!(status.not_granted[0].reason, "owned");
    assert_eq!(status.stale_allow[0].entry, Some(GrantEntry::Inherited));
    assert_eq!(status.replaced_files, [r"C:\z"]);
    assert_eq!(status.extra_exclusions, [".cfg"]);
    assert_eq!(
        status.counts,
        ProfileGrantCounts {
            containers: 2,
            granted: 3,
            files: 4,
            denied: 5
        }
    );
    assert_eq!(
        status.paths,
        Some(vec![ProfileGrantPath {
            path: r"C:\p".to_string(),
            kind: ProfileGrantPathKind::Granted
        }])
    );
    Ok(())
}

#[test]
fn only_known_variables_of_a_current_grant_are_offered() -> TestResult {
    let status = parse_profile_read_status(CURRENT)?;
    assert_eq!(status.env.len(), 2);
    assert_eq!(
        status.read_env(["git_config_global"]),
        vec![(ProfileReadVar::NpmConfigUserconfig, r"C:\n".to_string())]
    );
    let outdated = parse_profile_read_status(&CURRENT.replace("\"current\"", "\"outdated\""))?;
    assert!(outdated.env.is_empty());
    assert!(outdated.read_env([]).is_empty());
    assert_eq!(outdated.paths.as_ref().map(Vec::len), Some(1));
    Ok(())
}

#[test]
fn output_that_is_not_json_is_an_error() -> TestResult {
    let Err(error) = parse_profile_read_status("not json") else {
        return Err("expected an error".into());
    };
    assert_eq!(error.code(), WindowsSandboxErrorCode::FenceHelperBadJson);
    Ok(())
}

#[test]
fn every_state_name_and_each_unreadable_list_is_reported() -> TestResult {
    let incomplete = parse_profile_read_status(r#"{"state":"incomplete","reason":"half done"}"#)?;
    assert_eq!(incomplete.state, ProfileReadState::Incomplete);
    let error = incomplete.unusable().ok_or("expected an error")?;
    assert!(error.message().contains("half done"));
    let outdated = parse_profile_read_status(r#"{"state":"outdated"}"#)?;
    assert_eq!(outdated.state, ProfileReadState::Outdated);
    let missing = parse_profile_read_status(r#"{"state":"current"}"#)?;
    assert_eq!(missing.state, ProfileReadState::Outdated);
    assert!(
        missing
            .reason
            .as_deref()
            .ok_or("expected a reason")?
            .contains("undefined")
    );
    let no_exclusions = parse_profile_read_status(r#"{"state":"current","record_version":1,"exclusions":[]}"#)?;
    assert!(
        no_exclusions
            .reason
            .as_deref()
            .ok_or("expected a reason")?
            .contains("excluded paths")
    );
    let no_ungranted = parse_profile_read_status(r#"{"state":"current","record_version":1,"exclusions":["C:\\a"]}"#)?;
    assert!(
        no_ungranted
            .reason
            .as_deref()
            .ok_or("expected a reason")?
            .contains("ungranted directories")
    );
    Ok(())
}

#[test]
fn every_path_kind_is_read() -> TestResult {
    let status = parse_profile_read_status(
        r#"{"state":"none","paths":[{"path":"a","kind":"container"},{"path":"b","kind":"file"},{"path":"c","kind":"denied"}]}"#,
    )?;
    let kinds: Vec<ProfileGrantPathKind> = status.paths.ok_or("expected paths")?.iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        [
            ProfileGrantPathKind::Container,
            ProfileGrantPathKind::File,
            ProfileGrantPathKind::Denied
        ]
    );
    Ok(())
}

#[test]
fn a_skipped_path_says_whether_the_caller_reads_it() -> TestResult {
    let status = parse_profile_read_status(
        r#"{"state":"none","not_granted":[{"path":"C:\\a","caller_reads":true},{"path":"C:\\b","caller_reads":false},{"path":"C:\\c","caller_reads":"unknown"},{"path":"C:\\d","caller_reads":3}]}"#,
    )?;
    let reads: Vec<_> = status.not_granted.iter().map(|item| item.caller_reads).collect();
    assert_eq!(
        reads,
        [
            Some(CallerReads::Yes),
            Some(CallerReads::No),
            Some(CallerReads::Unknown),
            None
        ]
    );
    Ok(())
}

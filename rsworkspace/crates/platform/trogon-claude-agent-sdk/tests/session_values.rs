use std::path::Path;
#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
use trogon_claude_agent_sdk::{
    SessionEntry, SessionEntryError, SessionId, SessionKey, SessionKeyError, SessionProjectKey, SessionSubpath,
    SessionSubpathError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn entry(text: &str) -> Result<SessionEntry, Box<dyn std::error::Error>> {
    Ok(SessionEntry::try_from(serde_json::from_str::<serde_json::Value>(
        text,
    )?)?)
}

fn key(session: &str) -> Result<SessionKey, Box<dyn std::error::Error>> {
    Ok(SessionKey::new(
        SessionProjectKey::new("project")?,
        SessionId::from(session),
    )?)
}

#[test]
fn entries_expose_their_opaque_object_and_known_fields() -> TestResult {
    let value = entry(r#"{"type":"assistant","uuid":"entry-1","isSidechain":true,"extra":[1,2]}"#)?;
    assert_eq!(value.kind(), "assistant");
    assert_eq!(value.uuid(), Some("entry-1"));
    assert!(value.is_sidechain());
    assert_eq!(value.as_object().len(), 4);
    assert_eq!(format!("{value:?}"), r#"SessionEntry { kind: "assistant", .. }"#);
    let object = value.clone().into_object();
    assert_eq!(SessionEntry::from(object.clone()), value);
    assert_eq!(SessionEntry::new(object), value);
    Ok(())
}

#[test]
fn entries_default_missing_or_mistyped_fields() -> TestResult {
    for text in [r#"{}"#, r#"{"type":1,"uuid":2,"isSidechain":"yes"}"#] {
        let value = entry(text)?;
        assert_eq!(value.kind(), "");
        assert_eq!(value.uuid(), None);
        assert!(!value.is_sidechain());
    }
    Ok(())
}

#[test]
fn entries_reject_values_that_are_not_objects() -> TestResult {
    for text in ["[]", "1", "\"text\"", "null", "true"] {
        let value: serde_json::Value = serde_json::from_str(text)?;
        let error = SessionEntry::try_from(value).err().ok_or("accepted a non-object")?;
        assert!(matches!(error, SessionEntryError::NotObject));
        assert_eq!(error.to_string(), "session entry must be a JSON object");
    }
    Ok(())
}

#[test]
fn session_keys_accept_safe_identifiers_and_expose_their_parts() -> TestResult {
    let main = key("123e4567-e89b-42d3-a456-426614174000")?;
    assert_eq!(main.project_key().as_str(), "project");
    assert_eq!(main.session_id().as_str(), "123e4567-e89b-42d3-a456-426614174000");
    assert!(main.subpath().is_none());
    let nested = main.clone().with_subpath(SessionSubpath::new("subagents/agent-1")?);
    assert_eq!(nested.subpath().map(SessionSubpath::as_str), Some("subagents/agent-1"));
    assert_ne!(nested, main);
    assert_eq!(nested.main(), main);
    Ok(())
}

#[test]
fn session_keys_reject_unsafe_identifiers() -> TestResult {
    for value in [
        "", ".", "..", "a/b", "a\\b", "a:b", "a<b", "a>b", "a\"b", "a|b", "a?b", "a*b", "a.", "a ", "a\nb", "con",
        "PRN", "Aux.log", "nul", "COM1", "com9.txt", "LPT1", "lpt9",
    ] {
        let error = SessionKey::new(SessionProjectKey::new("project")?, SessionId::from(value))
            .err()
            .ok_or("accepted an unsafe session id")?;
        assert!(matches!(error, SessionKeyError::UnsafeSessionId), "{value:?}");
        assert_eq!(error.to_string(), "session id is not a safe file component");
    }
    Ok(())
}

#[test]
fn subpaths_stay_inside_the_session_directory() -> TestResult {
    for value in ["a", "a/b", "subagents/agent-1", "a\\b", "weird name", ".hidden"] {
        let subpath = SessionSubpath::new(value)?;
        assert_eq!(subpath.as_str(), value);
        assert_eq!(subpath.to_string(), value);
    }
    for value in ["", "/a", "\\a", "a/../b", "..", ".", "a/./b", "a\0b", "C:evil", "z:/x"] {
        let error = SessionSubpath::new(value).err().ok_or("accepted an unsafe subpath")?;
        assert_eq!(error, SessionSubpathError);
        assert_eq!(
            error.to_string(),
            "session subpath must be a relative path that stays inside the session directory"
        );
    }
    Ok(())
}

#[test]
fn transcript_paths_map_to_main_and_subagent_keys() -> TestResult {
    let root = Path::new("/home/user/.claude/projects");
    let main = SessionKey::from_transcript_path(&root.join("proj/abc.jsonl"), root)?;
    assert_eq!(main.project_key().as_str(), "proj");
    assert_eq!(main.session_id().as_str(), "abc");
    assert!(main.subpath().is_none());

    let sub = SessionKey::from_transcript_path(&root.join("proj/abc/subagents/agent-1.jsonl"), root)?;
    assert_eq!(sub.session_id().as_str(), "abc");
    assert_eq!(sub.subpath().map(SessionSubpath::as_str), Some("subagents/agent-1"));

    let bare = SessionKey::from_transcript_path(&root.join("proj/abc/subagents/agent-2"), root)?;
    assert_eq!(bare.subpath().map(SessionSubpath::as_str), Some("subagents/agent-2"));
    Ok(())
}

#[test]
fn transcript_paths_outside_the_projects_directory_are_rejected() {
    let root = Path::new("/home/user/.claude/projects");
    for path in [
        "/elsewhere/proj/abc.jsonl",
        "relative/proj/abc.jsonl",
        "/home/user/.claude/projects/proj/../abc.jsonl",
        "/home/user/.claude/projects",
    ] {
        let result = SessionKey::from_transcript_path(Path::new(path), root);
        assert!(
            matches!(
                result,
                Err(SessionKeyError::OutsideProjects) | Err(SessionKeyError::UnrecognizedPath)
            ),
            "{path}"
        );
    }
    let relative_root = SessionKey::from_transcript_path(Path::new("/a/b/c.jsonl"), Path::new("a"));
    assert!(matches!(relative_root, Err(SessionKeyError::OutsideProjects)));
}

#[test]
fn unrecognized_or_unsafe_transcript_paths_report_typed_errors() -> TestResult {
    let root = Path::new("/home/user/.claude/projects");
    let no_extension = SessionKey::from_transcript_path(&root.join("proj/abc.txt"), root);
    assert!(matches!(no_extension, Err(SessionKeyError::UnrecognizedPath)));
    let too_short = SessionKey::from_transcript_path(&root.join("proj/abc/only.jsonl"), root);
    assert!(matches!(too_short, Err(SessionKeyError::UnrecognizedPath)));
    let unsafe_project = SessionKey::from_transcript_path(&root.join("con/abc.jsonl"), root);
    assert!(matches!(unsafe_project, Err(SessionKeyError::Project(_))));
    let unsafe_session = SessionKey::from_transcript_path(&root.join("proj/con.jsonl"), root);
    assert!(matches!(unsafe_session, Err(SessionKeyError::UnsafeSessionId)));
    let unsafe_subpath = SessionKey::from_transcript_path(&root.join("proj/abc/a/\\..\\x.jsonl"), root);
    assert!(matches!(unsafe_subpath, Err(SessionKeyError::Subpath(_))));
    for error in [
        SessionKeyError::UnsafeSessionId,
        SessionKeyError::OutsideProjects,
        SessionKeyError::UnrecognizedPath,
        SessionKeyError::NotUnicode,
    ] {
        assert!(!error.to_string().is_empty());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_unicode_transcript_paths_are_rejected() {
    let root = Path::new("/projects");
    let path = Path::new(OsStr::from_bytes(b"/projects/proj/\xff.jsonl"));
    assert!(matches!(
        SessionKey::from_transcript_path(path, root),
        Err(SessionKeyError::NotUnicode)
    ));
}

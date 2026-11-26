#![cfg(unix)]

use std::path::Path;

use serde_json::Value;
use trogon_claude_agent_sdk::{SessionKey, SessionSubpath};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Goldens captured from the real TypeScript `sdk.mjs` and Python 0.2.181 transcript path functions; both returned
/// the same key for every row. `$CWD` stands for the working directory.
const GOLDENS: &str = include_str!("fixtures/session_key/transcript_paths.json");

#[test]
fn transcript_path_keys_match_the_typescript_and_python_sdks() -> TestResult {
    let cwd = std::env::current_dir()?;
    let cwd = cwd.to_str().ok_or("the working directory is not Unicode")?;
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let cases = goldens["cases"].as_array().ok_or("missing cases")?;
    let mut mapped = 0;
    for case in cases {
        let file = case["file_path"]
            .as_str()
            .ok_or("missing file path")?
            .replace("$CWD", cwd);
        let directory = case["projects_dir"]
            .as_str()
            .ok_or("missing projects directory")?
            .replace("$CWD", cwd);
        let derived = SessionKey::from_transcript_path(Path::new(&file), Path::new(&directory));
        let expected = &case["key"];
        if expected.is_null() {
            assert!(derived.is_err(), "{file:?} under {directory:?} should have no key");
            continue;
        }
        let key = derived.map_err(|error| format!("{file:?} under {directory:?}: {error}"))?;
        assert_eq!(
            key.project_key().as_str(),
            expected["project_key"].as_str().ok_or("missing project key")?,
            "{file:?} under {directory:?}"
        );
        assert_eq!(
            key.session_id().as_str(),
            expected["session_id"].as_str().ok_or("missing session id")?,
            "{file:?} under {directory:?}"
        );
        assert_eq!(
            key.subpath().map(SessionSubpath::as_str),
            expected["subpath"].as_str(),
            "{file:?} under {directory:?}"
        );
        mapped += 1;
    }
    assert!(mapped > 50, "the table should map many paths");
    assert!(cases.len() > mapped, "the table should also reject paths");
    Ok(())
}

#[test]
fn relative_paths_resolve_against_the_working_directory_like_absolute_ones() -> TestResult {
    let cwd = std::env::current_dir()?;
    let relative = SessionKey::from_transcript_path(Path::new("projects/proj/session.jsonl"), Path::new("projects"))?;
    let absolute = SessionKey::from_transcript_path(&cwd.join("projects/proj/session.jsonl"), &cwd.join("projects"))?;
    assert_eq!(relative, absolute);
    let mixed = SessionKey::from_transcript_path(&cwd.join("projects/proj/session.jsonl"), Path::new("projects"))?;
    assert_eq!(mixed, absolute);
    Ok(())
}

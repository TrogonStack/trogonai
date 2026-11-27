use serde_json::{Value, json};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn workspace(change: impl FnOnce(&mut Value)) -> Result<WorkspaceSource, serde_json::Error> {
    let mut clone = json!({
        "url": "https://git.example.com/org/repo.git",
        "sha": "0123456789abcdef",
        "base_sha": null,
        "auth": {"none": {}}
    });
    change(&mut clone);
    serde_json::from_value(json!({ "clone": clone }))
}

fn refused(source: &WorkspaceSource) -> bool {
    matches!(CloneTarget::parse(source), Err(LeaseError::InvalidSpec { .. }))
}

#[test]
fn an_empty_workspace_has_nothing_to_clone() -> TestResult {
    assert_eq!(CloneTarget::parse(&WorkspaceSource::empty())?, None);
    Ok(())
}

#[test]
fn an_anonymous_clone_carries_no_credentials() -> TestResult {
    let target = CloneTarget::parse(&workspace(|_| {})?)?;
    assert_eq!(
        target,
        Some(CloneTarget {
            url: "https://git.example.com/org/repo.git".to_owned(),
            commit: "0123456789abcdef".to_owned(),
            credentials: None,
        })
    );
    Ok(())
}

#[test]
fn a_literal_token_becomes_the_password() -> TestResult {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let source =
        workspace(|clone| clone["auth"] = json!({"bearer_token": {"token_ref": {"literal": {"value": token}}}}))?;
    let credentials = CloneTarget::parse(&source)?.and_then(|target| target.credentials);
    assert_eq!(credentials, Some(("oauth2".to_owned(), token)));
    Ok(())
}

#[test]
fn what_cannot_be_cloned_is_refused() -> TestResult {
    let cases = [
        json!({"url": "http://git.example.com/org/repo.git"}),
        json!({"url": "not a url"}),
        json!({"url": "https://user@git.example.com/org/repo.git"}),
        json!({"url": "https://git.example.com/org/repo.git?x=1"}),
        json!({"sha": "main"}),
        json!({"sha": "abc"}),
        json!({"sha": "a".repeat(65)}),
        json!({"auth": {"bearer_token": {"token_ref": {"literal": {"value": ""}}}}}),
        json!({"auth": {"bearer_token": {"token_ref": {"nats_kv": {"bucket": "b", "key": "k"}}}}}),
        json!({"auth": {"basic": {"username": "u", "password_ref": {"literal": {"value": "p"}}}}}),
    ];
    for case in cases {
        let source = workspace(|clone| {
            if let (Some(clone), Some(case)) = (clone.as_object_mut(), case.as_object()) {
                clone.extend(case.clone());
            }
        })?;
        assert!(refused(&source), "{case}");
    }
    Ok(())
}

#[test]
fn a_host_path_and_an_s3_volume_are_refused() -> TestResult {
    let host: WorkspaceSource = serde_json::from_value(json!({"host_mount": {"path": "/srv"}}))?;
    assert!(refused(&host));
    let s3: WorkspaceSource =
        serde_json::from_value(json!({"s3_volume": {"bucket": "b", "prefix": "p", "mode": "read_write"}}))?;
    assert!(refused(&s3));
    Ok(())
}

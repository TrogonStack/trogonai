use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request(password: Option<String>) -> GitCloneRequest {
    GitCloneRequest {
        url: "https://git.example.com/org/repo.git".to_owned(),
        path: "/workspace".to_owned(),
        commit_id: Some("abc1234".to_owned()),
        username: password.as_ref().map(|_| "oauth2".to_owned()),
        password,
    }
}

#[test]
fn debug_never_shows_the_password() {
    let password = uuid::Uuid::new_v4().simple().to_string();
    let shown = format!("{:?}", request(Some(password.clone())));
    assert!(!shown.contains(&password));
    assert!(shown.contains("[REDACTED]"));
}

#[test]
fn absent_credentials_are_not_serialized() -> TestResult {
    let body = serde_json::to_value(request(None))?;
    assert!(body.get("username").is_none() && body.get("password").is_none());
    assert_eq!(body["commit_id"], "abc1234");
    Ok(())
}

#[test]
fn credentials_serialize_in_their_own_fields() -> TestResult {
    let body = serde_json::to_value(request(Some("value".to_owned())))?;
    assert_eq!(body["username"], "oauth2");
    assert_eq!(body["password"], "value");
    Ok(())
}

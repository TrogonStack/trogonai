use super::*;

#[test]
fn valid_secret() {
    let secret = SecretString::new("my-secret").unwrap();
    assert_eq!(secret.as_str(), "my-secret");
}

#[test]
fn empty_secret_rejected() {
    assert!(matches!(SecretString::new(""), Err(EmptySecretError)));
}

#[test]
fn debug_redacts_value() {
    let secret = SecretString::new("super-secret").unwrap();
    assert_eq!(format!("{secret:?}"), "SecretString(***)");
}

#[test]
fn clone_shares_arc() {
    let a = SecretString::new("secret").unwrap();
    let b = a.clone();
    assert_eq!(a.as_str(), b.as_str());
}

#[test]
fn error_display() {
    assert_eq!(EmptySecretError.to_string(), "secret must not be empty");
}

#[test]
fn parses_from_str() {
    assert_eq!("my-secret".parse::<SecretString>().unwrap().as_str(), "my-secret");
    assert_eq!("".parse::<SecretString>().unwrap_err(), EmptySecretError);
}

#[test]
fn deserializes_from_a_string() {
    let secret: SecretString = serde_json::from_str(r#""my-secret""#).unwrap();
    assert_eq!(secret.as_str(), "my-secret");

    let err = serde_json::from_str::<SecretString>(r#""""#).unwrap_err();
    assert!(err.to_string().contains("secret must not be empty"));
}

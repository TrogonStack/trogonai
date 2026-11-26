use oci_client::secrets::RegistryAuth;

use super::apply_registry_auth;

fn authorization(auth: &RegistryAuth) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let request = reqwest::Client::new().delete("http://localhost/v2/repo/manifests/x");
    let built = apply_registry_auth(request, auth).build()?;
    Ok(built
        .headers()
        .get("authorization")
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()?)
}

#[test]
fn anonymous_access_sends_no_authorization() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(authorization(&RegistryAuth::Anonymous)?, None);
    Ok(())
}

#[test]
fn basic_credentials_are_sent_as_basic_authorization() -> Result<(), Box<dyn std::error::Error>> {
    let header = authorization(&RegistryAuth::Basic("user".into(), "pass".into()))?;
    assert_eq!(header.as_deref(), Some("Basic dXNlcjpwYXNz"));
    Ok(())
}

#[test]
fn bearer_tokens_are_sent_as_bearer_authorization() -> Result<(), Box<dyn std::error::Error>> {
    let header = authorization(&RegistryAuth::Bearer("tok".into()))?;
    assert_eq!(header.as_deref(), Some("Bearer tok"));
    Ok(())
}

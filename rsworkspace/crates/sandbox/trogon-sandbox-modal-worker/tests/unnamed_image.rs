mod support;

use support::*;

/// The contract refuses an image with no repository when the spec is built, so such a lease never reaches
/// the provider and Modal is never called.
#[test]
fn a_lease_whose_image_names_no_repository_is_refused_before_modal_is_called() -> TestResult {
    let env = Env::start()?;
    let refused = request_with(|wire| {
        wire["spec"]["image"]["reference"] = serde_json::json!({"registry": "", "repository": ""});
    });
    let error = refused.err().ok_or("the request parsed")?;
    assert!(error.to_string().contains("image must not be empty"));
    assert!(env.api.with(|state| state.created.is_empty()));
    Ok(())
}

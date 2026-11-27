use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_undecodable_request_is_denied_with_the_reason() -> TestResult {
    let error = serde_json::from_str::<NativeSnapshotDeleteRequest>("{}")
        .err()
        .ok_or("decoded")?;
    assert!(matches!(
        invalid_request(&error),
        NativeSnapshotDeleteResponse::Denied(LeaseError::Backend { message })
            if message.starts_with("invalid native snapshot delete request: ")
    ));
    Ok(())
}

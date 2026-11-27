use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_adapter_failure_keeps_the_adapters_message() {
    let error = E2bWorkerError::from(E2bProviderError::TemplateRequired);
    assert!(matches!(&error, E2bWorkerError::Provider(_)));
    assert!(matches!(
        &error,
        E2bWorkerError::Provider(E2bProviderError::TemplateRequired)
    ));
    let error = E2bWorkerError::from(E2bDispatchError::NotConfigured);
    assert!(matches!(&error, E2bWorkerError::Dispatch(_)));
}

#[test]
fn an_encode_failure_names_the_encoding() -> TestResult {
    let source = serde_json::from_str::<u8>("x").err().ok_or("parsed")?;
    assert!(matches!(E2bWorkerError::from(source), E2bWorkerError::Encode(_)));
    Ok(())
}

#[test]
fn an_undecodable_request_names_what_it_was() -> TestResult {
    let source = serde_json::from_str::<u8>("x").err().ok_or("parsed")?;
    let error = E2bWorkerError::Decode {
        what: "invalid thing",
        source,
    };
    assert!(matches!(
        error,
        E2bWorkerError::Decode {
            what: "invalid thing",
            ..
        }
    ));
    Ok(())
}

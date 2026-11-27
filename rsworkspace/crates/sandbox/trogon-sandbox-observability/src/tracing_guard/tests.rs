use super::*;

#[test]
fn drop_survives_a_provider_that_was_already_shut_down() -> Result<(), Box<dyn std::error::Error>> {
    let _subscriber = tracing::subscriber::set_default(tracing_subscriber::fmt().with_writer(std::io::sink).finish());
    let provider = SdkTracerProvider::builder().build();
    provider.shutdown()?;
    let guard = TracingGuard::new(Some(provider));
    assert!(guard.exports_spans());
    drop(guard);
    Ok(())
}

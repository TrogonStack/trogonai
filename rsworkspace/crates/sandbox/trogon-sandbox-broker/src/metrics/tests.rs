use super::*;
use trogon_sandbox_contract::ProviderKind;

#[test]
fn emits_all_families() -> Result<(), Box<dyn std::error::Error>> {
    let metrics = BrokerMetrics::new()?;
    metrics.record_dispatch("tenant-a", ProviderKind::Mock, "granted");
    metrics.inc_active_leases("tenant-a");
    metrics.locality_hit_total.with_label_values(&["mock"]).inc();
    metrics.digest_pin_mismatch_total.inc();

    let text = metrics.gather_text()?;
    assert!(text.contains("sandbox_broker_lease_dispatch_total"));
    assert!(text.contains("sandbox_broker_active_leases"));
    assert!(text.contains("sandbox_broker_locality_hit_total"));
    assert!(text.contains("sandbox_broker_digest_pin_mismatch_total"));
    Ok(())
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the test asserts the operator-facing message text itself"
    )
)]
fn the_error_names_the_registration_failure() {
    let error = BrokerMetricsError::from(prometheus::Error::Msg("duplicate".to_string()));
    assert!(error.to_string().contains("duplicate"));
}

//! E2B fixes cpu, memory and disk in the template, so the advertised profile carries no size range. Admission
//! refuses a request that names a size against a profile without a range (the broker's
//! `disk_request_is_refused_when_no_worker_supports_disk_quotas` covers that rule).

use trogon_sandbox_contract::{CapabilitySupport, ResourceCapability};
use trogon_sandbox_e2b_worker::PROFILE;

#[test]
fn a_sized_request_is_unsatisfiable_because_e2b_sizes_in_the_template() {
    assert_eq!(PROFILE.resources.vcpu_millis, None);
    assert_eq!(PROFILE.resources.disk_mib, None);
}

#[test]
fn the_profile_keeps_the_default_resource_capability() {
    assert_eq!(PROFILE.resources, ResourceCapability::default());
    assert_eq!(PROFILE.resources.reports_usage_gauges, CapabilitySupport::Unsupported);
}

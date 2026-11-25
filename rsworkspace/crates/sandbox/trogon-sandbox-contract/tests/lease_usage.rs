use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    AccountLimits, GetAccountLimitsRequest, GetLeaseUsageRequest, GetTenantUsageRequest, LeaseCounters, LeaseId,
    LeaseStatus, LeaseUsage, ResourceGauges, ResourceUsageRequest, TagLimits, TenantId, TenantUsage,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn time(secs: u64) -> serde_json::Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

#[test]
fn lease_statuses_use_snake_case_names() -> TestResult {
    for (status, name) in [
        (LeaseStatus::Pending, "pending"),
        (LeaseStatus::Running, "running"),
        (LeaseStatus::Paused, "paused"),
        (LeaseStatus::Suspending, "suspending"),
        (LeaseStatus::Suspended, "suspended"),
        (LeaseStatus::Waking, "waking"),
        (LeaseStatus::Cancelling, "cancelling"),
        (LeaseStatus::Released, "released"),
        (LeaseStatus::Failed, "failed"),
    ] {
        assert_eq!(serde_json::to_value(status)?, json!(name));
        assert_eq!(serde_json::from_value::<LeaseStatus>(json!(name))?, status);
    }
    assert!(serde_json::from_value::<LeaseStatus>(json!("stopped")).is_err());
    Ok(())
}

#[test]
fn lease_counters_default_every_field_and_omit_idle_bookkeeping() -> TestResult {
    let empty: LeaseCounters = serde_json::from_value(json!({}))?;
    assert_eq!(empty, LeaseCounters::default());
    assert_eq!(
        serde_json::to_value(&empty)?,
        json!({
            "commands_run": 0,
            "command_output_bytes": 0,
            "file_bytes_in": 0,
            "file_bytes_out": 0,
            "paused_seconds": 0,
            "suspended_seconds": 0,
        })
    );

    let suspended = LeaseCounters {
        suspended_at: Some(at(10)),
        suspend_artifact_bytes: Some(4_096),
        paused_at: Some(at(5)),
        ..LeaseCounters::default()
    };
    let wire = serde_json::to_value(&suspended)?;
    assert_eq!(wire["paused_at"], time(5));
    assert_eq!(wire["suspended_at"], time(10));
    assert_eq!(wire["suspend_artifact_bytes"], json!(4_096));
    assert_eq!(serde_json::from_value::<LeaseCounters>(wire)?, suspended);
    Ok(())
}

#[test]
fn lease_counters_add_saturates_and_keeps_idle_bookkeeping() {
    let mut counters = LeaseCounters {
        commands_run: u64::MAX - 1,
        command_output_bytes: 10,
        file_bytes_in: 20,
        file_bytes_out: 30,
        paused_seconds: 40,
        paused_at: Some(at(1)),
        suspended_seconds: 50,
        suspended_at: Some(at(2)),
        suspend_artifact_bytes: Some(60),
    };
    counters.add(&LeaseCounters {
        commands_run: 5,
        command_output_bytes: 1,
        file_bytes_in: 2,
        file_bytes_out: 3,
        paused_seconds: 4,
        paused_at: Some(at(99)),
        suspended_seconds: 5,
        suspended_at: Some(at(99)),
        suspend_artifact_bytes: Some(99),
    });
    assert_eq!(
        counters,
        LeaseCounters {
            commands_run: u64::MAX,
            command_output_bytes: 11,
            file_bytes_in: 22,
            file_bytes_out: 33,
            paused_seconds: 44,
            paused_at: Some(at(1)),
            suspended_seconds: 55,
            suspended_at: Some(at(2)),
            suspend_artifact_bytes: Some(60),
        }
    );
}

#[test]
fn resource_gauges_omit_unreported_readings() -> TestResult {
    assert_eq!(serde_json::to_value(ResourceGauges::default())?, json!({}));
    let gauges = ResourceGauges {
        cpu_nanos_total: Some(1),
        memory_bytes: Some(2),
        memory_peak_bytes: Some(3),
        memory_limit_bytes: Some(4),
    };
    let wire = serde_json::to_value(gauges)?;
    assert_eq!(
        wire,
        json!({ "cpu_nanos_total": 1, "memory_bytes": 2, "memory_peak_bytes": 3, "memory_limit_bytes": 4 })
    );
    assert_eq!(serde_json::from_value::<ResourceGauges>(wire)?, gauges);
    Ok(())
}

fn released_usage() -> Result<LeaseUsage, Box<dyn std::error::Error>> {
    Ok(LeaseUsage {
        lease_id: LeaseId::new("lease-1"),
        tenant: TenantId::new("acme")?,
        status: LeaseStatus::Released,
        created_at: at(100),
        ended_at: Some(at(400)),
        last_activity_at: at(300),
        commands_run: 3,
        command_output_bytes: 4,
        file_bytes_in: 5,
        file_bytes_out: 6,
        running_seconds: 200,
        paused_seconds: 60,
        suspended_seconds: 40,
        suspend_artifact_bytes: None,
        gauges: Some(ResourceGauges {
            memory_bytes: Some(1_024),
            ..ResourceGauges::default()
        }),
    })
}

#[test]
fn lease_usage_round_trips_through_json() -> TestResult {
    let original = released_usage()?;
    let wire = serde_json::to_value(&original)?;
    assert_eq!(
        wire,
        json!({
            "lease_id": "lease-1",
            "tenant": "acme",
            "status": "released",
            "created_at": time(100),
            "ended_at": time(400),
            "last_activity_at": time(300),
            "commands_run": 3,
            "command_output_bytes": 4,
            "file_bytes_in": 5,
            "file_bytes_out": 6,
            "running_seconds": 200,
            "paused_seconds": 60,
            "suspended_seconds": 40,
            "gauges": { "memory_bytes": 1_024 },
        })
    );
    assert_eq!(serde_json::from_value::<LeaseUsage>(wire)?, original);
    Ok(())
}

#[test]
fn lease_usage_defaults_fields_older_brokers_omit_and_rejects_invalid_tenants() -> TestResult {
    let mut wire = serde_json::to_value(released_usage()?)?;
    let object = wire.as_object_mut().ok_or("usage is not an object")?;
    for field in ["ended_at", "suspended_seconds", "gauges"] {
        assert!(object.remove(field).is_some(), "{field}");
    }
    let parsed: LeaseUsage = serde_json::from_value(wire.clone())?;
    assert_eq!(parsed.ended_at, None);
    assert_eq!(parsed.suspended_seconds, 0);
    assert_eq!(parsed.suspend_artifact_bytes, None);
    assert_eq!(parsed.gauges, None);

    wire["tenant"] = json!("");
    let error = serde_json::from_value::<LeaseUsage>(wire)
        .err()
        .ok_or("empty tenant deserialized")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn tenant_usage_defaults_fields_older_brokers_omit() -> TestResult {
    let usage = TenantUsage {
        live_leases: 2,
        ended_leases: 1,
        commands_run: 3,
        command_output_bytes: 4,
        file_bytes_in: 5,
        file_bytes_out: 6,
        running_seconds: 7,
        paused_seconds: 8,
        suspended_seconds: 9,
    };
    let wire = serde_json::to_value(usage)?;
    assert_eq!(serde_json::from_value::<TenantUsage>(wire.clone())?, usage);

    let mut older = wire;
    let object = older.as_object_mut().ok_or("usage is not an object")?;
    for field in ["ended_leases", "suspended_seconds"] {
        assert!(object.remove(field).is_some(), "{field}");
    }
    let parsed: TenantUsage = serde_json::from_value(older)?;
    assert_eq!(
        parsed,
        TenantUsage {
            ended_leases: 0,
            suspended_seconds: 0,
            ..usage
        }
    );
    Ok(())
}

#[test]
fn account_limits_round_trip_with_and_without_a_tag_cap() -> TestResult {
    let limits = AccountLimits {
        active_leases: 2,
        max_active_leases: 10,
        cpu_seconds_used: 30,
        cpu_seconds_cap: 3_600,
        wall_seconds_used: 60,
        wall_seconds_cap: 7_200,
        max_cpu_seconds_per_request: 600,
        max_memory_mib_per_request: 4_096,
        per_tag: Some(TagLimits {
            tag_key: "user".to_owned(),
            max_active_leases: 2,
            in_use: BTreeMap::from([("alice".to_owned(), 1)]),
        }),
    };
    let wire = serde_json::to_value(&limits)?;
    assert_eq!(
        wire["per_tag"],
        json!({ "tag_key": "user", "max_active_leases": 2, "in_use": { "alice": 1 } })
    );
    assert_eq!(serde_json::from_value::<AccountLimits>(wire)?, limits);

    let untagged = AccountLimits {
        per_tag: None,
        ..limits
    };
    let wire = serde_json::to_value(&untagged)?;
    assert!(wire.get("per_tag").is_none());
    assert_eq!(serde_json::from_value::<AccountLimits>(wire)?, untagged);

    let tag: TagLimits = serde_json::from_value(json!({ "tag_key": "user", "max_active_leases": 1 }))?;
    assert!(tag.in_use.is_empty());
    Ok(())
}

#[test]
fn usage_requests_carry_validated_identifiers() -> TestResult {
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("lease-1");

    let resource = ResourceUsageRequest {
        lease_id: lease_id.clone(),
    };
    assert_eq!(serde_json::to_value(&resource)?, json!({ "lease_id": "lease-1" }));
    assert_eq!(
        serde_json::from_value::<ResourceUsageRequest>(json!({ "lease_id": "lease-1" }))?,
        resource
    );

    let lease = GetLeaseUsageRequest {
        tenant: tenant.clone(),
        lease_id,
    };
    let wire = json!({ "tenant": "acme", "lease_id": "lease-1" });
    assert_eq!(serde_json::to_value(&lease)?, wire);
    assert_eq!(serde_json::from_value::<GetLeaseUsageRequest>(wire)?, lease);

    let usage = GetTenantUsageRequest { tenant: tenant.clone() };
    assert_eq!(serde_json::to_value(&usage)?, json!({ "tenant": "acme" }));
    assert_eq!(
        serde_json::from_value::<GetTenantUsageRequest>(json!({ "tenant": "acme" }))?,
        usage
    );

    let limits = GetAccountLimitsRequest { tenant };
    assert_eq!(serde_json::to_value(&limits)?, json!({ "tenant": "acme" }));
    assert_eq!(
        serde_json::from_value::<GetAccountLimitsRequest>(json!({ "tenant": "acme" }))?,
        limits
    );

    let invalid = json!({ "tenant": "acme\n", "lease_id": "lease-1" });
    assert!(serde_json::from_value::<GetLeaseUsageRequest>(invalid.clone()).is_err());
    assert!(serde_json::from_value::<GetTenantUsageRequest>(invalid.clone()).is_err());
    assert!(serde_json::from_value::<GetAccountLimitsRequest>(invalid).is_err());
    Ok(())
}

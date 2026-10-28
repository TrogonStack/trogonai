use buffa::type_registry::TypeRegistry;
use buffa::{Message, MessageName};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
use crate::google::rpc;
#[cfg(feature = "schedules")]
use crate::google::r#type as calendar;
#[cfg(feature = "schedules")]
use crate::scheduler::schedules::{checkpoints_v1, projections_v1, state_v1, v1};
#[cfg(feature = "grpc-nats-micro")]
use crate::{grpc_nats_micro::v1 as echo, nats::micro::v1alpha1 as micro};

fn assert_registered<M: Message + MessageName + DeserializeOwned + Serialize>(registry: &TypeRegistry, input: Value) {
    let expected: M = serde_json::from_value(input.clone()).expect("typed registry fixture");
    let entry = registry.json_any_by_url(M::TYPE_URL).expect("registered message URL");
    let wire = (entry.from_json)(input).expect("registered JSON codec");
    assert_eq!(wire, expected.encode_to_vec());
    assert_eq!(
        (entry.to_json)(&wire).expect("registered wire codec"),
        serde_json::to_value(&expected).expect("typed JSON")
    );
    assert!((entry.to_json)(b"\x00").is_err());
    assert!((entry.from_json)(json!(false)).is_err());
}

#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
#[test]
fn rpc_type_urls_dispatch_all_structured_error_detail_codecs() {
    let mut registry = TypeRegistry::new();
    rpc::register_types(&mut registry);
    assert_registered::<rpc::ErrorInfo>(&registry, json!({"reason": "QUOTA", "domain": "scheduler.example"}));
    assert_registered::<rpc::RetryInfo>(&registry, json!({"retryDelay": "1s"}));
    assert_registered::<rpc::DebugInfo>(&registry, json!({"stackEntries": ["deliver"], "detail": "timeout"}));
    assert_registered::<rpc::QuotaFailure>(&registry, json!({"violations": [{"subject": "jobs"}]}));
    assert_registered::<rpc::quota_failure::Violation>(&registry, json!({"subject": "jobs"}));
    assert_registered::<rpc::PreconditionFailure>(&registry, json!({"violations": [{"type": "VERSION"}]}));
    assert_registered::<rpc::precondition_failure::Violation>(&registry, json!({"type": "VERSION"}));
    assert_registered::<rpc::BadRequest>(&registry, json!({"fieldViolations": [{"field": "cron"}]}));
    assert_registered::<rpc::bad_request::FieldViolation>(&registry, json!({"field": "cron"}));
    assert_registered::<rpc::RequestInfo>(&registry, json!({"requestId": "request-7"}));
    assert_registered::<rpc::ResourceInfo>(&registry, json!({"resourceName": "backup"}));
    assert_registered::<rpc::Help>(&registry, json!({"links": [{"url": "https://example.com/help"}]}));
    assert_registered::<rpc::help::Link>(&registry, json!({"url": "https://example.com/help"}));
    assert_registered::<rpc::LocalizedMessage>(&registry, json!({"locale": "es", "message": "Error"}));
    assert_registered::<rpc::Status>(&registry, json!({"code": 5, "message": "missing"}));
}

#[cfg(feature = "schedules")]
macro_rules! schema_registry_contracts {
    ($name:ident, $schema:ident) => {
        mod $name {
            use super::*;
            use $schema as schema;

            #[test]
            fn package_registry_preserves_message_and_nested_type_urls() {
                let mut registry = TypeRegistry::new();
                schema::register_types(&mut registry);
                assert_registered::<schema::Schedule>(&registry, json!({"every": {"every": "60s"}}));
                assert_registered::<schema::schedule::At>(&registry, json!({"at": "2026-01-01T00:00:00Z"}));
                assert_registered::<schema::schedule::Every>(&registry, json!({"every": "60s"}));
                assert_registered::<schema::schedule::Cron>(&registry, json!({"expr": "0 9 * * *", "timezone": {"id": "UTC"}}));
                assert_registered::<schema::schedule::RRule>(&registry, json!({"rrule": "FREQ=DAILY"}));
                assert_registered::<schema::Delivery>(&registry, json!({"natsMessage": {"subject": "jobs.backup"}}));
                assert_registered::<schema::delivery::NatsMessage>(&registry, json!({"subject": "jobs.backup"}));
                assert_registered::<schema::delivery::nats_message::Source>(&registry, json!({"latestFromSubject": {"subject": "jobs.template"}}));
                assert_registered::<schema::delivery::nats_message::LatestFromSubject>(&registry, json!({"subject": "jobs.template"}));
                assert_registered::<schema::Message>(&registry, json!({"content": {"data": "aGk="}}));
                assert_registered::<schema::Header>(&registry, json!({"name": "region", "value": "west"}));
            }
        }
    };
}

#[cfg(feature = "schedules")]
schema_registry_contracts!(live_tests, v1);
#[cfg(feature = "schedules")]
schema_registry_contracts!(checkpoint_tests, checkpoints_v1);
#[cfg(feature = "schedules")]
schema_registry_contracts!(projection_tests, projections_v1);

#[cfg(feature = "schedules")]
#[test]
fn registered_calendar_state_and_storage_schemas_use_distinct_type_urls() {
    let mut registry = TypeRegistry::new();
    calendar::register_types(&mut registry);
    state_v1::register_types(&mut registry);
    checkpoints_v1::register_types(&mut registry);
    projections_v1::register_types(&mut registry);
    assert_registered::<calendar::DateTime>(&registry, json!({"year": 2026, "timeZone": {"id": "UTC"}}));
    assert_registered::<calendar::TimeZone>(&registry, json!({"id": "UTC", "version": "2025b"}));
    assert_registered::<state_v1::State>(&registry, json!({"state": "STATE_VALUE_PRESENT_ENABLED"}));
    assert_registered::<checkpoints_v1::ScheduleCheckpoint>(&registry, json!({"scheduleId": "backup"}));
    assert_registered::<projections_v1::ScheduleProjection>(&registry, json!({"scheduleId": "backup"}));
    assert_registered::<projections_v1::ScheduleStatus>(&registry, json!({"scheduled": {}}));
    assert_registered::<projections_v1::schedule_status::Scheduled>(&registry, json!({}));
    assert_registered::<projections_v1::schedule_status::Paused>(&registry, json!({}));
}

#[cfg(feature = "grpc-nats-micro")]
#[test]
fn registered_transport_types_preserve_discovery_and_content_payloads() {
    let mut registry = TypeRegistry::new();
    echo::register_types(&mut registry);
    micro::register_types(&mut registry);
    crate::content::v1alpha1::register_types(&mut registry);
    assert_registered::<echo::SayRequest>(&registry, json!({"message": "hello"}));
    assert_registered::<echo::SayResponse>(&registry, json!({"message": "hello"}));
    assert_registered::<echo::FailRequest>(&registry, json!({"code": "UNAVAILABLE", "message": "retry"}));
    assert_registered::<echo::FailResponse>(&registry, json!({"message": "failed"}));
    assert_registered::<micro::ServiceOptions>(&registry, json!({"version": "1.2.3"}));
    assert_registered::<micro::MethodOptions>(&registry, json!({"metadata": {"region": "west"}}));
    assert_registered::<crate::content::v1alpha1::Content>(
        &registry,
        json!({"contentType": "text/plain", "data": "aGk="}),
    );
}

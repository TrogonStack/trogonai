use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::r#gen::trogon::error::v1alpha1::{FieldOptions, MessageOptions};
#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
use crate::google::rpc;
#[cfg(feature = "schedules")]
use crate::google::r#type::DateTime;
#[cfg(feature = "schedules")]
use crate::scheduler::schedules::{checkpoints_v1, projections_v1, state_v1, v1};
#[cfg(feature = "grpc-nats-micro")]
use crate::{grpc_nats_micro::v1 as echo, nats::micro::v1alpha1 as micro};

fn assert_diagnostic<M: DeserializeOwned + Debug>(input: Value, fields: &[&str]) {
    let message: M = serde_json::from_value(input).expect("diagnostic fixture");
    let diagnostic = format!("{message:?}");
    for field in fields {
        assert!(diagnostic.contains(field), "missing {field:?} in {diagnostic}");
    }
}

#[test]
fn error_annotations_expose_identity_and_selected_policy_in_diagnostics() {
    assert_diagnostic::<MessageOptions>(
        json!({"template": {
            "domain": "scheduler.example", "reason": "CAPACITY", "message": "No slots",
            "code": "RESOURCE_EXHAUSTED", "visibility": "VISIBILITY_PUBLIC",
            "helpLinks": [{"url": "https://example.com/help", "description": "Limits"}],
            "metadata": [{"key": "limit", "value": "jobs", "visibility": "VISIBILITY_PRIVATE"}]
        }}),
        &[
            "MessageOptions",
            "Template",
            "domain: \"scheduler.example\"",
            "reason: \"CAPACITY\"",
            "message: \"No slots\"",
            "RESOURCE_EXHAUSTED",
            "VISIBILITY_PUBLIC",
            "HelpLink",
            "url: \"https://example.com/help\"",
            "description: \"Limits\"",
            "MetadataEntry",
            "key: \"limit\"",
            "value: \"jobs\"",
            "VISIBILITY_PRIVATE",
        ],
    );
    for (input, policy) in [
        (json!({"defaultValue": "fallback"}), "DefaultValue(\"fallback\")"),
        (json!({"value": "fixed"}), "Value(\"fixed\")"),
    ] {
        assert_diagnostic::<FieldOptions>(input, &["FieldOptions", policy]);
    }
    assert_diagnostic::<crate::r#gen::elixirpb::FileOptions>(
        json!({"modulePrefix": "Example"}),
        &["FileOptions", "module_prefix: Some(\"Example\")"],
    );
}

#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
#[test]
fn rpc_diagnostics_retain_detail_identity_and_nested_payload_values() {
    assert_diagnostic::<rpc::ErrorInfo>(
        json!({"reason": "QUOTA", "domain": "scheduler.example", "metadata": {"limit": "100"}}),
        &[
            "ErrorInfo",
            "reason: \"QUOTA\"",
            "domain: \"scheduler.example\"",
            "\"limit\": \"100\"",
        ],
    );
    assert_diagnostic::<rpc::RetryInfo>(
        json!({"retryDelay": "1.000000002s"}),
        &["RetryInfo", "seconds: 1", "nanos: 2"],
    );
    assert_diagnostic::<rpc::DebugInfo>(
        json!({"stackEntries": ["schedule", "deliver"], "detail": "timeout"}),
        &[
            "DebugInfo",
            "stack_entries: [\"schedule\", \"deliver\"]",
            "detail: \"timeout\"",
        ],
    );
    assert_diagnostic::<rpc::QuotaFailure>(
        json!({"violations": [{"subject": "jobs", "description": "limit", "apiService": "scheduler",
            "quotaMetric": "deliveries", "quotaId": "daily", "quotaDimensions": {"region": "west"},
            "quotaValue": "100", "futureQuotaValue": "200"}]}),
        &[
            "QuotaFailure",
            "Violation",
            "subject: \"jobs\"",
            "description: \"limit\"",
            "api_service: \"scheduler\"",
            "quota_metric: \"deliveries\"",
            "quota_id: \"daily\"",
            "\"region\": \"west\"",
            "quota_value: 100",
            "future_quota_value: Some(200)",
        ],
    );
    assert_diagnostic::<rpc::PreconditionFailure>(
        json!({"violations": [{"type": "VERSION", "subject": "backup", "description": "changed"}]}),
        &[
            "PreconditionFailure",
            "Violation",
            "type: \"VERSION\"",
            "subject: \"backup\"",
            "description: \"changed\"",
        ],
    );
    assert_diagnostic::<rpc::BadRequest>(
        json!({"fieldViolations": [{"field": "cron", "description": "invalid", "reason": "SYNTAX",
            "localizedMessage": {"locale": "es", "message": "Inválido"}}]}),
        &[
            "BadRequest",
            "FieldViolation",
            "field: \"cron\"",
            "description: \"invalid\"",
            "reason: \"SYNTAX\"",
            "LocalizedMessage",
            "locale: \"es\"",
            "message: \"Inválido\"",
        ],
    );
    assert_diagnostic::<rpc::RequestInfo>(
        json!({"requestId": "request-7", "servingData": "west"}),
        &["RequestInfo", "request_id: \"request-7\"", "serving_data: \"west\""],
    );
    assert_diagnostic::<rpc::ResourceInfo>(
        json!({"resourceType": "schedule", "resourceName": "backup", "owner": "project", "description": "removed"}),
        &[
            "ResourceInfo",
            "resource_type: \"schedule\"",
            "resource_name: \"backup\"",
            "owner: \"project\"",
            "description: \"removed\"",
        ],
    );
    assert_diagnostic::<rpc::Help>(
        json!({"links": [{"description": "Limits", "url": "https://example.com/help"}]}),
        &[
            "Help",
            "Link",
            "description: \"Limits\"",
            "url: \"https://example.com/help\"",
        ],
    );
    assert_diagnostic::<rpc::Status>(
        json!({"code": 5, "message": "missing"}),
        &["Status", "code: 5", "message: \"missing\"", "details: []"],
    );
}

#[cfg(feature = "schedules")]
#[test]
fn civil_datetime_diagnostics_distinguish_zone_rules_from_fixed_offsets() {
    for (offset, fragments) in [
        (json!({"utcOffset": "3600s"}), vec!["UtcOffset", "seconds: 3600"]),
        (
            json!({"timeZone": {"id": "Europe/Paris", "version": "2025b"}}),
            vec!["TimeZone", "id: \"Europe/Paris\"", "version: \"2025b\""],
        ),
    ] {
        let mut input =
            json!({"year": 2026, "month": 9, "day": 5, "hours": 11, "minutes": 12, "seconds": 13, "nanos": 123});
        input
            .as_object_mut()
            .expect("datetime")
            .extend(offset.as_object().expect("offset").clone());
        assert_diagnostic::<DateTime>(
            input.clone(),
            &[
                "DateTime",
                "year: 2026",
                "month: 9",
                "day: 5",
                "hours: 11",
                "minutes: 12",
                "seconds: 13",
                "nanos: 123",
            ],
        );
        assert_diagnostic::<DateTime>(input, &fragments);
    }
}

#[cfg(feature = "schedules")]
macro_rules! schema_diagnostic_contracts {
    ($name:ident, $schema:ident) => {
        mod $name {
            use super::*;
            use $schema as schema;

            #[test]
            fn schedule_diagnostics_identify_timing_and_delivery_variants() {
                for (schedule, fields) in [
                    (json!({"at": {"at": "2026-01-01T00:00:00Z"}}),
                        vec!["Schedule", "At", "seconds: 1767225600"]),
                    (json!({"every": {"every": "60s"}}), vec!["Schedule", "Every", "seconds: 60"]),
                    (json!({"cron": {"expr": "0 9 * * *", "timezone": {"id": "UTC"}}}),
                        vec!["Schedule", "Cron", "expr: \"0 9 * * *\"", "id: \"UTC\""]),
                    (json!({"rrule": {"dtstart": "2026-01-01T00:00:00Z", "rrule": "FREQ=DAILY",
                        "timezone": {"id": "UTC"}, "rdate": ["2026-01-02T00:00:00Z"],
                        "exdate": ["2026-01-03T00:00:00Z"]}}),
                        vec!["Schedule", "RRule", "rrule: \"FREQ=DAILY\"", "rdate:", "exdate:", "id: \"UTC\""]),
                ] {
                    assert_diagnostic::<schema::Schedule>(schedule, &fields);
                }
                assert_diagnostic::<schema::Delivery>(json!({"natsMessage": {"subject": "jobs.backup",
                    "ttl": "30s", "source": {"latestFromSubject": {"subject": "jobs.template"}}}}),
                    &["Delivery", "NatsMessage", "subject: \"jobs.backup\"", "seconds: 30",
                        "LatestFromSubject", "subject: \"jobs.template\""]);
                assert_diagnostic::<schema::Message>(json!({"content": {"contentType": "text/plain", "data": "aGk="},
                    "headers": [{"name": "region", "value": "west"}]}),
                    &["Message", "Content", "content_type: \"text/plain\"", "Header", "name: \"region\"", "value: \"west\""]);
            }
        }
    };
}

#[cfg(feature = "schedules")]
schema_diagnostic_contracts!(live_tests, v1);
#[cfg(feature = "schedules")]
schema_diagnostic_contracts!(checkpoint_tests, checkpoints_v1);
#[cfg(feature = "schedules")]
schema_diagnostic_contracts!(projection_tests, projections_v1);

#[cfg(feature = "schedules")]
#[test]
fn lifecycle_diagnostics_retain_event_type_and_schedule_identity() {
    for (event, variant) in [
        (
            json!({"scheduleCreated": {"scheduleId": "backup", "status": {"scheduled": {}}}}),
            "ScheduleCreated",
        ),
        (json!({"schedulePaused": {"scheduleId": "backup"}}), "SchedulePaused"),
        (json!({"scheduleResumed": {"scheduleId": "backup"}}), "ScheduleResumed"),
        (json!({"scheduleRemoved": {"scheduleId": "backup"}}), "ScheduleRemoved"),
        (
            json!({"scheduleOccurrenceScheduled": {"scheduleId": "backup", "occurrenceSequence": "7"}}),
            "ScheduleOccurrenceScheduled",
        ),
        (
            json!({"scheduleOccurrenceRecorded": {"scheduleId": "backup", "occurrenceSequence": "7"}}),
            "ScheduleOccurrenceRecorded",
        ),
        (
            json!({"scheduleCompleted": {"scheduleId": "backup", "lastOccurrenceSequence": "7"}}),
            "ScheduleCompleted",
        ),
    ] {
        assert_diagnostic::<v1::ScheduleEvent>(event, &["ScheduleEvent", variant, "schedule_id: \"backup\""]);
    }
    assert_diagnostic::<v1::CreateSchedule>(
        json!({"scheduleId": "backup", "status": {"paused": {}}}),
        &["CreateSchedule", "schedule_id: \"backup\"", "ScheduleStatus", "Paused"],
    );
    assert_diagnostic::<v1::PauseSchedule>(
        json!({"scheduleId": "backup"}),
        &["PauseSchedule", "schedule_id: \"backup\""],
    );
    assert_diagnostic::<v1::ResumeSchedule>(
        json!({"scheduleId": "backup"}),
        &["ResumeSchedule", "schedule_id: \"backup\""],
    );
    assert_diagnostic::<v1::RemoveSchedule>(
        json!({"scheduleId": "backup"}),
        &["RemoveSchedule", "schedule_id: \"backup\""],
    );
    assert_diagnostic::<projections_v1::ScheduleProjection>(
        json!({"scheduleId": "backup", "status": {"scheduled": {}}, "completed": false}),
        &[
            "ScheduleProjection",
            "schedule_id: \"backup\"",
            "Scheduled",
            "completed: Some(false)",
        ],
    );
    assert_diagnostic::<projections_v1::ScheduleStatus>(json!({"paused": {}}), &["ScheduleStatus", "Paused"]);
    assert_diagnostic::<checkpoints_v1::ScheduleCheckpoint>(
        json!({"scheduleId": "backup", "status": "SCHEDULE_CHECKPOINT_STATUS_PAUSED",
        "lastAppliedStreamPosition": "7", "lastAppliedEventId": "event-7", "lastOutcome": "RECONCILE_OUTCOME_STORED_PAUSED"}),
        &[
            "ScheduleCheckpoint",
            "schedule_id: Some(\"backup\")",
            "SCHEDULE_CHECKPOINT_STATUS_PAUSED",
            "last_applied_stream_position: Some(7)",
            "last_applied_event_id: Some(\"event-7\")",
            "RECONCILE_OUTCOME_STORED_PAUSED",
        ],
    );
    assert_diagnostic::<state_v1::State>(
        json!({"state": "STATE_VALUE_PRESENT_ENABLED", "lastOccurrenceSequence": "7", "completed": false}),
        &[
            "State",
            "STATE_VALUE_PRESENT_ENABLED",
            "last_occurrence_sequence: Some(7)",
            "completed: Some(false)",
        ],
    );
}

#[cfg(feature = "grpc-nats-micro")]
#[test]
fn transport_diagnostics_retain_response_identity_and_discovery_metadata() {
    assert_diagnostic::<echo::SayRequest>(json!({"message": "hello"}), &["SayRequest", "message: Some(\"hello\")"]);
    assert_diagnostic::<echo::SayResponse>(
        json!({"message": "hello"}),
        &["SayResponse", "message: Some(\"hello\")"],
    );
    assert_diagnostic::<echo::FailRequest>(
        json!({"code": "UNAVAILABLE", "message": "retry"}),
        &["FailRequest", "UNAVAILABLE", "message: Some(\"retry\")"],
    );
    assert_diagnostic::<echo::FailResponse>(
        json!({"message": "failed"}),
        &["FailResponse", "message: Some(\"failed\")"],
    );
    assert_diagnostic::<micro::ServiceOptions>(
        json!({"version": "1.2.3", "description": "Delivery",
        "metadata": {"region": "west"}, "contentType": "CONTENT_TYPE_PROTOBUF"}),
        &[
            "ServiceOptions",
            "version: Some(\"1.2.3\")",
            "description: Some(\"Delivery\")",
            "\"region\": \"west\"",
            "CONTENT_TYPE_PROTOBUF",
        ],
    );
    assert_diagnostic::<micro::MethodOptions>(
        json!({"metadata": {"region": "west"}}),
        &["MethodOptions", "\"region\": \"west\""],
    );
}

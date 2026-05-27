use crate::event::{TrafficEvent, TrafficSource};

pub trait OcsfExporter {
    fn emit(&self, event: &TrafficEvent) -> serde_json::Value;
}

pub struct DefaultOcsfExporter;

impl OcsfExporter for DefaultOcsfExporter {
    fn emit(&self, event: &TrafficEvent) -> serde_json::Value {
        match event.source {
            TrafficSource::Sts => authorization_activity(event),
            TrafficSource::Gateway | TrafficSource::Registry => application_activity(event),
        }
    }
}

fn authorization_activity(event: &TrafficEvent) -> serde_json::Value {
    let (status_id, status) = outcome_status(&event.outcome);
    serde_json::json!({
        "activity_id": 1,
        "activity_name": "Authorize",
        "category_uid": 3,
        "category_name": "Identity & Access Management",
        "class_uid": 3003,
        "class_name": "Authorization Activity",
        "type_uid": 300301,
        "type_name": "Authorization Activity: Authorize",
        "severity_id": if status_id == 1 { 1 } else { 3 },
        "severity": if status_id == 1 { "Informational" } else { "Medium" },
        "time": event.ts.timestamp_millis(),
        "metadata": {
            "version": "1.3.0",
            "product": { "name": "trogon-traffic-view", "vendor_name": "TrogonStack" },
            "profiles": ["host"]
        },
        "actor": {
            "user": {
                "uid": event.caller_sub.clone().unwrap_or_default(),
                "name": event.caller_sub.clone().unwrap_or_default()
            }
        },
        "src_endpoint": { "name": event.caller_wkl.clone().unwrap_or_default() },
        "service": { "name": "trogon-sts" },
        "status_id": status_id,
        "status": status,
        "unmapped": {
            "trogon.event_id": event.event_id,
            "trogon.tenant": event.tenant,
            "trogon.audience": event.target_aud,
            "trogon.purpose": event.purpose,
            "trogon.scope": event.scope,
            "trogon.reason": event.reason,
            "trogon.session_id": event.session_id
        }
    })
}

fn application_activity(event: &TrafficEvent) -> serde_json::Value {
    let (status_id, status) = outcome_status(&event.outcome);
    serde_json::json!({
        "activity_id": 1,
        "activity_name": "Access",
        "category_uid": 6,
        "category_name": "Application Activity",
        "class_uid": 6002,
        "class_name": "Application Activity",
        "type_uid": 600201,
        "type_name": "Application Activity: Access",
        "severity_id": if status_id == 1 { 1 } else { 3 },
        "severity": if status_id == 1 { "Informational" } else { "Medium" },
        "time": event.ts.timestamp_millis(),
        "metadata": {
            "version": "1.3.0",
            "product": { "name": "trogon-traffic-view", "vendor_name": "TrogonStack" },
            "profiles": ["host"]
        },
        "actor": {
            "user": {
                "uid": event.caller_sub.clone().unwrap_or_default(),
                "name": event.caller_sub.clone().unwrap_or_default()
            }
        },
        "src_endpoint": { "name": event.caller_wkl.clone().unwrap_or_default() },
        "dst_endpoint": { "name": event.target_aud.clone().unwrap_or_default() },
        "status_id": status_id,
        "status": status,
        "unmapped": {
            "trogon.event_id": event.event_id,
            "trogon.tenant": event.tenant,
            "trogon.purpose": event.purpose,
            "trogon.session_id": event.session_id,
            "trogon.request_id": event.request_id
        }
    })
}

fn outcome_status(outcome: &str) -> (u8, &'static str) {
    match outcome {
        "allow" => (1, "Success"),
        "deny" | "rate_limited" => (2, "Failure"),
        _ => (99, "Other"),
    }
}

pub fn emit_ndjson(events: &[TrafficEvent], exporter: &dyn OcsfExporter) -> String {
    events
        .iter()
        .map(|event| exporter.emit(event))
        .map(|value| serde_json::to_string(&value).unwrap_or_else(|_| "{}".into()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono::Utc;

    use super::*;
    use crate::event::TrafficSource;

    #[test]
    fn sts_maps_to_authorization_activity() {
        let event = TrafficEvent {
            event_id: "evt-1".into(),
            ts: Utc.with_ymd_and_hms(2026, 5, 27, 14, 2, 1).unwrap(),
            tenant: "acme".into(),
            caller_sub: Some("acme/triage-router".into()),
            caller_wkl: Some("spiffe://acme.local/ns/prod/sa/triage-router".into()),
            target_aud: Some("urn:trogon:a2a:agent:acme:oncall-agent".into()),
            purpose: Some("incident.response".into()),
            scope: Some("mcp:tools".into()),
            outcome: "allow".into(),
            reason: Some("ok".into()),
            act_chain: None,
            request_id: None,
            session_id: Some("sess-1".into()),
            source: TrafficSource::Sts,
        };
        let value = DefaultOcsfExporter.emit(&event);
        assert_eq!(value.get("class_uid").and_then(|v| v.as_u64()), Some(3003));
        assert_eq!(
            value.pointer("/unmapped/trogon.audience").and_then(|v| v.as_str()),
            Some("urn:trogon:a2a:agent:acme:oncall-agent")
        );
    }

    #[test]
    fn gateway_maps_to_application_activity() {
        let event = TrafficEvent {
            event_id: "evt-2".into(),
            ts: Utc.with_ymd_and_hms(2026, 5, 27, 14, 2, 2).unwrap(),
            tenant: "acme".into(),
            caller_sub: Some("acme/oncall-agent".into()),
            caller_wkl: Some("spiffe://acme.local/ns/prod/sa/oncall-agent".into()),
            target_aud: Some("urn:backend:fs".into()),
            purpose: Some("incident.response".into()),
            scope: None,
            outcome: "deny".into(),
            reason: None,
            act_chain: None,
            request_id: Some("req-1".into()),
            session_id: None,
            source: TrafficSource::Gateway,
        };
        let value = DefaultOcsfExporter.emit(&event);
        assert_eq!(value.get("class_uid").and_then(|v| v.as_u64()), Some(6002));
        assert_eq!(value.get("status_id").and_then(|v| v.as_u64()), Some(2));
    }
}

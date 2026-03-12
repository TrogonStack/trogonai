//! incident.io webhook event types and NATS subject routing.

/// Derive the NATS subject suffix from an incident.io webhook payload.
///
/// incident.io sends an `event_type` field like `"incident.created"`.
/// That string is used directly as a multi-level NATS subject suffix so
/// `incidentio.incident.created`, `incidentio.incident.resolved`, etc. are
/// all captured by a `incidentio.>` filter.
///
/// Unknown or missing `event_type` fields fall back to `"event"`.
pub fn nats_subject_suffix(body: &[u8]) -> String {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return "event".to_string();
    };
    match v["event_type"].as_str() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => "event".to_string(),
    }
}

/// Extract the incident ID from an incident.io webhook payload.
///
/// Returns `None` when the payload does not contain `incident.id`.
pub fn incident_id(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v["incident"]["id"].as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incident_created_suffix() {
        assert_eq!(
            nats_subject_suffix(br#"{"event_type":"incident.created"}"#),
            "incident.created"
        );
    }

    #[test]
    fn incident_resolved_suffix() {
        assert_eq!(
            nats_subject_suffix(br#"{"event_type":"incident.resolved"}"#),
            "incident.resolved"
        );
    }

    #[test]
    fn incident_updated_suffix() {
        assert_eq!(
            nats_subject_suffix(br#"{"event_type":"incident.updated"}"#),
            "incident.updated"
        );
    }

    #[test]
    fn missing_event_type_falls_back_to_event() {
        assert_eq!(nats_subject_suffix(br#"{"incident":{}}"#), "event");
    }

    #[test]
    fn empty_event_type_falls_back_to_event() {
        assert_eq!(nats_subject_suffix(br#"{"event_type":""}"#), "event");
    }

    #[test]
    fn invalid_json_falls_back_to_event() {
        assert_eq!(nats_subject_suffix(b"not json"), "event");
    }

    #[test]
    fn incident_id_extracted() {
        let body = br#"{"event_type":"incident.created","incident":{"id":"inc-123","name":"test"}}"#;
        assert_eq!(incident_id(body), Some("inc-123".to_string()));
    }

    #[test]
    fn incident_id_missing_returns_none() {
        assert_eq!(incident_id(br#"{"event_type":"incident.created"}"#), None);
    }

    #[test]
    fn incident_id_invalid_json_returns_none() {
        assert_eq!(incident_id(b"not json"), None);
    }
}

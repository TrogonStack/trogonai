use chrono::{DateTime, TimeZone, Utc};

use crate::envelope::AuditEnvelope;
use crate::error::ProjectorError;
use crate::event::{ActChainHop, TrafficDecision, TrafficEvent, TrafficSource};

pub fn normalize_envelope(envelope: &AuditEnvelope) -> Result<TrafficEvent, ProjectorError> {
    let source = classify_subject(&envelope.subject)?;
    match source {
        TrafficSource::Sts => normalize_sts(envelope),
        TrafficSource::Registry => normalize_registry(envelope),
        TrafficSource::Gateway => normalize_gateway(envelope),
    }
}

fn classify_subject(subject: &str) -> Result<TrafficSource, ProjectorError> {
    if subject.starts_with("mcp.audit.sts.") {
        Ok(TrafficSource::Sts)
    } else if subject.starts_with("mcp.audit.registry.") {
        Ok(TrafficSource::Registry)
    } else if subject.contains(".audit.") {
        Ok(TrafficSource::Gateway)
    } else {
        Err(ProjectorError::Normalize(format!("unsupported audit subject: {subject}")))
    }
}

fn normalize_sts(envelope: &AuditEnvelope) -> Result<TrafficEvent, ProjectorError> {
    let payload = &envelope.payload;
    let outcome = string_field(payload, "outcome")
        .or_else(|| envelope.subject.rsplit('.').next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    let request = payload.get("request");
    let minted = payload.get("minted");
    let act_chain = parse_act_chain(payload.get("act_chain").or_else(|| minted.and_then(|m| m.get("act_chain"))));
    Ok(TrafficEvent {
        event_id: envelope.event_id.clone(),
        ts: parse_ts(payload)?,
        tenant: string_field(payload, "tenant")
            .or_else(|| nested_string_field(minted, "tenant"))
            .unwrap_or_else(|| "default".into()),
        caller_sub: string_field(payload, "agent_id")
            .or_else(|| nested_string_field(minted, "sub"))
            .or_else(|| request.and_then(|r| string_field(r, "subject_sub"))),
        caller_wkl: string_field(payload, "wkl")
            .or_else(|| nested_string_field(minted, "wkl"))
            .or_else(|| request.and_then(|r| string_field(r, "wkl"))),
        target_aud: request
            .and_then(|r| string_field(r, "audience"))
            .or_else(|| nested_string_field(minted, "aud")),
        purpose: request
            .and_then(|r| string_field(r, "purpose"))
            .or_else(|| nested_string_field(minted, "purpose"))
            .or_else(|| string_field(payload, "purpose")),
        scope: request
            .and_then(|r| string_field(r, "scope"))
            .or_else(|| nested_string_field(minted, "scope")),
        outcome: TrafficDecision::parse(&outcome).as_str().into(),
        reason: string_field(payload, "decision_reason").or_else(|| string_field(payload, "reason")),
        act_chain,
        request_id: json_request_id(payload.get("request_id")),
        session_id: string_field(payload, "session_id"),
        source: TrafficSource::Sts,
    })
}

fn normalize_registry(envelope: &AuditEnvelope) -> Result<TrafficEvent, ProjectorError> {
    let payload = &envelope.payload;
    let outcome = string_field(payload, "outcome")
        .or_else(|| envelope.subject.rsplit('.').next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    Ok(TrafficEvent {
        event_id: envelope.event_id.clone(),
        ts: parse_ts(payload)?,
        tenant: string_field(payload, "tenant").unwrap_or_else(|| "default".into()),
        caller_sub: string_field(payload, "agent_id"),
        caller_wkl: string_field(payload, "wkl"),
        target_aud: string_field(payload, "audience"),
        purpose: string_field(payload, "purpose"),
        scope: string_field(payload, "scope"),
        outcome: TrafficDecision::parse(&outcome).as_str().into(),
        reason: string_field(payload, "reason"),
        act_chain: parse_act_chain(payload.get("act_chain")),
        request_id: json_request_id(payload.get("request_id")),
        session_id: string_field(payload, "session_id"),
        source: TrafficSource::Registry,
    })
}

fn normalize_gateway(envelope: &AuditEnvelope) -> Result<TrafficEvent, ProjectorError> {
    let payload = &envelope.payload;
    let outcome = string_field(payload, "outcome").unwrap_or_else(|| "unknown".into());
    Ok(TrafficEvent {
        event_id: envelope.event_id.clone(),
        ts: parse_ts(payload)?,
        tenant: string_field(payload, "tenant").unwrap_or_else(|| "default".into()),
        caller_sub: string_field(payload, "caller_sub").or_else(|| string_field(payload, "agent_id")),
        caller_wkl: string_field(payload, "wkl"),
        target_aud: string_field(payload, "subject_out"),
        purpose: string_field(payload, "purpose"),
        scope: None,
        outcome: TrafficDecision::parse(&outcome).as_str().into(),
        reason: None,
        act_chain: parse_act_chain(payload.get("act_chain")),
        request_id: json_request_id(payload.get("request_id")),
        session_id: string_field(payload, "session_id"),
        source: TrafficSource::Gateway,
    })
}

fn parse_ts(payload: &serde_json::Value) -> Result<DateTime<Utc>, ProjectorError> {
    if let Some(ts) = string_field(payload, "ts") {
        return DateTime::parse_from_rfc3339(&ts)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|error| ProjectorError::Normalize(error.to_string()));
    }
    if let Some(ms) = payload.get("latency_ms").and_then(|v| v.as_u64()) {
        let now = Utc::now();
        return Ok(now - chrono::Duration::milliseconds(ms as i64));
    }
    if let Some(epoch) = payload.get("time").and_then(|v| v.as_i64()) {
        return Utc
            .timestamp_millis_opt(epoch)
            .single()
            .ok_or_else(|| ProjectorError::Normalize("invalid epoch time".into()));
    }
    Ok(Utc::now())
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|field| field.as_str())
        .map(str::to_owned)
}

fn nested_string_field(value: Option<&serde_json::Value>, key: &str) -> Option<String> {
    value.and_then(|object| string_field(object, key))
}

fn json_request_id(value: Option<&serde_json::Value>) -> Option<String> {
    value.map(|field| match field {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

fn parse_act_chain(value: Option<&serde_json::Value>) -> Option<Vec<ActChainHop>> {
    let array = value?.as_array()?;
    let mut hops = Vec::with_capacity(array.len());
    for entry in array {
        hops.push(ActChainHop {
            sub: entry.get("sub")?.as_str()?.to_owned(),
            agent_id: entry.get("agent_id").and_then(|v| v.as_str()).map(str::to_owned),
            wkl: entry
                .get("wkl")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_owned(),
            iat: entry.get("iat")?.as_i64()?,
        });
    }
    Some(hops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::AuditEnvelope;

    #[test]
    fn normalize_sts_success() {
        let envelope = AuditEnvelope {
            event_id: "evt-sts".into(),
            subject: "mcp.audit.sts.success".into(),
            payload: serde_json::json!({
                "outcome": "success",
                "ts": "2026-05-27T14:02:01Z",
                "tenant": "acme",
                "decision_reason": "ok",
                "request": {
                    "audience": "urn:trogon:a2a:agent:acme:oncall-agent",
                    "purpose": "incident.response",
                    "scope": "mcp:tools",
                    "wkl": "spiffe://acme.local/ns/prod/sa/triage-router"
                },
                "minted": {
                    "sub": "agent:acme/oncall-agent",
                    "aud": "urn:trogon:a2a:agent:acme:oncall-agent",
                    "scope": "mcp:tools"
                },
                "agent_id": "acme/triage-router",
                "session_id": "sess-1"
            }),
        };
        let event = normalize_envelope(&envelope).expect("normalize");
        assert_eq!(event.outcome, "allow");
        assert_eq!(event.target_aud.as_deref(), Some("urn:trogon:a2a:agent:acme:oncall-agent"));
        assert_eq!(event.session_id.as_deref(), Some("sess-1"));
    }
}

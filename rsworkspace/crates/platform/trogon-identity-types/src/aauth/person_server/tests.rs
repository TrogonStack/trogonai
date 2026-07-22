use super::*;

#[test]
fn token_request_serializes_only_required_field_by_default() {
    let req = TokenRequest::new("eyJhbGc...");
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json, serde_json::json!({"resource_token": "eyJhbGc..."}));
}

#[test]
fn token_request_matches_draft_example() {
    let raw = serde_json::json!({
        "resource_token": "eyJhbGc...",
        "justification": "Find available meeting times"
    });
    let req: TokenRequest = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(req.justification.as_deref(), Some("Find available meeting times"));
    assert_eq!(serde_json::to_value(&req).unwrap(), raw);
}

#[test]
fn token_grant_response_matches_draft_example() {
    let raw = serde_json::json!({"auth_token": "eyJhbGc...", "expires_in": 3600});
    let resp: TokenGrantResponse = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(resp.expires_in, 3600);
    assert_eq!(serde_json::to_value(&resp).unwrap(), raw);
}

#[test]
fn pending_response_helper_and_status_checks() {
    let pending = PendingResponse::pending();
    assert_eq!(pending.status, "pending");
    assert!(!pending.is_interacting());

    let interacting = PendingResponse {
        status: PendingStatus::Interacting.as_str().to_string(),
        ..PendingResponse::pending()
    };
    assert!(interacting.is_interacting());
}

#[test]
fn pending_response_matches_clarification_example() {
    let raw = serde_json::json!({
        "status": "pending",
        "clarification": "Why do you need write access to my calendar?",
        "timeout": 120
    });
    let resp: PendingResponse = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(resp.timeout, Some(120));
    assert_eq!(serde_json::to_value(&resp).unwrap(), raw);
}

#[test]
fn pending_response_matches_claims_required_example() {
    let raw = serde_json::json!({
        "status": "pending",
        "required_claims": ["email", "tenant"]
    });
    let resp: PendingResponse = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(
        resp.required_claims.unwrap(),
        vec!["email".to_string(), "tenant".to_string()]
    );
}

#[test]
fn resource_interaction_claim_serde_round_trip() {
    let interaction = ResourceInteraction {
        url: "https://resource.example/interaction".into(),
        code: "A1B2-C3D4".into(),
    };
    let json = serde_json::to_value(&interaction).unwrap();
    let back: ResourceInteraction = serde_json::from_value(json).unwrap();
    assert_eq!(back, interaction);
}

#[test]
fn clarification_response_request_matches_draft_example() {
    let raw = serde_json::json!({
        "action": "clarification_response",
        "clarification_response": "I need to create a meeting invite for the participants you listed."
    });
    let req: ClarificationResponseRequest = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(req.action, ClarificationAction::ClarificationResponse);
    assert_eq!(serde_json::to_value(&req).unwrap(), raw);
}

#[test]
fn updated_request_matches_draft_example() {
    let raw = serde_json::json!({
        "action": "updated_request",
        "resource_token": "eyJ...",
        "justification": "I've reduced my request to read-only access."
    });
    let req: UpdatedRequest = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(req.action, ClarificationAction::UpdatedRequest);
    assert_eq!(serde_json::to_value(&req).unwrap(), raw);
}

#[test]
fn permission_request_matches_draft_example() {
    let raw = serde_json::json!({
        "action": "SendEmail",
        "description": "Send the proposed itinerary to the user",
        "parameters": {
            "to": "user@example.com",
            "subject": "Japan trip itinerary"
        },
        "mission": {
            "approver": "https://ps.example",
            "s256": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        }
    });
    let req: PermissionRequest = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(req.action, "SendEmail");
    assert_eq!(req.mission.as_ref().unwrap().approver, "https://ps.example");
    assert_eq!(serde_json::to_value(&req).unwrap(), raw);
}

#[test]
fn permission_response_granted_and_denied() {
    let granted = PermissionResponse {
        permission: PermissionDecision::Granted,
        reason: None,
    };
    assert_eq!(
        serde_json::to_value(&granted).unwrap(),
        serde_json::json!({"permission": "granted"})
    );

    let denied = PermissionResponse {
        permission: PermissionDecision::Denied,
        reason: Some("Not within mission scope".into()),
    };
    let json = serde_json::to_value(&denied).unwrap();
    assert_eq!(json["permission"], "denied");
    assert_eq!(json["reason"], "Not within mission scope");
}

#[test]
fn audit_request_matches_draft_example() {
    let raw = serde_json::json!({
        "mission": {
            "approver": "https://ps.example",
            "s256": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        },
        "action": "WebSearch",
        "description": "Searched for flights to Tokyo in May",
        "parameters": {"query": "flights to Tokyo May 2026"},
        "result": {"status": "completed", "summary": "Found 12 flight options"}
    });
    let req: AuditRequest = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(req.action, "WebSearch");
    assert_eq!(serde_json::to_value(&req).unwrap(), raw);
}

#[test]
fn interaction_request_relay_matches_draft_example() {
    let raw = serde_json::json!({
        "type": "interaction",
        "description": "The booking service needs you to confirm payment",
        "url": "https://booking.example/confirm",
        "code": "X7K2-M9P4",
        "mission": {
            "approver": "https://ps.example",
            "s256": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        }
    });
    let req: InteractionRequest = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(req.type_, InteractionRequestType::Interaction);
    assert_eq!(serde_json::to_value(&req).unwrap(), raw);
}

#[test]
fn interaction_request_completion_matches_draft_example() {
    let raw = serde_json::json!({
        "type": "completion",
        "summary": "# Japan Trip Booked",
        "mission": {
            "approver": "https://ps.example",
            "s256": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        }
    });
    let req: InteractionRequest = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(req.type_, InteractionRequestType::Completion);
    assert!(req.url.is_none());
    assert_eq!(serde_json::to_value(&req).unwrap(), raw);
}

#[test]
fn question_answer_matches_draft_example() {
    let raw = serde_json::json!({"answer": "Yes, go ahead with the refundable option."});
    let answer: QuestionAnswer = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(serde_json::to_value(&answer).unwrap(), raw);
}

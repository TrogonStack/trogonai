use super::*;

#[test]
fn mission_proposal_serde_matches_draft_example() {
    let proposal = MissionProposal {
        description: "# Plan Japan Vacation".into(),
        tools: Some(vec![
            MissionTool {
                name: "WebSearch".into(),
                description: Some("Search the web".into()),
            },
            MissionTool {
                name: "BookFlight".into(),
                description: Some("Book flights".into()),
            },
        ]),
    };
    let json = serde_json::to_value(&proposal).unwrap();
    assert_eq!(json["tools"][0]["name"], "WebSearch");
    let back: MissionProposal = serde_json::from_value(json).unwrap();
    assert_eq!(back, proposal);
}

#[test]
fn mission_blob_serde_matches_draft_example() {
    let raw = serde_json::json!({
        "approver": "https://ps.example",
        "agent": "aauth:assistant@agent.example",
        "approved_at": "2026-04-07T14:30:00Z",
        "description": "# Plan Japan Vacation",
        "approved_tools": [
            {"name": "WebSearch", "description": "Search the web"},
            {"name": "Read", "description": "Read files and web pages"}
        ],
        "capabilities": ["interaction", "payment"]
    });
    let blob: MissionBlob = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(blob.approver, "https://ps.example");
    assert_eq!(
        blob.capabilities.as_ref().unwrap(),
        &vec!["interaction".to_string(), "payment".to_string()]
    );
    let round = serde_json::to_value(&blob).unwrap();
    assert_eq!(round, raw);
}

#[test]
fn mission_blob_optional_fields_omitted_when_absent() {
    let blob = MissionBlob {
        approver: "https://ps.example".into(),
        agent: "aauth:assistant@agent.example".into(),
        approved_at: "2026-04-07T14:30:00Z".into(),
        description: "desc".into(),
        approved_tools: None,
        capabilities: None,
    };
    let json = serde_json::to_value(&blob).unwrap();
    assert!(json.get("approved_tools").is_none());
    assert!(json.get("capabilities").is_none());
}

#[test]
fn mission_completion_serde_matches_draft_example() {
    let completion = MissionCompletion {
        summary: "# Japan Trip Booked".into(),
        mission: super::super::MissionRef {
            approver: "https://ps.example".into(),
            s256: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
        },
    };
    let json = serde_json::to_value(&completion).unwrap();
    assert_eq!(json["mission"]["approver"], "https://ps.example");
    let back: MissionCompletion = serde_json::from_value(json).unwrap();
    assert_eq!(back, completion);
}

#[test]
fn mission_header_parse_and_render_round_trip() {
    let raw = "approver=\"https://ps.example\"; s256=\"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk\"";
    let header = MissionHeader::parse(raw).unwrap();
    assert_eq!(header.approver, "https://ps.example");
    assert_eq!(header.s256, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(header.to_header_value(), raw);
}

#[test]
fn mission_header_parse_skips_empty_segments_from_stray_semicolons() {
    let raw = "approver=\"https://ps.example\";; s256=\"hash\";";
    let header = MissionHeader::parse(raw).unwrap();
    assert_eq!(header.approver, "https://ps.example");
    assert_eq!(header.s256, "hash");
}

#[test]
fn mission_header_parse_ignores_unknown_parameters() {
    let raw = "approver=\"https://ps.example\"; s256=\"hash\"; future-param=\"x\"";
    let header = MissionHeader::parse(raw).unwrap();
    assert_eq!(header.approver, "https://ps.example");
    assert_eq!(header.s256, "hash");
}

#[test]
fn mission_header_parse_accepts_unquoted_values() {
    let raw = "approver=https://ps.example; s256=hash";
    let header = MissionHeader::parse(raw).unwrap();
    assert_eq!(header.approver, "https://ps.example");
    assert_eq!(header.s256, "hash");
}

#[test]
fn mission_header_converts_into_mission_ref() {
    let header = MissionHeader {
        approver: "https://ps.example".into(),
        s256: "hash".into(),
    };
    let mission_ref: super::super::MissionRef = header.into();
    assert_eq!(mission_ref.approver, "https://ps.example");
    assert_eq!(mission_ref.s256, "hash");
}

#[test]
fn mission_log_entry_variants_serde_round_trip() {
    let entries = vec![
        MissionLogEntry::TokenRequest {
            justification: Some("Find available meeting times".into()),
        },
        MissionLogEntry::PermissionRequest {
            action: "SendEmail".into(),
            description: None,
        },
        MissionLogEntry::PermissionResponse {
            permission: "granted".into(),
            reason: None,
        },
        MissionLogEntry::AuditRecord {
            action: "WebSearch".into(),
            description: Some("Searched for flights".into()),
        },
        MissionLogEntry::InteractionRequest {
            type_: "completion".into(),
        },
        MissionLogEntry::ClarificationChat {
            clarification: "Why do you need write access?".into(),
            clarification_response: Some("To create invites".into()),
        },
    ];
    for entry in entries {
        let json = serde_json::to_string(&entry).unwrap();
        let back: MissionLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }
}

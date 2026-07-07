use trogon_identity_types::aauth::mission::MissionLogEntry;

use super::*;

fn blob() -> MissionBlob {
    MissionBlob {
        approver: "https://ps.example".to_string(),
        agent: "aauth:assistant@agent.example".to_string(),
        approved_at: "2026-01-01T00:00:00Z".to_string(),
        description: "Plan Japan vacation".to_string(),
        approved_tools: None,
        capabilities: None,
    }
}

#[test]
fn mission_id_is_deterministic_over_same_bytes() {
    let bytes = serde_json::to_vec(&blob()).unwrap();
    let a = MissionId::from_blob_bytes(&bytes);
    let b = MissionId::from_blob_bytes(&bytes);
    assert_eq!(a, b);
}

#[test]
fn mission_id_differs_for_different_bytes() {
    let bytes_a = serde_json::to_vec(&blob()).unwrap();
    let mut other = blob();
    other.description = "Different mission".to_string();
    let bytes_b = serde_json::to_vec(&other).unwrap();
    assert_ne!(
        MissionId::from_blob_bytes(&bytes_a),
        MissionId::from_blob_bytes(&bytes_b)
    );
}

#[test]
fn approve_creates_active_mission_with_empty_log() {
    let bytes = serde_json::to_vec(&blob()).unwrap();
    let mission = Mission::approve(bytes, blob()).unwrap();
    assert!(mission.is_active());
    assert!(mission.log.is_empty());
}

#[test]
fn mission_ref_uses_blob_approver_and_id_as_s256() {
    let bytes = serde_json::to_vec(&blob()).unwrap();
    let mission = Mission::approve(bytes, blob()).unwrap();
    let mission_ref = mission.mission_ref();
    assert_eq!(mission_ref.approver, "https://ps.example");
    assert_eq!(mission_ref.s256, mission.id.as_str());
}

#[test]
fn append_log_accumulates_entries_in_order() {
    let bytes = serde_json::to_vec(&blob()).unwrap();
    let mut mission = Mission::approve(bytes, blob()).unwrap();
    mission.append_log(MissionLogEntry::TokenRequest { justification: None });
    mission.append_log(MissionLogEntry::AuditRecord {
        action: "WebSearch".to_string(),
        description: None,
    });
    assert_eq!(mission.log.len(), 2);
}

#[test]
fn complete_transitions_to_terminated_and_is_one_way() {
    let bytes = serde_json::to_vec(&blob()).unwrap();
    let mut mission = Mission::approve(bytes, blob()).unwrap();
    mission.complete();
    assert!(!mission.is_active());
    assert_eq!(mission.status, MissionStatus::Terminated);
}

#[test]
fn mission_context_from_mission_carries_status() {
    let bytes = serde_json::to_vec(&blob()).unwrap();
    let mut mission = Mission::approve(bytes, blob()).unwrap();
    mission.complete();
    let ctx = MissionContext::from(&mission);
    assert_eq!(ctx.status, MissionStatus::Terminated);
    assert_eq!(ctx.mission_ref.s256, mission.id.as_str());
}

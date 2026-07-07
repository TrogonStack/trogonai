use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims};

use super::*;
use crate::mission::Mission;
use crate::pending::PendingRequest;

fn agent_claims() -> AgentClaims {
    AgentClaims {
        iss: "https://ap.example".to_string(),
        sub: "aauth:assistant@agent.example".to_string(),
        jti: "agent-jti".to_string(),
        iat: 0,
        exp: 1000,
        dwk: "aauth-agent.json".to_string(),
        cnf: Cnf {
            jwk: serde_json::json!({"kty": "EC"}),
        },
        ps: None,
    }
}

fn resource_claims() -> ResourceClaims {
    ResourceClaims {
        iss: "https://calendar.example".to_string(),
        aud: "https://ps.example".to_string(),
        jti: "resource-jti".to_string(),
        iat: 0,
        exp: 1000,
        dwk: "aauth-resource.json".to_string(),
        agent: "aauth:assistant@agent.example".to_string(),
        agent_jkt: "jkt".to_string(),
        scope: "calendar.read".to_string(),
        mission: None,
    }
}

#[tokio::test]
async fn insert_and_get_pending_round_trips() {
    let store = InMemoryStore::new();
    let pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    let id = pending.id.clone();
    store.insert_pending(pending.clone()).await.unwrap();
    let fetched = store.get_pending(&id).await.unwrap().unwrap();
    assert_eq!(fetched.id, id);
}

#[tokio::test]
async fn find_pending_by_correlation_returns_inserted_id() {
    let store = InMemoryStore::new();
    let pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    let key = pending.correlation_key.clone();
    let id = pending.id.clone();
    store.insert_pending(pending).await.unwrap();

    let found = store.find_pending_by_correlation(&key).await.unwrap();
    assert_eq!(found, Some(id));
}

#[tokio::test]
async fn remove_pending_clears_correlation_index() {
    let store = InMemoryStore::new();
    let pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    let key = pending.correlation_key.clone();
    let id = pending.id.clone();
    store.insert_pending(pending).await.unwrap();
    store.remove_pending(&id).await.unwrap();

    assert!(store.get_pending(&id).await.unwrap().is_none());
    assert!(store.find_pending_by_correlation(&key).await.unwrap().is_none());
}

#[tokio::test]
async fn update_pending_overwrites_existing_entry() {
    let store = InMemoryStore::new();
    let mut pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    let id = pending.id.clone();
    store.insert_pending(pending.clone()).await.unwrap();

    pending.grant("token".to_string(), 3600);
    store.update_pending(pending).await.unwrap();

    let fetched = store.get_pending(&id).await.unwrap().unwrap();
    assert!(fetched.is_terminal());
}

#[tokio::test]
async fn mission_insert_get_update_round_trips() {
    let store = InMemoryStore::new();
    let blob = trogon_identity_types::aauth::mission::MissionBlob {
        approver: "https://ps.example".to_string(),
        agent: "aauth:assistant@agent.example".to_string(),
        approved_at: "2026-01-01T00:00:00Z".to_string(),
        description: "Plan trip".to_string(),
        approved_tools: None,
        capabilities: None,
    };
    let bytes = serde_json::to_vec(&blob).unwrap();
    let mission = Mission::approve(bytes, blob).expect("valid mission blob");
    let id = mission.id.clone();

    store.insert_mission(mission).await.unwrap();
    let mut fetched = store.get_mission(&id).await.unwrap().unwrap();
    assert!(fetched.is_active());

    fetched.complete();
    store.update_mission(fetched).await.unwrap();

    let after = store.get_mission(&id).await.unwrap().unwrap();
    assert!(!after.is_active());
}

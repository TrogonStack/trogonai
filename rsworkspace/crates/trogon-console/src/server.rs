use axum::{Router, routing::get, routing::post};
use std::sync::Arc;

use crate::{
    routes::{agents, credentials, environments, mcp_registry, sessions, skills},
    store::traits::{
        AgentRepository, CredentialRepository, EnvironmentRepository, SessionRepository,
        SkillRepository,
    },
};

pub struct AppState {
    pub agents: Arc<dyn AgentRepository>,
    pub skills: Arc<dyn SkillRepository>,
    pub environments: Arc<dyn EnvironmentRepository>,
    pub credentials: Arc<dyn CredentialRepository>,
    pub sessions: Arc<dyn SessionRepository>,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        // Sessions (read-only)
        .route("/sessions", get(sessions::list_sessions))
        .route("/sessions/{tenant_id}/{session_id}", get(sessions::get_session))
        // Agents
        .route("/agents", get(agents::list_agents).post(agents::create_agent))
        .route(
            "/agents/{id}",
            get(agents::get_agent)
                .put(agents::update_agent)
                .delete(agents::delete_agent),
        )
        .route("/agents/{id}/versions", get(agents::list_agent_versions))
        .route("/agents/{id}/sessions", get(agents::list_agent_sessions))
        // Skills
        .route("/skills", get(skills::list_skills).post(skills::create_skill))
        .route("/skills/{id}", get(skills::get_skill).delete(skills::delete_skill))
        .route(
            "/skills/{id}/versions",
            get(skills::list_skill_versions).post(skills::create_skill_version),
        )
        // Environments
        .route(
            "/environments",
            get(environments::list_environments).post(environments::create_environment),
        )
        .route(
            "/environments/{id}",
            get(environments::get_environment)
                .put(environments::update_environment)
                .delete(environments::delete_environment),
        )
        .route("/environments/{id}/archive", post(environments::archive_environment))
        .route("/environments/{id}/vault", get(credentials::get_vault))
        .route(
            "/environments/{id}/credentials",
            get(credentials::list_credentials).post(credentials::create_credential),
        )
        .route(
            "/environments/{id}/credentials/{cred_id}",
            get(credentials::get_credential).delete(credentials::delete_credential),
        )
        // MCP registry
        .route("/mcp-registry", get(mcp_registry::list_mcp_servers))
        // Health
        .route("/-/health", get(|| async { "ok" }))
        .with_state(state)
}

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::agent::{AgentDefinition, AgentStatus, CreateAgentRequest, UpdateAgentRequest},
    server::AppState,
};

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs}")
}

pub async fn list_agents(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let agents = state.agents.list().await.map_err(AppError::Store)?;
    Ok(Json(agents))
}

pub async fn get_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state
        .agents
        .get(&id)
        .await
        .map_err(AppError::Store)?
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("agent {id} not found")))
}

pub async fn create_agent(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateAgentRequest>,
) -> Result<impl IntoResponse, AppError> {
    let now = now_iso();
    let agent = AgentDefinition {
        id: format!("agent_{}", Uuid::new_v4().simple()),
        name: req.name,
        description: req.description,
        status: AgentStatus::Active,
        version: 1,
        model: req.model,
        system_prompt: req.system_prompt,
        skill_ids: req.skill_ids,
        tools: req.tools,
        mcp_servers: req.mcp_servers,
        metadata: req.metadata,
        created_at: now.clone(),
        updated_at: now,
    };
    state.agents.put(&agent).await.map_err(AppError::Store)?;
    Ok((StatusCode::CREATED, Json(agent)))
}

pub async fn update_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateAgentRequest>,
) -> Result<impl IntoResponse, AppError> {
    let mut agent = state
        .agents
        .get(&id)
        .await
        .map_err(AppError::Store)?
        .ok_or_else(|| AppError::NotFound(format!("agent {id} not found")))?;

    if let Some(name) = req.name            { agent.name = name; }
    if let Some(desc) = req.description     { agent.description = desc; }
    if let Some(status) = req.status        { agent.status = status; }
    if let Some(model) = req.model          { agent.model = model; }
    if let Some(prompt) = req.system_prompt { agent.system_prompt = prompt; }
    if let Some(skills) = req.skill_ids     { agent.skill_ids = skills; }
    if let Some(tools) = req.tools          { agent.tools = tools; }
    if let Some(mcps) = req.mcp_servers     { agent.mcp_servers = mcps; }
    if let Some(meta) = req.metadata        { agent.metadata = meta; }

    agent.version += 1;
    agent.updated_at = now_iso();

    state.agents.put(&agent).await.map_err(AppError::Store)?;
    Ok(Json(agent))
}

pub async fn delete_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state.agents.delete(&id).await.map_err(AppError::Store)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_agent_versions(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let versions = state.agents.list_versions(&id).await.map_err(AppError::Store)?;
    Ok(Json(versions))
}

pub async fn list_agent_sessions(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let sessions = state.sessions.list_by_tenant(&id).await.map_err(AppError::Store)?;
    Ok(Json(sessions))
}

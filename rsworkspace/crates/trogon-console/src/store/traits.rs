use std::future::Future;
use std::pin::Pin;

use crate::models::{
    agent::{AgentDefinition, AgentVersion},
    credential::{Credential, CredentialVault},
    environment::Environment,
    session::ConsoleSession,
    skill::{Skill, SkillVersion},
};

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
type Res<T> = Result<T, String>;

// ── Agent ─────────────────────────────────────────────────────────────────────

pub trait AgentRepository: Send + Sync + 'static {
    fn list(&self) -> BoxFuture<'_, Res<Vec<AgentDefinition>>>;
    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Res<Option<AgentDefinition>>>;
    fn put<'a>(&'a self, agent: &'a AgentDefinition) -> BoxFuture<'a, Res<()>>;
    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Res<()>>;
    fn list_versions<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Res<Vec<AgentVersion>>>;
}

// ── Skill ─────────────────────────────────────────────────────────────────────

pub trait SkillRepository: Send + Sync + 'static {
    fn list(&self) -> BoxFuture<'_, Res<Vec<Skill>>>;
    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Res<Option<Skill>>>;
    fn put<'a>(&'a self, skill: &'a Skill) -> BoxFuture<'a, Res<()>>;
    fn list_versions<'a>(&'a self, skill_id: &'a str) -> BoxFuture<'a, Res<Vec<SkillVersion>>>;
    fn put_version<'a>(&'a self, version: &'a SkillVersion) -> BoxFuture<'a, Res<()>>;
}

// ── Environment ───────────────────────────────────────────────────────────────

pub trait EnvironmentRepository: Send + Sync + 'static {
    fn list(&self) -> BoxFuture<'_, Res<Vec<Environment>>>;
    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Res<Option<Environment>>>;
    fn put<'a>(&'a self, env: &'a Environment) -> BoxFuture<'a, Res<()>>;
    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Res<()>>;
}

// ── Credential ────────────────────────────────────────────────────────────────

pub trait CredentialRepository: Send + Sync + 'static {
    fn get_or_create_vault<'a>(&'a self, env_id: &'a str) -> BoxFuture<'a, Res<CredentialVault>>;
    fn get_vault<'a>(&'a self, env_id: &'a str) -> BoxFuture<'a, Res<Option<CredentialVault>>>;
    fn list<'a>(&'a self, env_id: &'a str) -> BoxFuture<'a, Res<Vec<Credential>>>;
    fn put<'a>(&'a self, cred: &'a Credential) -> BoxFuture<'a, Res<()>>;
    fn delete<'a>(&'a self, env_id: &'a str, cred_id: &'a str) -> BoxFuture<'a, Res<()>>;
}

// ── Session ───────────────────────────────────────────────────────────────────

pub trait SessionRepository: Send + Sync + 'static {
    fn list(&self) -> BoxFuture<'_, Res<Vec<ConsoleSession>>>;
    fn list_by_tenant<'a>(&'a self, tenant_id: &'a str) -> BoxFuture<'a, Res<Vec<ConsoleSession>>>;
    fn get<'a>(
        &'a self,
        tenant_id: &'a str,
        session_id: &'a str,
    ) -> BoxFuture<'a, Res<Option<ConsoleSession>>>;
}

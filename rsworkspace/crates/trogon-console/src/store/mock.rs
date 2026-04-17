use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::models::{
    agent::{AgentDefinition, AgentVersion},
    credential::{Credential, CredentialVault},
    environment::Environment,
    session::ConsoleSession,
    skill::{Skill, SkillVersion},
};
use crate::store::traits::{
    AgentRepository, CredentialRepository, EnvironmentRepository, SessionRepository,
    SkillRepository,
};

// ── Agent ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct MockAgentStore {
    agents: Arc<Mutex<HashMap<String, AgentDefinition>>>,
    versions: Arc<Mutex<HashMap<String, Vec<AgentVersion>>>>,
}

impl MockAgentStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AgentRepository for MockAgentStore {
    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<AgentDefinition>, String>> + Send + '_>> {
        let mut agents: Vec<_> = self.agents.lock().unwrap().values().cloned().collect();
        agents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Box::pin(std::future::ready(Ok(agents)))
    }

    fn get<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<AgentDefinition>, String>> + Send + 'a>> {
        let result = self.agents.lock().unwrap().get(id).cloned();
        Box::pin(std::future::ready(Ok(result)))
    }

    fn put<'a>(&'a self, agent: &'a AgentDefinition) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        let ver = AgentVersion {
            version: agent.version,
            updated_at: agent.updated_at.clone(),
            model_id: agent.model.id.clone(),
        };
        self.versions
            .lock()
            .unwrap()
            .entry(agent.id.clone())
            .or_default()
            .push(ver);
        self.agents.lock().unwrap().insert(agent.id.clone(), agent.clone());
        Box::pin(std::future::ready(Ok(())))
    }

    fn delete<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        self.agents.lock().unwrap().remove(id);
        Box::pin(std::future::ready(Ok(())))
    }

    fn list_versions<'a>(&'a self, agent_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<AgentVersion>, String>> + Send + 'a>> {
        let mut versions = self
            .versions
            .lock()
            .unwrap()
            .get(agent_id)
            .cloned()
            .unwrap_or_default();
        versions.sort_by_key(|v| v.version);
        Box::pin(std::future::ready(Ok(versions)))
    }
}

// ── Skill ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct MockSkillStore {
    skills: Arc<Mutex<HashMap<String, Skill>>>,
    versions: Arc<Mutex<HashMap<String, SkillVersion>>>,
}

impl MockSkillStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SkillRepository for MockSkillStore {
    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Skill>, String>> + Send + '_>> {
        let mut skills: Vec<_> = self.skills.lock().unwrap().values().cloned().collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Box::pin(std::future::ready(Ok(skills)))
    }

    fn get<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Skill>, String>> + Send + 'a>> {
        let result = self.skills.lock().unwrap().get(id).cloned();
        Box::pin(std::future::ready(Ok(result)))
    }

    fn put<'a>(&'a self, skill: &'a Skill) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        self.skills.lock().unwrap().insert(skill.id.clone(), skill.clone());
        Box::pin(std::future::ready(Ok(())))
    }

    fn list_versions<'a>(&'a self, skill_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<SkillVersion>, String>> + Send + 'a>> {
        let prefix = format!("{skill_id}.");
        let versions: Vec<_> = self.versions
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v.clone())
            .collect();
        Box::pin(std::future::ready(Ok(versions)))
    }

    fn put_version<'a>(&'a self, version: &'a SkillVersion) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        let key = format!("{}.{}", version.skill_id, version.version);
        self.versions.lock().unwrap().insert(key, version.clone());
        Box::pin(std::future::ready(Ok(())))
    }
}

// ── Environment ───────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct MockEnvironmentStore {
    envs: Arc<Mutex<HashMap<String, Environment>>>,
}

impl MockEnvironmentStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EnvironmentRepository for MockEnvironmentStore {
    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Environment>, String>> + Send + '_>> {
        let mut envs: Vec<_> = self.envs.lock().unwrap().values().cloned().collect();
        envs.sort_by(|a, b| a.name.cmp(&b.name));
        Box::pin(std::future::ready(Ok(envs)))
    }

    fn get<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Environment>, String>> + Send + 'a>> {
        let result = self.envs.lock().unwrap().get(id).cloned();
        Box::pin(std::future::ready(Ok(result)))
    }

    fn put<'a>(&'a self, env: &'a Environment) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        self.envs.lock().unwrap().insert(env.id.clone(), env.clone());
        Box::pin(std::future::ready(Ok(())))
    }

    fn delete<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        self.envs.lock().unwrap().remove(id);
        Box::pin(std::future::ready(Ok(())))
    }
}

// ── Credential ────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct MockCredentialStore {
    vaults: Arc<Mutex<HashMap<String, CredentialVault>>>,
    creds: Arc<Mutex<HashMap<String, Credential>>>,
}

impl MockCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialRepository for MockCredentialStore {
    fn get_or_create_vault<'a>(&'a self, env_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<CredentialVault, String>> + Send + 'a>> {
        let mut vaults = self.vaults.lock().unwrap();
        let vault = vaults.entry(env_id.to_string()).or_insert_with(|| CredentialVault {
            id: format!("vlt_{}", uuid::Uuid::new_v4().simple()),
            env_id: env_id.to_string(),
            created_at: "0".to_string(),
        });
        let v = vault.clone();
        Box::pin(std::future::ready(Ok(v)))
    }

    fn get_vault<'a>(&'a self, env_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<CredentialVault>, String>> + Send + 'a>> {
        let result = self.vaults.lock().unwrap().get(env_id).cloned();
        Box::pin(std::future::ready(Ok(result)))
    }

    fn list<'a>(&'a self, env_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Credential>, String>> + Send + 'a>> {
        let prefix = format!("{env_id}.");
        let creds: Vec<_> = self.creds
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v.clone())
            .collect();
        Box::pin(std::future::ready(Ok(creds)))
    }

    fn put<'a>(&'a self, cred: &'a Credential) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        let key = format!("{}.{}", cred.env_id, cred.id);
        self.creds.lock().unwrap().insert(key, cred.clone());
        Box::pin(std::future::ready(Ok(())))
    }

    fn delete<'a>(&'a self, env_id: &'a str, cred_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        let key = format!("{env_id}.{cred_id}");
        self.creds.lock().unwrap().remove(&key);
        Box::pin(std::future::ready(Ok(())))
    }
}

// ── Session ───────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct MockSessionStore {
    sessions: Arc<Mutex<HashMap<String, ConsoleSession>>>,
}

impl MockSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, session: ConsoleSession) {
        let key = format!("{}.{}", session.tenant_id, session.id);
        self.sessions.lock().unwrap().insert(key, session);
    }
}

impl SessionRepository for MockSessionStore {
    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<ConsoleSession>, String>> + Send + '_>> {
        let mut sessions: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Box::pin(std::future::ready(Ok(sessions)))
    }

    fn list_by_tenant<'a>(&'a self, tenant_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<ConsoleSession>, String>> + Send + 'a>> {
        let sessions: Vec<_> = self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.tenant_id == tenant_id)
            .cloned()
            .collect();
        Box::pin(std::future::ready(Ok(sessions)))
    }

    fn get<'a>(&'a self, tenant_id: &'a str, session_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<ConsoleSession>, String>> + Send + 'a>> {
        let key = format!("{tenant_id}.{session_id}");
        let result = self.sessions.lock().unwrap().get(&key).cloned();
        Box::pin(std::future::ready(Ok(result)))
    }
}

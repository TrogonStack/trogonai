mod engine;

use std::collections::HashMap;
use std::sync::RwLock;

use a2a_types::{Artifact, Message};
use wasmtime::{Engine, Module};

use crate::error::RedactionError;
use crate::redactor::{self, Redactor};
use crate::skill_id::SkillId;
use crate::wasm_bundle_path::WasmBundlePath;

pub struct WasmRedactorHost {
    engine: Engine,
    modules: RwLock<HashMap<SkillId, Module>>,
    bundles_base: WasmBundlePath,
}

impl WasmRedactorHost {
    pub fn new(bundles_base: WasmBundlePath) -> Result<Self, RedactionError> {
        Ok(Self {
            engine: engine::new_engine()?,
            modules: RwLock::new(HashMap::new()),
            bundles_base,
        })
    }

    pub fn bundles_base(&self) -> &WasmBundlePath {
        &self.bundles_base
    }

    pub fn register_skill_wasm(&self, skill: SkillId, wasm_binary: &[u8]) -> Result<(), RedactionError> {
        let compiled = Module::from_binary(&self.engine, wasm_binary)
            .map_err(|e| RedactionError::WasmModule(format!("skill {skill}: {e}")))?;
        let mut guard = self
            .modules
            .write()
            .unwrap_or_else(|e| e.into_inner());
        guard.insert(skill, compiled);
        Ok(())
    }

    pub fn register_skill_bundle_file(&self, skill: SkillId) -> Result<(), RedactionError> {
        let path = self.bundles_base.join_skill_wasm(&skill);
        let bytes =
            std::fs::read(&path).map_err(|e| RedactionError::WasmModule(format!("read {}: {e}", path.display())))?;
        self.register_skill_wasm(skill, &bytes)
    }
}

impl Redactor for WasmRedactorHost {
    fn redact_message(&self, message: Message, skill: &SkillId) -> Result<Message, RedactionError> {
        let modules = self.modules.read().unwrap_or_else(|e| {
            tracing::error!("wasm redactor skill module cache poisoned after write failure");
            e.into_inner()
        });

        let Some(wasm_mod) = modules.get(skill) else {
            return Ok(message);
        };

        redactor::redact_message_parts_with(message, |json| engine::redact_part_guest(&self.engine, wasm_mod, json))
    }

    fn redact_artifact(&self, artifact: Artifact, skill: &SkillId) -> Result<Artifact, RedactionError> {
        let modules = self.modules.read().unwrap_or_else(|e| {
            tracing::error!("wasm redactor skill module cache poisoned after write failure");
            e.into_inner()
        });

        let Some(wasm_mod) = modules.get(skill) else {
            return Ok(artifact);
        };

        redactor::redact_artifact_parts_with(artifact, |json| engine::redact_part_guest(&self.engine, wasm_mod, json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_types::{part, Role};

    #[test]
    fn passthrough_when_no_registered_module_for_message() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let msg = Message {
            message_id: "m".into(),
            role: Role::Agent.into(),
            ..Default::default()
        };
        let out = host.redact_message(msg.clone(), &SkillId::new("missing")).unwrap();
        assert_eq!(
            serde_json::to_value(out).unwrap(),
            serde_json::to_value(msg).unwrap()
        );
    }

    #[test]
    fn passthrough_when_no_registered_module_for_artifact() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let art = Artifact {
            artifact_id: "aid".into(),
            ..Default::default()
        };
        let out = host.redact_artifact(art.clone(), &SkillId::new("missing")).unwrap();
        assert_eq!(
            serde_json::to_value(out).unwrap(),
            serde_json::to_value(art).unwrap()
        );
    }

    #[test]
    fn wasm_skill_dispatches_through_message_parts() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let wasm = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/identity_redact_part.wasm"
        ));
        let skill = SkillId::new("fixture");
        host.register_skill_wasm(skill.clone(), wasm).unwrap();

        let msg_in = Message {
            message_id: "m".into(),
            role: Role::Agent.into(),
            parts: vec![a2a_types::Part {
                content: Some(part::Content::Text("x".into())),
                ..Default::default()
            }],
            ..Default::default()
        };
        let got = host.redact_message(msg_in.clone(), &skill).unwrap();
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            serde_json::to_value(msg_in).unwrap()
        );
    }

    #[test]
    fn wasm_skill_dispatches_through_artifact_parts() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let wasm = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/identity_redact_part.wasm"
        ));
        let skill = SkillId::new("fixture");
        host.register_skill_wasm(skill.clone(), wasm).unwrap();

        let art_in = Artifact {
            artifact_id: "a".into(),
            parts: vec![a2a_types::Part {
                content: Some(part::Content::Text("blob".into())),
                ..Default::default()
            }],
            ..Default::default()
        };
        let got = host.redact_artifact(art_in.clone(), &skill).unwrap();
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            serde_json::to_value(art_in).unwrap()
        );
    }
}

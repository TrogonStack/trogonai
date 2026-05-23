mod engine;

use std::collections::HashMap;
use std::sync::RwLock;

use a2a_types::{Artifact, Message};
use wasmtime::{Engine, Module};

use crate::error::RedactionError;
use crate::redactor::Redactor;
use crate::skill_id::SkillId;
use crate::wasm_bundle_path::WasmBundlePath;

pub struct WasmRedactorHost {
    #[allow(dead_code)]
    engine: Engine,
    modules: RwLock<HashMap<SkillId, Module>>,
    #[allow(dead_code)]
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
}

impl Redactor for WasmRedactorHost {
    fn redact_message(&self, message: Message, skill: &SkillId) -> Result<Message, RedactionError> {
        let lookup = match self.modules.read() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::error!("wasm redactor skill module cache poisoned after write failure");
                return Ok(message);
            }
        };

        match lookup.get(skill) {
            None => Ok(message),
            Some(_module) => {
                // TODO(bundle-format): WASM invocation + host ABI lands when Tier 3 bundle format exists.
                unimplemented!("wasm invocation lands when bundle format is fixed")
            }
        }
    }

    fn redact_artifact(&self, artifact: Artifact, skill: &SkillId) -> Result<Artifact, RedactionError> {
        let lookup = match self.modules.read() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::error!("wasm redactor skill module cache poisoned after write failure");
                return Ok(artifact);
            }
        };

        match lookup.get(skill) {
            None => Ok(artifact),
            Some(_module) => {
                // TODO(bundle-format): WASM invocation + host ABI lands when Tier 3 bundle format exists.
                unimplemented!("wasm invocation lands when bundle format is fixed")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_types::Role;

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
}

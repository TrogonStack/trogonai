use std::path::PathBuf;

pub fn bundled_skills_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills")
}

#[cfg(test)]
mod tests {
    use a2a_redaction::{SkillId, SkillManifestRegistry};

    use super::bundled_skills_dir;

    #[test]
    fn bundled_reference_manifests_load() {
        let registry = SkillManifestRegistry::load_from_dir(&bundled_skills_dir()).unwrap();
        for skill_id in [
            "pii.email_mask.v1",
            "credentials.bearer_redact.v1",
            "internal_route.x_internal_strip.v1",
        ] {
            assert!(
                registry.lookup(&SkillId::new(skill_id)).is_some(),
                "missing bundled skill {skill_id}"
            );
        }
    }
}

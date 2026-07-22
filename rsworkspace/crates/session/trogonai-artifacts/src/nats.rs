use trogonai_session_contracts::{ArtifactId, SessionId};

/// Object Store key for a session artifact.
pub fn artifact_object_key(session_id: &SessionId, artifact_id: &ArtifactId) -> String {
    format!("{}/{}", session_id.as_str(), artifact_id.as_str())
}

/// Canonical durable reference to an object store artifact.
pub fn artifact_storage_ref(bucket: &str, object_key: &str) -> String {
    format!("obj://{bucket}/{object_key}")
}

/// Extracts the object store key from a canonical `obj://` storage reference.
pub fn object_key_from_storage_ref(storage_ref: &str) -> Option<&str> {
    let rest = storage_ref.strip_prefix("obj://")?;
    rest.split_once('/').map(|(_, key)| key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogonai_session_contracts::ArtifactId;

    #[test]
    fn storage_ref_round_trips_object_key() {
        let session_id = SessionId::new("sess_test").unwrap();
        let artifact_id = ArtifactId::new("artifact_test_log").unwrap();
        let key = artifact_object_key(&session_id, &artifact_id);
        let storage_ref = artifact_storage_ref("ACP_SESSION_ARTIFACTS", &key);
        assert_eq!(
            storage_ref,
            "obj://ACP_SESSION_ARTIFACTS/sess_test/artifact_test_log"
        );
        assert_eq!(
            object_key_from_storage_ref(&storage_ref),
            Some("sess_test/artifact_test_log")
        );
    }
}

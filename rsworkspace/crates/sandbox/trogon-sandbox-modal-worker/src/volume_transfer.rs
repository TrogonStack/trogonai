use std::path::Path;

use bytes::Bytes;
use futures::TryStreamExt;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use trogon_sandbox_volumes::{ManifestKey, VolumePrefix, build_changed_manifest};

use crate::WorkspaceError;

/// Copies every object under `prefix` into `dest`, keeping its relative path.
pub(crate) async fn pull(store: &dyn ObjectStore, prefix: &VolumePrefix, dest: &Path) -> Result<(), WorkspaceError> {
    tokio::fs::create_dir_all(dest).await?;
    let root = ObjectPath::from(prefix.as_str());
    let objects: Vec<_> = store.list(Some(&root)).try_collect().await?;
    for object in objects {
        // Every listed object lies under the root, so the match always yields its relative parts.
        let parts = object.location.prefix_match(&root).into_iter().flatten();
        let relative = parts.map(|part| part.as_ref().to_owned()).collect::<Vec<_>>().join("/");
        let path = ManifestKey::new(relative)?.to_path(dest);
        tokio::fs::create_dir_all(path.parent().unwrap_or(dest)).await?;
        let bytes = store.get(&object.location).await?.bytes().await?;
        tokio::fs::write(&path, &bytes).await?;
    }
    Ok(())
}

/// Writes every file under `source` that differs from an empty volume to `prefix`.
pub(crate) async fn push(store: &dyn ObjectStore, prefix: &VolumePrefix, source: &Path) -> Result<(), WorkspaceError> {
    let manifest = build_changed_manifest(source, None)?;
    for entry in manifest.entries {
        let bytes = Bytes::from(tokio::fs::read(entry.key.to_path(source)).await?);
        store
            .put(&object_path_for(prefix, &entry.key), PutPayload::from_bytes(bytes))
            .await?;
    }
    Ok(())
}

fn object_path_for(prefix: &VolumePrefix, key: &ManifestKey) -> ObjectPath {
    if prefix.is_root() {
        ObjectPath::from(key.as_str())
    } else {
        ObjectPath::from(format!("{}/{}", prefix.as_str(), key.as_str()))
    }
}

//! Where a host gets a module's bytes, per ADR#0058.
//!
//! The trait is generic rather than boxed so the host is compiled against one
//! source and cannot branch on which it has. Choosing between the
//! implementations is the binary's job, and it is the only place in the crate
//! that knows more than one exists.

use std::error::Error as StdError;
use std::future::Future;
use std::path::{Path, PathBuf};

use async_nats::jetstream::object_store::ObjectStore;
use tokio::io::AsyncReadExt as _;

use crate::module_reference::ModuleReference;

/// A store that can produce a decider component's bytes by reference.
pub trait ModuleSource {
    /// Why this source could not produce the bytes.
    type Error: StdError + Send + Sync + 'static;

    /// Fetches the component `reference` names.
    fn fetch(&self, reference: &ModuleReference) -> impl Future<Output = Result<Vec<u8>, Self::Error>> + Send;

    /// Names this source in a startup log, so an operator reading "module not
    /// found" learns which store was searched.
    fn describe(&self) -> String;
}

/// Reads components from a local directory, for development and tests.
///
/// The filename is the reference's written form, not an arbitrary path, so a
/// directory that a host reads is a directory whose contents are named the way
/// every other layer names them.
#[derive(Debug, Clone)]
pub struct FileModuleSource {
    root: PathBuf,
}

impl FileModuleSource {
    /// Binds the source to the directory it reads.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The path `reference` resolves to under this source's root.
    pub fn path_for(&self, reference: &ModuleReference) -> PathBuf {
        self.root.join(reference.file_name())
    }

    /// Returns the directory this source reads.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl ModuleSource for FileModuleSource {
    type Error = FileModuleSourceError;

    async fn fetch(&self, reference: &ModuleReference) -> Result<Vec<u8>, Self::Error> {
        let path = self.path_for(reference);
        tokio::fs::read(&path)
            .await
            .map_err(|source| FileModuleSourceError::Read { path, source })
    }

    fn describe(&self) -> String {
        format!("directory {}", self.root.display())
    }
}

/// Why a directory could not produce a component.
#[derive(Debug, thiserror::Error)]
pub enum FileModuleSourceError {
    /// The file naming the reference could not be read.
    #[error("failed to read '{path}'", path = path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Reads components from a JetStream object store, the store ADR#0058 picks.
#[derive(Clone)]
pub struct ObjectStoreModuleSource {
    bucket: String,
    store: ObjectStore,
}

impl ObjectStoreModuleSource {
    /// Opens an existing bucket without creating it.
    ///
    /// Creating one here would turn "the publisher never ran" into "the module
    /// is missing from a bucket that exists", which reads like a typo in a
    /// reference rather than a step nobody performed.
    pub async fn open(
        js: &async_nats::jetstream::Context,
        bucket: impl Into<String>,
    ) -> Result<Self, OpenModuleBucketError> {
        let bucket = bucket.into();
        let store = js
            .get_object_store(&bucket)
            .await
            .map_err(|source| OpenModuleBucketError {
                bucket: bucket.clone(),
                source,
            })?;
        Ok(Self { bucket, store })
    }

    /// Opens the bucket, creating it if this is the first publish into it.
    ///
    /// The publishing counterpart to [`Self::open`]: a publisher is the thing
    /// whose lifecycle the bucket belongs to, so it is the one place that may
    /// bring the bucket into existence.
    pub async fn provision(
        js: &async_nats::jetstream::Context,
        bucket: impl Into<String>,
    ) -> Result<Self, ProvisionModuleBucketError> {
        let bucket = bucket.into();
        let config = async_nats::jetstream::object_store::Config {
            bucket: bucket.clone(),
            ..Default::default()
        };
        match js.create_object_store(config).await {
            Ok(store) => Ok(Self { bucket, store }),
            Err(create) => {
                let store = js
                    .get_object_store(&bucket)
                    .await
                    .map_err(|source| ProvisionModuleBucketError {
                        bucket: bucket.clone(),
                        create: Box::new(create),
                        source,
                    })?;
                Ok(Self { bucket, store })
            }
        }
    }

    /// Publishes `bytes` as the component `reference` names.
    ///
    /// Overwriting rather than refusing an existing key is deliberate: a
    /// republished version is the one case where a store entry legitimately
    /// changes, and a publisher that had to delete first would leave the bucket
    /// briefly missing a module every host is configured to fetch.
    pub async fn publish(&self, reference: &ModuleReference, bytes: &[u8]) -> Result<(), PublishModuleError> {
        let key = reference.object_key();
        self.store
            .put(key.as_str(), &mut &bytes[..])
            .await
            .map_err(|source| PublishModuleError {
                key,
                bucket: self.bucket.clone(),
                source,
            })?;
        Ok(())
    }

    /// Returns the bucket this source reads.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }
}

impl ModuleSource for ObjectStoreModuleSource {
    type Error = ObjectStoreModuleSourceError;

    async fn fetch(&self, reference: &ModuleReference) -> Result<Vec<u8>, Self::Error> {
        let key = reference.object_key();
        let mut object = self
            .store
            .get(&key)
            .await
            .map_err(|source| ObjectStoreModuleSourceError::Get {
                key: key.clone(),
                bucket: self.bucket.clone(),
                source,
            })?;
        let mut bytes = Vec::new();
        object
            .read_to_end(&mut bytes)
            .await
            .map_err(|source| ObjectStoreModuleSourceError::Read { key, source })?;
        Ok(bytes)
    }

    fn describe(&self) -> String {
        format!("object store bucket '{}'", self.bucket)
    }
}

/// The module bucket could not be opened.
#[derive(Debug, thiserror::Error)]
#[error("failed to open module bucket '{bucket}'")]
pub struct OpenModuleBucketError {
    bucket: String,
    #[source]
    source: async_nats::jetstream::context::ObjectStoreError,
}

/// The module bucket could neither be created nor opened.
#[derive(Debug, thiserror::Error)]
#[error("failed to provision module bucket '{bucket}' (creating it failed with: {create})")]
pub struct ProvisionModuleBucketError {
    bucket: String,
    create: Box<async_nats::jetstream::context::CreateObjectStoreError>,
    #[source]
    source: async_nats::jetstream::context::ObjectStoreError,
}

/// A component could not be written into the module bucket.
#[derive(Debug, thiserror::Error)]
#[error("failed to publish '{key}' into bucket '{bucket}'")]
pub struct PublishModuleError {
    key: String,
    bucket: String,
    #[source]
    source: async_nats::jetstream::object_store::PutError,
}

/// Why an object store could not produce a component.
#[derive(Debug, thiserror::Error)]
pub enum ObjectStoreModuleSourceError {
    /// The bucket holds no object under the reference's key.
    #[error("no object '{key}' in bucket '{bucket}'")]
    Get {
        key: String,
        bucket: String,
        #[source]
        source: async_nats::jetstream::object_store::GetError,
    },
    /// The object exists but its chunks could not be read through.
    #[error("failed to read object '{key}'")]
    Read {
        key: String,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests;

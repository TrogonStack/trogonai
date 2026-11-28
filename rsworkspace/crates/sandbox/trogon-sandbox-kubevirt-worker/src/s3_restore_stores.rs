use std::sync::Arc;

use object_store::ObjectStore;
use object_store::aws::AmazonS3Builder;
use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_volumes::{VolumeEndpoint, VolumeRegion};
use trogon_std::env::ReadEnv;

use crate::constants::{AWS_ACCESS_KEY_ID_ENV, AWS_SECRET_ACCESS_KEY_ENV, AWS_SESSION_TOKEN_ENV};
use crate::{RestoreError, RestoreStores};

/// Opens S3 stores with the credentials in the worker's environment.
///
/// Only the three credential variables are read, through `env`, so no other variable can change where restores are
/// staged. Without any credential the store uses the instance or pod identity of the host. A partial set, such as a
/// key id without its secret or a session token without a key pair, is refused rather than ignored.
pub struct S3RestoreStores<E> {
    env: E,
}

impl<E> S3RestoreStores<E> {
    pub fn new(env: E) -> Self {
        Self { env }
    }
}

impl<E: ReadEnv> S3RestoreStores<E> {
    /// The value of `name`, or `None` when it is unset. A value that is not valid Unicode is refused so a
    /// malformed credential never silently switches the store to the host identity.
    fn optional_var(&self, name: &'static str) -> Result<Option<String>, RestoreError> {
        match self.env.var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(RestoreError::CredentialNotUnicode { name }),
        }
    }
}

impl<E> RestoreStores for S3RestoreStores<E>
where
    E: ReadEnv + Send + Sync,
{
    fn open(
        &self,
        bucket: &S3BucketName,
        region: &VolumeRegion,
        endpoint: Option<&VolumeEndpoint>,
    ) -> Result<Arc<dyn ObjectStore>, RestoreError> {
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(bucket.as_str())
            .with_region(region.as_str());
        match (
            self.optional_var(AWS_ACCESS_KEY_ID_ENV)?,
            self.optional_var(AWS_SECRET_ACCESS_KEY_ENV)?,
            self.optional_var(AWS_SESSION_TOKEN_ENV)?,
        ) {
            (Some(key), Some(secret), token) => {
                builder = builder.with_access_key_id(key).with_secret_access_key(secret);
                if let Some(token) = token {
                    builder = builder.with_token(token);
                }
            }
            (None, None, None) => {}
            _ => return Err(RestoreError::IncompleteCredentials),
        }
        if let Some(endpoint) = endpoint {
            builder = builder
                .with_endpoint(endpoint.as_str())
                .with_allow_http(endpoint.is_plain_http());
        }
        Ok(Arc::new(builder.build()?))
    }
}

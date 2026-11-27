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
/// staged. Without credentials the store falls back to the instance or pod identity of the host.
pub struct S3RestoreStores<E> {
    env: E,
}

impl<E> S3RestoreStores<E> {
    pub fn new(env: E) -> Self {
        Self { env }
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
        if let (Ok(key), Ok(secret)) = (
            self.env.var(AWS_ACCESS_KEY_ID_ENV),
            self.env.var(AWS_SECRET_ACCESS_KEY_ENV),
        ) {
            builder = builder.with_access_key_id(key).with_secret_access_key(secret);
        }
        if let Ok(token) = self.env.var(AWS_SESSION_TOKEN_ENV) {
            builder = builder.with_token(token);
        }
        if let Some(endpoint) = endpoint {
            builder = builder
                .with_endpoint(endpoint.as_str())
                .with_allow_http(endpoint.is_plain_http());
        }
        Ok(Arc::new(builder.build()?))
    }
}

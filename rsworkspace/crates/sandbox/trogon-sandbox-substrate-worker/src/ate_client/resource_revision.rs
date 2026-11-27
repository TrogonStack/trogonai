use super::{AteError, AteResult};
use crate::proto;

#[derive(Clone, Debug)]
pub struct ResourceRevision {
    resource: proto::ObjectRef,
    uid: String,
    version: i64,
}

impl ResourceRevision {
    pub fn from_metadata(metadata: Option<&proto::ResourceMetadata>) -> AteResult<Self> {
        let metadata = metadata.ok_or_else(|| AteError::InvalidArgument {
            message: "resource metadata is missing".into(),
        })?;
        if metadata.atespace.is_empty() || metadata.name.is_empty() || metadata.uid.is_empty() || metadata.version <= 0
        {
            return Err(AteError::InvalidArgument {
                message: "deletion requires the observed resource address, UID and positive version".into(),
            });
        }
        Ok(Self {
            resource: proto::ObjectRef {
                atespace: metadata.atespace.clone(),
                name: metadata.name.clone(),
            },
            uid: metadata.uid.clone(),
            version: metadata.version,
        })
    }

    pub fn resource(&self) -> &proto::ObjectRef {
        &self.resource
    }

    pub fn options(&self) -> proto::DeleteOptions {
        proto::DeleteOptions {
            version: self.version,
            uid: self.uid.clone(),
        }
    }
}

#[cfg(test)]
mod tests;

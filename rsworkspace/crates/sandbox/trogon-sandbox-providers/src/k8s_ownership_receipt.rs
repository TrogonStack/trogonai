use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};

use crate::{K8sOwnershipError, K8sResourceKind};

/// Proof that the cluster confirmed one exact object: its kind, namespace, name and UID. Deleting or adopting by
/// receipt is fenced on the UID, so a replacement object of the same name is never touched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct K8sResourceReceipt {
    kind: K8sResourceKind,
    namespace: String,
    name: String,
    uid: String,
}

impl K8sResourceReceipt {
    /// Reads a receipt from object metadata that the cluster returned, requiring the confirmed namespace, a name
    /// and a UID.
    pub fn from_metadata(
        kind: K8sResourceKind,
        namespace: &str,
        metadata: &ObjectMeta,
    ) -> Result<Self, K8sOwnershipError> {
        if metadata.namespace.as_deref() != Some(namespace) {
            return Err(K8sOwnershipError::NamespaceNotConfirmed);
        }
        let name = metadata
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .ok_or(K8sOwnershipError::NameNotConfirmed)?;
        let uid = metadata
            .uid
            .as_deref()
            .filter(|uid| !uid.is_empty())
            .ok_or(K8sOwnershipError::UidNotConfirmed)?;
        Ok(Self {
            kind,
            namespace: namespace.into(),
            name: name.into(),
            uid: uid.into(),
        })
    }

    pub fn kind(&self) -> K8sResourceKind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn uid(&self) -> &str {
        &self.uid
    }

    pub fn authority_key(&self) -> String {
        format!("{:?}/{}/{}/{}", self.kind, self.namespace, self.name, self.uid)
    }

    /// Whether `metadata` lists this exact object as an owner.
    pub(crate) fn owns(&self, metadata: &ObjectMeta) -> bool {
        metadata.owner_references.as_ref().is_some_and(|owners| {
            owners.iter().any(|owner| {
                owner.uid == self.uid && owner.name == self.name && owner.kind == self.kind.api_resource().kind
            })
        })
    }

    /// The owner reference that makes this object the owner of another.
    pub(crate) fn owner_reference(&self) -> OwnerReference {
        let kind = self.kind.api_resource();
        OwnerReference {
            api_version: kind.api_version,
            kind: kind.kind,
            name: self.name.clone(),
            uid: self.uid.clone(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests;

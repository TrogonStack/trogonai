//! UID-fenced operations on single cluster objects, each guarded by a [`K8sResourceReceipt`] so that a replacement
//! object of the same name is never inspected, deleted or adopted by mistake.

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::api::networking::v1::NetworkPolicy;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use kube::api::{DeleteParams, Patch, PatchParams, PostParams, Preconditions, PropagationPolicy};
use kube::core::DynamicObject;
use kube::{Api, Client};
use serde::Serialize;

use crate::{K8sOwnershipError, K8sResourceKind, K8sResourceReceipt};

#[derive(Debug, Serialize)]
struct OwnerPatch {
    metadata: OwnerPatchMetadata,
}

#[derive(Debug, Serialize)]
struct OwnerPatchMetadata {
    uid: String,
    #[serde(rename = "resourceVersion")]
    resource_version: String,
    #[serde(rename = "ownerReferences")]
    owner_references: [OwnerReference; 1],
}

fn resource_api(client: &Client, kind: K8sResourceKind, namespace: &str) -> Api<DynamicObject> {
    Api::namespaced_with(client.clone(), namespace, &kind.api_resource())
}

async fn exact_resource_is_gone(client: &Client, receipt: &K8sResourceReceipt) -> Result<bool, K8sOwnershipError> {
    let Some(metadata) = inspect_resource_metadata(client, receipt.kind(), receipt.namespace(), receipt.name()).await?
    else {
        return Ok(true);
    };
    let observed = K8sResourceReceipt::from_metadata(receipt.kind(), receipt.namespace(), &metadata)?;
    if observed.name() != receipt.name() {
        return Err(K8sOwnershipError::InspectionNameMismatch);
    }
    Ok(observed.uid() != receipt.uid())
}

/// Reads the metadata of one object, or `None` when it does not exist.
pub(crate) async fn inspect_resource_metadata(
    client: &Client,
    kind: K8sResourceKind,
    namespace: &str,
    name: &str,
) -> Result<Option<ObjectMeta>, K8sOwnershipError> {
    Ok(resource_api(client, kind, namespace)
        .get_opt(name)
        .await?
        .map(|object| object.metadata))
}

/// Deletes the object `receipt` names, fenced on its UID (and on `resource_version` when given), and confirms it is
/// gone.
pub(crate) async fn delete_exact_resource(
    client: &Client,
    receipt: &K8sResourceReceipt,
    resource_version: Option<&str>,
) -> Result<(), K8sOwnershipError> {
    let api = resource_api(client, receipt.kind(), receipt.namespace());
    let params = DeleteParams {
        propagation_policy: Some(PropagationPolicy::Background),
        preconditions: Some(Preconditions {
            uid: Some(receipt.uid().to_string()),
            resource_version: resource_version.map(str::to_string),
        }),
        ..Default::default()
    };
    match api.delete(receipt.name(), &params).await {
        Ok(_) => {}
        Err(kube::Error::Api(error)) if error.code == 404 => {}
        Err(error) => {
            if !exact_resource_is_gone(client, receipt).await? {
                return Err(error.into());
            }
        }
    }
    if !exact_resource_is_gone(client, receipt).await? {
        return Err(K8sOwnershipError::CleanupNotConfirmed);
    }
    Ok(())
}

/// Makes `owner` the owner of the object `receipt` names, fenced on the object's UID and revision.
pub(crate) async fn set_exact_resource_owner(
    client: &Client,
    receipt: &K8sResourceReceipt,
    owner: &K8sResourceReceipt,
) -> Result<(), K8sOwnershipError> {
    let api = resource_api(client, receipt.kind(), receipt.namespace());
    let current = api.get(receipt.name()).await?;
    if current.metadata.uid.as_deref() != Some(receipt.uid()) {
        return Err(K8sOwnershipError::ReplacedBeforeOwnership);
    }
    let resource_version = current
        .metadata
        .resource_version
        .ok_or(K8sOwnershipError::OwnershipRevisionMissing)?;
    let patch = OwnerPatch {
        metadata: OwnerPatchMetadata {
            uid: receipt.uid().to_string(),
            resource_version,
            owner_references: [owner.owner_reference()],
        },
    };
    api.patch(receipt.name(), &PatchParams::default(), &Patch::Merge(patch))
        .await?;
    Ok(())
}

/// Creates the Secret, or replaces the one of that name when `owner` already owns it.
pub(crate) async fn put_exact_owned_secret(
    client: &Client,
    owner: &K8sResourceReceipt,
    mut secret: Secret,
) -> Result<Secret, K8sOwnershipError> {
    let api: Api<Secret> = Api::namespaced(client.clone(), owner.namespace());
    let name = secret
        .metadata
        .name
        .clone()
        .ok_or(K8sOwnershipError::OwnedSecretNameMissing)?;
    match api.get_opt(&name).await? {
        Some(current) => {
            if !owner.owns(&current.metadata) {
                return Err(K8sOwnershipError::StoredRequestForeign);
            }
            secret.metadata.uid = Some(current.metadata.uid.ok_or(K8sOwnershipError::StoredRequestUidMissing)?);
            secret.metadata.resource_version = Some(
                current
                    .metadata
                    .resource_version
                    .ok_or(K8sOwnershipError::StoredRequestRevisionMissing)?,
            );
            Ok(api.replace(&name, &PostParams::default(), &secret).await?)
        }
        None => Ok(api.create(&PostParams::default(), &secret).await?),
    }
}

/// Creates or replaces the network policy, or deletes `previous` when `policy` is `None`.
pub(crate) async fn apply_exact_network_policy(
    client: &Client,
    owner: &K8sResourceReceipt,
    previous: Option<&K8sResourceReceipt>,
    policy: Option<NetworkPolicy>,
) -> Result<Option<NetworkPolicy>, K8sOwnershipError> {
    let Some(mut policy) = policy else {
        if let Some(previous) = previous {
            delete_exact_resource(client, previous, None).await?;
        }
        return Ok(None);
    };
    let api: Api<NetworkPolicy> = Api::namespaced(client.clone(), owner.namespace());
    let name = policy
        .metadata
        .name
        .clone()
        .ok_or(K8sOwnershipError::NetworkPolicyNameMissing)?;
    match api.get_opt(&name).await? {
        Some(current) => {
            let held = previous.is_some_and(|receipt| current.metadata.uid.as_deref() == Some(receipt.uid()));
            if !(held && owner.owns(&current.metadata)) {
                return Err(K8sOwnershipError::NetworkPolicyNotHeld);
            }
            policy.metadata.uid = current.metadata.uid;
            policy.metadata.resource_version = Some(
                current
                    .metadata
                    .resource_version
                    .ok_or(K8sOwnershipError::NetworkPolicyRevisionMissing)?,
            );
            Ok(Some(api.replace(&name, &PostParams::default(), &policy).await?))
        }
        None => Ok(Some(api.create(&PostParams::default(), &policy).await?)),
    }
}

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use k8s_openapi::api::networking::v1::NetworkPolicySpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use serde::Serialize;

/// Merge patch body that replaces a Secret's owner references.
#[derive(Debug, Serialize)]
pub(crate) struct OwnerReferencesPatch {
    metadata: OwnerReferencesMeta,
}

#[derive(Debug, Serialize)]
struct OwnerReferencesMeta {
    #[serde(rename = "ownerReferences")]
    owner_references: Vec<OwnerReference>,
}

impl OwnerReferencesPatch {
    pub(crate) fn new(owner: OwnerReference) -> Self {
        Self {
            metadata: OwnerReferencesMeta {
                owner_references: vec![owner],
            },
        }
    }
}

/// Merge patch body that sets labels, fenced on the object's `resourceVersion`.
#[derive(Debug, Serialize)]
pub(crate) struct FencedLabelsPatch<'a> {
    metadata: FencedLabelsMeta<'a>,
}

#[derive(Debug, Serialize)]
struct FencedLabelsMeta<'a> {
    #[serde(rename = "resourceVersion")]
    resource_version: &'a str,
    labels: BTreeMap<String, String>,
}

impl<'a> FencedLabelsPatch<'a> {
    pub(crate) fn new(resource_version: &'a str, labels: BTreeMap<String, String>) -> Self {
        Self {
            metadata: FencedLabelsMeta {
                resource_version,
                labels,
            },
        }
    }
}

/// Merge patch body that replaces a ConfigMap's data.
#[derive(Debug, Serialize)]
pub(crate) struct ConfigMapDataPatch {
    data: BTreeMap<String, String>,
}

impl ConfigMapDataPatch {
    pub(crate) fn new(data: BTreeMap<String, String>) -> Self {
        Self { data }
    }
}

/// Merge patch body that replaces a NetworkPolicy's spec.
#[derive(Debug, Serialize)]
pub(crate) struct NetworkPolicySpecPatch<'a> {
    spec: &'a NetworkPolicySpec,
}

impl<'a> NetworkPolicySpecPatch<'a> {
    pub(crate) fn new(spec: &'a NetworkPolicySpec) -> Self {
        Self { spec }
    }
}

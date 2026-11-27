use std::collections::BTreeMap;

use serde::Serialize;

/// The body of `POST /v1/sandboxes`.
#[derive(Serialize)]
pub(crate) struct CreateBody {
    pub(crate) template: String,
    pub(crate) runtime: &'static str,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) metadata: CreateMetadata,
    pub(crate) resources: CreateResources,
}

/// The ownership tags a created sandbox carries.
#[derive(Serialize)]
pub(crate) struct CreateMetadata {
    pub(crate) sandbox_owner: String,
    pub(crate) sandbox_lease: String,
    pub(crate) sandbox_attempt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sandbox_dispatch_attempt: Option<String>,
}

/// The resources a created sandbox asks for.
#[derive(Serialize)]
pub(crate) struct CreateResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vcpu: Option<u32>,
    pub(crate) memory_mib: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disk_mib: Option<u32>,
}

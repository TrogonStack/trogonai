use kube::core::{ApiResource, GroupVersionKind};

/// The kinds of Kubernetes object the provider creates and holds receipts for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum K8sResourceKind {
    Job,
    Pod,
    Secret,
    NetworkPolicy,
    ConfigMap,
}

impl K8sResourceKind {
    pub(crate) fn api_resource(self) -> ApiResource {
        let (group, version, kind) = match self {
            Self::Job => ("batch", "v1", "Job"),
            Self::Pod => ("", "v1", "Pod"),
            Self::Secret => ("", "v1", "Secret"),
            Self::NetworkPolicy => ("networking.k8s.io", "v1", "NetworkPolicy"),
            Self::ConfigMap => ("", "v1", "ConfigMap"),
        };
        ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind))
    }
}

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{ConfigMap, Pod, Secret};
use k8s_openapi::api::networking::v1::{NetworkPolicy, NetworkPolicySpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use tokio::sync::watch;
use trogon_sandbox_contract::OutputRetention;

use crate::k8s_ops::{FencedPatchOutcome, InteractivePodExec, JobListing, PodExecOutput, PodListing, TtyPodExec};
use crate::{K8sOpsError, K8sResourceKind, K8sResourceReceipt, SandboxStatus};

/// The Kubernetes API calls the provider makes, so the provider's decisions
/// can be exercised without a cluster.
#[async_trait]
pub(crate) trait K8sOps: Send + Sync {
    async fn resource_metadata(
        &self,
        _kind: K8sResourceKind,
        _namespace: &str,
        _name: &str,
    ) -> Result<Option<ObjectMeta>, K8sOpsError> {
        Err(K8sOpsError::Unsupported("resource identity inspection"))
    }

    async fn delete_resource(
        &self,
        _receipt: &K8sResourceReceipt,
        _resource_version: Option<&str>,
    ) -> Result<(), K8sOpsError> {
        Err(K8sOpsError::Unsupported("UID-fenced resource deletion"))
    }

    async fn set_resource_owner(
        &self,
        _receipt: &K8sResourceReceipt,
        _owner: &K8sResourceReceipt,
    ) -> Result<(), K8sOpsError> {
        Err(K8sOpsError::Unsupported("UID-fenced resource ownership"))
    }

    async fn put_owned_secret(&self, _owner: &K8sResourceReceipt, _secret: Secret) -> Result<Secret, K8sOpsError> {
        Err(K8sOpsError::Unsupported("UID-fenced Secret persistence"))
    }

    async fn apply_owned_network_policy(
        &self,
        _owner: &K8sResourceReceipt,
        _previous: Option<&K8sResourceReceipt>,
        _policy: Option<NetworkPolicy>,
    ) -> Result<Option<NetworkPolicy>, K8sOpsError> {
        Err(K8sOpsError::Unsupported("UID-fenced network-policy updates"))
    }

    async fn create_job(&self, namespace: &str, job: Job) -> Result<Job, K8sOpsError>;
    async fn create_pod(&self, namespace: &str, pod: Pod) -> Result<Pod, K8sOpsError>;
    async fn create_secret(&self, namespace: &str, secret: Secret) -> Result<Secret, K8sOpsError>;
    async fn create_network_policy(&self, namespace: &str, policy: NetworkPolicy)
    -> Result<NetworkPolicy, K8sOpsError>;
    async fn create_config_map(&self, namespace: &str, config_map: ConfigMap) -> Result<ConfigMap, K8sOpsError>;
    /// Creates the Secret, or replaces it when one of that name exists.
    async fn put_secret(&self, namespace: &str, secret: Secret) -> Result<(), K8sOpsError>;
    /// Reads one key of a Secret; `None` when the Secret or key is missing.
    async fn read_secret_key(&self, namespace: &str, name: &str, key: &str) -> Result<Option<Vec<u8>>, K8sOpsError>;
    async fn delete_job(&self, namespace: &str, name: &str) -> Result<(), K8sOpsError>;
    async fn delete_pod(&self, namespace: &str, name: &str) -> Result<(), K8sOpsError>;
    async fn delete_secret(&self, namespace: &str, name: &str) -> Result<(), K8sOpsError>;
    async fn delete_network_policy(&self, namespace: &str, name: &str) -> Result<(), K8sOpsError>;
    /// Makes `owner` the owner of the Secret named `name`, so the cluster's
    /// garbage collector deletes the Secret with it.
    async fn set_secret_owner(&self, namespace: &str, name: &str, owner: OwnerReference) -> Result<(), K8sOpsError>;
    async fn find_pod_for_job(&self, namespace: &str, job_name: &str) -> Result<String, K8sOpsError>;
    /// Lists pods and reports the apiserver clock from the same response, so an
    /// age computed over these objects never mixes two clocks.
    async fn list_pods(&self, namespace: &str, label_selector: &str) -> Result<PodListing, K8sOpsError>;
    /// Lists Jobs with the apiserver clock, like [`K8sOps::list_pods`].
    async fn list_jobs(&self, namespace: &str, label_selector: &str) -> Result<JobListing, K8sOpsError>;
    /// Patches labels, fenced on `resource_version`. A caller that cannot fence
    /// its write cannot claim exclusivity, so the fence is required rather than
    /// optional.
    async fn patch_pod_labels(
        &self,
        namespace: &str,
        name: &str,
        labels: BTreeMap<String, String>,
        resource_version: &str,
    ) -> Result<FencedPatchOutcome, K8sOpsError>;
    async fn patch_config_map(
        &self,
        namespace: &str,
        name: &str,
        data: BTreeMap<String, String>,
    ) -> Result<(), K8sOpsError>;
    /// Replaces the spec of the `NetworkPolicy` named `name`, creating the
    /// policy when it does not exist (a lease that started open has none).
    async fn patch_network_policy(
        &self,
        namespace: &str,
        name: &str,
        spec: NetworkPolicySpec,
    ) -> Result<(), K8sOpsError>;
    async fn exec_collect_stdout(
        &self,
        namespace: &str,
        pod_name: &str,
        container: &str,
        command: Vec<String>,
    ) -> Result<Vec<u8>, K8sOpsError>;
    /// Like [`K8sOps::exec_collect_stdout`], but also captures stderr and the
    /// process exit code, and optionally feeds `stdin_data` to the exec'd
    /// process before closing its stdin.
    async fn exec_with_output(
        &self,
        namespace: &str,
        pod_name: &str,
        container: &str,
        command: Vec<String>,
        stdin_data: Option<Vec<u8>>,
        max_output_bytes: usize,
    ) -> Result<PodExecOutput, K8sOpsError>;
    /// Starts an exec with stdin attached and returns it unread, so the
    /// caller owns stdin for the process lifetime. `retention` bounds what
    /// `completion` keeps of the output, and `max_output_bytes` caps each
    /// retained stream.
    async fn exec_interactive(
        &self,
        namespace: &str,
        pod_name: &str,
        container: &str,
        command: Vec<String>,
        retention: OutputRetention,
        max_output_bytes: usize,
    ) -> Result<InteractivePodExec, K8sOpsError>;
    /// Starts an exec with a pseudo-TTY and stdin attached, sized `size`
    /// (`cols`, `rows`) from the start.
    async fn exec_tty(
        &self,
        namespace: &str,
        pod_name: &str,
        container: &str,
        command: Vec<String>,
        size: (u16, u16),
    ) -> Result<TtyPodExec, K8sOpsError>;
    fn spawn_job_watcher(
        &self,
        namespace: &str,
        name: &str,
        tx: watch::Sender<SandboxStatus>,
    ) -> tokio::task::JoinHandle<()>;
}

use std::collections::BTreeMap;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{ConfigMap, Pod, Secret};
use k8s_openapi::api::networking::v1::{NetworkPolicy, NetworkPolicySpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use tokio::sync::watch;
use trogon_sandbox_contract::OutputRetention;

use super::test_objects_tests::{network_policy, secret};
use crate::k8s_ops::{
    FencedPatchOutcome, InteractivePodExec, JobListing, K8sOps, PodExecOutput, PodListing, TtyPodExec,
};
use crate::{K8sOpsError, K8sResourceKind, K8sResourceReceipt, SandboxStatus};

/// A client that supports only the required operations, each of which refuses.
struct Refusing;

fn refuse<T>() -> Result<T, K8sOpsError> {
    Err(K8sOpsError::Other("refused".to_string()))
}

#[async_trait]
impl K8sOps for Refusing {
    async fn create_job(&self, _: &str, _: Job) -> Result<Job, K8sOpsError> {
        refuse()
    }
    async fn create_pod(&self, _: &str, _: Pod) -> Result<Pod, K8sOpsError> {
        refuse()
    }
    async fn create_secret(&self, _: &str, _: Secret) -> Result<Secret, K8sOpsError> {
        refuse()
    }
    async fn create_network_policy(&self, _: &str, _: NetworkPolicy) -> Result<NetworkPolicy, K8sOpsError> {
        refuse()
    }
    async fn create_config_map(&self, _: &str, _: ConfigMap) -> Result<ConfigMap, K8sOpsError> {
        refuse()
    }
    async fn put_secret(&self, _: &str, _: Secret) -> Result<(), K8sOpsError> {
        refuse()
    }
    async fn read_secret_key(&self, _: &str, _: &str, _: &str) -> Result<Option<Vec<u8>>, K8sOpsError> {
        refuse()
    }
    async fn delete_job(&self, _: &str, _: &str) -> Result<(), K8sOpsError> {
        refuse()
    }
    async fn delete_pod(&self, _: &str, _: &str) -> Result<(), K8sOpsError> {
        refuse()
    }
    async fn delete_secret(&self, _: &str, _: &str) -> Result<(), K8sOpsError> {
        refuse()
    }
    async fn delete_network_policy(&self, _: &str, _: &str) -> Result<(), K8sOpsError> {
        refuse()
    }
    async fn set_secret_owner(&self, _: &str, _: &str, _: OwnerReference) -> Result<(), K8sOpsError> {
        refuse()
    }
    async fn find_pod_for_job(&self, _: &str, _: &str) -> Result<String, K8sOpsError> {
        refuse()
    }
    async fn list_pods(&self, _: &str, _: &str) -> Result<PodListing, K8sOpsError> {
        refuse()
    }
    async fn list_jobs(&self, _: &str, _: &str) -> Result<JobListing, K8sOpsError> {
        refuse()
    }
    async fn patch_pod_labels(
        &self,
        _: &str,
        _: &str,
        _: BTreeMap<String, String>,
        _: &str,
    ) -> Result<FencedPatchOutcome, K8sOpsError> {
        refuse()
    }
    async fn patch_config_map(&self, _: &str, _: &str, _: BTreeMap<String, String>) -> Result<(), K8sOpsError> {
        refuse()
    }
    async fn patch_network_policy(&self, _: &str, _: &str, _: NetworkPolicySpec) -> Result<(), K8sOpsError> {
        refuse()
    }
    async fn exec_collect_stdout(&self, _: &str, _: &str, _: &str, _: Vec<String>) -> Result<Vec<u8>, K8sOpsError> {
        refuse()
    }
    async fn exec_with_output(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Vec<String>,
        _: Option<Vec<u8>>,
        _: usize,
    ) -> Result<PodExecOutput, K8sOpsError> {
        refuse()
    }
    async fn exec_interactive(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Vec<String>,
        _: OutputRetention,
        _: usize,
    ) -> Result<InteractivePodExec, K8sOpsError> {
        refuse()
    }
    async fn exec_tty(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Vec<String>,
        _: (u16, u16),
    ) -> Result<TtyPodExec, K8sOpsError> {
        refuse()
    }
    fn spawn_job_watcher(&self, _: &str, _: &str, _: watch::Sender<SandboxStatus>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {})
    }
}

fn receipt(kind: K8sResourceKind) -> Result<K8sResourceReceipt, Box<dyn std::error::Error>> {
    let metadata = ObjectMeta {
        name: Some("n".to_string()),
        namespace: Some("work".to_string()),
        uid: Some("u".to_string()),
        ..Default::default()
    };
    Ok(K8sResourceReceipt::from_metadata(kind, "work", &metadata)?)
}

#[tokio::test]
async fn the_fenced_operations_are_unsupported_unless_a_client_provides_them() -> Result<(), Box<dyn std::error::Error>>
{
    let client = Refusing;
    let pod = receipt(K8sResourceKind::Pod)?;

    assert!(matches!(
        client.resource_metadata(K8sResourceKind::Pod, "work", "n").await,
        Err(K8sOpsError::Unsupported("resource identity inspection"))
    ));
    assert!(matches!(
        client.delete_resource(&pod, None).await,
        Err(K8sOpsError::Unsupported("UID-fenced resource deletion"))
    ));
    assert!(matches!(
        client.set_resource_owner(&pod, &pod).await,
        Err(K8sOpsError::Unsupported("UID-fenced resource ownership"))
    ));
    assert!(matches!(
        client.put_owned_secret(&pod, secret("s")).await,
        Err(K8sOpsError::Unsupported("UID-fenced Secret persistence"))
    ));
    assert!(matches!(
        client
            .apply_owned_network_policy(&pod, None, Some(network_policy("np")))
            .await,
        Err(K8sOpsError::Unsupported("UID-fenced network-policy updates"))
    ));
    Ok(())
}

#[tokio::test]
async fn the_required_operations_all_run_on_the_client() -> Result<(), Box<dyn std::error::Error>> {
    let client = Refusing;
    let none = || Vec::<String>::new();
    let labels = BTreeMap::new;

    assert!(client.create_job("n", Job::default()).await.is_err());
    assert!(client.create_pod("n", Pod::default()).await.is_err());
    assert!(client.create_secret("n", Secret::default()).await.is_err());
    assert!(
        client
            .create_network_policy("n", NetworkPolicy::default())
            .await
            .is_err()
    );
    assert!(client.create_config_map("n", ConfigMap::default()).await.is_err());
    assert!(client.put_secret("n", Secret::default()).await.is_err());
    assert!(client.read_secret_key("n", "s", "k").await.is_err());
    assert!(client.delete_job("n", "j").await.is_err());
    assert!(client.delete_pod("n", "p").await.is_err());
    assert!(client.delete_secret("n", "s").await.is_err());
    assert!(client.delete_network_policy("n", "np").await.is_err());
    assert!(
        client
            .set_secret_owner("n", "s", OwnerReference::default())
            .await
            .is_err()
    );
    assert!(client.find_pod_for_job("n", "j").await.is_err());
    assert!(client.list_pods("n", "").await.is_err());
    assert!(client.list_jobs("n", "").await.is_err());
    assert!(client.patch_pod_labels("n", "p", labels(), "1").await.is_err());
    assert!(client.patch_config_map("n", "cm", labels()).await.is_err());
    assert!(
        client
            .patch_network_policy("n", "np", NetworkPolicySpec::default())
            .await
            .is_err()
    );
    assert!(client.exec_collect_stdout("n", "p", "c", none()).await.is_err());
    assert!(client.exec_with_output("n", "p", "c", none(), None, 1).await.is_err());
    assert!(
        client
            .exec_interactive("n", "p", "c", none(), OutputRetention::Buffer, 1)
            .await
            .is_err()
    );
    assert!(client.exec_tty("n", "p", "c", none(), (1, 1)).await.is_err());
    let (tx, _rx) = watch::channel(SandboxStatus::Pending);
    client.spawn_job_watcher("n", "j", tx).await?;
    Ok(())
}

use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use super::fake_api_server_tests::FakeApiServer;
use super::test_objects_tests::{network_policy, secret};
use crate::k8s_ops::K8sOps;
use crate::{K8sResourceKind, K8sResourceReceipt};

fn receipt(kind: K8sResourceKind, name: &str) -> Result<K8sResourceReceipt, Box<dyn std::error::Error>> {
    let metadata = ObjectMeta {
        name: Some(name.to_string()),
        namespace: Some("work".to_string()),
        uid: Some(format!("uid-{name}")),
        ..Default::default()
    };
    Ok(K8sResourceReceipt::from_metadata(kind, "work", &metadata)?)
}

/// With the apiserver refusing every request, each fenced operation hands the
/// ownership layer's failure back to the caller instead of swallowing it.
#[tokio::test]
async fn the_fenced_operations_surface_the_ownership_layers_failures() -> Result<(), Box<dyn std::error::Error>> {
    let ops = FakeApiServer::new(Vec::new()).ops();
    let job = receipt(K8sResourceKind::Job, "job")?;
    let pod = receipt(K8sResourceKind::Pod, "pod")?;

    assert!(
        ops.resource_metadata(K8sResourceKind::Pod, "work", "pod")
            .await
            .is_err()
    );
    assert!(ops.delete_resource(&pod, Some("5")).await.is_err());
    assert!(ops.set_resource_owner(&pod, &job).await.is_err());
    assert!(ops.put_owned_secret(&job, secret("s")).await.is_err());
    assert!(
        ops.apply_owned_network_policy(&job, Some(&pod), Some(network_policy("np")))
            .await
            .is_err()
    );
    Ok(())
}

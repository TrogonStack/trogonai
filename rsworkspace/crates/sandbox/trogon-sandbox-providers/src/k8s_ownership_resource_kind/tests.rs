use crate::K8sResourceKind;

#[test]
fn every_kind_maps_to_its_api_group_version_and_kind() {
    let expected = [
        (K8sResourceKind::Job, "batch/v1", "Job"),
        (K8sResourceKind::Pod, "v1", "Pod"),
        (K8sResourceKind::Secret, "v1", "Secret"),
        (K8sResourceKind::NetworkPolicy, "networking.k8s.io/v1", "NetworkPolicy"),
        (K8sResourceKind::ConfigMap, "v1", "ConfigMap"),
    ];

    for (kind, api_version, name) in expected {
        let resource = kind.api_resource();
        assert_eq!(resource.api_version, api_version);
        assert_eq!(resource.kind, name);
    }
}

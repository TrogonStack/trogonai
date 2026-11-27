use k8s_openapi::ByteString;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{ConfigMap, Pod, Secret};
use k8s_openapi::api::networking::v1::{NetworkPolicy, NetworkPolicySpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

fn named(name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        ..Default::default()
    }
}

pub(super) fn job(name: &str) -> Job {
    Job {
        metadata: named(name),
        ..Default::default()
    }
}

pub(super) fn pod(name: &str) -> Pod {
    Pod {
        metadata: named(name),
        ..Default::default()
    }
}

pub(super) fn secret(name: &str) -> Secret {
    Secret {
        metadata: named(name),
        ..Default::default()
    }
}

pub(super) fn secret_with(name: &str, key: &str, value: &[u8]) -> Secret {
    Secret {
        data: Some([(key.to_string(), ByteString(value.to_vec()))].into()),
        ..secret(name)
    }
}

pub(super) fn config_map(name: &str) -> ConfigMap {
    ConfigMap {
        metadata: named(name),
        ..Default::default()
    }
}

pub(super) fn network_policy(name: &str) -> NetworkPolicy {
    NetworkPolicy {
        metadata: named(name),
        spec: Some(NetworkPolicySpec::default()),
    }
}

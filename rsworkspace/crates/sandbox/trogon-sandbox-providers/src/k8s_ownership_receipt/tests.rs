use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};

use crate::{K8sOwnershipError, K8sResourceKind, K8sResourceReceipt};

fn metadata(namespace: Option<&str>, name: Option<&str>, uid: Option<&str>) -> ObjectMeta {
    ObjectMeta {
        namespace: namespace.map(str::to_string),
        name: name.map(str::to_string),
        uid: uid.map(str::to_string),
        ..Default::default()
    }
}

fn job() -> Result<K8sResourceReceipt, K8sOwnershipError> {
    K8sResourceReceipt::from_metadata(
        K8sResourceKind::Job,
        "work",
        &metadata(Some("work"), Some("run"), Some("u-1")),
    )
}

#[test]
fn a_receipt_records_the_confirmed_identity() -> Result<(), K8sOwnershipError> {
    let receipt = job()?;

    assert_eq!(receipt.kind(), K8sResourceKind::Job);
    assert_eq!(receipt.namespace(), "work");
    assert_eq!(receipt.name(), "run");
    assert_eq!(receipt.uid(), "u-1");
    assert_eq!(receipt.authority_key(), "Job/work/run/u-1");
    Ok(())
}

#[test]
fn a_receipt_needs_the_confirmed_namespace() {
    for namespace in [None, Some("elsewhere")] {
        let result = K8sResourceReceipt::from_metadata(
            K8sResourceKind::Job,
            "work",
            &metadata(namespace, Some("run"), Some("u-1")),
        );
        assert!(matches!(result, Err(K8sOwnershipError::NamespaceNotConfirmed)));
    }
}

#[test]
fn a_receipt_needs_a_name_and_a_uid() {
    for name in [None, Some("")] {
        let result =
            K8sResourceReceipt::from_metadata(K8sResourceKind::Pod, "work", &metadata(Some("work"), name, Some("u")));
        assert!(matches!(result, Err(K8sOwnershipError::NameNotConfirmed)));
    }
    for uid in [None, Some("")] {
        let result =
            K8sResourceReceipt::from_metadata(K8sResourceKind::Pod, "work", &metadata(Some("work"), Some("run"), uid));
        assert!(matches!(result, Err(K8sOwnershipError::UidNotConfirmed)));
    }
}

#[test]
fn the_owner_reference_names_the_exact_object() -> Result<(), K8sOwnershipError> {
    let reference = job()?.owner_reference();

    assert_eq!(reference.api_version, "batch/v1");
    assert_eq!(reference.kind, "Job");
    assert_eq!(reference.name, "run");
    assert_eq!(reference.uid, "u-1");
    Ok(())
}

#[test]
fn ownership_requires_the_matching_uid_name_and_kind() -> Result<(), K8sOwnershipError> {
    let owner = job()?;
    let owned_by = |reference: OwnerReference| ObjectMeta {
        owner_references: Some(vec![reference]),
        ..Default::default()
    };

    assert!(owner.owns(&owned_by(owner.owner_reference())));
    assert!(!owner.owns(&ObjectMeta::default()));
    assert!(!owner.owns(&owned_by(OwnerReference {
        uid: "replacement".into(),
        ..owner.owner_reference()
    })));
    assert!(!owner.owns(&owned_by(OwnerReference {
        name: "other".into(),
        ..owner.owner_reference()
    })));
    assert!(!owner.owns(&owned_by(OwnerReference {
        kind: "Pod".into(),
        ..owner.owner_reference()
    })));
    Ok(())
}

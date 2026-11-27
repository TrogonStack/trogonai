use kube::api::ApiResource;
use kube::core::GroupVersionKind;

fn resource(group: &str, version: &str, kind: &str, plural: &str) -> ApiResource {
    ApiResource::from_gvk_with_plural(&GroupVersionKind::gvk(group, version, kind), plural)
}

/// `kubevirt.io/v1` `VirtualMachine`.
pub(super) fn vm_resource() -> ApiResource {
    resource("kubevirt.io", "v1", "VirtualMachine", "virtualmachines")
}

/// `kubevirt.io/v1` `VirtualMachineInstance`.
pub(super) fn vmi_resource() -> ApiResource {
    resource("kubevirt.io", "v1", "VirtualMachineInstance", "virtualmachineinstances")
}

/// `cdi.kubevirt.io/v1beta1` `DataVolume`.
pub(super) fn data_volume_resource() -> ApiResource {
    resource("cdi.kubevirt.io", "v1beta1", "DataVolume", "datavolumes")
}

/// `snapshot.kubevirt.io/v1beta1` `VirtualMachineSnapshot`.
pub(super) fn snapshot_resource() -> ApiResource {
    resource(
        "snapshot.kubevirt.io",
        "v1beta1",
        "VirtualMachineSnapshot",
        "virtualmachinesnapshots",
    )
}

/// `snapshot.kubevirt.io/v1beta1` `VirtualMachineSnapshotContent`.
pub(super) fn snapshot_content_resource() -> ApiResource {
    resource(
        "snapshot.kubevirt.io",
        "v1beta1",
        "VirtualMachineSnapshotContent",
        "virtualmachinesnapshotcontents",
    )
}

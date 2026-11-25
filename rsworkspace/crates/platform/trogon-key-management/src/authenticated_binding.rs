use crate::{BindingVersion, KeyPurpose, ResourceId, ResourceTypeId, TenantId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedBinding {
    tenant: TenantId,
    resource_type: ResourceTypeId,
    resource: ResourceId,
    purpose: KeyPurpose,
    version: BindingVersion,
}

impl AuthenticatedBinding {
    pub fn new(
        tenant: TenantId,
        resource_type: ResourceTypeId,
        resource: ResourceId,
        purpose: KeyPurpose,
        version: BindingVersion,
    ) -> Self {
        Self {
            tenant,
            resource_type,
            resource,
            purpose,
            version,
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn resource_type(&self) -> &ResourceTypeId {
        &self.resource_type
    }
    pub fn resource(&self) -> &ResourceId {
        &self.resource
    }
    pub fn purpose(&self) -> &KeyPurpose {
        &self.purpose
    }
    pub fn version(&self) -> BindingVersion {
        self.version
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        append_field(&mut bytes, "trogon/key-wrapping/binding/v1");
        bytes.extend_from_slice(&self.version.get().to_be_bytes());
        for field in [
            self.tenant.as_str(),
            self.resource_type.as_str(),
            self.resource.as_str(),
            self.purpose.as_str(),
        ] {
            append_field(&mut bytes, field);
        }
        bytes
    }
}

fn append_field(bytes: &mut Vec<u8>, field: &str) {
    // Every field is a fixed label or a validated identifier of at most 256 Unicode scalars.
    let length = field.bytes().fold(0_u32, |length, _| length + 1);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(field.as_bytes());
}

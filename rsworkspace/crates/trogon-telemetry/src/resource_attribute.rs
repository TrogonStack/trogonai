use opentelemetry::KeyValue;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceAttribute {
    AcpPrefix(String),
}

impl ResourceAttribute {
    pub fn acp_prefix(prefix: impl std::fmt::Display) -> Self {
        Self::AcpPrefix(prefix.to_string())
    }
}

impl From<ResourceAttribute> for KeyValue {
    fn from(attribute: ResourceAttribute) -> Self {
        match attribute {
            ResourceAttribute::AcpPrefix(prefix) => {
                KeyValue::new(crate::constants::RESOURCE_ATTRIBUTE_ACP_PREFIX, prefix)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_prefix_maps_to_expected_key_value() {
        assert_eq!(
            KeyValue::from(ResourceAttribute::acp_prefix("test")),
            KeyValue::new(crate::constants::RESOURCE_ATTRIBUTE_ACP_PREFIX, "test")
        );
    }
}

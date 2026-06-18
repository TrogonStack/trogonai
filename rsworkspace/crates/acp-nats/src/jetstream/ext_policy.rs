use crate::ext_method_name::ExtMethodName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtPersistence {
    Stream,
    Ephemeral,
}

pub trait ExtStreamPolicy: Send + Sync {
    fn persistence(&self, method: &ExtMethodName) -> ExtPersistence;
}

pub struct DefaultExtStreamPolicy;

impl ExtStreamPolicy for DefaultExtStreamPolicy {
    fn persistence(&self, _method: &ExtMethodName) -> ExtPersistence {
        ExtPersistence::Stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_persists_everything() {
        let policy = DefaultExtStreamPolicy;
        let method = ExtMethodName::new("tool_call").unwrap();
        assert_eq!(policy.persistence(&method), ExtPersistence::Stream);
    }

    #[test]
    fn custom_policy() {
        struct SelectivePolicy;
        impl ExtStreamPolicy for SelectivePolicy {
            fn persistence(&self, method: &ExtMethodName) -> ExtPersistence {
                match method.as_str() {
                    "heartbeat" | "ping" => ExtPersistence::Ephemeral,
                    _ => ExtPersistence::Stream,
                }
            }
        }

        let policy = SelectivePolicy;
        assert_eq!(
            policy.persistence(&ExtMethodName::new("heartbeat").unwrap()),
            ExtPersistence::Ephemeral
        );
        assert_eq!(
            policy.persistence(&ExtMethodName::new("ping").unwrap()),
            ExtPersistence::Ephemeral
        );
        assert_eq!(
            policy.persistence(&ExtMethodName::new("tool_call").unwrap()),
            ExtPersistence::Stream
        );
    }

    #[test]
    fn ext_persistence_debug() {
        assert_eq!(format!("{:?}", ExtPersistence::Stream), "Stream");
        assert_eq!(format!("{:?}", ExtPersistence::Ephemeral), "Ephemeral");
    }
}

use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotStoreConfig {
    key_prefix: Cow<'static, str>,
    checkpoint_name: Option<Cow<'static, str>>,
}

impl SnapshotStoreConfig {
    pub fn new(key_prefix: impl Into<Cow<'static, str>>, checkpoint_name: Option<&'static str>) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_name: checkpoint_name.map(Cow::Borrowed),
        }
    }

    pub fn without_checkpoint(key_prefix: impl Into<Cow<'static, str>>) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_name: None,
        }
    }

    pub fn with_checkpoint_name(
        key_prefix: impl Into<Cow<'static, str>>,
        checkpoint_name: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_name: Some(checkpoint_name.into()),
        }
    }

    pub fn key_prefix(&self) -> &str {
        self.key_prefix.as_ref()
    }

    pub fn checkpoint_name(&self) -> Option<&str> {
        self.checkpoint_name.as_deref()
    }
}

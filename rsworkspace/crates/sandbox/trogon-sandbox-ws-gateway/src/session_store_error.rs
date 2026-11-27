use std::error::Error;

/// A session store could not read or write a record.
#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("session store backend failed")]
    Backend(#[source] Box<dyn Error + Send + Sync>),
    #[error("session record could not be encoded or decoded")]
    Record(#[from] serde_json::Error),
}

impl SessionStoreError {
    pub(crate) fn backend(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(source))
    }
}

#[cfg(test)]
mod tests;

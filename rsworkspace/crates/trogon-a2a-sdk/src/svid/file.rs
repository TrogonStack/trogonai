use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use crate::traits::SvidSource;
use crate::types::SdkError;

pub struct FileSvidSource {
    path: PathBuf,
}

impl FileSvidSource {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }
}

#[async_trait]
impl SvidSource for FileSvidSource {
    async fn current(&self) -> Result<String, SdkError> {
        fs::read_to_string(&self.path)
            .await
            .map(|s| s.trim().to_owned())
            .map_err(|e| SdkError::Config(format!("read SVID from {}: {e}", self.path.display())))
    }
}

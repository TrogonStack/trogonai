use super::{NativeKvCasError, NativeKvKey};
use crate::NatsToken;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeKvScope {
    bucket: NatsToken,
    stream: NatsToken,
    prefix: String,
}

impl NativeKvScope {
    pub fn new(bucket: NatsToken) -> Result<Self, NativeKvCasError> {
        if !bucket
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return Err(NativeKvCasError::InvalidBinding);
        }
        let stream = NatsToken::new(format!("KV_{bucket}")).map_err(|_| NativeKvCasError::InvalidBinding)?;
        Ok(Self {
            stream,
            prefix: format!("$KV.{bucket}."),
            bucket,
        })
    }

    pub fn bucket(&self) -> &NatsToken {
        &self.bucket
    }

    pub fn stream(&self) -> &str {
        self.stream.as_str()
    }

    pub(crate) fn stream_token(&self) -> &NatsToken {
        &self.stream
    }

    pub fn subject(&self, key: &NativeKvKey) -> String {
        format!("{}{}", self.prefix, key.as_str())
    }
}

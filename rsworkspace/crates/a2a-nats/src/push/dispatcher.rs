use std::fmt;

use crate::push::target::{WebhookUrl, WebhookUrlError};

#[derive(Debug)]
pub enum DispatchError {
    InvalidUrl(WebhookUrlError),
    Http(reqwest::Error),
    UnexpectedStatus { status: u16, url: String },
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(e) => write!(f, "invalid push notification URL: {e}"),
            Self::Http(e) => write!(f, "HTTP request failed: {e}"),
            Self::UnexpectedStatus { status, url } => {
                write!(f, "push notification to {url} returned status {status}")
            }
        }
    }
}

impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidUrl(e) => Some(e),
            Self::Http(e) => Some(e),
            Self::UnexpectedStatus { .. } => None,
        }
    }
}

impl From<WebhookUrlError> for DispatchError {
    fn from(e: WebhookUrlError) -> Self {
        Self::InvalidUrl(e)
    }
}

#[async_trait::async_trait]
pub trait PushDispatcher: Send + Sync + 'static {
    async fn dispatch(
        &self,
        config: &a2a_types::TaskPushNotificationConfig,
        payload: &[u8],
    ) -> Result<(), DispatchError>;
}

pub struct HttpPushDispatcher {
    client: reqwest::Client,
}

impl HttpPushDispatcher {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl PushDispatcher for HttpPushDispatcher {
    async fn dispatch(
        &self,
        config: &a2a_types::TaskPushNotificationConfig,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        let url = WebhookUrl::new(&config.url)?;

        let mut request = self
            .client
            .post(url.as_str())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload.to_vec());

        if let Some(auth) = &config.authentication
            && auth.scheme.eq_ignore_ascii_case("bearer")
            && !auth.credentials.is_empty()
        {
            request = request.header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", auth.credentials),
            );
        }

        let response = request.send().await.map_err(DispatchError::Http)?;

        if !response.status().is_success() {
            return Err(DispatchError::UnexpectedStatus {
                status: response.status().as_u16(),
                url: url.to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    pub struct MockPushDispatcher {
        #[allow(clippy::type_complexity)]
        pub calls: Arc<Mutex<Vec<(a2a_types::TaskPushNotificationConfig, Vec<u8>)>>>,
        pub result: Arc<Mutex<Option<Result<(), String>>>>,
    }

    impl Default for MockPushDispatcher {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockPushDispatcher {
        pub fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(vec![])),
                result: Arc::new(Mutex::new(None)),
            }
        }

        pub fn fail_with(error: impl Into<String>) -> Self {
            let d = Self::new();
            *d.result.lock().unwrap() = Some(Err(error.into()));
            d
        }

        pub fn recorded_calls(&self) -> Vec<(a2a_types::TaskPushNotificationConfig, Vec<u8>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl PushDispatcher for MockPushDispatcher {
        async fn dispatch(
            &self,
            config: &a2a_types::TaskPushNotificationConfig,
            payload: &[u8],
        ) -> Result<(), DispatchError> {
            self.calls
                .lock()
                .unwrap()
                .push((config.clone(), payload.to_vec()));
            match self.result.lock().unwrap().take() {
                Some(Err(msg)) => Err(DispatchError::UnexpectedStatus { status: 500, url: msg }),
                _ => Ok(()),
            }
        }
    }

    #[test]
    fn error_display_invalid_url() {
        let err = DispatchError::InvalidUrl(
            crate::push::target::WebhookUrl::new("ftp://bad").unwrap_err(),
        );
        assert!(err.to_string().contains("invalid push notification URL"));
    }

    #[test]
    fn error_display_unexpected_status() {
        let err = DispatchError::UnexpectedStatus {
            status: 429,
            url: "https://example.com".into(),
        };
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().contains("https://example.com"));
    }
}

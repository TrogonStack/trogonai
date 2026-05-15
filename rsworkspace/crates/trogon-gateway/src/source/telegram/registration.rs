#![cfg_attr(coverage, allow(dead_code))]

use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::config::TelegramSourceConfig;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const TELEGRAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TELEGRAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum RegistrationError {
    Request(reqwest::Error),
    Rejected { status: StatusCode, description: String },
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(error) => write!(f, "Telegram webhook registration request failed: {error}"),
            Self::Rejected { status, description } => {
                write!(f, "Telegram webhook registration rejected with {status}: {description}")
            }
        }
    }
}

impl std::error::Error for RegistrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(error) => Some(error),
            Self::Rejected { .. } => None,
        }
    }
}

impl From<reqwest::Error> for RegistrationError {
    fn from(error: reqwest::Error) -> Self {
        Self::Request(error.without_url())
    }
}

#[derive(Serialize)]
struct SetWebhookRequest<'a> {
    url: &'a str,
    secret_token: &'a str,
}

#[derive(Deserialize)]
struct TelegramResponse {
    ok: bool,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct SetWebhook {
    bot_token: String,
    public_webhook_url: String,
    webhook_secret: String,
}

#[derive(Debug, PartialEq, Eq)]
struct SetWebhookResponse {
    status: StatusCode,
    ok: bool,
    description: Option<String>,
}

trait HttpPost {
    type Response: HttpResponse;
    type Error;

    async fn post_json<B: Serialize>(&self, url: &str, body: &B) -> Result<Self::Response, Self::Error>;
}

trait HttpResponse {
    type Error;

    fn status(&self) -> StatusCode;

    async fn json<T: DeserializeOwned>(self) -> Result<T, Self::Error>;
}

impl HttpPost for reqwest::Client {
    type Response = reqwest::Response;
    type Error = reqwest::Error;

    async fn post_json<B: Serialize>(&self, url: &str, body: &B) -> Result<Self::Response, Self::Error> {
        self.post(url).json(body).send().await
    }
}

impl HttpResponse for reqwest::Response {
    type Error = reqwest::Error;

    fn status(&self) -> StatusCode {
        reqwest::Response::status(self)
    }

    async fn json<T: DeserializeOwned>(self) -> Result<T, Self::Error> {
        reqwest::Response::json(self).await
    }
}

pub fn registration_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(TELEGRAM_CONNECT_TIMEOUT)
        .timeout(TELEGRAM_REQUEST_TIMEOUT)
        .build()
}

pub async fn register_webhook(
    config: &TelegramSourceConfig,
    http_client: &reqwest::Client,
) -> Result<(), RegistrationError> {
    register_with_http(http_client, TELEGRAM_API_BASE, config).await
}

async fn register_with_http<H>(
    http: &H,
    api_base: &str,
    config: &TelegramSourceConfig,
) -> Result<(), RegistrationError>
where
    H: HttpPost,
    RegistrationError: From<H::Error> + From<<H::Response as HttpResponse>::Error>,
{
    let Some(registration) = config.registration.as_ref() else {
        return Ok(());
    };

    let request = SetWebhook {
        bot_token: registration.bot_token.as_str().to_string(),
        public_webhook_url: registration.public_webhook_url.as_str().to_string(),
        webhook_secret: config.webhook_secret.as_str().to_string(),
    };

    let response = set_webhook(http, api_base, &request).await?;

    if !response.status.is_success() || !response.ok {
        return Err(RegistrationError::Rejected {
            status: response.status,
            description: response
                .description
                .unwrap_or_else(|| "missing description".to_string()),
        });
    }

    info!("Telegram webhook registered");
    Ok(())
}

async fn set_webhook<H>(
    http: &H,
    api_base: &str,
    request: &SetWebhook,
) -> Result<SetWebhookResponse, RegistrationError>
where
    H: HttpPost,
    RegistrationError: From<H::Error> + From<<H::Response as HttpResponse>::Error>,
{
    let endpoint = set_webhook_endpoint(api_base, &request.bot_token);
    let request_body = SetWebhookRequest {
        url: &request.public_webhook_url,
        secret_token: &request.webhook_secret,
    };

    let response = http.post_json(&endpoint, &request_body).await.map_err(RegistrationError::from)?;
    let status = response.status();
    let body = response
        .json::<TelegramResponse>()
        .await
        .map_err(RegistrationError::from)?;

    Ok(SetWebhookResponse {
        status,
        ok: body.ok,
        description: body.description,
    })
}

fn set_webhook_endpoint(api_base: &str, bot_token: &str) -> String {
    format!("{}/bot{}/setWebhook", api_base.trim_end_matches('/'), bot_token)
}

#[cfg(test)]
mod tests {
    use super::super::config::{
        TelegramBotToken, TelegramPublicWebhookUrl, TelegramWebhookRegistrationConfig, TelegramWebhookSecret,
    };
    use super::*;
    use serde_json::{Value, json};
    use std::error::Error;
    #[cfg(coverage)]
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[cfg(coverage)]
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use trogon_nats::NatsToken;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_std::NonZeroDuration;

    const TEST_BOT_TOKEN: &str = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ";

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct RecordedRequest {
        url: String,
        body: Value,
    }

    #[derive(Debug)]
    struct FakeHttpError(String);

    impl FakeHttpError {
        fn new(message: impl Into<String>) -> Self {
            Self(message.into())
        }
    }

    impl fmt::Display for FakeHttpError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl Error for FakeHttpError {}

    impl From<serde_json::Error> for FakeHttpError {
        fn from(error: serde_json::Error) -> Self {
            Self::new(error.to_string())
        }
    }

    impl From<FakeHttpError> for RegistrationError {
        fn from(error: FakeHttpError) -> Self {
            Self::Rejected {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                description: error.to_string(),
            }
        }
    }

    struct FakeHttpResponse {
        status: StatusCode,
        body: Result<Value, FakeHttpError>,
    }

    impl FakeHttpResponse {
        fn json(status: StatusCode, body: Value) -> Self {
            Self { status, body: Ok(body) }
        }

        fn decode_error(message: impl Into<String>) -> Self {
            Self {
                status: StatusCode::OK,
                body: Err(FakeHttpError::new(message)),
            }
        }
    }

    impl HttpResponse for FakeHttpResponse {
        type Error = FakeHttpError;

        fn status(&self) -> StatusCode {
            self.status
        }

        async fn json<T: DeserializeOwned>(self) -> Result<T, Self::Error> {
            serde_json::from_value(self.body?).map_err(FakeHttpError::from)
        }
    }

    #[derive(Default)]
    struct FakeHttpPost {
        requests: Mutex<Vec<RecordedRequest>>,
        response: Mutex<Option<Result<FakeHttpResponse, FakeHttpError>>>,
    }

    impl FakeHttpPost {
        async fn respond_with(&self, response: Result<FakeHttpResponse, FakeHttpError>) {
            *self.response.lock().await = Some(response);
        }

        async fn requests(&self) -> Vec<RecordedRequest> {
            self.requests.lock().await.clone()
        }
    }

    impl HttpPost for FakeHttpPost {
        type Response = FakeHttpResponse;
        type Error = FakeHttpError;

        async fn post_json<B: Serialize>(&self, url: &str, body: &B) -> Result<Self::Response, Self::Error> {
            let body_value = serde_json::to_value(body).map_err(FakeHttpError::from)?;
            self.requests.lock().await.push(RecordedRequest {
                url: url.to_string(),
                body: body_value,
            });
            self.response
                .lock()
                .await
                .take()
                .unwrap_or_else(|| Ok(FakeHttpResponse::json(StatusCode::OK, json!({ "ok": true }))))
        }
    }

    fn test_config() -> TelegramSourceConfig {
        TelegramSourceConfig {
            webhook_secret: TelegramWebhookSecret::new("webhook-secret").unwrap(),
            registration: Some(TelegramWebhookRegistrationConfig {
                bot_token: TelegramBotToken::new(TEST_BOT_TOKEN).unwrap(),
                public_webhook_url: TelegramPublicWebhookUrl::new(
                    "https://example.com/sources/telegram/primary/webhook",
                )
                .unwrap(),
            }),
            subject_prefix: NatsToken::new("telegram").unwrap(),
            stream_name: NatsToken::new("TELEGRAM").unwrap(),
            stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        }
    }

    #[cfg(coverage)]
    async fn reqwest_test_api(body: &'static str) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        format!("http://{addr}")
    }

    #[test]
    fn set_webhook_endpoint_uses_bot_token() {
        assert_eq!(
            set_webhook_endpoint("https://api.telegram.org/", TEST_BOT_TOKEN),
            "https://api.telegram.org/bot123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ/setWebhook"
        );
    }

    #[test]
    fn registration_http_client_builds() {
        registration_http_client().unwrap();
    }

    #[tokio::test]
    async fn missing_registration_is_noop() {
        let mut config = test_config();
        config.registration = None;
        let http = FakeHttpPost::default();

        register_with_http(&http, TELEGRAM_API_BASE, &config)
            .await
            .unwrap();

        assert!(http.requests().await.is_empty());
    }

    #[tokio::test]
    async fn public_register_webhook_noops_without_registration() {
        let mut config = test_config();
        config.registration = None;
        let client = reqwest::Client::new();

        register_webhook(&config, &client).await.unwrap();
    }

    #[tokio::test]
    async fn successful_registration_sets_webhook_with_config_values() {
        let config = test_config();
        let http = FakeHttpPost::default();

        register_with_http(&http, TELEGRAM_API_BASE, &config)
            .await
            .unwrap();

        let requests = http.requests().await;
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(
            request.url,
            "https://api.telegram.org/bot123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ/setWebhook"
        );
        assert_eq!(
            request.body,
            json!({
                "url": "https://example.com/sources/telegram/primary/webhook",
                "secret_token": "webhook-secret",
            })
        );
    }

    #[tokio::test]
    async fn telegram_rejection_preserves_description() {
        let config = test_config();
        let http = FakeHttpPost::default();
        http.respond_with(Ok(FakeHttpResponse::json(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "description": "bad token" }),
        )))
        .await;

        let err = register_with_http(&http, TELEGRAM_API_BASE, &config)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Telegram webhook registration rejected with 400 Bad Request: bad token"
        );
        assert!(err.source().is_none());
    }

    #[tokio::test]
    async fn telegram_rejection_without_description_uses_fallback() {
        let config = test_config();
        let http = FakeHttpPost::default();
        http.respond_with(Ok(FakeHttpResponse::json(StatusCode::OK, json!({ "ok": false }))))
            .await;

        let err = register_with_http(&http, TELEGRAM_API_BASE, &config)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Telegram webhook registration rejected with 200 OK: missing description"
        );
    }

    #[tokio::test]
    async fn http_send_error_propagates() {
        let config = test_config();
        let http = FakeHttpPost::default();
        http.respond_with(Err(FakeHttpError::new("network down"))).await;

        let err = register_with_http(&http, TELEGRAM_API_BASE, &config)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Telegram webhook registration rejected with 500 Internal Server Error: network down"
        );
    }

    #[tokio::test]
    async fn http_response_decode_error_propagates() {
        let config = test_config();
        let http = FakeHttpPost::default();
        http.respond_with(Ok(FakeHttpResponse::decode_error("not json"))).await;

        let err = register_with_http(&http, TELEGRAM_API_BASE, &config)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Telegram webhook registration rejected with 500 Internal Server Error: not json"
        );
    }

    #[tokio::test]
    async fn http_response_json_error_propagates() {
        let config = test_config();
        let http = FakeHttpPost::default();
        http.respond_with(Ok(FakeHttpResponse::json(
            StatusCode::OK,
            json!({ "description": "missing ok" }),
        )))
        .await;

        let err = register_with_http(&http, TELEGRAM_API_BASE, &config)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("missing field `ok`"));
    }

    #[cfg(coverage)]
    #[tokio::test]
    async fn reqwest_http_traits_delegate_to_reqwest() {
        let config = test_config();
        let api_base = reqwest_test_api(r#"{"ok":true}"#).await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        register_with_http(&client, &api_base, &config).await.unwrap();
    }

    // Verifies that URL-embedded bot tokens are stripped from error messages via `without_url()`.
    // Uses reqwest::Client directly because the error originates from URL parsing in the concrete layer.
    #[tokio::test]
    async fn request_build_failure_is_sanitized_by_registration_error() {
        let client = reqwest::Client::new();

        let err = set_webhook(
            &client,
            "http://not a valid url",
            &SetWebhook {
                bot_token: TEST_BOT_TOKEN.to_string(),
                public_webhook_url: "https://example.com/sources/telegram/primary/webhook".to_string(),
                webhook_secret: "webhook-secret".to_string(),
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, RegistrationError::Request(_)));
        assert!(
            err.to_string()
                .starts_with("Telegram webhook registration request failed:")
        );
        assert!(!err.to_string().contains(TEST_BOT_TOKEN));
        assert!(!err.to_string().contains("/bot"));
        assert!(err.source().is_some());
    }
}

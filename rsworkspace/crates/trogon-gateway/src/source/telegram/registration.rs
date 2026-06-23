#![cfg_attr(coverage, allow(dead_code))]

use std::time::Duration;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::config::TelegramSourceConfig;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const TELEGRAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TELEGRAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("Telegram webhook registration request failed: {0}")]
    Request(#[source] reqwest::Error),
    #[error("Telegram webhook registration rejected with {status}: {description}")]
    Rejected { status: StatusCode, description: String },
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

trait HttpSend {
    type Response: HttpResponse;
    type Error;

    async fn send(&self, request: reqwest::Request) -> Result<Self::Response, Self::Error>;
}

trait HttpResponse {
    type Error;

    fn status(&self) -> StatusCode;

    async fn json<T: DeserializeOwned>(self) -> Result<T, Self::Error>;
}

impl HttpSend for reqwest::Client {
    type Response = reqwest::Response;
    type Error = reqwest::Error;

    async fn send(&self, request: reqwest::Request) -> Result<Self::Response, Self::Error> {
        self.execute(request).await
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
    register_with_http(http_client, http_client, TELEGRAM_API_BASE, config).await
}

async fn register_with_http<H>(
    request_client: &reqwest::Client,
    http: &H,
    api_base: &str,
    config: &TelegramSourceConfig,
) -> Result<(), RegistrationError>
where
    H: HttpSend,
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

    let response = set_webhook(request_client, http, api_base, &request).await?;

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
    request_client: &reqwest::Client,
    http: &H,
    api_base: &str,
    request: &SetWebhook,
) -> Result<SetWebhookResponse, RegistrationError>
where
    H: HttpSend,
    RegistrationError: From<H::Error> + From<<H::Response as HttpResponse>::Error>,
{
    let endpoint = set_webhook_endpoint(api_base, &request.bot_token);
    let request_body = SetWebhookRequest {
        url: &request.public_webhook_url,
        secret_token: &request.webhook_secret,
    };

    let http_request = request_client.post(endpoint).json(&request_body).build()?;
    let response = http.send(http_request).await.map_err(RegistrationError::from)?;
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
mod tests;

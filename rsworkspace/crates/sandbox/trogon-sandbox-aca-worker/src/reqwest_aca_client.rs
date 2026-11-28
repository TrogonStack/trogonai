use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::ShortLivedToken;
use url::Url;

use crate::constants::DEFAULT_API_VERSION;
use crate::directory_url::directory_url;
use crate::{
    AcaError, AcaSessionApi, ExecuteResult, SessionDeleteOutcome, SessionIdentifier, SessionInfo, SessionStatus,
};

/// The Dynamic Sessions data plane over HTTP.
#[derive(Debug, Clone)]
pub struct ReqwestAcaClient {
    base: Url,
    api_version: String,
    http: Client,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequest<'a> {
    properties: ExecuteRequestProperties<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequestProperties<'a> {
    code_input_type: &'static str,
    execution_type: &'static str,
    code_input_type_language: &'a str,
    code: &'a str,
}

#[derive(Deserialize)]
struct ExecuteResponse {
    properties: Option<ExecuteResponseProperties>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteResponseProperties {
    stdout: Option<String>,
    stderr: Option<String>,
    exit_code: Option<i32>,
    status: Option<String>,
}

impl ExecuteResponseProperties {
    /// The exit code the service reported. Without one, only an explicit `Succeeded` status stands for exit
    /// code 0; a response that reports neither is refused instead of read as success.
    fn exit_code(&self) -> Result<i32, AcaError> {
        match (self.exit_code, self.status.as_deref()) {
            (Some(code), _) => Ok(code),
            (None, Some(status)) if status.eq_ignore_ascii_case("succeeded") => Ok(0),
            (None, status) => Err(AcaError::aca_api(
                200,
                format!(
                    "execution response has no exit code and status {}",
                    status.unwrap_or("absent")
                ),
            )),
        }
    }
}

impl ReqwestAcaClient {
    /// A client for the session pool management endpoint.
    pub fn new(management_endpoint: Url) -> Self {
        Self {
            base: directory_url(management_endpoint),
            api_version: DEFAULT_API_VERSION.to_owned(),
            http: Client::new(),
        }
    }

    pub fn with_api_version(mut self, api_version: impl Into<String>) -> Self {
        self.api_version = api_version.into();
        self
    }

    fn session_url(&self, path: &str, identifier: &SessionIdentifier) -> Result<Url, AcaError> {
        let mut url = self.base.join(path)?;
        url.query_pairs_mut()
            .append_pair("api-version", &self.api_version)
            .append_pair("identifier", identifier.as_str());
        Ok(url)
    }
}

async fn failure(response: Response) -> AcaError {
    let status = response.status().as_u16();
    AcaError::aca_api(status, response.text().await.unwrap_or_default())
}

#[async_trait]
impl AcaSessionApi for ReqwestAcaClient {
    async fn get_session(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
    ) -> Result<SessionInfo, AcaError> {
        let url = self.session_url("session", identifier)?;
        let response = self.http.get(url).bearer_auth(bearer.expose_secret()).send().await?;
        if !response.status().is_success() {
            return Err(failure(response).await);
        }
        let body = response.text().await?;
        Ok(SessionInfo {
            identifier: identifier.clone(),
            status: SessionStatus::from_body(&body)?,
        })
    }

    async fn execute_code(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
        code: &str,
        language: &str,
    ) -> Result<ExecuteResult, AcaError> {
        let url = self.session_url("code/execute", identifier)?;
        let request = ExecuteRequest {
            properties: ExecuteRequestProperties {
                code_input_type: "inline",
                execution_type: "synchronous",
                code_input_type_language: language,
                code,
            },
        };
        let response = self
            .http
            .post(url)
            .bearer_auth(bearer.expose_secret())
            .json(&request)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(failure(response).await);
        }
        let properties = response
            .json::<ExecuteResponse>()
            .await?
            .properties
            .ok_or_else(|| AcaError::aca_api(200, "execution response has no properties"))?;
        let exit_code = properties.exit_code()?;
        Ok(ExecuteResult {
            stdout: properties.stdout.unwrap_or_default(),
            stderr: properties.stderr.unwrap_or_default(),
            exit_code,
        })
    }

    async fn upload_file(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
        remote_path: &str,
        data: &[u8],
    ) -> Result<(), AcaError> {
        let url = self.session_url("files/upload", identifier)?;
        let part = Part::bytes(data.to_vec())
            .file_name(remote_path.to_owned())
            .mime_str("application/octet-stream")?;
        let response = self
            .http
            .post(url)
            .bearer_auth(bearer.expose_secret())
            .multipart(Form::new().part("file", part))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(failure(response).await);
        }
        Ok(())
    }

    async fn delete_session(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
    ) -> Result<SessionDeleteOutcome, AcaError> {
        let url = self.session_url("session", identifier)?;
        let response = self.http.delete(url).bearer_auth(bearer.expose_secret()).send().await?;
        match response.status().as_u16() {
            404 => Ok(SessionDeleteOutcome::Absent),
            202 => Err(AcaError::aca_api(202, "session deletion is still pending")),
            _ if response.status().is_success() => Ok(SessionDeleteOutcome::Deleted),
            _ => Err(failure(response).await),
        }
    }
}

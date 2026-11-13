use std::ffi::OsStr;
use std::future::Future;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value, json};

use crate::adapters::{CurrentDirectory, EnvironmentVariable, HttpExecute, SystemEnvironment};
use crate::bridge_git::{DefaultGitProbe, GitContextProbe, git_entries};
use crate::bridge_js::{js_number, js_trim, plain_detail, safe_integer};
use crate::bridge_types::{
    CreateCodeSessionOptions, CreateSessionFailure, CreateSessionOutcome, CredentialsFailure, CredentialsRejection,
    DeviceRefusal, FailureSource, FetchRemoteCredentialsOptions, RemoteCredentials, RemoteCredentialsOutcome,
    RemoteMcpConfig,
};
use crate::settings_json::js_stringify;
use crate::settings_numbers::js_value;
use crate::settings_parse::parse_json;
use crate::{BranchDropLogDedup, SensitiveString};

pub(crate) const USER_AGENT: &str = "claude-code/0.3.323";
const ENTRYPOINT_VARIABLE: &str = "CLAUDE_CODE_ENTRYPOINT";

static REQUEST_ID: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^req_[A-Za-z0-9_-]{1,36}$").ok());
static SNAKE_CASE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^[a-z0-9_]{1,64}$").ok());

struct Reply {
    status: u16,
    headers: HeaderMap,
    data: Value,
}

/// Talks to the remote session service the way the upstream bridge entrypoint does.
#[derive(Clone, Debug)]
pub struct BridgeClient<H = reqwest::Client, E = SystemEnvironment, G = DefaultGitProbe> {
    http: H,
    environment: E,
    git: G,
}

impl BridgeClient {
    /// A client over the system network, environment and working directory that follows up to 21 redirects.
    pub fn native() -> reqwest::Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(21))
            .referer(false)
            .build()?;
        Ok(Self::new(http, SystemEnvironment, DefaultGitProbe))
    }
}

impl<H, E, G> BridgeClient<H, E, G> {
    pub fn new(http: H, environment: E, git: G) -> Self {
        Self { http, environment, git }
    }
}

impl<H, E, G> BridgeClient<H, E, G>
where
    H: HttpExecute,
    E: EnvironmentVariable + CurrentDirectory,
    G: GitContextProbe,
{
    /// POST `/v1/code/sessions/{id}/bridge`.
    pub async fn fetch_remote_credentials(&self, options: &FetchRemoteCredentialsOptions) -> RemoteCredentialsOutcome {
        let id = &options.session_id;
        if id.is_empty() || !id.chars().all(valid_id_char) {
            return RemoteCredentialsOutcome::Failure(CredentialsFailure::InvalidSessionId);
        }
        let url = format!("{}/v1/code/sessions/{id}/bridge", options.base_url);
        let Some(reply) = self
            .post(
                &url,
                &Value::Object(Map::new()),
                &options.access_token,
                options.timeout,
                options.trusted_device_token.as_ref(),
            )
            .await
        else {
            return RemoteCredentialsOutcome::Unavailable;
        };
        if reply.status == 200 {
            return credentials(&reply.data);
        }
        if reply.status == 401 {
            return RemoteCredentialsOutcome::Rejected(CredentialsRejection);
        }
        if reply.status == 403
            && let Some(failure) = forbidden(&reply.data)
        {
            return RemoteCredentialsOutcome::Failure(failure);
        }
        match reply.status {
            408 | 429 => RemoteCredentialsOutcome::Unavailable,
            status => RemoteCredentialsOutcome::Failure(CredentialsFailure::RequestRejected {
                status,
                source: (status == 403).then(|| failure_source(&reply.headers)),
            }),
        }
    }

    /// POST `/v1/code/sessions`.
    ///
    /// The only error is the one the working directory lookup reports when `cwd` is not given.
    pub async fn create_code_session(
        &self,
        options: &CreateCodeSessionOptions,
        dedup: Option<&mut BranchDropLogDedup>,
    ) -> Result<CreateSessionOutcome, E::Error> {
        let mut config = Map::new();
        let cwd = match &options.cwd {
            Some(cwd) => cwd.clone(),
            None => self.environment.current_dir()?.to_string_lossy().into_owned(),
        };
        config.insert("cwd".to_owned(), Value::String(cwd));
        if let Some(model) = options.model.as_deref().filter(|model| !model.is_empty()) {
            config.insert("model".to_owned(), Value::String(model.to_owned()));
        }
        if let Some(context) = options.git_context.as_ref()
            && let Some((source, outcome)) = git_entries(&self.git, context, dedup).await
        {
            config.insert("sources".to_owned(), json!([source]));
            config.insert("outcomes".to_owned(), json!([outcome]));
            config.insert("reuse_outcome_branches".to_owned(), Value::Bool(true));
        }
        let mut body = Map::new();
        body.insert("title".to_owned(), Value::String(options.title.clone()));
        body.insert("bridge".to_owned(), Value::Object(Map::new()));
        if !options.tags.is_empty() {
            body.insert("tags".to_owned(), json!(options.tags));
        }
        if let Some(group) = options.session_grouping_id.as_deref().filter(|group| !group.is_empty()) {
            body.insert("session_grouping_id".to_owned(), Value::String(group.to_owned()));
        }
        body.insert("config".to_owned(), Value::Object(config));
        let url = format!("{}/v1/code/sessions", options.base_url);
        let Some(reply) = self
            .post(&url, &Value::Object(body), &options.access_token, options.timeout, None)
            .await
        else {
            return Ok(CreateSessionOutcome::Unavailable);
        };
        Ok(created(&reply))
    }

    async fn post(
        &self,
        url: &str,
        body: &Value,
        token: &SensitiveString,
        timeout: Option<Duration>,
        device_token: Option<&SensitiveString>,
    ) -> Option<Reply> {
        let url = reqwest::Url::parse(url).ok()?;
        let mut request = reqwest::Request::new(reqwest::Method::POST, url);
        let entrypoint = self.environment.var_os(OsStr::new(ENTRYPOINT_VARIABLE));
        let platform = client_platform(entrypoint.as_deref().and_then(OsStr::to_str).unwrap_or(""));
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", token.expose())).ok()?;
        authorization.set_sensitive(true);
        let headers = request.headers_mut();
        headers.insert("accept", HeaderValue::from_static("application/json, text/plain, */*"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("authorization", authorization);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("anthropic-client-platform", HeaderValue::from_str(platform).ok()?);
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        if let Some(device) = device_token.filter(|device| !device.expose().is_empty()) {
            let mut value = HeaderValue::from_str(device.expose()).ok()?;
            value.set_sensitive(true);
            headers.insert("x-trusted-device-token", value);
        }
        *request.body_mut() = Some(js_stringify(body).into_bytes().into());
        let mut response = within(timeout, self.http.execute(request)).await?.ok()?;
        let status = response.status().as_u16();
        if status >= 500 {
            return None;
        }
        let headers = response.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = within(timeout, response.chunk()).await?.ok()? {
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let data = if text.is_empty() {
            Value::String(String::new())
        } else {
            parse_json(text).unwrap_or_else(|_| Value::String(text.to_owned()))
        };
        Some(Reply { status, headers, data })
    }
}

async fn within<T>(timeout: Option<Duration>, future: impl Future<Output = T>) -> Option<T> {
    match timeout {
        Some(timeout) => tokio::time::timeout(timeout, future).await.ok(),
        None => Some(future.await),
    }
}

fn valid_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

pub(crate) fn client_platform(entrypoint: &str) -> &'static str {
    match entrypoint {
        "claude-vscode" => "claude_code_vscode",
        "remote" | "remote_baku" | "remote_cowork" | "remote_desktop" | "remote_mobile" | "remote_projects"
        | "claude-in-teams" => "claude_code_remote",
        "sdk-cli" | "sdk-ts" | "sdk-py" => "claude_code_sdk",
        "mcp" => "claude_code_mcp",
        "claude-code-github-action" => "claude_code_github_action",
        "local-agent" => "claude_code_local_agent",
        "claude_in_slack" => "claude_in_slack",
        "claude-in-slack" => "claude-in-slack",
        _ => "claude_code_cli",
    }
}

pub(crate) fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
    let mut values = headers
        .get_all(&name)
        .iter()
        .map(|value| value.as_bytes().iter().map(|b| char::from(*b)).collect::<String>());
    let first = values.next()?;
    if name == "server" {
        return Some(first);
    }
    Some(values.fold(first, |joined, next| format!("{joined}, {next}")))
}

fn failure_source(headers: &HeaderMap) -> FailureSource {
    let request_id = header_text(headers, "request-id");
    if request_id
        .as_deref()
        .is_some_and(|id| REQUEST_ID.as_ref().is_some_and(|pattern| pattern.is_match(js_trim(id))))
    {
        return FailureSource::Origin;
    }
    let ray = header_text(headers, "cf-ray").is_some_and(|ray| !ray.is_empty());
    let server = header_text(headers, "server").is_some_and(|server| server.to_lowercase().contains("cloudflare"));
    if ray || server {
        FailureSource::NonoriginCf
    } else {
        FailureSource::NonoriginOther
    }
}

fn forbidden(data: &Value) -> Option<CredentialsFailure> {
    let error = data.as_object()?.get("error")?.as_object()?;
    match error.get("resource")?.as_str()? {
        "untrusted_device" => Some(CredentialsFailure::UntrustedDevice {
            refusal: refusal(error.get("sub_reason")),
        }),
        "session_stale_relogin" => Some(CredentialsFailure::SessionStaleRelogin),
        "session_security_tier_below_org_policy" => Some(CredentialsFailure::SessionSecurityTierBelowOrgPolicy),
        _ => None,
    }
}

fn refusal(sub_reason: Option<&Value>) -> DeviceRefusal {
    let unrecognized = |sub_reason: &str| DeviceRefusal::Unrecognized {
        sub_reason: sub_reason.to_owned(),
    };
    match sub_reason {
        None | Some(Value::Null) => DeviceRefusal::TokenRefused,
        Some(Value::String(text)) if text.is_empty() || text == "bound_unattested" => DeviceRefusal::TokenRefused,
        Some(Value::String(text)) if text == "served_call_unsigned" => DeviceRefusal::SignatureWanted {
            sub_reason: text.clone(),
        },
        Some(Value::String(text)) if SNAKE_CASE.as_ref().is_some_and(|pattern| pattern.is_match(text)) => {
            unrecognized(text)
        }
        Some(Value::String(_)) => unrecognized("(string, too long or not snake_case)"),
        Some(Value::Number(_)) => unrecognized("(number, not a string)"),
        Some(Value::Bool(_)) => unrecognized("(boolean, not a string)"),
        Some(Value::Array(_) | Value::Object(_)) => unrecognized("(object, not a string)"),
    }
}

fn credentials(data: &Value) -> RemoteCredentialsOutcome {
    let malformed = || RemoteCredentialsOutcome::Failure(CredentialsFailure::MalformedResponse { status: 200 });
    let Some(object) = data.as_object() else {
        return malformed();
    };
    let (Some(Value::String(jwt)), Some(Value::Number(expires)), Some(Value::String(base)), Some(epoch)) = (
        object.get("worker_jwt"),
        object.get("expires_in"),
        object.get("api_base_url"),
        object.get("worker_epoch"),
    ) else {
        return malformed();
    };
    let epoch = match epoch {
        Value::Number(number) => js_value(number),
        Value::String(text) => Some(js_number(text)),
        _ => None,
    };
    let Some(worker_epoch) = epoch.and_then(safe_integer) else {
        return malformed();
    };
    let mcp_config = object
        .get("mcp_config")
        .and_then(Value::as_object)
        .and_then(|config| config.get("content"))
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
        .map(|content| RemoteMcpConfig {
            content: content.to_owned(),
        });
    RemoteCredentialsOutcome::Credentials(RemoteCredentials {
        worker_jwt: SensitiveString::from(jwt.as_str()),
        api_base_url: base.clone(),
        expires_in: js_value(expires).unwrap_or(f64::NAN),
        worker_epoch,
        mcp_config,
    })
}

fn created(reply: &Reply) -> CreateSessionOutcome {
    let status = reply.status;
    if status == 200 || status == 201 {
        let id = reply
            .data
            .as_object()
            .and_then(|data| data.get("session"))
            .and_then(Value::as_object)
            .and_then(|session| session.get("id"))
            .and_then(Value::as_str)
            .filter(|id| id.starts_with("cse_") && id.chars().all(valid_id_char) && !id.starts_with('-'));
        return match id {
            Some(id) => CreateSessionOutcome::Created(id.to_owned()),
            None => CreateSessionOutcome::Failure(CreateSessionFailure::MalformedResponse { status }),
        };
    }
    let detail = error_detail(&reply.data);
    if (400..500).contains(&status) && grouping_rejected(&reply.data) {
        return CreateSessionOutcome::Failure(CreateSessionFailure::GroupingRejected { status, detail });
    }
    match status {
        401 => CreateSessionOutcome::Rejected(CredentialsRejection),
        408 | 429 => CreateSessionOutcome::Unavailable,
        _ => CreateSessionOutcome::Failure(CreateSessionFailure::RequestRejected { status, detail }),
    }
}

fn grouping_rejected(data: &Value) -> bool {
    let Some(error) = data
        .as_object()
        .and_then(|data| data.get("error"))
        .and_then(Value::as_object)
    else {
        return false;
    };
    let text = |key: &str| error.get(key).and_then(Value::as_str);
    (text("type") == Some("not_found_error") && text("resource_type") == Some("session_grouping"))
        || matches!(text("reason"), Some("public_grouping_hosted_only" | "feature_disabled"))
}

fn error_detail(data: &Value) -> Option<String> {
    let object = data.as_object()?;
    let message = match object.get("message") {
        Some(Value::String(message)) => message.as_str(),
        _ => object
            .get("error")
            .and_then(Value::as_object)
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)?,
    };
    if message.is_empty() {
        return None;
    }
    plain_detail(message)
}

#[cfg(test)]
#[path = "bridge_client_tests.rs"]
mod tests;

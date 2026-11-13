//! Test doubles shared by the OpenBao adapter's tests: a clock the tests move
//! by hand, a scripted OpenBao that answers calls in order, and a real OpenBao
//! container set up the way the secrets service expects to find it.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ExecCommand, IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use trogon_std::GetNow;
use trogon_std::fs::MemFs;
use url::Url;

use super::constants::{CORRELATION_ID_HEADER, TOKEN_HEADER};
use super::openbao_app_role_session::OpenBaoClock;
use super::{
    OpenBaoAppRole, OpenBaoChildTokens, OpenBaoPolicyName, OpenBaoSecretStore, OpenBaoTokenRole, OpenBaoTransport,
};

const OPENBAO_IMAGE: &str = "openbao/openbao";
const OPENBAO_IMAGE_TAG: &str = "2.5.5";
const OPENBAO_ROOT_TOKEN: &str = "dev-only-token";
const OPENBAO_PORT: u16 = 8200;
const AUDIT_LOG_PATH: &str = "/tmp/openbao-audit.log";
const AUDIT_CONFIG_PATH: &str = "/etc/openbao/audit.hcl";
const APPROLE_NAME: &str = "secrets-service";
const SECRET_ID_FILE: &str = "/run/secrets/openbao-secret-id";
pub(crate) const LOGIN_POLICY: &str = "secrets_service_login";
pub(crate) const RELEASE_READ_POLICY: &str = "secrets_service_release_read";
pub(crate) const RELEASE_TOKEN_ROLE: &str = "secrets_service_release";
const LOGIN_POLICY_HCL: &str = include_str!("../../../../../devops/openbao/policies/secrets-service-login.hcl");
const RELEASE_READ_POLICY_HCL: &str =
    include_str!("../../../../../devops/openbao/policies/secrets-service-release-read.hcl");

/// A clock that moves only when a test advances it.
#[derive(Clone)]
pub(crate) struct StepClock {
    start: Instant,
    offset: Arc<std::sync::Mutex<Duration>>,
}

impl StepClock {
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            offset: Arc::default(),
        }
    }

    pub(crate) fn advance(&self, by: Duration) {
        *self.offset.lock().unwrap() += by;
    }

    pub(crate) fn shared(&self) -> OpenBaoClock {
        Arc::new(self.clone())
    }
}

impl GetNow for StepClock {
    type Instant = Instant;

    fn now(&self) -> Instant {
        self.start + *self.offset.lock().unwrap()
    }
}

/// An AppRole whose secret id is read from an in-memory file.
pub(crate) fn approle(role_id: &str, secret_id: &str) -> OpenBaoAppRole {
    let fs = MemFs::new();
    let path = PathBuf::from(SECRET_ID_FILE);
    fs.insert(path.clone(), format!("{secret_id}\n"));
    OpenBaoAppRole::from_secret_id_file(role_id, &path, &fs).unwrap()
}

pub(crate) fn child_tokens() -> OpenBaoChildTokens {
    OpenBaoChildTokens::new(
        OpenBaoTokenRole::new(RELEASE_TOKEN_ROLE).unwrap(),
        OpenBaoPolicyName::new(RELEASE_READ_POLICY).unwrap(),
    )
}

/// The answer OpenBao gives a login, a renewal, or a token creation.
pub(crate) fn grant(token: &str, ttl_secs: u64, renewable: bool) -> (StatusCode, Value) {
    (
        StatusCode::OK,
        json!({ "auth": { "client_token": token, "lease_duration": ttl_secs, "renewable": renewable } }),
    )
}

pub(crate) fn refusal(status: StatusCode, message: &str) -> (StatusCode, Value) {
    (status, json!({ "errors": [message] }))
}

/// One call the scripted OpenBao received.
#[derive(Clone, Debug)]
pub(crate) struct ScriptedCall {
    pub(crate) path: String,
    pub(crate) token: Option<String>,
    pub(crate) correlation_id: Option<String>,
    pub(crate) body: Value,
}

/// An OpenBao that answers each call with the next scripted answer, and a 500
/// once the script runs out.
#[derive(Clone)]
pub(crate) struct ScriptedOpenBao {
    address: SocketAddr,
    answers: Arc<Mutex<VecDeque<(StatusCode, Value)>>>,
    calls: Arc<Mutex<Vec<ScriptedCall>>>,
}

impl ScriptedOpenBao {
    pub(crate) async fn start(answers: impl IntoIterator<Item = (StatusCode, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let openbao = Self {
            address: listener.local_addr().unwrap(),
            answers: Arc::new(Mutex::new(answers.into_iter().collect())),
            calls: Arc::default(),
        };
        let app = Router::new().fallback(answer_from_script).with_state(openbao.clone());
        tokio::spawn(axum::serve(listener, app).into_future());
        openbao
    }

    pub(crate) fn address(&self) -> String {
        format!("http://{}", self.address)
    }

    pub(crate) fn url(&self) -> Url {
        Url::parse(&self.address()).unwrap()
    }

    pub(crate) async fn answer_next(&self, answers: impl IntoIterator<Item = (StatusCode, Value)>) {
        self.answers.lock().await.extend(answers);
    }

    pub(crate) async fn calls(&self) -> Vec<ScriptedCall> {
        self.calls.lock().await.clone()
    }

    pub(crate) async fn paths(&self) -> Vec<String> {
        self.calls().await.into_iter().map(|call| call.path).collect()
    }
}

async fn answer_from_script(
    State(openbao): State<ScriptedOpenBao>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    openbao.calls.lock().await.push(ScriptedCall {
        path: uri.path().to_string(),
        token: header(TOKEN_HEADER),
        correlation_id: header(CORRELATION_ID_HEADER),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    let (status, body) = openbao
        .answers
        .lock()
        .await
        .pop_front()
        .unwrap_or_else(|| refusal(StatusCode::INTERNAL_SERVER_ERROR, "unscripted call"));
    (status, axum::Json(body)).into_response()
}

/// A real OpenBao with a file audit device, the correlation header audited in
/// plaintext, and the secrets service's AppRole, token role, and policies in
/// place.
pub(crate) struct AuditedOpenBao {
    container: ContainerAsync<GenericImage>,
    address: String,
    http: reqwest::Client,
}

impl AuditedOpenBao {
    pub(crate) async fn start() -> Self {
        let audit_config =
            format!("audit \"file\" \"release\" {{\n  options = {{\n    file_path = \"{AUDIT_LOG_PATH}\"\n  }}\n}}\n");
        let container = GenericImage::new(OPENBAO_IMAGE, OPENBAO_IMAGE_TAG)
            .with_wait_for(WaitFor::message_on_stdout(
                "Development mode should NOT be used in production",
            ))
            .with_exposed_port(OPENBAO_PORT.tcp())
            .with_copy_to(AUDIT_CONFIG_PATH, audit_config.into_bytes())
            .with_cmd(vec![
                "server".to_string(),
                "-dev".to_string(),
                format!("-dev-root-token-id={OPENBAO_ROOT_TOKEN}"),
                "-dev-listen-address=0.0.0.0:8200".to_string(),
                format!("-config={AUDIT_CONFIG_PATH}"),
            ])
            .start()
            .await
            .expect("start OpenBao testcontainer");
        let host = container.get_host().await.expect("get OpenBao testcontainer host");
        let port = container
            .get_host_port_ipv4(OPENBAO_PORT)
            .await
            .expect("get OpenBao testcontainer port");
        let openbao = Self {
            container,
            address: format!("http://{host}:{port}"),
            http: reqwest::Client::new(),
        };
        openbao.provision().await;
        openbao
    }

    async fn provision(&self) {
        let header = CORRELATION_ID_HEADER.to_ascii_lowercase();
        self.root_ok(
            "POST",
            &format!("sys/config/auditing/request-headers/{header}"),
            json!({ "hmac": false }),
        )
        .await;
        self.root_ok("POST", "sys/auth/approle", json!({ "type": "approle" }))
            .await;
        self.root_ok(
            "PUT",
            &format!("sys/policies/acl/{LOGIN_POLICY}"),
            json!({ "policy": LOGIN_POLICY_HCL }),
        )
        .await;
        self.root_ok(
            "PUT",
            &format!("sys/policies/acl/{RELEASE_READ_POLICY}"),
            json!({ "policy": RELEASE_READ_POLICY_HCL }),
        )
        .await;
        self.root_ok(
            "POST",
            &format!("auth/token/roles/{RELEASE_TOKEN_ROLE}"),
            json!({
                "allowed_policies": [RELEASE_READ_POLICY],
                "orphan": false,
                "renewable": false,
                "token_no_default_policy": true,
                "token_explicit_max_ttl": "30s",
            }),
        )
        .await;
        self.root_ok(
            "POST",
            &format!("auth/approle/role/{APPROLE_NAME}"),
            json!({
                "token_policies": [LOGIN_POLICY],
                "token_no_default_policy": true,
                "token_ttl": "60s",
                "token_max_ttl": "120s",
            }),
        )
        .await;
    }

    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    /// A store holding the root token, for seeding credentials the release
    /// reader then reads.
    pub(crate) fn root_store(&self) -> OpenBaoSecretStore {
        OpenBaoSecretStore::new(&self.address, OPENBAO_ROOT_TOKEN, OpenBaoTransport::PlaintextAllowed).unwrap()
    }

    pub(crate) async fn approle(&self) -> OpenBaoAppRole {
        let (_, role) = self
            .call(
                OPENBAO_ROOT_TOKEN,
                "GET",
                &format!("auth/approle/role/{APPROLE_NAME}/role-id"),
                None,
            )
            .await;
        let (_, secret) = self
            .call(
                OPENBAO_ROOT_TOKEN,
                "POST",
                &format!("auth/approle/role/{APPROLE_NAME}/secret-id"),
                Some(json!({})),
            )
            .await;
        approle(
            role["data"]["role_id"].as_str().unwrap(),
            secret["data"]["secret_id"].as_str().unwrap(),
        )
    }

    /// Logs in with `approle` directly, returning the raw login token.
    pub(crate) async fn login_token(&self, approle: &OpenBaoAppRole) -> String {
        let (status, body) = self
            .call(
                "",
                "POST",
                "auth/approle/login",
                Some(json!({ "role_id": approle.role_id(), "secret_id": approle.secret_id().as_str() })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["auth"]["client_token"].as_str().unwrap().to_string()
    }

    /// Mints a child exactly as the release reader does, returning its token.
    pub(crate) async fn mint_child(&self, login_token: &str, policy: &str) -> (StatusCode, Value) {
        self.call(
            login_token,
            "POST",
            &format!("auth/token/create/{RELEASE_TOKEN_ROLE}"),
            Some(json!({ "policies": [policy], "ttl": "5s", "num_uses": 2, "no_default_policy": true })),
        )
        .await
    }

    pub(crate) async fn revoke(&self, token: &str) {
        self.root_ok("POST", "auth/token/revoke", json!({ "token": token }))
            .await;
    }

    pub(crate) async fn call(&self, token: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = self.http.request(method, format!("{}/v1/{path}", self.address));
        if !token.is_empty() {
            request = request.header(TOKEN_HEADER, token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn root_ok(&self, method: &str, path: &str, body: Value) {
        let (status, response) = self.call(OPENBAO_ROOT_TOKEN, method, path, Some(body)).await;
        assert!(status.is_success(), "{method} {path} -> {status}: {response}");
    }

    /// Every entry the file audit device has written so far.
    pub(crate) async fn audit_log(&self) -> Vec<Value> {
        let mut result = self
            .container
            .exec(ExecCommand::new(["cat", AUDIT_LOG_PATH]))
            .await
            .expect("read the OpenBao audit log");
        let stdout = result.stdout_to_vec().await.expect("collect the OpenBao audit log");
        String::from_utf8(stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

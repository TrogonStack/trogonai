use serde_json::json;
use trogon_sandbox_aca_worker::{
    AcaError, AcaSessionApi, ExecuteResult, ReqwestAcaClient, ServicePrincipal, SessionDeleteOutcome,
    SessionIdentifier, SessionStatus, fetch_dynamic_sessions_token,
};
use trogon_sandbox_contract::ShortLivedToken;
use url::Url;
use wiremock::matchers::{bearer_token, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn client(server: &MockServer) -> Result<ReqwestAcaClient, Box<dyn std::error::Error>> {
    Ok(ReqwestAcaClient::new(Url::parse(&format!(
        "{}/management",
        server.uri()
    ))?))
}

fn bearer() -> ShortLivedToken {
    ShortLivedToken::new("token")
}

fn session() -> SessionIdentifier {
    SessionIdentifier::new("session-1")
}

async fn answer(server: &MockServer, verb: &str, route: &str, response: ResponseTemplate) {
    Mock::given(method(verb))
        .and(path(route))
        .and(query_param("api-version", "2024-02-02-preview"))
        .and(query_param("identifier", "session-1"))
        .and(bearer_token("token"))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_session_that_reports_ready_is_ready() -> TestResult {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/management/session",
        ResponseTemplate::new(200).set_body_json(json!({ "properties": { "status": "Ready" } })),
    )
    .await;

    let info = client(&server)?.get_session(&bearer(), &session()).await?;

    assert_eq!(info.identifier, session());
    assert!(info.status.is_ready());
    Ok(())
}

#[tokio::test]
async fn a_session_that_reports_another_status_is_pending() -> TestResult {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/management/session",
        ResponseTemplate::new(200).set_body_json(json!({ "status": "Creating" })),
    )
    .await;

    let info = client(&server)?.get_session(&bearer(), &session()).await?;

    assert_eq!(info.status, SessionStatus::Pending("Creating".to_owned()));
    Ok(())
}

#[tokio::test]
async fn an_unknown_session_is_an_api_error() -> TestResult {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/management/session",
        ResponseTemplate::new(404).set_body_string("nope"),
    )
    .await;

    let error = client(&server)?.get_session(&bearer(), &session()).await.err();

    assert!(matches!(error, Some(AcaError::AcaApi { status: 404, message }) if message == "nope"));
    Ok(())
}

#[tokio::test]
async fn code_executes_synchronously_and_reports_its_output() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/management/code/execute"))
        .and(bearer_token("token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "properties": { "stdout": "out", "stderr": "err", "exitCode": 3 }
        })))
        .mount(&server)
        .await;

    let result = client(&server)?
        .execute_code(&bearer(), &session(), "print(1)", "python")
        .await?;

    assert_eq!(
        (result.stdout.as_str(), result.stderr.as_str(), result.exit_code),
        ("out", "err", 3)
    );
    let request = server.received_requests().await.ok_or("no requests")?.remove(0);
    let body: serde_json::Value = serde_json::from_slice(&request.body)?;
    assert_eq!(body["properties"]["code"], "print(1)");
    assert_eq!(body["properties"]["codeInputType"], "inline");
    assert_eq!(body["properties"]["executionType"], "synchronous");
    assert_eq!(body["properties"]["codeInputTypeLanguage"], "python");
    Ok(())
}

async fn execute_with(body: serde_json::Value) -> Result<ExecuteResult, AcaError> {
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/management/code/execute",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;
    client(&server)
        .map_err(|error| AcaError::Auth(error.to_string()))?
        .execute_code(&bearer(), &session(), "pass", "python")
        .await
}

#[tokio::test]
async fn a_succeeded_status_without_an_exit_code_means_exit_code_zero() -> TestResult {
    let result = execute_with(json!({"properties": {"status": "Succeeded"}})).await?;
    assert_eq!(
        (result.stdout.as_str(), result.stderr.as_str(), result.exit_code),
        ("", "", 0)
    );
    Ok(())
}

#[tokio::test]
async fn an_execution_with_no_properties_is_refused_not_read_as_success() {
    let error = execute_with(json!({})).await.err();
    assert!(matches!(error, Some(AcaError::AcaApi { .. })));
}

#[tokio::test]
async fn an_execution_with_no_exit_code_and_no_succeeded_status_is_refused() {
    for body in [
        json!({"properties": {"stdout": "out"}}),
        json!({"properties": {"status": "Failed"}}),
    ] {
        let error = execute_with(body).await.err();
        assert!(matches!(error, Some(AcaError::AcaApi { .. })));
    }
}

#[tokio::test]
async fn a_failed_execution_is_an_api_error() -> TestResult {
    let server = MockServer::start().await;
    answer(&server, "POST", "/management/code/execute", ResponseTemplate::new(500)).await;

    let error = client(&server)?
        .execute_code(&bearer(), &session(), "pass", "python")
        .await
        .err();

    assert!(matches!(error, Some(AcaError::AcaApi { status: 500, .. })));
    Ok(())
}

#[tokio::test]
async fn a_file_is_uploaded_as_a_multipart_part() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/management/files/upload"))
        .and(header_exists("content-type"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    client(&server)?
        .upload_file(&bearer(), &session(), "dir/file.txt", b"contents")
        .await?;

    let request = server.received_requests().await.ok_or("no requests")?.remove(0);
    let body = String::from_utf8_lossy(&request.body).into_owned();
    assert!(body.contains("name=\"file\""));
    assert!(body.contains("filename=\"dir/file.txt\""));
    assert!(body.contains("contents"));
    Ok(())
}

#[tokio::test]
async fn a_refused_upload_is_an_api_error() -> TestResult {
    let server = MockServer::start().await;
    answer(&server, "POST", "/management/files/upload", ResponseTemplate::new(403)).await;

    let error = client(&server)?
        .upload_file(&bearer(), &session(), "file.txt", b"x")
        .await
        .err();

    assert!(matches!(error, Some(AcaError::AcaApi { status: 403, .. })));
    Ok(())
}

#[tokio::test]
async fn deleting_a_session_reports_what_the_service_did() -> TestResult {
    let server = MockServer::start().await;
    answer(&server, "DELETE", "/management/session", ResponseTemplate::new(204)).await;
    let deleted = client(&server)?.delete_session(&bearer(), &session()).await?;

    let server = MockServer::start().await;
    answer(&server, "DELETE", "/management/session", ResponseTemplate::new(404)).await;
    let absent = client(&server)?.delete_session(&bearer(), &session()).await?;

    assert_eq!(deleted, SessionDeleteOutcome::Deleted);
    assert_eq!(absent, SessionDeleteOutcome::Absent);
    Ok(())
}

#[tokio::test]
async fn a_pending_or_failed_deletion_is_an_api_error() -> TestResult {
    let server = MockServer::start().await;
    answer(&server, "DELETE", "/management/session", ResponseTemplate::new(202)).await;
    let pending = client(&server)?.delete_session(&bearer(), &session()).await.err();

    let server = MockServer::start().await;
    answer(&server, "DELETE", "/management/session", ResponseTemplate::new(500)).await;
    let failed = client(&server)?.delete_session(&bearer(), &session()).await.err();

    assert!(matches!(pending, Some(AcaError::AcaApi { status: 202, .. })));
    assert!(matches!(failed, Some(AcaError::AcaApi { status: 500, .. })));
    Ok(())
}

#[tokio::test]
async fn the_api_version_can_be_overridden() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/management/session"))
        .and(query_param("api-version", "2099-01-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "Ready" })))
        .mount(&server)
        .await;

    let info = client(&server)?
        .with_api_version("2099-01-01")
        .get_session(&bearer(), &session())
        .await?;

    assert!(info.status.is_ready());
    Ok(())
}

#[tokio::test]
async fn an_unreachable_service_is_a_transport_error() -> TestResult {
    let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let client = ReqwestAcaClient::new(Url::parse(&format!("http://127.0.0.1:{port}"))?);

    let error = client.get_session(&bearer(), &session()).await.err();

    assert!(matches!(error, Some(AcaError::Transport(_))));
    Ok(())
}

fn principal() -> ServicePrincipal {
    ServicePrincipal {
        client_id: "client".to_owned(),
        client_secret: ShortLivedToken::new("secret"),
        tenant_id: "tenant".to_owned(),
    }
}

#[tokio::test]
async fn the_token_endpoint_exchanges_the_principal_for_a_bearer() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tenant/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "exchanged" })))
        .mount(&server)
        .await;

    let token =
        fetch_dynamic_sessions_token(&reqwest::Client::new(), &Url::parse(&server.uri())?, &principal()).await?;

    assert_eq!(token.expose_secret(), "exchanged");
    let request = server.received_requests().await.ok_or("no requests")?.remove(0);
    let form = String::from_utf8_lossy(&request.body).into_owned();
    assert!(form.contains("client_id=client"));
    assert!(form.contains("client_secret=secret"));
    assert!(form.contains("grant_type=client_credentials"));
    assert!(form.contains("scope=https%3A%2F%2Fdynamicsessions.io%2F.default"));
    Ok(())
}

#[tokio::test]
async fn a_refused_exchange_is_an_auth_error() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("denied"))
        .mount(&server)
        .await;

    let error = fetch_dynamic_sessions_token(&reqwest::Client::new(), &Url::parse(&server.uri())?, &principal())
        .await
        .err();

    assert!(matches!(error, Some(AcaError::Auth(reason)) if reason.contains("401") && reason.contains("denied")));
    Ok(())
}

#[tokio::test]
async fn an_exchange_without_a_token_is_an_auth_error() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let error = fetch_dynamic_sessions_token(&reqwest::Client::new(), &Url::parse(&server.uri())?, &principal())
        .await
        .err();

    assert!(matches!(error, Some(AcaError::Auth(reason)) if reason.contains("access_token")));
    Ok(())
}

#[tokio::test]
async fn a_200_that_states_no_status_is_an_error() -> TestResult {
    for body in [
        ResponseTemplate::new(200),
        ResponseTemplate::new(200).set_body_string("not json"),
        ResponseTemplate::new(200).set_body_json(json!({})),
    ] {
        let server = MockServer::start().await;
        answer(&server, "GET", "/management/session", body).await;
        let error = client(&server)?.get_session(&bearer(), &session()).await.err();
        assert!(matches!(error, Some(AcaError::SessionStatus(_))), "{error:?}");
    }
    Ok(())
}

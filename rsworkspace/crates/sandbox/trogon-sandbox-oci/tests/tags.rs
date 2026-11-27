mod support;

use support::{FakeRegistry, TestResult};
use trogon_sandbox_oci::{OciError, RegistryHost};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn host(server: &MockServer) -> Result<RegistryHost, Box<dyn std::error::Error>> {
    Ok(RegistryHost::new(server.address().to_string())?)
}

async fn mount_page(server: &MockServer, query: Option<(&str, &str)>, body: serde_json::Value, link: Option<String>) {
    let mut template = ResponseTemplate::new(200).set_body_json(body);
    if let Some(link) = link {
        template = template.insert_header("link", link.as_str());
    }
    let mut mock = Mock::given(method("GET")).and(path("/v2/acme/app/tags/list"));
    if let Some((key, value)) = query {
        mock = mock.and(query_param(key, value));
    }
    mock.respond_with(template).mount(server).await;
}

#[tokio::test]
async fn tags_are_collected_across_pages() -> TestResult {
    let server = MockServer::start().await;
    mount_page(
        &server,
        Some(("last", "b")),
        serde_json::json!({"name": "acme/app", "tags": ["c"]}),
        None,
    )
    .await;
    mount_page(
        &server,
        Some(("n", "1000")),
        serde_json::json!({"name": "acme/app", "tags": ["a", "b"]}),
        Some("</v2/acme/app/tags/list?n=1000&last=b>; rel=\"next\", <x>; rel=\"prev\"; foo=bar".into()),
    )
    .await;
    let client = support::client_for(&server.address().to_string())?;
    let tags = client.list_tags(&host(&server)?, "acme/app").await?;
    assert_eq!(tags, vec!["a", "b", "c"]);
    Ok(())
}

#[tokio::test]
async fn a_repository_without_a_tag_list_has_no_tags() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let tags = registry.client()?.list_tags(&registry.host()?, "never/pushed").await?;
    assert!(tags.is_empty());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let client = support::client_for(&server.address().to_string())?;
    assert!(client.list_tags(&host(&server)?, "acme/app").await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_repositories_and_responses_are_refused() -> TestResult {
    let server = MockServer::start().await;
    let client = support::client_for(&server.address().to_string())?;
    let error = client.list_tags(&host(&server)?, "Bad Repo!").await.err();
    assert!(matches!(error, Some(OciError::InvalidReference(_))), "{error:?}");

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    assert!(client.list_tags(&host(&server)?, "acme/app").await.is_err());
    Ok(())
}

#[tokio::test]
async fn pagination_links_must_stay_on_the_tag_list() -> TestResult {
    for link in [
        "<http://other.invalid/v2/acme/app/tags/list?last=a>; rel=\"next\"",
        "</v2/acme/other/tags/list?last=a>; rel=\"next\"",
        "<http://[bad>; rel=\"next\"",
    ] {
        let server = MockServer::start().await;
        mount_page(&server, None, serde_json::json!({"tags": ["a"]}), Some(link.into())).await;
        let client = support::client_for(&server.address().to_string())?;
        assert!(client.list_tags(&host(&server)?, "acme/app").await.is_err(), "{link}");
    }
    Ok(())
}

#[tokio::test]
async fn endless_pagination_is_cut_off() -> TestResult {
    let server = MockServer::start().await;
    mount_page(
        &server,
        None,
        serde_json::json!({"tags": []}),
        Some("</v2/acme/app/tags/list?last=a>; rel=\"next\"".into()),
    )
    .await;
    let client = support::client_for(&server.address().to_string())?;
    let error = client.list_tags(&host(&server)?, "acme/app").await.err();
    assert!(matches!(error, Some(OciError::RegistryRejected(message)) if message.contains("page budget")));
    Ok(())
}

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use secrecy::SecretString;
use serde_json::{Value, json};
use support::gated::{gated_registry, gated_registry_with_native_status};
use support::{
    RejectionMessage, TestResult, artifact, body_has, body_lacks, calls, config_for, digest_of, limits,
    no_authorization, sha256,
};
use trogon_sandbox_contract::{ArtifactType, Digest};
use trogon_sandbox_oci::{
    ArtifactPullLimits, Layer, OciClient, OciClientConfig, RegistryCredentials, RegistryHost, build_push_target,
};
use wiremock::matchers::{header, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_with_credentials(registry: &str, extra_insecure: &[&str]) -> Result<OciClient, Box<dyn std::error::Error>> {
    let mut config = config_for(registry)?;
    for extra in extra_insecure {
        config.insecure_registries.push(RegistryHost::new(*extra)?);
    }
    config.credentials = BTreeMap::from([(
        RegistryHost::new(registry)?,
        RegistryCredentials::new("test", SecretString::from("secret")),
    )]);
    Ok(OciClient::new(config))
}

async fn refuses_before_eof(token: bool) -> TestResult {
    let server = gated_registry(token).await?;
    let client = support::client_for(&server.address)?;
    let target = support::artifact_ref(
        &server.address,
        "test",
        Digest::new(format!("sha256:{}", "0".repeat(64)))?,
        ArtifactType::FirecrackerSnapshotV1,
    );
    let mut pull = tokio::spawn(async move {
        client
            .pull_artifact_bounded(
                &target,
                ArtifactPullLimits {
                    max_layers: 4,
                    max_layer_bytes: 1024 * 1024,
                    max_total_bytes: 4 * 1024 * 1024,
                    timeout: Duration::from_secs(30),
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), server.sent.notified()).await?;
    let result = tokio::time::timeout(Duration::from_secs(1), &mut pull).await;
    pull.abort();
    let error = result??.err().ok_or("pull must fail")?.rejection_message()?.to_owned();
    assert!(error.contains("budget"), "{error}");
    Ok(())
}

#[tokio::test]
async fn oversized_chunked_manifest_is_rejected_before_eof() -> TestResult {
    refuses_before_eof(false).await
}

#[tokio::test]
async fn oversized_chunked_auth_token_is_rejected_before_eof() -> TestResult {
    refuses_before_eof(true).await
}

#[derive(Clone, Copy)]
enum Metadata {
    ListReferrers,
    AttachWithMediaType,
    AttachReferrer,
    HeadManifest,
}

async fn oversized_metadata(operation: Metadata, fallback: bool) -> TestResult {
    let server = gated_registry_with_native_status(false, fallback.then_some(404)).await?;
    let client = support::client_for(&server.address)?;
    let target = support::artifact_ref(
        &server.address,
        "test",
        Digest::new(format!("sha256:{}", "0".repeat(64)))?,
        ArtifactType::FirecrackerSnapshotV1,
    );
    let mut request = tokio::spawn(async move {
        match operation {
            Metadata::ListReferrers => client.list_referrers(&target).await.map(|_| ()),
            Metadata::AttachWithMediaType => {
                client
                    .attach_referrer_with_media_type(&target, &target.digest, 2, "fixture/report")
                    .await
            }
            Metadata::AttachReferrer => client.attach_referrer(&target, &target).await,
            Metadata::HeadManifest => client.head_manifest(&target).await.map(|_| ()),
        }
    });
    tokio::time::timeout(Duration::from_secs(5), server.sent.notified()).await?;
    let result = tokio::time::timeout(Duration::from_secs(1), &mut request).await;
    request.abort();
    let error = result??
        .err()
        .ok_or("operation must fail")?
        .rejection_message()?
        .to_owned();
    assert!(error.contains("budget"), "{error}");
    Ok(())
}

#[tokio::test]
async fn list_referrers_rejects_oversized_chunks_before_eof() -> TestResult {
    oversized_metadata(Metadata::ListReferrers, false).await
}

#[tokio::test]
async fn external_referrer_index_rejects_oversized_chunks_before_eof() -> TestResult {
    oversized_metadata(Metadata::AttachWithMediaType, false).await
}

#[tokio::test]
async fn attached_referrer_manifest_rejects_oversized_chunks_before_eof() -> TestResult {
    oversized_metadata(Metadata::AttachReferrer, false).await
}

#[tokio::test]
async fn head_manifest_rejects_oversized_chunks_before_eof() -> TestResult {
    oversized_metadata(Metadata::HeadManifest, false).await
}

#[tokio::test]
async fn fallback_referrers_rejects_oversized_chunks_before_eof() -> TestResult {
    oversized_metadata(Metadata::ListReferrers, true).await
}

#[tokio::test]
async fn invalid_digest_header_is_not_echoed_into_errors() -> TestResult {
    let server = MockServer::start().await;
    let registry = server.address().to_string();
    let (target, raw) = artifact(&registry, b"blob")?;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/manifests/{}", target.digest)))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("docker-content-digest", "Bearer fixture-token-value")
                .set_body_bytes(raw),
        )
        .mount(&server)
        .await;
    let error = support::client_for(&registry)?
        .head_manifest(&target)
        .await
        .err()
        .ok_or("must fail")?
        .rejection_message()?
        .to_owned();
    assert!(!error.contains("fixture-token-value"), "{error}");
    Ok(())
}

#[tokio::test]
async fn mismatching_digest_header_is_rejected() -> TestResult {
    let server = MockServer::start().await;
    let registry = server.address().to_string();
    let (target, raw) = artifact(&registry, b"blob")?;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/manifests/{}", target.digest)))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("docker-content-digest", format!("sha256:{}", "b".repeat(64)))
                .set_body_bytes(raw),
        )
        .mount(&server)
        .await;
    let error = support::client_for(&registry)?.head_manifest(&target).await.err();
    assert!(matches!(
        error,
        Some(trogon_sandbox_oci::OciError::DigestMismatch { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn manifest_bytes_that_do_not_hash_to_the_selector_are_rejected() -> TestResult {
    let server = MockServer::start().await;
    let registry = server.address().to_string();
    let (target, _) = artifact(&registry, b"blob")?;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/manifests/{}", target.digest)))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"{}".to_vec()))
        .mount(&server)
        .await;
    let error = support::client_for(&registry)?.head_manifest(&target).await.err();
    assert!(matches!(
        error,
        Some(trogon_sandbox_oci::OciError::DigestMismatch { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn anonymous_basic_and_bearer_challenges_preserve_standard_registry_flows() -> TestResult {
    for mode in 0..4 {
        let server = MockServer::start().await;
        let registry = server.address().to_string();
        let (target, raw) = artifact(&registry, b"blob")?;
        let manifest_path = format!("/v2/test/manifests/{}", target.digest);
        let blob_path = format!("/v2/test/blobs/{}", sha256(b"blob"));
        let auth = if mode == 1 {
            "Basic dGVzdDpzZWNyZXQ="
        } else {
            "Bearer fixture-token"
        };
        if mode == 0 {
            Mock::given(method("GET"))
                .and(path(&manifest_path))
                .and(no_authorization)
                .respond_with(ResponseTemplate::new(200).set_body_bytes(raw.clone()))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(&blob_path))
                .and(no_authorization)
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"blob".to_vec()))
                .mount(&server)
                .await;
        } else {
            Mock::given(method("GET"))
                .and(path(&manifest_path))
                .and(header("authorization", auth))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(raw.clone()))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(&blob_path))
                .and(header("authorization", auth))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"blob".to_vec()))
                .mount(&server)
                .await;
            let challenge = if mode == 1 {
                "Basic realm=\"fixture\"".to_string()
            } else {
                format!(
                    "bEaReR realm=\"http://{registry}/token\",service=\"fixture,service\",scope=\"repository:test:pull\""
                )
            };
            Mock::given(method("GET"))
                .and(path(&manifest_path))
                .and(no_authorization)
                .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
                .mount(&server)
                .await;
        }
        if mode >= 2 {
            let token = Mock::given(method("GET"))
                .and(path("/token"))
                .and(query_param("scope", "repository:test:pull"))
                .and(query_param("service", "fixture,service"));
            if mode == 3 {
                token
                    .and(header("authorization", "Basic dGVzdDpzZWNyZXQ="))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "fixture-token"})))
                    .mount(&server)
                    .await;
            } else {
                token
                    .and(no_authorization)
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({"token": "fixture-token"})))
                    .mount(&server)
                    .await;
            }
        }
        let client = if mode == 1 || mode == 3 {
            client_with_credentials(&registry, &[])?
        } else {
            support::client_for(&registry)?
        };
        let pulled = client.pull_artifact_bounded(&target, limits()).await?;
        assert_eq!(pulled.layers[0].data, b"blob");
        if mode >= 2 {
            assert_eq!(calls(&server, "GET", "/token").await, 1);
        }
    }
    Ok(())
}

#[tokio::test]
async fn cross_origin_blob_redirect_strips_credentials_and_preserves_before_eof_caps() -> TestResult {
    let origin = MockServer::start().await;
    let registry = origin.address().to_string();
    let cdn = gated_registry(false).await?;
    let (target, raw) = artifact(&registry, b"blob")?;
    let manifest_path = format!("/v2/test/manifests/{}", target.digest);
    Mock::given(method("GET"))
        .and(path(&manifest_path))
        .and(no_authorization)
        .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", "Basic realm=\"fixture\""))
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .and(path(&manifest_path))
        .and(header("authorization", "Basic dGVzdDpzZWNyZXQ="))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(raw))
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("/blobs/"))
        .and(header("authorization", "Basic dGVzdDpzZWNyZXQ="))
        .respond_with(ResponseTemplate::new(307).insert_header(
            "location",
            format!("http://{}/download?signature=fixture-signed-secret", cdn.address),
        ))
        .mount(&origin)
        .await;
    let client = client_with_credentials(&registry, &[&cdn.address])?;
    let mut pull = tokio::spawn(async move { client.pull_artifact_bounded(&target, limits()).await });
    tokio::time::timeout(Duration::from_secs(5), cdn.sent.notified()).await?;
    let result = tokio::time::timeout(Duration::from_secs(1), &mut pull).await;
    pull.abort();
    let error = result??.err().ok_or("pull must fail")?.rejection_message()?.to_owned();
    assert!(!cdn.credentials_received.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!error.contains("fixture-signed-secret"), "{error}");
    assert!(error.contains("budget"), "{error}");
    Ok(())
}

#[tokio::test]
async fn credential_urls_and_manifest_or_auth_redirects_fail_without_following() -> TestResult {
    for case in 0..5 {
        let server = MockServer::start().await;
        let registry = server.address().to_string();
        let (mut target, _) = artifact(&registry, b"blob")?;
        let manifest_path = format!("/v2/test/manifests/{}", target.digest);
        Mock::given(method("GET"))
            .and(path("/stolen"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("http://{registry}/stolen?token=fixture-secret")),
            )
            .mount(&server)
            .await;
        if case == 3 {
            Mock::given(method("GET"))
                .and(path(&manifest_path))
                .respond_with(
                    ResponseTemplate::new(302)
                        .insert_header("location", format!("http://{registry}/stolen?token=fixture-secret")),
                )
                .mount(&server)
                .await;
        } else {
            let realm = match case {
                0 => format!("http://user:fixture-secret@{registry}/token"),
                1 => format!("ftp://{registry}/token"),
                _ => format!("http://{registry}/token"),
            };
            Mock::given(method("GET"))
                .and(path(&manifest_path))
                .respond_with(ResponseTemplate::new(401).insert_header(
                    "www-authenticate",
                    format!("Bearer realm=\"{realm}\",service=\"fixture\""),
                ))
                .mount(&server)
                .await;
        }
        if case == 4 {
            target.registry = format!("user:fixture-secret@{registry}");
        }
        let failure = support::client_for(&registry)?
            .pull_artifact_bounded(&target, limits())
            .await
            .err()
            .ok_or("pull must fail")?;
        let error = format!("{failure:?}");
        assert!(!error.contains("fixture-secret"), "{error}");
        assert_eq!(calls(&server, "GET", "/stolen").await, 0);
        assert_eq!(calls(&server, "GET", "/token").await, usize::from(case == 2));
    }
    Ok(())
}

fn referrers_index(entries: Value) -> Value {
    json!({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.index.v1+json", "manifests": entries})
}

#[tokio::test]
async fn bounded_referrers_preserve_native_fallback_and_attachment_flows() -> TestResult {
    for mode in 0..4 {
        let server = MockServer::start().await;
        let registry = server.address().to_string();
        let (target, raw) = artifact(&registry, b"blob")?;
        let client = support::client_for(&registry)?;
        let index = referrers_index(json!([{
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": target.digest.as_str(),
            "size": raw.len(),
            "artifactType": target.artifact_type.media_type(),
        }]));
        let empty = referrers_index(json!([]));
        let referrers_path = format!("/v2/test/referrers/{}", target.digest);
        let native = if mode == 1 || mode == 3 {
            ResponseTemplate::new(404)
        } else {
            ResponseTemplate::new(200).set_body_json(if mode == 2 { empty } else { index.clone() })
        };
        Mock::given(method("GET"))
            .and(path(&referrers_path))
            .respond_with(native)
            .mount(&server)
            .await;
        let fallback_path = format!("/v2/test/manifests/{}", target.digest.as_str().replace(':', "-"));
        let fallback = if mode == 3 {
            ResponseTemplate::new(404)
        } else {
            ResponseTemplate::new(200).set_body_json(index)
        };
        Mock::given(method("GET"))
            .and(path(&fallback_path))
            .respond_with(fallback)
            .mount(&server)
            .await;
        let listed = client.list_referrers(&target).await?;
        if mode == 3 {
            assert!(listed.is_empty());
        } else {
            assert_eq!(listed, vec![target.clone()]);
        }
        assert_eq!(calls(&server, "GET", &referrers_path).await, 1);
        assert_eq!(calls(&server, "GET", &fallback_path).await, usize::from(mode > 0));
        if mode == 3 {
            continue;
        }
        Mock::given(method("GET"))
            .and(path("/v2/"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/manifests/{}", target.digest)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(raw))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(&fallback_path))
            .and(body_has("fixture/external-report"))
            .respond_with(ResponseTemplate::new(201).insert_header("location", "/fixture-written"))
            .mount(&server)
            .await;
        let external = Digest::new(format!("sha256:{}", "a".repeat(64)))?;
        client
            .attach_referrer_with_media_type(&target, &external, 12, "fixture/external-report")
            .await?;
        assert_eq!(calls(&server, "PUT", &fallback_path).await, 1);
        Mock::given(method("PUT"))
            .and(path(&fallback_path))
            .and(body_lacks("fixture/external-report"))
            .respond_with(ResponseTemplate::new(201).insert_header("location", "/fixture-written"))
            .mount(&server)
            .await;
        client.attach_referrer(&target, &target).await?;
        assert_eq!(calls(&server, "PUT", &fallback_path).await, 2);
        assert_eq!(client.head_manifest(&target).await?.digest, target.digest);
    }
    Ok(())
}

#[tokio::test]
async fn artifact_push_reserves_the_standard_referrers_tag_for_indexes() -> TestResult {
    let server = MockServer::start().await;
    let registry = server.address().to_string();
    let layers = vec![Layer {
        data: b"fixture".to_vec(),
        media_type: "application/octet-stream".into(),
        annotations: Default::default(),
    }];
    let target = build_push_target(
        &registry,
        "test",
        ArtifactType::FirecrackerSnapshotV1,
        &layers,
        &Default::default(),
    )?;
    Mock::given(method("GET"))
        .and(path("/v2/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/test/blobs/uploads/"))
        .respond_with(ResponseTemplate::new(202).insert_header("location", "/fixture-upload"))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/fixture-upload"))
        .respond_with(ResponseTemplate::new(202).insert_header("location", "/fixture-upload"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/fixture-upload"))
        .respond_with(ResponseTemplate::new(201).insert_header("location", "/fixture-blob"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("^/v2/test/manifests/"))
        .respond_with(ResponseTemplate::new(201).insert_header("location", "/fixture-manifest"))
        .mount(&server)
        .await;
    support::client_for(&registry)?
        .push_artifact(&target, layers, Default::default())
        .await?;
    let standard_tag = target.digest.as_str().replace(':', "-");
    assert_eq!(
        calls(&server, "PUT", &format!("/v2/test/manifests/{standard_tag}")).await,
        0
    );
    assert_eq!(
        calls(
            &server,
            "PUT",
            &format!("/v2/test/manifests/sandbox-artifact-{standard_tag}")
        )
        .await,
        1
    );
    assert_eq!(
        calls(&server, "PUT", &format!("/v2/test/manifests/{}", target.digest)).await,
        1
    );
    Ok(())
}

#[tokio::test]
async fn legacy_referrers_tag_accepts_only_the_exact_valid_source_artifact() -> TestResult {
    for mode in 0..10 {
        let server = MockServer::start().await;
        let registry = server.address().to_string();
        let (mut subject, mut raw) = artifact(&registry, b"fixture")?;
        if mode >= 2 {
            let mut manifest: Value = serde_json::from_slice(&raw)?;
            match mode {
                2 => manifest["artifactType"] = json!("fixture/unknown"),
                3 => manifest["layers"][0]["digest"] = json!("not-a-digest"),
                4 => manifest["schemaVersion"] = json!(1),
                5 => manifest["layers"][0]["size"] = json!(-1),
                6 => manifest["layers"][0]["urls"] = json!(["https://fixture.invalid/foreign"]),
                7 => {
                    subject.artifact_type = ArtifactType::ScanReportV1;
                    manifest["artifactType"] = json!(ArtifactType::ScanReportV1.media_type());
                }
                8 => manifest = json!({"not": "a manifest"}),
                _ => manifest["artifactType"] = json!(ArtifactType::SandboxRootfsV1.media_type()),
            }
            raw = serde_json::to_vec(&manifest)?;
            subject.digest = digest_of(&raw)?;
        } else if mode == 1 {
            subject.digest = Digest::new(format!("sha256:{}", "0".repeat(64)))?;
        }
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/referrers/{}", subject.digest)))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let fallback_path = format!("/v2/test/manifests/{}", subject.digest.as_str().replace(':', "-"));
        Mock::given(method("GET"))
            .and(path(&fallback_path))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(raw))
            .mount(&server)
            .await;
        let client = support::client_for(&registry)?;
        let listed = client.list_referrers(&subject).await;
        if mode == 0 {
            assert!(listed?.is_empty());
            Mock::given(method("GET"))
                .and(path("/v2/"))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
            let report = Digest::new(format!("sha256:{}", "f".repeat(64)))?;
            Mock::given(method("PUT"))
                .and(path(&fallback_path))
                .and(body_has("fixture/report"))
                .and(body_lacks("fixture-never"))
                .respond_with(ResponseTemplate::new(201).insert_header("location", "/fixture-index"))
                .mount(&server)
                .await;
            client
                .attach_referrer_with_media_type(&subject, &report, 12, "fixture/report")
                .await?;
            assert_eq!(calls(&server, "PUT", &fallback_path).await, 1);
        } else {
            assert!(listed.is_err(), "invalid legacy source mode {mode} must fail closed");
        }
    }
    Ok(())
}

#[tokio::test]
async fn raw_manifest_stages_in_the_original_repository_tag_namespace() -> TestResult {
    let server = MockServer::start().await;
    let registry = server.address().to_string();
    let (target, raw) = artifact(&registry, b"fixture")?;
    let manifest = serde_json::from_slice(&raw)?;
    let layers = vec![Layer {
        data: b"fixture".to_vec(),
        media_type: "application/octet-stream".into(),
        annotations: Default::default(),
    }];
    Mock::given(method("GET"))
        .and(path("/v2/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex("/blobs/uploads/"))
        .respond_with(ResponseTemplate::new(202).insert_header("location", "/fixture-upload"))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/fixture-upload"))
        .respond_with(ResponseTemplate::new(202).insert_header("location", "/fixture-upload"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/fixture-upload"))
        .respond_with(ResponseTemplate::new(201).insert_header("location", "/fixture-blob"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("^/v2/test/manifests/"))
        .respond_with(ResponseTemplate::new(201).insert_header("location", "/fixture-manifest"))
        .mount(&server)
        .await;
    support::client_for(&registry)?
        .push_raw_manifest(&target, &manifest, &layers)
        .await?;
    let requests = server.received_requests().await.ok_or("requests recorded")?;
    let manifest_puts: Vec<&str> = requests
        .iter()
        .filter(|request| request.method.as_str() == "PUT" && request.url.path().contains("/manifests/"))
        .map(|request| request.url.path())
        .collect();
    assert_eq!(manifest_puts.len(), 2);
    assert!(manifest_puts.iter().all(|path| !path.ends_with("/latest")));
    assert!(
        manifest_puts
            .iter()
            .any(|path| path.starts_with("/v2/test/manifests/sandbox-artifact-"))
    );
    assert!(
        manifest_puts
            .iter()
            .any(|path| path.starts_with("/v2/test/manifests/sha256:"))
    );
    Ok(())
}

#[test]
fn registry_credentials_never_print_their_password() {
    let credentials = RegistryCredentials::new("test", SecretString::from("secret"));
    assert!(!format!("{credentials:?}").contains("secret"));
    let _ = OciClientConfig::default();
}

use super::*;
use serde_json::{Value, json};
use std::time::Duration;

const SUPPORT: &str = "/sources/whatsapp/support/webhook";
const SALES: &str = "/sources/whatsapp/sales/webhook";
const CONFIG: &str = r#"
[sources.whatsapp.integrations.support]
stream_max_age_secs = 3600

[sources.whatsapp.integrations.support.webhook]
waba_id = "100"
phone_number_id = "101"
verify_token = "support-test-verification"
app_secret = "test-meta-app-signing"
max_body_bytes = 4096

[sources.whatsapp.integrations.sales.webhook]
waba_id = "200"
phone_number_id = "201"
verify_token = "sales-test-verification"
app_secret = "test-meta-app-signing"
"#;
const APP_SECRET: &str = "test-meta-app-signing";

fn config(toml: &str) -> ResolvedConfig {
    load(Some(write_toml(toml).path())).expect("WhatsApp source configuration must load")
}

fn app(publisher: &MockJetStreamPublisher) -> Router {
    mount_sources(
        config(CONFIG),
        wrap_publisher(publisher.clone()),
        &AdvancedMockNatsClient::new(),
    )
}

async fn send(app: &Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.unwrap()
}

fn get(uri: &str, query: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("{uri}?{query}"))
        .body(Body::empty())
        .unwrap()
}

fn post(uri: &str, body: Vec<u8>, signature: Option<&str>) -> Request<Body> {
    let mut request = Request::post(uri).header("content-type", "application/json");
    if let Some(signature) = signature {
        request = request.header("x-hub-signature-256", signature);
    }
    request.body(Body::from(body)).unwrap()
}

async fn deliver(app: &Router, uri: &str, envelope: Value) -> StatusCode {
    // Whitespace makes this proof depend on the original signed bytes.
    let mut body = serde_json::to_vec_pretty(&envelope).unwrap();
    body.push(b'\n');
    let signature = compute_github_sig(APP_SECRET, &body);
    send(app, post(uri, body, Some(&signature))).await.status()
}

fn envelope(entries: Vec<Value>) -> Value {
    json!({"object":"whatsapp_business_account","entry":entries})
}

fn entry(waba_id: &str, changes: Vec<Value>) -> Value {
    json!({"id":waba_id,"changes":changes})
}

fn change(phone_id: &str, messages_and_statuses: Value) -> Value {
    let mut value = json!({
        "messaging_product": "whatsapp",
        "metadata": {"phone_number_id": phone_id, "display_phone_number": "15550123"}
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(messages_and_statuses.as_object().unwrap().clone());
    json!({"field":"messages","value":value})
}

fn text(id: &str, body: &str) -> Value {
    json!({
        "id": id, "type":"text", "timestamp":"1749416383",
        "from_user_id":"US.13491208655302741918",
        "text":{"body":body}
    })
}

#[tokio::test]
async fn callback_verifies_the_named_integration_and_returns_a_plain_challenge() {
    let publisher = MockJetStreamPublisher::new();
    let app = app(&publisher);
    let query = "hub.mode=subscribe&hub.verify_token=support-test-verification&hub.challenge=001%2B9";
    let response = send(&app, get(SUPPORT, query)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(axum::body::to_bytes(response.into_body(), 32).await.unwrap(), "001+9");
    assert_eq!(send(&app, get(SALES, query)).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        send(
            &app,
            get(SUPPORT, &query.replace("support-test-verification", APP_SECRET))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for query in [
        query.replace("subscribe", "unsubscribe"),
        query.split("&hub.challenge").next().unwrap().to_owned(),
        format!("{query}&hub.verify_token=support-test-verification"),
    ] {
        assert_eq!(send(&app, get(SUPPORT, &query)).await.status(), StatusCode::BAD_REQUEST);
    }
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn rejects_bad_signatures_and_enforces_the_managed_body_limit() {
    let publisher = MockJetStreamPublisher::new();
    let app = app(&publisher);
    let event = envelope(vec![entry(
        "100",
        vec![change("101", json!({"messages":[text("wamid.example=", "hello")]}))],
    )]);
    let body = serde_json::to_vec(&event).unwrap();
    let using_verify_token = compute_github_sig("support-test-verification", &body);
    let original_signature = compute_github_sig(APP_SECRET, &body);
    let mut modified = body.clone();
    modified.push(b' ');
    for (body, signature) in [
        (body.clone(), None),
        (body.clone(), Some("sha256=abcd")),
        (body.clone(), Some(using_verify_token.as_str())),
        (modified, Some(original_signature.as_str())),
    ] {
        assert_eq!(
            send(&app, post(SUPPORT, body, signature)).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let mut duplicate = post(SUPPORT, body.clone(), Some(&original_signature));
    duplicate
        .headers_mut()
        .append("x-hub-signature-256", original_signature.parse().unwrap());
    assert_eq!(send(&app, duplicate).await.status(), StatusCode::UNAUTHORIZED);
    let mut oversized = body;
    oversized.resize(4097, b' ');
    let signature = compute_github_sig(APP_SECRET, &oversized);
    assert_eq!(
        send(&app, post(SUPPORT, oversized, Some(&signature))).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn mixed_batch_publishes_only_inbound_messages_from_the_bound_account_and_number() {
    let publisher = MockJetStreamPublisher::new();
    let app = app(&publisher);
    let expected = text("wamid.inbound=", "Keep this message");
    let media = json!({"id":"wamid.media=","type":"image","from":"15550001",
        "timestamp":"1749416383","image":{"id":"222","caption":"original caption"}});
    let status = json!({"id":"wamid.outbound=","status":"delivered"});
    let foreign_phone = change(
        "102",
        json!({"messages":[text("wamid.bad-number=", "for another number")]}),
    );
    let non_messages = json!({"field":"account_update","value":{"messages":[text("wamid.wrong-field=", "ignore")]}});
    let event = envelope(vec![
        entry(
            "200",
            vec![change(
                "101",
                json!({"messages":[text("wamid.bad-waba=", "for another WABA")]}),
            )],
        ),
        entry(
            "100",
            vec![
                foreign_phone.clone(),
                change(
                    "101",
                    json!({"messages":[expected.clone(), media.clone()],"statuses":[status]}),
                ),
                non_messages,
            ],
        ),
    ]);
    assert_eq!(deliver(&app, SUPPORT, event).await, StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 2);
    let payloads: Vec<Value> = messages
        .iter()
        .map(|message| {
            assert_eq!(message.subject, "whatsapp-support.message.received");
            assert!(message.headers.get("Nats-Msg-Id").is_some());
            serde_json::from_slice(&message.payload).unwrap()
        })
        .collect();
    for message in [expected, media] {
        assert!(payloads.contains(&json!({"waba_id":"100","phone_number_id":"101","message":message})));
    }

    let statuses = envelope(vec![entry(
        "100",
        vec![change(
            "101",
            json!({"statuses":[json!({"id":"wamid.outbound=","status":"delivered"})]}),
        )],
    )]);
    assert_eq!(deliver(&app, SUPPORT, statuses).await, StatusCode::OK);
    assert_eq!(
        deliver(&app, SUPPORT, envelope(vec![entry("100", vec![foreign_phone])])).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(publisher.published_messages().len(), 2);
}

#[tokio::test]
async fn invalid_or_over_count_batches_cannot_partially_publish() {
    let publisher = MockJetStreamPublisher::new();
    let app = app(&publisher);
    let valid = text("wamid.one=", "first");
    let no_id = json!({"type":"text","text":{"body":"missing id"}});
    assert_eq!(
        deliver(
            &app,
            SUPPORT,
            envelope(vec![entry(
                "100",
                vec![change("101", json!({"messages":[valid, no_id]}))]
            )])
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    let too_many = vec![json!({"id":"wamid.repeated=","type":"text"}); 1001];
    assert_eq!(
        deliver(
            &app,
            SALES,
            envelope(vec![entry("200", vec![change("201", json!({"messages":too_many}))])])
        )
        .await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn retryable_publication_failure_and_rebatching_keep_per_message_identity() {
    let publisher = MockJetStreamPublisher::new();
    let app = app(&publisher);
    let first = text("wamid.first=", "first");
    let second = text("wamid.second=", "second");
    let batch = envelope(vec![entry(
        "100",
        vec![change("101", json!({"messages":[first.clone(), second.clone()]}))],
    )]);
    assert_eq!(deliver(&app, SUPPORT, batch.clone()).await, StatusCode::OK);
    let id_for = |wamid: &str| {
        publisher
            .published_messages()
            .iter()
            .find(|message| serde_json::from_slice::<Value>(&message.payload).unwrap()["message"]["id"] == wamid)
            .unwrap()
            .headers
            .get("Nats-Msg-Id")
            .unwrap()
            .to_string()
    };
    let first_id = id_for("wamid.first=");
    let second_id = id_for("wamid.second=");
    assert_ne!(first_id, second_id);
    publisher.fail_next_js_publish();
    assert_eq!(deliver(&app, SUPPORT, batch).await, StatusCode::SERVICE_UNAVAILABLE);
    let single = envelope(vec![entry(
        "100",
        vec![change("101", json!({"messages":[first.clone()]}))],
    )]);
    assert_eq!(deliver(&app, SUPPORT, single).await, StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(
        messages.last().unwrap().headers.get("Nats-Msg-Id").unwrap().as_str(),
        first_id
    );
    let sale = envelope(vec![entry("200", vec![change("201", json!({"messages":[first]}))])]);
    assert_eq!(deliver(&app, SALES, sale).await, StatusCode::OK);
    let messages = publisher.published_messages();
    assert_ne!(
        messages.last().unwrap().headers.get("Nats-Msg-Id").unwrap().as_str(),
        first_id
    );
}

#[tokio::test]
async fn publishes_large_admitted_messages_through_claim_check_with_runtime_mounts() {
    let published = MockJetStreamPublisher::new();
    let store = MockObjectStore::new();
    let publisher = ClaimCheckPublisher::new(
        published.clone(),
        ClaimBucketBinding::for_test(store.clone(), ClaimBucket::default()),
        MaxPayload::from_server_limit(1024),
    );
    let app = mount_sources_with_runtime_credentials(
        config(CONFIG),
        publisher,
        &AdvancedMockNatsClient::new(),
        RuntimeCredentialMounts::<MockOpenBaoSecretStore>::default(),
    );
    let message = text("wamid.large=", &"x".repeat(2000));
    let event = envelope(vec![entry(
        "100",
        vec![change("101", json!({"messages":[message.clone()]}))],
    )]);
    assert_eq!(deliver(&app, SUPPORT, event).await, StatusCode::OK);
    let messages = published.published_messages();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].payload.is_empty());
    assert!(trogon_nats::jetstream::is_claim(&messages[0].headers));
    let payload = trogon_nats::jetstream::resolve_claim(&messages[0].headers, messages[0].payload.clone(), &store)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&payload).unwrap(),
        json!({"waba_id":"100","phone_number_id":"101","message":message})
    );
}

#[tokio::test]
async fn provisions_a_named_stream_with_dedup_bounded_by_retention() {
    let config = config(CONFIG);
    assert!(config.has_any_source());
    assert_eq!(
        Duration::from(config.max_stream_max_age().unwrap()),
        Duration::from_secs(604_800)
    );
    let context = trogon_nats::jetstream::MockJetStreamContext::new();
    crate::streams::provision(&context, &config).await.unwrap();
    let streams = context.created_streams();
    assert_eq!(streams.len(), 2);
    let support = streams.iter().find(|stream| stream.name == "WHATSAPP_SUPPORT").unwrap();
    assert_eq!(support.subjects, ["whatsapp-support.>"]);
    assert_eq!(support.max_age, Duration::from_secs(3600));
    assert_eq!(support.duplicate_window, support.max_age);
    let sales = streams.iter().find(|stream| stream.name == "WHATSAPP_SALES").unwrap();
    assert_eq!(sales.subjects, ["whatsapp-sales.>"]);
    assert_eq!(sales.duplicate_window, Duration::from_secs(604_800));
}

#[test]
fn refuses_missing_or_invalid_authority_and_credential_reuse() {
    for invalid in [
        CONFIG.replace("waba_id = \"100\"", "waba_id = \"\""),
        CONFIG.replace("phone_number_id = \"101\"", "phone_number_id = \"human-phone\""),
        CONFIG.replace("verify_token = \"support-test-verification\"", ""),
        CONFIG.replace("support-test-verification", APP_SECRET),
        CONFIG.replace("max_body_bytes = 4096", "max_body_bytes = 3000001"),
    ] {
        assert!(load(Some(write_toml(&invalid).path())).is_err());
    }
}

use super::*;

/// A native process reply: one data event, the end event and the trailer.
fn completed(stdout: &str) -> Vec<u8> {
    let stdout = base64::engine::general_purpose::STANDARD.encode(stdout);
    [
        (0, json!({"event":{"data":{"stdout":stdout}}})),
        (0, json!({"event":{"end":{"exited":true}}})),
        (2, json!({})),
    ]
    .into_iter()
    .flat_map(|(flag, event): (u8, Value)| {
        let body = event.to_string().into_bytes();
        [vec![flag], (body.len() as u32).to_be_bytes().to_vec(), body].concat()
    })
    .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_public_exec_release_serialization_is_recognized_by_the_hold_classifier() -> Result<(), Boxed> {
    let nats = Nats::start().await;
    let captured = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::clone(&captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = axum::Router::new().fallback(move |request: Request| {
        let captured = Arc::clone(&received);
        async move {
            (request.method() == Method::POST && request.uri().path() == "/process.Process/Start")
                .then_some(())
                .ok_or(StatusCode::NOT_FOUND)?;
            let body = to_bytes(request.into_body(), MAX_BODY)
                .await
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            let value: Value = serde_json::from_slice(body.get(5..).ok_or(StatusCode::BAD_REQUEST)?)
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            let script = value
                .pointer("/process/args/6")
                .and_then(Value::as_str)
                .ok_or(StatusCode::BAD_REQUEST)?;
            let release = script == RELEASE_SCRIPT;
            let stdout = (release || (script.contains("runner-ready") && script.contains("nohup setsid")))
                .then_some(if release { "" } else { "123 456\n" })
                .ok_or(StatusCode::NOT_FOUND)?;
            captured
                .lock()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .extend(release.then(|| body.to_vec()));
            Ok::<_, StatusCode>(Response::new(Body::from(completed(stdout))))
        }
    });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let exercised = async {
        let config = E2bConfig::new(endpoint.clone(), "e2b.test", "serialization-fixture")?.with_envd_url(endpoint)?;
        let sandbox = LeaseSandbox {
            sandbox_id: "serialization-owned-native".into(),
            target: EnvdTarget::for_sandbox(&config, "serialization-owned-native", None, None)?,
            traffic_token: None,
            domain: "e2b.test".into(),
            exposed_ports: vec![],
        };
        let exec = E2bExec::new(Envd::new(Duration::from_secs(2))?);
        exec.configure_command_launches(async_nats::jetstream::new(nats.client.clone()))?;
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":"serialization-lease","launch_key":"serialization-launch",
            "require_launch_cancellation":true,"command":["true"],"env":BTreeMap::<String,String>::new(),
            "detached":true,"deadline":"lease","run_as_user":"root",
        }))?;
        let signer = DispatchSigner::new("serialization-fixture", [17; 32]);
        let verifier = DispatchVerifier::new(EnforcementMode::Require)
            .with_epoch_mode(EpochMode::Require)
            .with_key(signer.key_id(), &signer.public_key_b64())?;
        let scope = VerbScope {
            tenant: crate::authority_support::lease_request()?.tenant,
            lease_id: request.lease_id.clone(),
            worker_id: Some(WorkerId::new("serialization-worker")?),
            attempt_id: Some(DispatchAttemptId::new("serialization-attempt")?),
            epoch: Some(LeaseEpoch::new(7)?),
        };
        let subject = subjects::provider_run_command(ProviderKind::E2b, &request.lease_id);
        let payload = serde_json::to_vec_pretty(&request)?;
        let signature = signer.sign_verb(&subject, &payload, &scope)?;
        let verified = verifier.verify_verb_scope(&subject, &signature, &payload, false, SystemTime::now())?;
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            exec.handle_run_command_with_scope(&sandbox, &request, &verified, &payload),
        )
        .await?;
        (matches!(response, RunCommandResponse::Started(_)))
            .then_some(())
            .context("the public exec serializer must complete its local captured launch")
    }
    .await;
    server.abort();
    let _ = server.await;
    exercised?;
    let captures = captured.lock().map_err(|_| "the capture record is poisoned")?;
    assert_eq!(
        captures.len(),
        1,
        "the real public exec must emit exactly one native Release request"
    );
    let capture = captures.first().context("no Release request was captured")?;
    let value: Value = serde_json::from_slice(capture.get(5..).context("the Release request has no frame")?)?;
    let args = value["process"]["args"].as_array().context("missing serialized argv")?;
    assert!(
        value["process"]["cmd"] == "/bin/bash"
            && args.len() == 11
            && args[6] == RELEASE_SCRIPT
            && args[9] == "123"
            && args[10] == "456",
        "the public exec must use the production login-shell serialization envelope"
    );
    assert!(
        release_binding(capture).is_some_and(|binding| binding.pid == 123 && binding.started == 456),
        "the release hold classifier missed the actual public serialization envelope"
    );
    Ok(())
}

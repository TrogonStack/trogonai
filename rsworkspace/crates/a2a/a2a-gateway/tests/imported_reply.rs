use std::time::Duration;

use a2a_gateway::Args;
use a2a_gateway::runtime::run_with_args;
use futures::StreamExt;
use serde_json::json;
use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::InMemoryEnv;

#[tokio::test]
async fn an_imported_request_receives_the_agent_reply_through_the_gateway() -> Result<(), Box<dyn std::error::Error>> {
    tokio::task::LocalSet::new()
        .run_until(imported_roundtrip(Scenario::Capacity))
        .await
}

#[derive(Clone, Copy)]
enum Scenario {
    Capacity,
    NoResponders,
    Deadline,
    Completion,
    Bootstrap,
}

#[tokio::test]
async fn imported_no_responders_replies_and_audits_failure() -> Result<(), Box<dyn std::error::Error>> {
    tokio::task::LocalSet::new()
        .run_until(imported_roundtrip(Scenario::NoResponders))
        .await
}

#[tokio::test]
async fn imported_deadline_replies_and_audits_failure() -> Result<(), Box<dyn std::error::Error>> {
    tokio::task::LocalSet::new()
        .run_until(imported_roundtrip(Scenario::Deadline))
        .await
}

#[tokio::test]
async fn imported_success_is_audited_only_after_delivery() -> Result<(), Box<dyn std::error::Error>> {
    tokio::task::LocalSet::new()
        .run_until(imported_roundtrip(Scenario::Completion))
        .await
}

#[tokio::test]
async fn imported_bootstrap_can_outlive_the_old_unary_cutoff() -> Result<(), Box<dyn std::error::Error>> {
    tokio::task::LocalSet::new()
        .run_until(imported_roundtrip(Scenario::Bootstrap))
        .await
}

async fn imported_roundtrip(scenario: Scenario) -> Result<(), Box<dyn std::error::Error>> {
    static RELAY_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _serial = RELAY_TEST.lock().await;
    let password = uuid::Uuid::new_v4().to_string();
    let config = format!(
        r#"
        jetstream {{}}
        accounts {{
            PLATFORM {{
                jetstream: enabled
                users: [{{user: platform, password: "{password}"}}]
                exports: [{{service: "a2a.v1.gateway.>"}}]
            }}
            TENANT {{
                users: [{{user: tenant, password: "{password}"}}]
                imports: [{{service: {{account: PLATFORM, subject: "a2a.v1.gateway.>"}}}}]
            }}
        }}
    "#
    );
    let server = CoreTestServer::start_with_config(&config).await;
    let platform = async_nats::ConnectOptions::new()
        .user_and_password("platform".into(), password.clone())
        .connect(server.address())
        .await?;
    let tenant = async_nats::ConnectOptions::new()
        .user_and_password("tenant".into(), password.clone())
        .connect(server.address())
        .await?;
    async_nats::jetstream::new(platform.clone())
        .create_stream(async_nats::jetstream::stream::Config {
            name: "AUDIT".into(),
            subjects: vec!["platform.v1.audit.>".into()],
            ..Default::default()
        })
        .await?;
    let method_dots = if matches!(scenario, Scenario::Bootstrap) {
        "message.stream"
    } else {
        "message.send"
    };
    let method = if matches!(scenario, Scenario::Bootstrap) {
        "message/stream"
    } else {
        "message/send"
    };
    let mut agent = platform
        .subscribe(format!("a2a.v1.agents.probe.person.alice.{method_dots}"))
        .await?;
    platform.flush().await?;
    let env = InMemoryEnv::new();
    env.set("NATS_TLS_DISABLED", "true");
    env.set("A2A_GATEWAY_NON_UNARY_RELAY_TIMEOUT_SECS", "120");
    if matches!(scenario, Scenario::Deadline) {
        env.set("A2A_GATEWAY_UNARY_DEADLINE_SECS", "1");
    }
    env.set("A2A_GATEWAY_AUDIT_PUBLISH", "true");
    env.set("A2A_GATEWAY_AUDIT_TENANT", "test");
    env.set("A2A_GATEWAY_AUDIT_PROJECT", "test");
    env.set("A2A_GATEWAY_AUDIT_INSTANCE", "relay");
    env.set("NATS_USER", "platform");
    env.set("NATS_PASSWORD", password);
    env.set(
        "A2A_GATEWAY_CALLER_JWT_ISSUER_KEYS",
        nkeys::KeyPair::new_account().public_key(),
    );
    let args = Args {
        nats_url: server.address().into(),
        prefix: "a2a".into(),
        queue_group: None,
    };
    let gateway = tokio::task::spawn_local(async move { run_with_args(args, &env).await });
    let ready = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let reply = platform
                .request(
                    "a2a.v1.gateway.probe.person.alice.unknown",
                    json!({"jsonrpc":"2.0","id":"ready","method":"unknown"})
                        .to_string()
                        .into(),
                )
                .await;
            match reply {
                Err(error) if error.kind() == async_nats::RequestErrorKind::NoResponders => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => return other,
            }
        }
    })
    .await;
    if gateway.is_finished() {
        gateway.await??;
        return Err("gateway stopped before readiness".into());
    }
    if ready.as_ref().is_err() || ready.as_ref().is_ok_and(|result| result.is_err()) {
        gateway.abort();
    }
    ready.map_err(|_| "gateway did not answer the readiness request")??;
    if matches!(scenario, Scenario::NoResponders | Scenario::Deadline) {
        let result = async {
            let mut errors = platform.subscribe("a2a.v1.audit.probe.err").await?;
            if matches!(scenario, Scenario::NoResponders) {
                agent.unsubscribe().await?;
            }
            platform.flush().await?;
            let response = tenant
                .send_request(
                    "a2a.v1.gateway.probe.person.alice.message.send",
                    async_nats::Request::new()
                        .timeout(Some(Duration::from_secs(5)))
                        .payload(
                            json!({"jsonrpc":"2.0","id":"failure","method":"message/send","params":{}})
                                .to_string()
                                .into(),
                        ),
                )
                .await?;
            let response: serde_json::Value = serde_json::from_slice(&response.payload)?;
            assert_eq!(response["id"], "failure");
            let expected = if matches!(scenario, Scenario::Deadline) {
                -32_800
            } else {
                -32_803
            };
            assert_eq!(response["error"]["code"], expected);
            let audit = tokio::time::timeout(Duration::from_secs(5), errors.next())
                .await?
                .ok_or("missing failure audit")?;
            let audit: serde_json::Value = serde_json::from_slice(&audit.payload)?;
            assert_eq!(audit["req_id"], "failure");
            assert_eq!(audit["code"], expected);
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        gateway.abort();
        let _ = gateway.await;
        return result;
    }
    let mut successes = platform.subscribe("a2a.v1.audit.probe.ok").await?;
    platform.flush().await?;
    let request = json!({"jsonrpc":"2.0","id":"imported","method":method,"params":{}}).to_string();
    let requester = tenant.clone();
    let mut caller = tokio::task::spawn_local(async move {
        requester
            .send_request(
                format!("a2a.v1.gateway.probe.person.alice.{method_dots}"),
                async_nats::Request::new().timeout(None).payload(request.into()),
            )
            .await
    });
    let forwarded = tokio::time::timeout(Duration::from_secs(5), agent.next()).await;
    let result = async {
        let forwarded = forwarded
            .map_err(|_| "agent did not receive the imported request")?
            .ok_or("agent subscription closed")?;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&forwarded.payload)?["id"],
            "imported"
        );
        let parallel = tokio::time::timeout(
            Duration::from_secs(5),
            tenant.request(
                "a2a.v1.gateway.probe.person.alice.unknown",
                json!({"jsonrpc":"2.0","id":"parallel","method":"unknown"})
                    .to_string()
                    .into(),
            ),
        )
        .await
        .map_err(|_| "an imported reply blocked another ingress request")??;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&parallel.payload)?["id"],
            "parallel"
        );
        if matches!(scenario, Scenario::Bootstrap) {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(31)).await;
            tokio::time::resume();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if matches!(scenario, Scenario::Completion) {
            assert!(
                tokio::time::timeout(Duration::from_millis(100), successes.next())
                    .await
                    .is_err(),
                "relay was audited successful before agent response"
            );
        }
        platform
            .publish(
                forwarded.reply.ok_or("agent reply missing")?,
                json!({"jsonrpc":"2.0","id":"imported","result":{"relayed":true}})
                    .to_string()
                    .into(),
            )
            .await?;
        let response = tokio::time::timeout(Duration::from_secs(5), &mut caller)
            .await
            .map_err(|_| "bootstrap relay lost its caller reply")???;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response.payload)?["result"]["relayed"],
            true
        );
        if matches!(scenario, Scenario::Completion | Scenario::Bootstrap) {
            let audit = tokio::time::timeout(Duration::from_secs(5), successes.next())
                .await?
                .ok_or("missing completion audit")?;
            let audit: serde_json::Value = serde_json::from_slice(&audit.payload)?;
            assert_eq!(audit["req_id"], "imported");
            assert_eq!(audit["outcome"], "ok");
            return Ok(());
        }
        let mut errors = platform.subscribe("a2a.v1.audit.probe.err").await?;
        platform.flush().await?;
        let inbox = tenant.new_inbox();
        let mut replies = tenant.subscribe(inbox.clone()).await?;
        tenant.flush().await?;
        let mut pending = Vec::new();
        for index in 0..64 {
            tenant
                .publish_with_reply(
                    "a2a.v1.gateway.probe.person.alice.message.send",
                    inbox.clone(),
                    json!({"jsonrpc":"2.0","id":index,"method":"message/send","params":{}})
                        .to_string()
                        .into(),
                )
                .await?;
            let request = tokio::time::timeout(Duration::from_secs(5), agent.next())
                .await?
                .ok_or("agent subscription closed while filling relay capacity")?;
            pending.push(request.reply.ok_or("missing pending reply")?);
        }
        tenant
            .publish_with_reply(
                "a2a.v1.gateway.probe.person.alice.message.send",
                inbox,
                json!({"jsonrpc":"2.0","id":"capacity","method":"message/send","params":{}})
                    .to_string()
                    .into(),
            )
            .await?;
        let rejected = tokio::time::timeout(Duration::from_secs(5), errors.next())
            .await?
            .ok_or("audit subscription closed")?;
        let audit: serde_json::Value = serde_json::from_slice(&rejected.payload)?;
        assert_eq!(audit["req_id"], "capacity");
        assert_eq!(audit["code"], -32_803);
        let response = tokio::time::timeout(Duration::from_secs(5), replies.next())
            .await
            .map_err(|_| "capacity refusal did not reply to the caller")?
            .ok_or("caller subscription closed")?;
        let response: serde_json::Value = serde_json::from_slice(&response.payload)?;
        assert_eq!(response["id"], "capacity");
        assert_eq!(response["error"]["code"], -32_803);
        assert_eq!(
            response["error"]["message"],
            "gateway imported reply capacity exhausted"
        );
        assert_eq!(audit["message"], "gateway imported reply capacity exhausted");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), agent.next())
                .await
                .is_err()
        );
        for reply in pending {
            platform
                .publish(reply, json!({"jsonrpc":"2.0","id":0,"result":{}}).to_string().into())
                .await?;
        }
        for _ in 0..64 {
            tokio::time::timeout(Duration::from_secs(5), replies.next())
                .await?
                .ok_or("pending reply lost")?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    caller.abort();
    gateway.abort();
    let _ = gateway.await;
    result
}

use std::time::Duration;

use a2a_gateway::Args;
use a2a_gateway::runtime::run_with_args;
use futures::StreamExt;
use serde_json::json;
use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::InMemoryEnv;

#[tokio::test]
async fn an_imported_request_receives_the_agent_reply_through_the_gateway() -> Result<(), Box<dyn std::error::Error>> {
    tokio::task::LocalSet::new().run_until(imported_roundtrip()).await
}

async fn imported_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
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
    let mut agent = platform
        .subscribe("a2a.v1.agents.probe.person.alice.message.send")
        .await?;
    platform.flush().await?;
    let env = InMemoryEnv::new();
    env.set("NATS_TLS_DISABLED", "true");
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
    let request = json!({"jsonrpc":"2.0","id":"imported","method":"message/send","params":{}}).to_string();
    let requester = tenant.clone();
    let mut caller = tokio::task::spawn_local(async move {
        requester
            .request("a2a.v1.gateway.probe.person.alice.message.send", request.into())
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
        platform
            .publish(
                forwarded.reply.ok_or("agent reply missing")?,
                json!({"jsonrpc":"2.0","id":"imported","result":{"relayed":true}})
                    .to_string()
                    .into(),
            )
            .await?;
        let response = (&mut caller).await??;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response.payload)?["result"]["relayed"],
            true
        );
        let mut errors = platform.subscribe("a2a.v1.audit.probe.err").await?;
        platform.flush().await?;
        let inbox = tenant.new_inbox();
        let _replies = tenant.subscribe(inbox.clone()).await?;
        tenant.flush().await?;
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
            tokio::time::timeout(Duration::from_secs(5), agent.next())
                .await?
                .ok_or("agent subscription closed while filling relay capacity")?;
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
        assert!(
            tokio::time::timeout(Duration::from_millis(100), agent.next())
                .await
                .is_err()
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    caller.abort();
    gateway.abort();
    let _ = gateway.await;
    result
}

use bytes::Bytes;
        use trogon_nats::AdvancedMockNatsClient;

        use super::*;

        fn agent_card_payload(name: &str) -> Bytes {
            let card = a2a::agent_card::AgentCard {
                name: name.to_string(),
                description: String::new(),
                version: String::new(),
                supported_interfaces: vec![a2a::agent_card::AgentInterface {
                    url: "https://example.com/a2a".to_string(),
                    protocol_binding: "JSONRPC".to_string(),
                    protocol_version: "0.2.0".to_string(),
                    tenant: None,
                }],
                capabilities: a2a::agent_card::AgentCapabilities::default(),
                default_input_modes: vec![],
                default_output_modes: vec![],
                skills: vec![],
                provider: None,
                documentation_url: None,
                icon_url: None,
                security_schemes: None,
                security_requirements: None,
                signatures: None,
            };
            let json = serde_json::json!({"jsonrpc":"2.0","id":"any","result":card});
            serde_json::to_vec(&json).unwrap().into()
        }

        #[tokio::test]
        async fn agent_card_targets_agent_subject_by_default() {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.agents.test-agent.card", agent_card_payload("bot"));
            let client = A2aClient::new(prefix(), agent_id(), nats, ());
            let card = client.agent_card().await.unwrap();
            assert_eq!(card.name, "bot");
        }

        #[tokio::test]
        async fn agent_card_targets_gateway_subject_under_gateway_routing() {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.gateway.test-agent.card", agent_card_payload("via-gw"));
            let jwt =
                MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
            let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
            let card = client.agent_card().await.unwrap();
            assert_eq!(card.name, "via-gw");
        }

        #[tokio::test]
        async fn agent_card_propagates_transport_errors() {
            let nats = AdvancedMockNatsClient::new();
            nats.fail_next_request();
            let client = A2aClient::new(prefix(), agent_id(), nats, ());
            assert!(matches!(client.agent_card().await, Err(ClientError::Transport(_))));
        }

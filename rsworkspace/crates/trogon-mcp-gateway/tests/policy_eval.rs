use cel_interpreter::{Program, Value};
use trogon_mcp_gateway::act_chain::ActChainEntry;
use trogon_mcp_gateway::authz::{GatewayIdentity, IdentitySource};
use trogon_mcp_gateway::jwt::VerifiedJwtClaims;
use trogon_mcp_gateway::policy::new_policy_cel_context;

fn sample_identity() -> GatewayIdentity {
    GatewayIdentity {
        tenant: None,
        caller_sub: Some("alice".into()),
        issuer: None,
        jti: None,
        source: IdentitySource::Jwt,
    }
}

fn synthetic_chain() -> Vec<ActChainEntry> {
    vec![
        ActChainEntry {
            sub: "user:alice".into(),
            agent_id: None,
            wkl: None,
            iat: 1_700_000_000,
        },
        ActChainEntry {
            sub: "agent:acme/oncall".into(),
            agent_id: Some("agent:acme/oncall".into()),
            wkl: Some("spiffe://acme.local/ns/prod/sa/oncall".into()),
            iat: 1_700_000_100,
        },
    ]
}

#[test]
fn act_chain_size_rule_fires() {
    let program = Program::compile("jwt.act_chain.size() >= 2").unwrap();
    let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &synthetic_chain()).unwrap();
    assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
}

#[test]
fn chain_contains_agent_id() {
    let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &synthetic_chain()).unwrap();

    let present = Program::compile(r#"chain.contains("agent:acme/oncall")"#).unwrap();
    assert!(matches!(present.execute(&ctx), Ok(Value::Bool(true))));

    let absent = Program::compile(r#"chain.contains("agent:other")"#).unwrap();
    assert!(matches!(absent.execute(&ctx), Ok(Value::Bool(false))));
}

#[test]
fn chain_originator_sub_matches_root_hop() {
    let program = Program::compile(r#"chain.originator().sub == "user:alice""#).unwrap();
    let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &synthetic_chain()).unwrap();
    assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
}

#[test]
fn cel_without_act_chain_references_still_compiles_and_runs() {
    let program = Program::compile(r#"jwt.sub == "alice" && jwt.tenant == null"#).unwrap();
    let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &[]).unwrap();
    assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
}

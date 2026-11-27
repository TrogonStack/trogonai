#![cfg(any(target_os = "linux", target_os = "macos"))]

use serde_json::json;
use trogon_sandbox_contract::DomainPattern;
use trogon_sandbox_policy_exec::{
    CommandId, CommandNetworkLists, NetworkPolicy, PolicyExec, PolicyExecConfig, RuntimeError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn runtime() -> Result<(tempfile::TempDir, PolicyExec), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config = PolicyExecConfig::new(dir.path()).network(NetworkPolicy::DenyAll);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    Ok((dir, PolicyExec::new(config)?))
}

fn lists(domain: &str) -> Result<CommandNetworkLists, Box<dyn std::error::Error>> {
    Ok(CommandNetworkLists::new(vec![DomainPattern::new(domain)?]))
}

const ID: &str = "0123456789abcdefghijkl";

#[tokio::test]
async fn a_short_command_id_is_refused_without_echoing_it() -> TestResult {
    let (_dir, runtime) = runtime()?;
    let id = CommandId::new("short-secret-id");
    let result = runtime.register_command_network_lists(&id, lists("example.com")?).await;
    let Err(err) = result else {
        return Err("a short command id was accepted".into());
    };
    assert!(matches!(err, RuntimeError::Invalid(_)));
    assert!(!format!("{err}").contains("short-secret-id"));
    Ok(())
}

#[tokio::test]
async fn replacement_characters_do_not_count_toward_the_minimum_length() -> TestResult {
    let (_dir, runtime) = runtime()?;
    let id = CommandId::new(format!("{}{}", "a".repeat(21), "\u{FFFD}".repeat(5)));
    assert!(
        runtime
            .register_command_network_lists(&id, lists("example.com")?)
            .await
            .is_err()
    );
    let id = CommandId::new(format!("{}{}", "a".repeat(22), "\u{FFFD}"));
    assert!(
        runtime
            .register_command_network_lists(&id, lists("example.com")?)
            .await
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn registering_replaces_and_unregistering_an_unknown_id_is_a_no_op() -> TestResult {
    let (_dir, runtime) = runtime()?;
    let id = CommandId::new(ID);
    runtime
        .register_command_network_lists(&id, lists("a.example.com")?)
        .await?;
    runtime
        .register_command_network_lists(&id, lists("b.example.com")?)
        .await?;
    runtime.unregister_command_network_lists(&id).await?;
    runtime.unregister_command_network_lists(&id).await?;
    runtime
        .unregister_command_network_lists(&CommandId::new("never-registered-0000000"))
        .await?;
    Ok(())
}

#[test]
fn the_allowed_domains_grammar_accepts_and_refuses_the_expected_spellings() {
    for valid in [
        "example.com",
        "*.example.com",
        "api.example.com:443",
        "localhost",
        "127.0.0.1:3000",
        "[::1]",
        "[2001:db8::1]:443",
    ] {
        assert!(DomainPattern::new(valid).is_ok(), "{valid}");
    }
    for invalid in [
        "*",
        "*:443",
        "*.com",
        "https://example.com",
        "example.com/path",
        "example.com:0",
        "example.com:443.evil.test",
        "2001:db8::1",
        "nodots",
        "",
    ] {
        assert!(DomainPattern::new(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn json_lists_are_read_as_they_are_registered() -> TestResult {
    let lists = CommandNetworkLists::from_json(&json!({"allowedDomains": ["a.com", "b.com:443"]}))?;
    assert_eq!(lists.allowed_domains.len(), 2);
    assert!(CommandNetworkLists::from_json(&json!({}))?.allowed_domains.is_empty());
    assert!(
        CommandNetworkLists::from_json(&json!({"allowedDomains": null}))?
            .allowed_domains
            .is_empty()
    );

    for entry in [json!(null), json!(443), json!({"host": "example.com"}), json!("*.com")] {
        let result = CommandNetworkLists::from_json(&json!({"allowedDomains": ["a.com", entry]}));
        assert!(matches!(result, Err(RuntimeError::Invalid(_))));
    }

    let not_an_array = CommandNetworkLists::from_json(&json!({"allowedDomains": "example.com"}));
    assert!(matches!(not_an_array, Err(RuntimeError::Invalid(_))));
    Ok(())
}

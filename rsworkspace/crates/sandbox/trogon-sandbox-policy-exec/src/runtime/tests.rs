use super::*;
use trogon_sandbox_contract::{DeniedDomain, ExternalProxy, PlaintextHeaderSet, Port, ReasonedRefusals, TunnelCap};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn rule(domain: &str) -> Result<DomainRule, Box<dyn std::error::Error>> {
    Ok(DomainRule {
        domain: DomainPattern::new(domain)?,
        action: DomainRuleAction::PassThrough,
    })
}

fn lists(domain: &str) -> Result<CommandNetworkLists, Box<dyn std::error::Error>> {
    Ok(CommandNetworkLists::new(vec![DomainPattern::new(domain)?]))
}

fn runtime_in(dir: &tempfile::TempDir, options: NetworkOptions) -> Result<PolicyExec, Box<dyn std::error::Error>> {
    Ok(PolicyExec::new(
        PolicyExecConfig::new(dir.path()).network_options(options),
    )?)
}

#[tokio::test]
async fn the_configured_plaintext_header_set_reaches_the_proxy_callbacks() -> TestResult {
    let dir = tempfile::tempdir()?;
    let callbacks_for = |options: NetworkOptions| -> Result<_, Box<dyn std::error::Error>> {
        let runtime = runtime_in(&dir, options)?;
        Ok(runtime.callbacks(&runtime.live().config))
    };
    assert!(!callbacks_for(NetworkOptions::default())?.plaintext_header_set);
    let enabled = NetworkOptions {
        plaintext_header_set: PlaintextHeaderSet::Enabled,
        ..NetworkOptions::default()
    };
    assert!(callbacks_for(enabled)?.plaintext_header_set);
    Ok(())
}

#[tokio::test]
async fn an_updated_config_changes_the_tunnel_limits_every_command_shares() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runtime = runtime_in(&dir, NetworkOptions::default())?;
    let gate = runtime
        .callbacks(&runtime.live().config)
        .tunnel_gate
        .ok_or("no tunnel gate")?;
    assert_eq!(gate.cap(), None);
    assert_eq!(gate.limit(), 256);
    assert_eq!(gate.handshake(), None);

    let options = NetworkOptions {
        tunnel_cap: Some(TunnelCap::new(3)?),
        handshake_timeout: Some(Duration::from_secs(2)),
        ..NetworkOptions::default()
    };
    runtime
        .update_config(PolicyExecConfig::new(dir.path()).network_options(options))
        .await?;
    assert_eq!(gate.cap().map(|cap| cap.get()), Some(3));
    assert_eq!(gate.handshake(), Some(Duration::from_secs(2)));
    let later = runtime
        .callbacks(&runtime.live().config)
        .tunnel_gate
        .ok_or("no tunnel gate")?;
    assert!(Arc::ptr_eq(&gate, &later));
    Ok(())
}

#[tokio::test]
async fn registered_lists_join_the_spec_allow_and_a_strict_allowlist_ignores_them() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runtime = runtime_in(&dir, NetworkOptions::default())?;
    let id = CommandId::new("0123456789abcdefghijkl");
    let spec = vec![DomainPattern::new("a.com")?];
    let callbacks = runtime.callbacks(&runtime.live().config);
    runtime.register_command_network_lists(&id, lists("b.com")?).await?;
    let rules = runtime.command_rules(&callbacks, &runtime.allowed_for(&callbacks, &id, &spec));
    assert_eq!(rules, vec![rule("a.com")?, rule("b.com")?]);
    runtime.unregister_command_network_lists(&id).await?;
    let rules = runtime.command_rules(&callbacks, &runtime.allowed_for(&callbacks, &id, &spec));
    assert_eq!(rules, vec![rule("a.com")?]);
    runtime.register_command_network_lists(&id, lists("b.com")?).await?;
    runtime.reset().await;
    let rules = runtime.command_rules(&callbacks, &runtime.allowed_for(&callbacks, &id, &spec));
    assert_eq!(rules, vec![rule("a.com")?]);
    Ok(())
}

#[tokio::test]
async fn ids_sharing_their_first_100_units_share_a_list_and_one_too_long_for_the_user_name_gets_none() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runtime = runtime_in(&dir, NetworkOptions::default())?;
    let callbacks = runtime.callbacks(&runtime.live().config);
    let spec = vec![DomainPattern::new("a.com")?];
    let prefix = "p".repeat(100);
    let registered = CommandId::new(format!("{prefix}one"));
    let sibling = CommandId::new(format!("{prefix}two"));
    runtime
        .register_command_network_lists(&registered, lists("b.com")?)
        .await?;
    let rules = runtime.command_rules(&callbacks, &runtime.allowed_for(&callbacks, &sibling, &spec));
    assert_eq!(rules, vec![rule("a.com")?, rule("b.com")?]);
    runtime.unregister_command_network_lists(&sibling).await?;
    let rules = runtime.command_rules(&callbacks, &runtime.allowed_for(&callbacks, &registered, &spec));
    assert_eq!(rules, vec![rule("a.com")?]);

    let wide = CommandId::new("\u{20ac}".repeat(70));
    runtime.register_command_network_lists(&wide, lists("b.com")?).await?;
    let rules = runtime.command_rules(&callbacks, &runtime.allowed_for(&callbacks, &wide, &spec));
    assert_eq!(rules, vec![rule("a.com")?]);
    Ok(())
}

#[tokio::test]
async fn reasoned_refusals_with_an_external_http_proxy_never_apply_registered_lists() -> TestResult {
    let dir = tempfile::tempdir()?;
    let port = Port::new(8080)?;
    let id = CommandId::new("0123456789abcdefghijkl");
    let spec = vec![DomainPattern::new("a.com")?];
    let external = Some(ExternalProxy::http_only(port));
    let cases = [
        (
            NetworkOptions {
                external_proxy: external,
                reasoned_refusals: ReasonedRefusals::Enabled,
                ..NetworkOptions::default()
            },
            vec![rule("a.com")?],
        ),
        (
            NetworkOptions {
                external_proxy: external,
                ..NetworkOptions::default()
            },
            vec![rule("a.com")?, rule("b.com")?],
        ),
        (
            NetworkOptions {
                reasoned_refusals: ReasonedRefusals::Enabled,
                ..NetworkOptions::default()
            },
            vec![rule("a.com")?, rule("b.com")?],
        ),
    ];
    for (options, expected) in cases {
        let runtime = runtime_in(&dir, options)?;
        let callbacks = runtime.callbacks(&runtime.live().config);
        runtime.register_command_network_lists(&id, lists("b.com")?).await?;
        let rules = runtime.command_rules(&callbacks, &runtime.allowed_for(&callbacks, &id, &spec));
        assert_eq!(rules, expected);
    }
    Ok(())
}

#[derive(Default)]
struct DebugEvents(std::sync::atomic::AtomicUsize);

struct Counting(Arc<DebugEvents>);

impl tracing::Subscriber for Counting {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::DEBUG {
            self.0.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn registering_under_an_external_http_proxy_succeeds_and_says_so_in_the_debug_log() -> TestResult {
    let dir = tempfile::tempdir()?;
    let options = NetworkOptions {
        external_proxy: Some(ExternalProxy::http_only(Port::new(8080)?)),
        ..NetworkOptions::default()
    };
    let runtime = runtime_in(&dir, options)?;
    let events = Arc::new(DebugEvents::default());
    let _guard = tracing::subscriber::set_default(Counting(Arc::clone(&events)));
    let id = CommandId::new("0123456789abcdefghijkl");
    let mut said = 0;
    for _ in 0..50 {
        tracing::callsite::rebuild_interest_cache();
        events.0.store(0, std::sync::atomic::Ordering::SeqCst);
        runtime.register_command_network_lists(&id, lists("b.com")?).await?;
        said = events.0.load(std::sync::atomic::Ordering::SeqCst);
        if said > 0 {
            break;
        }
    }
    assert_eq!(said, 1);

    events.0.store(0, std::sync::atomic::Ordering::SeqCst);
    let plain = runtime_in(&dir, NetworkOptions::default())?;
    plain.register_command_network_lists(&id, lists("b.com")?).await?;
    assert_eq!(events.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn extra_rules_turn_deny_all_into_a_policy_that_admits_only_them() -> TestResult {
    let merged = merge_command_rules(&NetworkPolicy::DenyAll, &[rule("example.com")?]);
    let NetworkPolicy::UserDefined {
        rules, denied_domains, ..
    } = merged
    else {
        return Err("expected a user defined policy".into());
    };
    assert_eq!(rules.len(), 1);
    assert!(denied_domains.is_empty());
    Ok(())
}

#[test]
fn extra_rules_are_added_after_the_configured_ones_and_denied_domains_stay() -> TestResult {
    let base = NetworkPolicy::UserDefined {
        rules: vec![rule("a.com")?],
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: vec![DeniedDomain::new("b.com")?],
        opaque_tunnels: OpaqueTunnelPolicy::Allow,
        request_rules: Vec::new(),
    };
    let NetworkPolicy::UserDefined {
        rules, denied_domains, ..
    } = merge_command_rules(&base, &[rule("b.com")?])
    else {
        return Err("expected a user defined policy".into());
    };
    assert_eq!(rules.len(), 2);
    assert_eq!(denied_domains.len(), 1);
    Ok(())
}

#[test]
fn allow_all_and_no_extra_rules_change_nothing() -> TestResult {
    assert_eq!(
        merge_command_rules(&NetworkPolicy::AllowAll, &[rule("a.com")?]),
        NetworkPolicy::AllowAll
    );
    assert_eq!(
        merge_command_rules(&NetworkPolicy::DenyAll, &[]),
        NetworkPolicy::DenyAll
    );
    Ok(())
}

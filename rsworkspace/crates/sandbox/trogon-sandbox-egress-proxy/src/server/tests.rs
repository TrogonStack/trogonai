use super::*;

#[tokio::test]
async fn recover_original_destination_never_panics_on_a_plain_socket() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let accept_task = tokio::spawn(async move { listener.accept().await.map(|(stream, _)| stream) });
    let _client = TcpStream::connect(addr).await?;
    let server_stream = accept_task.await??;

    match recover_original_destination(&server_stream) {
        Err(_) => {}
        Ok(recovered) => assert_eq!(recovered, addr),
    }
    Ok(())
}

mod support;

use support::{
    AllowEverything, Captured, CollectingSink, IgnoringAmender, LeaveHeaders, STYLES, SilentTransport, TestResult,
    builder, ca, limits, loopback, plain_tcp, token,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::{EgressViolationReason, LeaseId, LiveFlowResweep, LoopbackNames, PrivateRangesByName};

use crate::decider::DeciderLimits;
use crate::denial::Denial;
#[cfg(unix)]
use crate::ingress::ListenAddress;
use crate::live_flows::FlowFacts;
use crate::tls_intercept_error::TlsInterceptError;
use crate::violation::ViolationRateLimit;

fn user_defined() -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

#[test]
fn every_policy_shape_has_permitted_domains() -> TestResult {
    permitted_domains_for(&user_defined())?;
    permitted_domains_for(&NetworkPolicy::AllowAll)?;
    permitted_domains_for(&NetworkPolicy::DenyAll)?;
    Ok(())
}

#[test]
fn upstream_bundles_that_do_not_parse_or_are_not_certificates_are_refused() -> TestResult {
    let unparsable = "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
    let unusable = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
    for bundle in [unparsable, unusable, "no certificate here"] {
        let result = upstream_tls_config_trusting(&UpstreamCaCertificates::new([bundle.to_string()]));
        assert!(matches!(result, Err(err) if err.kind() == io::ErrorKind::InvalidInput));
    }
    Ok(())
}

#[test]
fn a_real_ca_bundle_extends_the_upstream_trust() -> TestResult {
    let pem = ca()?.root_cert_pem();
    upstream_tls_config_trusting(&UpstreamCaCertificates::new([pem]))?;
    Ok(())
}

#[tokio::test]
async fn a_server_without_listeners_is_refused() -> TestResult {
    assert!(builder(NetworkPolicy::DenyAll)?.start().is_err());
    Ok(())
}

#[tokio::test]
async fn extra_upstream_cas_do_not_combine_with_a_custom_upstream_tls_config() -> TestResult {
    let options = NetworkOptions {
        upstream_ca: UpstreamCaCertificates::new([ca()?.root_cert_pem()]),
        ..NetworkOptions::default()
    };
    let result = builder(NetworkPolicy::DenyAll)?
        .explicit(loopback().await?, token()?)
        .upstream_tls_config(system_upstream_tls_config())
        .network_options(options)
        .start();
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn extra_upstream_cas_extend_the_trust_of_a_started_server() -> TestResult {
    let options = NetworkOptions {
        upstream_ca: UpstreamCaCertificates::new([ca()?.root_cert_pem()]),
        ..NetworkOptions::default()
    };
    let server = builder(NetworkPolicy::DenyAll)?
        .explicit(loopback().await?, token()?)
        .network_options(options)
        .start()?;
    server.shutdown(Duration::ZERO).await?;
    Ok(())
}

#[tokio::test]
async fn name_options_do_not_combine_with_a_custom_resolver() -> TestResult {
    let options = NetworkOptions {
        loopback_names: LoopbackNames::Enabled,
        ..NetworkOptions::default()
    };
    let result = builder(NetworkPolicy::DenyAll)?
        .explicit(loopback().await?, token()?)
        .resolver(Arc::new(VettingResolver::system()))
        .network_options(options)
        .start();
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn a_custom_resolver_serves_when_no_name_option_is_set() -> TestResult {
    let server = builder(NetworkPolicy::DenyAll)?
        .explicit(loopback().await?, token()?)
        .resolver(Arc::new(VettingResolver::system()))
        .start()?;
    server.shutdown(Duration::ZERO).await?;
    Ok(())
}

#[tokio::test]
async fn marking_that_cannot_be_applied_fails_the_start() -> TestResult {
    let result = builder(NetworkPolicy::DenyAll)?
        .explicit(loopback().await?, token()?)
        .marking(UpstreamMarking::Required)
        .start();
    assert_eq!(result.is_err(), UpstreamMarking::Required.verify().is_err());
    Ok(())
}

#[tokio::test]
async fn the_unauthenticated_and_open_explicit_listeners_report_their_address() -> TestResult {
    let unauthenticated = builder(NetworkPolicy::DenyAll)?
        .explicit_unauthenticated(loopback().await?)
        .start()?;
    assert!(matches!(unauthenticated.explicit_addr(), Some(BoundAddress::Tcp(_))));
    let open = builder(NetworkPolicy::DenyAll)?
        .explicit_open(loopback().await?)
        .start()?;
    assert!(matches!(open.explicit_addr(), Some(BoundAddress::Tcp(_))));
    assert!(open.transparent_addr().is_none());
    unauthenticated.shutdown(Duration::ZERO).await?;
    open.shutdown(Duration::ZERO).await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_unix_explicit_listener_reports_its_path_and_advertises_no_crl_address() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("proxy.sock");
    let listener = Listener::bind(&ListenAddress::parse(&format!("unix:{}", path.display()))).await?;
    let server = builder(NetworkPolicy::DenyAll)?
        .explicit(listener, token()?)
        .advertise_crl()
        .start()?;
    assert!(matches!(server.explicit_addr(), Some(BoundAddress::Unix(_))));
    server.shutdown(Duration::ZERO).await?;
    Ok(())
}

#[tokio::test]
async fn a_server_wired_with_every_option_serves_and_shuts_down() -> TestResult {
    let sink = Arc::new(CollectingSink::default());
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("lease-one"),
        sink,
        ViolationRateLimit::default(),
    ));
    let decider = Arc::new(Decider::new(
        LeaseId::new("lease-one"),
        Arc::new(SilentTransport),
        Arc::new(IgnoringAmender),
        DeciderLimits::default(),
    ));
    let options = NetworkOptions {
        resweep_live_flows: LiveFlowResweep::Enabled,
        loopback_names: LoopbackNames::Enabled,
        private_ranges_by_name: PrivateRangesByName::Enabled,
        binding_ttl: Some(Duration::from_secs(30)),
        revalidate_interval: Some(Duration::from_secs(60)),
        denied_resolved_addresses: vec![trogon_sandbox_contract::IpRange::new("10.0.0.0/8")?],
        ..NetworkOptions::default()
    };
    let identity = OidcIdentityContext::unconfigured(OidcSigner::generate()?, OIDC_ISSUER)?;
    let transparent = plain_tcp().await?;
    let transparent_addr = transparent.local_addr()?;
    let server_builder = builder(user_defined())?
        .transparent(transparent)
        .explicit(loopback().await?, token()?)
        .ingress_limits(limits())
        .serve_crl()
        .advertise_crl()
        .upstream_tls_config(system_upstream_tls_config())
        .credential_source(Arc::new(UnconfiguredCredentialSource))
        .oidc_identity(identity)
        .marking(UpstreamMarking::NotNeeded)
        .secrets(SecretSubstitution::new(Arc::new(
            trogon_sandbox_protocol::SentinelRegistry::new(),
        )))
        .decider(decider)
        .violations(reporter)
        .request_filter(Arc::new(AllowEverything))
        .header_mutator(Arc::new(LeaveHeaders))
        .plaintext_header_mutator(Arc::new(LeaveHeaders))
        .decision_window_capture(true)
        .plaintext_header_set(true)
        .plaintext_inject(true)
        .tunnel_gate(Arc::new(TunnelGate::new(None, None)))
        .denial_reasons(DenialReasons::new())
        .deny_header_style(STYLES[1]);
    let handle = server_builder.policy_handle();
    let result = server_builder.network_options(options).start();
    assert_eq!(handle.current().await, user_defined());
    let server = result?;
    assert_eq!(server.transparent_addr(), Some(transparent_addr));
    assert!(server.ca_certificate_pem().contains("BEGIN CERTIFICATE"));
    assert!(matches!(server.explicit_addr(), Some(BoundAddress::Tcp(_))));
    server.replace_policy(NetworkPolicy::DenyAll).await?;
    assert_eq!(server.policy_handle().current().await, NetworkPolicy::DenyAll);
    server.shutdown(Duration::from_millis(50)).await?;
    Ok(())
}

#[tokio::test]
async fn a_replacement_outside_the_ca_constraints_is_rejected_and_keeps_the_old_policy() -> TestResult {
    let handle = PolicyHandle::new(NetworkPolicy::DenyAll, PermittedDomains::from_rules(&[])?);
    let outside = NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "api.example.com".parse()?,
            action: DomainRuleAction::Forward(trogon_sandbox_contract::RequestForward {
                forward_url: trogon_sandbox_contract::ForwardUrl::new("https://forward.example.com/token")?,
                match_rules: vec![],
                ttl_seconds: 60,
            }),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let result = handle.replace(outside).await;
    assert!(matches!(result, Err(PolicyRejectedError::OutsideCaConstraints { .. })));
    assert_eq!(handle.current().await, NetworkPolicy::DenyAll);
    Ok(())
}

#[tokio::test]
async fn replacing_the_policy_ends_the_live_flows_it_no_longer_admits() -> TestResult {
    let captured = Captured::default();
    let _guard = tracing::subscriber::set_default(captured.subscriber());
    let options = NetworkOptions {
        resweep_live_flows: LiveFlowResweep::Enabled,
        ..NetworkOptions::default()
    };
    let server = builder(NetworkPolicy::AllowAll)?
        .explicit(loopback().await?, token()?)
        .network_options(options)
        .start()?;
    let mut flow = server.policy_handle().live_flows().register(FlowFacts {
        sni: None,
        dest_ip: "203.0.113.7".parse()?,
        dest_port: 443,
        decider_allowed: false,
    });

    server.replace_policy(NetworkPolicy::DenyAll).await?;

    let reason = tokio::time::timeout(Duration::from_secs(10), flow.cancelled()).await?;
    assert_eq!(reason, EgressViolationReason::PolicyDenied);
    assert!(captured.text().contains("ended live flows"));
    server.shutdown(Duration::ZERO).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn periodic_revalidation_ends_flows_the_policy_no_longer_admits_and_stops_on_shutdown() -> TestResult {
    let captured = Captured::default();
    let _guard = tracing::subscriber::set_default(captured.subscriber());
    let handle = PolicyHandle::new(NetworkPolicy::DenyAll, PermittedDomains::from_rules(&[])?);
    let mut flow = handle.live_flows().register(FlowFacts {
        sni: None,
        dest_ip: "203.0.113.7".parse()?,
        dest_port: 443,
        decider_allowed: false,
    });
    let (shutdown, signal) = watch::channel(None);
    let task = tokio::spawn(revalidate_periodically(
        handle,
        Arc::new(VettingResolver::system()),
        Duration::from_secs(1),
        signal,
    ));

    let reason = flow.cancelled().await;

    assert_eq!(reason, EgressViolationReason::PolicyDenied);
    assert!(captured.text().contains("periodic revalidation"));
    shutdown.send(Some(Duration::ZERO))?;
    task.await??;
    Ok(())
}

#[tokio::test]
async fn a_connection_that_was_not_redirected_is_closed_when_its_destination_cannot_be_recovered() -> TestResult {
    let captured = Captured::default();
    let _guard = tracing::subscriber::set_default(captured.subscriber());
    let transparent = plain_tcp().await?;
    let addr = transparent.local_addr()?;
    let server = builder(NetworkPolicy::DenyAll)?.transparent(transparent).start()?;

    let mut client = TcpStream::connect(addr).await?;
    let mut buf = [0u8; 8];
    let read = tokio::time::timeout(Duration::from_secs(10), client.read(&mut buf)).await?;

    assert!(matches!(read, Ok(0) | Err(_)));
    server.shutdown(Duration::from_millis(50)).await?;
    Ok(())
}

#[tokio::test]
async fn waiting_returns_once_the_primary_listener_stops() -> TestResult {
    let transparent = plain_tcp().await?;
    let server = builder(NetworkPolicy::DenyAll)?.transparent(transparent).start()?;
    server.shutdown.send(Some(Duration::ZERO))?;

    server.wait().await?;
    Ok(())
}

#[tokio::test]
async fn a_listener_task_that_failed_is_reported_by_shutdown() -> TestResult {
    let server = builder(NetworkPolicy::DenyAll)?
        .explicit(loopback().await?, token()?)
        .start()?;
    for task in &server.tasks {
        task.abort();
    }

    let result = server.shutdown(Duration::ZERO).await;

    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn a_denied_redirected_connection_is_reported_and_logged() -> TestResult {
    let captured = Captured::default();
    let _guard = tracing::subscriber::set_default(captured.subscriber());
    let sink = Arc::new(CollectingSink::default());
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("lease-one"),
        sink.clone(),
        ViolationRateLimit::default(),
    ));
    let listener = plain_tcp().await?;
    let addr = listener.local_addr()?;
    let mut client = TcpStream::connect(addr).await?;
    let (stream, peer_addr) = listener.accept().await?;
    client.write_all(b"hello").await?;
    client.shutdown().await?;
    let tls = TlsInterceptContext::new(
        ca()?,
        system_upstream_tls_config(),
        Arc::new(UnconfiguredCredentialSource),
        OidcIdentityContext::unconfigured(OidcSigner::generate()?, OIDC_ISSUER)?,
    );

    handle_redirected(
        stream,
        peer_addr,
        "203.0.113.7:443".parse()?,
        Arc::new(RwLock::new(NetworkPolicy::DenyAll)),
        Arc::new(VettingResolver::system()),
        Arc::new(tls),
        Some(reporter),
    )
    .await;

    assert!(captured.text().contains("connection denied by policy"));
    assert_eq!(sink.0.lock().map_err(|_| "poisoned")?.len(), 1);
    Ok(())
}

#[test]
fn every_outcome_is_logged_with_its_own_message() -> TestResult {
    let captured = Captured::default();
    let peer: SocketAddr = "127.0.0.1:1000".parse()?;
    let dest: SocketAddr = "203.0.113.7:443".parse()?;
    let outcomes = [
        ProxyOutcome::Completed {
            bytes_client_to_upstream: 1,
            bytes_upstream_to_client: 2,
        },
        ProxyOutcome::Denied(Denial {
            host: None,
            reason: EgressViolationReason::PolicyDenied,
            request: None,
        }),
        ProxyOutcome::UpstreamConnectFailed(io::Error::other("a")),
        ProxyOutcome::SniffFailed(io::Error::other("b")),
        ProxyOutcome::PostgresUpstreamConnectFailed(io::Error::other("c")),
        ProxyOutcome::PostgresNegotiationFailed(io::Error::other("d")),
        ProxyOutcome::TlsInterceptRequiresSni,
        ProxyOutcome::TlsInterceptFailed(TlsInterceptError::ClientHandshake(io::Error::other("e"))),
    ];

    tracing::subscriber::with_default(captured.subscriber(), || {
        for outcome in &outcomes {
            log_outcome(peer, dest, outcome);
        }
    });

    let text = captured.text();
    for message in [
        "connection completed",
        "connection denied by policy",
        "upstream connect failed",
        "sniff failed",
        "postgres upstream connect failed",
        "postgres negotiation failed",
        "no SNI available",
        "tls intercept failed",
    ] {
        assert!(text.contains(message), "{message}");
    }
    Ok(())
}

#[tokio::test]
async fn the_revocation_list_is_served_and_logged() -> TestResult {
    let captured = Captured::default();
    let _guard = tracing::subscriber::set_default(captured.subscriber());
    let server = builder(NetworkPolicy::DenyAll)?
        .explicit(loopback().await?, token()?)
        .serve_crl()
        .start()?;
    let Some(BoundAddress::Tcp(addr)) = server.explicit_addr().cloned() else {
        return Err("no tcp address".into());
    };

    let mut client = TcpStream::connect(addr).await?;
    client
        .write_all(b"GET /ca.crl HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await?;
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut reply)).await??;

    assert!(reply.starts_with(b"HTTP/1.1 200"));
    for _ in 0..200 {
        if captured.text().contains("served the revocation list") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(captured.text().contains("served the revocation list"));
    server.shutdown(Duration::from_millis(50)).await?;
    Ok(())
}

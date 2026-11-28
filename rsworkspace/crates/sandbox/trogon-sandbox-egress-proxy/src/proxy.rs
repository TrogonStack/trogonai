//! Async proxy runtime: accept a client TCP connection, sniff SNI (best
//! effort, bounded), decide allow/deny, and on allow either splice the client
//! connection bidirectionally with a fresh connection to the real
//! destination (`PassThrough`), or terminate the client's TLS and re-encrypt
//! to the real destination (`Transform`/`Forward`, see [`TlsInterceptContext`]
//! and [`crate::tls_intercept`]).

use crate::host_kind::{HostClassification, HostKind};
use std::io;
use std::net::IpAddr;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;
use trogon_sandbox_contract::{
    AddressDenialClass, CredentialTransform, DomainRuleAction, EgressViolationReason, LeaseId, NetworkPolicy,
    OpaqueTunnelPolicy, RequestForward, SandboxId, TenantId,
};
use trogon_sandbox_contract::{DenyReason, NetworkOptions};
use trogon_sandbox_contract::{Host, Port};

use crate::credential_source::CredentialSource;
use crate::decider::Decider;
use crate::deny_explanation::{deny_explanation, opaque_tunnels, standard_reason};
use crate::dial::{self, LegAddresses, UpstreamMarking};
use crate::header_mutator::HeaderMutator;
use crate::live_flows::{FlowFacts, LiveFlows};
use crate::oidc_signer::OidcSigner;
use crate::opaque_tunnel::{self, FirstBytes};
use crate::parent_pool::ParentPool;
use crate::policy::{Decision, Ruling, address_allowed_exactly, allow_rests_on_sni, decide, rule};
use crate::postgres::{self, PgHandshakeOutcome, PgProbe};
use crate::request_filter::{DeniedRequestObserver, RequestChecks, RequestFilter};
use crate::resolve::{AllowRanges, VettedAddr, VettedEndpoint, VettingResolver};
use crate::secret_substitution::{SecretSubstitution, SubstitutionScope};
use crate::sni::{SniExtraction, extract_sni};
use crate::tls_ca::SandboxCa;
use crate::tls_intercept::{CredentialInjectionContext, ForwardContext, PrefixedStream, terminate_and_forward_checked};
use crate::tls_target::TlsTarget;
use crate::tunnel_deadlines::{SharedQueueBudget, TunnelDeadlines};
use crate::violation_store::{DenialReason, DenialReasons};
use trogon_sandbox_protocol::{
    StandardDenyReason, domain_only_allows_requires_termination, response_strip, tls_exclusions_exclude,
    tls_exemptions_name_host, unlisted_destinations_refuse,
};

pub use crate::constants::{MAX_SNIFF_BYTES, SNIFF_TIMEOUT};
pub use crate::denial::Denial;
pub use crate::proxy_outcome::ProxyOutcome;

/// This running proxy process's own identity, used to mint the `iss`/`sub`/
/// `tenant_id`/`lease_id` claims of every [`trogon_sandbox_contract::OidcToken`]
/// minted for a `Forward` domain rule's matched requests. Deliberately
/// separate from [`RequestForward`] (see that type's doc comment): a
/// `NetworkPolicy` is author-declared and can be reused across many leases,
/// while these fields are this specific running process's runtime context.
pub struct OidcIdentityContext {
    pub signer: OidcSigner,
    pub issuer: String,
    pub subject: SandboxId,
    pub tenant_id: TenantId,
    pub lease_id: LeaseId,
}

impl OidcIdentityContext {
    /// An identity for a process that was given none, minting under placeholder
    /// claims. The caller supplies the signer.
    pub fn unconfigured(signer: OidcSigner, issuer: &str) -> io::Result<Self> {
        const PLACEHOLDER: &str = "unconfigured";
        Ok(Self {
            signer,
            issuer: issuer.to_string(),
            subject: SandboxId::new(PLACEHOLDER).map_err(io::Error::other)?,
            tenant_id: TenantId::new(PLACEHOLDER).map_err(io::Error::other)?,
            lease_id: LeaseId::new(PLACEHOLDER),
        })
    }
}

/// Bundles the TLS-intercept dependencies [`handle_connection`] needs on the
/// `Transform`/`Forward` domain-rule path, so callers building these once per
/// process (one [`SandboxCa`], one upstream [`rustls::ClientConfig`], one
/// [`CredentialSource`], and one [`OidcIdentityContext`], all expensive or
/// stateful to construct per-connection) can pass a single reference instead
/// of several separate parameters.
pub struct TlsInterceptContext {
    pub ca: SandboxCa,
    pub upstream_tls_config: Arc<rustls::ClientConfig>,
    pub credential_source: Arc<dyn CredentialSource>,
    pub oidc_identity: OidcIdentityContext,
    pub marking: UpstreamMarking,
    pub secrets: Option<SecretSubstitution>,
    pub decider: Option<Arc<Decider>>,
    pub deadlines: TunnelDeadlines,
    pub shared_queue: SharedQueueBudget,
    pub request_filter: Option<Arc<dyn RequestFilter>>,
    pub request_denials: Option<Arc<dyn DeniedRequestObserver>>,
    pub network_options: Arc<NetworkOptions>,
    pub denial_reasons: Option<DenialReasons>,
    pub plaintext_header_set: bool,
    pub decision_window_capture: bool,
    pub header_mutator: Option<Arc<dyn HeaderMutator>>,
    pub plaintext_header_mutator: Option<Arc<dyn HeaderMutator>>,
    pub plaintext_inject: bool,
    pub parent_pool: ParentPool,
    pub live_flows: Option<Arc<LiveFlows>>,
}

impl TlsInterceptContext {
    /// The request checks a connection under `policy` runs, or `None` when
    /// neither the policy nor this context supplies any.
    pub fn request_checks(&self, policy: &NetworkPolicy) -> Option<RequestChecks> {
        RequestChecks::assembled(
            policy,
            self.request_filter.as_ref(),
            self.request_denials.as_ref(),
            self.plaintext_header_set,
            self.header_mutator.as_ref(),
            self.plaintext_header_mutator.as_ref(),
        )
    }

    pub fn new(
        ca: SandboxCa,
        upstream_tls_config: Arc<rustls::ClientConfig>,
        credential_source: Arc<dyn CredentialSource>,
        oidc_identity: OidcIdentityContext,
    ) -> Self {
        Self {
            ca,
            upstream_tls_config,
            credential_source,
            oidc_identity,
            marking: UpstreamMarking::Required,
            secrets: None,
            decider: None,
            deadlines: TunnelDeadlines::default(),
            shared_queue: SharedQueueBudget::default(),
            request_filter: None,
            request_denials: None,
            network_options: Arc::default(),
            denial_reasons: None,
            plaintext_header_set: false,
            decision_window_capture: false,
            header_mutator: None,
            plaintext_header_mutator: None,
            plaintext_inject: false,
            parent_pool: ParentPool::default(),
            live_flows: None,
        }
    }

    /// Registers each admitted transparent flow so a policy replacement can
    /// revalidate it; see [`LiveFlows::sweep`].
    pub fn with_live_flows(mut self, flows: Arc<LiveFlows>) -> Self {
        self.live_flows = Some(flows);
        self
    }

    /// Substitutes sentinels in plain-HTTP requests too; the real value then travels in the clear.
    pub fn with_plaintext_inject(mut self, enabled: bool) -> Self {
        self.plaintext_inject = enabled;
        self
    }

    /// Reads what a CONNECT client sends while its destination is decided, and drops a client that
    /// closes or sends more than 64 KiB before the verdict.
    pub fn with_decision_window_capture(mut self, enabled: bool) -> Self {
        self.decision_window_capture = enabled;
        self
    }

    /// Lets a request filter's allow set headers on requests that arrive in
    /// cleartext.
    pub fn with_plaintext_header_set(mut self, enabled: bool) -> Self {
        self.plaintext_header_set = enabled;
        self
    }

    /// Edits the headers of each terminated request just before it goes
    /// upstream.
    pub fn with_header_mutator(mut self, mutator: Arc<dyn HeaderMutator>) -> Self {
        self.header_mutator = Some(mutator);
        self
    }

    /// The same for plain-HTTP requests; credentials set this way travel in the clear.
    pub fn with_plaintext_header_mutator(mut self, mutator: Arc<dyn HeaderMutator>) -> Self {
        self.plaintext_header_mutator = Some(mutator);
        self
    }

    pub fn with_marking(mut self, marking: UpstreamMarking) -> Self {
        self.marking = marking;
        self
    }

    pub fn with_secrets(mut self, secrets: SecretSubstitution) -> Self {
        self.secrets = Some(secrets);
        self
    }

    pub fn with_decider(mut self, decider: Arc<Decider>) -> Self {
        self.decider = Some(decider);
        self
    }

    pub fn with_shared_queue(mut self, shared_queue: SharedQueueBudget) -> Self {
        self.shared_queue = shared_queue;
        self
    }

    pub fn with_deadlines(mut self, deadlines: TunnelDeadlines) -> Self {
        self.deadlines = deadlines;
        self
    }

    /// The deadlines a tunnel runs under: the configured handshake timeout,
    /// when there is one, replaces the default.
    pub(crate) fn tunnel_deadlines(&self) -> TunnelDeadlines {
        match self.network_options.handshake_timeout {
            Some(handshake) => TunnelDeadlines {
                handshake,
                ..self.deadlines
            },
            None => self.deadlines,
        }
    }

    /// A filter consulted for each HTTP request in addition to the policy's
    /// own request rules.
    pub fn with_request_filter(mut self, filter: Arc<dyn RequestFilter>) -> Self {
        self.request_filter = Some(filter);
        self
    }

    /// Where requests denied on a terminated connection are reported.
    pub fn with_request_denials(mut self, observer: Arc<dyn DeniedRequestObserver>) -> Self {
        self.request_denials = Some(observer);
        self
    }

    pub fn with_network_options(mut self, options: Arc<NetworkOptions>) -> Self {
        self.network_options = options;
        self
    }

    /// Where the words behind a refusal are kept for the violation line.
    pub fn with_denial_reasons(mut self, reasons: DenialReasons) -> Self {
        self.denial_reasons = Some(reasons);
        self
    }

    /// The secret bindings that terminate TLS for `host:port`, none when the
    /// host is one the options exclude from termination.
    pub(crate) fn secrets_for(&self, host: Option<&str>, port: u16) -> Option<&SecretSubstitution> {
        let excluded = !self.network_options.tls_exclusions.is_empty()
            && host
                .and_then(|name| Host::new(name).ok())
                .is_some_and(|host| tls_exclusions_exclude(&self.network_options.tls_exclusions, &host, port));
        if excluded {
            return None;
        }
        self.secrets.as_ref()
    }

    pub(crate) fn terminates_every_allowed_host(&self) -> bool {
        self.network_options.terminate_allowed_tls.is_enabled()
    }

    /// Whether the terminate-all switch applies to `host:port`: it is on and
    /// the canonical form of the host is not one the options exclude.
    pub(crate) fn terminates_host(&self, host: Option<&str>, port: u16) -> bool {
        self.network_options.terminate_allowed_tls.is_enabled()
            && !host
                .and_then(|name| Host::new(name).ok())
                .is_some_and(|host| tls_exclusions_exclude(&self.network_options.tls_exclusions, &host, port))
    }

    /// Like `terminates_host`, for a target that may be an IP literal, which
    /// an exclusion names too.
    pub(crate) fn terminates_target(&self, host: &Host, port: u16) -> bool {
        self.network_options.terminate_allowed_tls.is_enabled()
            && !tls_exclusions_exclude(&self.network_options.tls_exclusions, host, port)
    }

    /// The substitution that applies to a plain-HTTP request for `host:port`,
    /// only when plaintext injection was asked for.
    pub(crate) fn plaintext_scope(&self, host: &str, port: u16) -> Option<SubstitutionScope> {
        if !self.plaintext_inject {
            return None;
        }
        self.secrets_for(Some(host), port)?
            .scope_for(&TlsTarget::new(host, port))
    }

    pub(crate) fn explain_denial(&self, host: impl std::fmt::Display, port: u16, explanation: Option<&DenyReason>) {
        let (Some(reasons), Some(explanation)) = (&self.denial_reasons, explanation) else {
            return;
        };
        reasons.remember(&host.to_string(), port, DenialReason::new(explanation.as_str()));
    }

    /// [`Self::dial_routed_any`] for a plain-HTTP request.
    pub(crate) async fn dial_plain_any(
        &self,
        target: Option<&str>,
        endpoints: &[VettedEndpoint],
    ) -> io::Result<dial::PlainUpstream> {
        self.routes().connect_plain_any(target, endpoints).await
    }

    /// Opens a connection to a destination the policy already admitted,
    /// through the MITM socket or parent proxy the options configure, else
    /// over the vetted endpoints as a dual-stack connect takes them
    /// (`address_turns`), returning the last failure.
    pub(crate) async fn dial_routed_any(
        &self,
        target: Option<&str>,
        tls: bool,
        endpoints: &[VettedEndpoint],
    ) -> io::Result<dial::UpstreamStream> {
        self.routes().connect_any(target, tls, endpoints).await
    }

    fn routes(&self) -> dial::UpstreamRoutes<'_> {
        dial::UpstreamRoutes {
            marking: self.marking,
            options: &self.network_options,
            tls_config: &self.upstream_tls_config,
            parent_pool: &self.parent_pool,
        }
    }
}

/// Whether a destination's connection is relayed as opaque bytes: no domain
/// action terminates it, and no secret binding covers it.
pub(crate) fn splices_opaquely(
    sni: Option<&str>,
    action: Option<&DomainRuleAction>,
    port: u16,
    secrets: Option<&SecretSubstitution>,
) -> bool {
    matches!(
        dispatch_with_secrets(dispatch_for_action(sni, action), sni, port, secrets),
        ConnectionDispatch::RawSplice
    )
}

/// Sniffs as much of a TLS ClientHello as is available (bounded by
/// [`MAX_SNIFF_BYTES`] and [`SNIFF_TIMEOUT`]), returning the sniffed SNI host
/// name (if any) plus the raw bytes read so far. The caller must forward
/// these bytes to the upstream itself; they are not consumed by the peer.
pub async fn sniff_client_hello(stream: &mut TcpStream) -> io::Result<(Option<String>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(4096);
    let deadline = timeout(SNIFF_TIMEOUT, async {
        loop {
            if buf.len() >= MAX_SNIFF_BYTES {
                return Ok::<_, io::Error>(SniExtraction::NotPresent);
            }

            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                // Peer closed before sending a full ClientHello.
                return Ok(extract_sni(&buf));
            }
            buf.extend_from_slice(&chunk[..n]);

            match extract_sni(&buf) {
                SniExtraction::Incomplete => continue,
                other => return Ok(other),
            }
        }
    })
    .await;

    let extraction = match deadline {
        Ok(result) => result?,
        // Timed out waiting for more bytes: proceed with whatever we have,
        // treated as "no SNI determinable" rather than failing the connection.
        Err(_) => SniExtraction::Incomplete,
    };

    let sni = match extraction {
        SniExtraction::Found(host) => Some(host),
        SniExtraction::NotPresent | SniExtraction::Unreadable | SniExtraction::NotTls | SniExtraction::Incomplete => {
            None
        }
    };

    Ok((sni, buf))
}

/// Which domain-rule action matched a TLS-intercepted connection, carried
/// alongside the connection's `sni_host` in [`ConnectionDispatch::TlsIntercept`]
/// so [`handle_connection`] has the full matched rule (not just a
/// `Transform`-shaped subset of it) to pass down to
/// [`crate::tls_intercept::terminate_and_forward`]. Modeled as an enum rather
/// than a second `Option<&RequestForward>` field alongside
/// `credential_transform` so "matched both a `Transform` and a `Forward`
/// rule" is unrepresentable at the type level, matching this codebase's
/// existing enum-over-multiple-`Option`s convention (see
/// [`trogon_sandbox_contract::DomainRuleAction`], [`trogon_sandbox_contract::CredentialInjection`]).
#[derive(Debug, Eq, PartialEq)]
enum DomainAction<'a> {
    Transform(&'a CredentialTransform),
    Forward(&'a RequestForward),
}

/// Which path [`handle_connection`] should take once a connection has been
/// sniffed and decided, kept as a small pure value so the branch it encodes
/// is unit-testable without any actual sockets or TLS.
#[derive(Debug, Eq, PartialEq)]
enum ConnectionDispatch<'a> {
    RawSplice,
    TlsIntercept {
        sni_host: &'a str,
        domain_action: Option<DomainAction<'a>>,
    },
    TlsInterceptRequiresSni,
}

/// Decides which of [`ConnectionDispatch`]'s paths applies for a given sniffed
/// `sni` and matched `action`. A domain rule can currently only match (and
/// thus only return `Some(action)`) via SNI, so `TlsInterceptRequiresSni`
/// should be unreachable through the public policy surface today; it exists
/// so a future policy shape that matches `Transform`/`Forward` some other way
/// cannot silently fall back to an unencrypted splice.
fn dispatch_for_action<'a>(sni: Option<&'a str>, action: Option<&'a DomainRuleAction>) -> ConnectionDispatch<'a> {
    let domain_action = match action {
        Some(DomainRuleAction::Transform(transform)) => Some(DomainAction::Transform(transform)),
        Some(DomainRuleAction::Forward(forward)) => Some(DomainAction::Forward(forward)),
        None | Some(DomainRuleAction::PassThrough) => None,
    };
    match action {
        None | Some(DomainRuleAction::PassThrough) => ConnectionDispatch::RawSplice,
        Some(DomainRuleAction::Transform(_)) | Some(DomainRuleAction::Forward(_)) => match sni {
            Some(sni_host) => ConnectionDispatch::TlsIntercept {
                sni_host,
                domain_action,
            },
            None => ConnectionDispatch::TlsInterceptRequiresSni,
        },
    }
}

/// Hosts a secret binding injects at are always TLS-terminated, whatever
/// their domain rule says. A `PassThrough` rule would otherwise splice the
/// encrypted bytes through and the sentinel would reach the upstream as the
/// useless fake. This is the one place that guarantees termination.
fn dispatch_with_secrets<'a>(
    dispatch: ConnectionDispatch<'a>,
    sni: Option<&'a str>,
    port: u16,
    secrets: Option<&SecretSubstitution>,
) -> ConnectionDispatch<'a> {
    match (dispatch, sni, secrets) {
        (ConnectionDispatch::RawSplice, Some(sni_host), Some(secrets)) if secrets.covers(sni_host, port) => {
            ConnectionDispatch::TlsIntercept {
                sni_host,
                domain_action: None,
            }
        }
        (dispatch, _, _) => dispatch,
    }
}

/// Handles one accepted client connection end to end: sniff, decide, and
/// either splice to `upstream` or close.
///
/// `dest_ip`/`dest_port` are the real destination the caller has already
/// resolved (e.g. from the original destination address captured before
/// transparent redirection); this crate does not perform DNS resolution or
/// destination-address recovery itself. The destination must also pass the
/// SSRF floor, with only the policy's `allow_ranges` able to carve out an
/// exception, before anything is dialed.
///
/// When the allow rests on the sniffed name (a domain rule matched it), that
/// name must resolve, through `resolver`, to `dest_ip`; see
/// `confirm_sni_binding`. Connections without a usable SNI, and allows that
/// rest on the destination address or `AllowAll`, are not subject to it.
pub async fn handle_connection(
    mut client: TcpStream,
    policy: &NetworkPolicy,
    dest_ip: IpAddr,
    dest_port: u16,
    resolver: &VettingResolver,
    tls_intercept: &TlsInterceptContext,
) -> ProxyOutcome {
    let vetted = resolver
        .floor()
        .vet(dest_ip, &AllowRanges::from_policy(policy).for_port(dest_port));
    let (prefix, probe) = match postgres::peek_pg_probe(&mut client).await {
        Ok(result) => result,
        Err(err) => return ProxyOutcome::SniffFailed(err),
    };

    match probe {
        PgProbe::SslRequest if opaque_tunnels(policy).refuses() => {
            return ProxyOutcome::Denied(opaque_tunnel::refusal(None));
        }
        PgProbe::SslRequest => {
            let dest = match vetted {
                Ok(dest) => dest,
                Err(class) => return address_denied(None, class),
            };
            return handle_postgres_connection(
                client,
                policy,
                dest.with_port(dest_port),
                prefix,
                resolver,
                tls_intercept,
            )
            .await;
        }
        PgProbe::GssEncRequest => {
            // sslmode=require+ parity: GSSAPI-encrypted Postgres connections
            // have no SNI-equivalent this proxy can inspect, so deny before
            // ever contacting upstream rather than falling back to IP ranges.
            return ProxyOutcome::Denied(Denial::new(None, EgressViolationReason::GssEncryptionUnsupported));
        }
        PgProbe::NotPostgres => {}
    }

    // Not Postgres: replay the 8 bytes already read into the generic
    // sniff-off-client-only path, unchanged, rather than losing them.
    let (sni, prefix) = match sniff_client_hello_with_seed(&mut client, prefix).await {
        Ok(result) => result,
        Err(err) => return ProxyOutcome::SniffFailed(err),
    };

    let mut decider_allowed_name = false;
    let action = match rule(policy, sni.as_deref(), dest_ip, dest_port) {
        Ruling::Allow(action) => action,
        Ruling::Deny => {
            let explanation = [
                sni.as_deref().and_then(|name| Host::new(name).ok()),
                Some(Host::from(dest_ip)),
            ]
            .into_iter()
            .flatten()
            .find_map(|host| deny_explanation(&tls_intercept.network_options, policy, &host, dest_port));
            if let Some(host) = &sni {
                tls_intercept.explain_denial(host, dest_port, explanation.as_ref());
            }
            return ProxyOutcome::Denied(Denial::new(sni, EgressViolationReason::PolicyDenied));
        }
        Ruling::Unlisted if unlisted_destinations_refuse(tls_intercept.network_options.unlisted) => {
            if let Some(host) = &sni {
                tls_intercept.explain_denial(
                    host,
                    dest_port,
                    standard_reason(StandardDenyReason::NotOnAllowList).as_ref(),
                );
            }
            return ProxyOutcome::Denied(Denial::new(sni, EgressViolationReason::PolicyDenied));
        }
        Ruling::Unlisted => {
            let settled = Decider::decide_unlisted(
                tls_intercept.decider.as_deref(),
                &vetted,
                sni.as_deref(),
                dest_ip,
                dest_port,
            )
            .await;
            if let Err(reason) = settled {
                return ProxyOutcome::Denied(Denial::new(sni, reason));
            }
            decider_allowed_name = sni.is_some();
            None
        }
    };
    let dest: VettedAddr = match vetted {
        Ok(dest) => dest,
        Err(class) => return address_denied(sni, class),
    };
    let rests_on_sni = decider_allowed_name || allow_rests_on_sni(policy, sni.as_deref(), dest_ip, dest_port);
    if let Some(denial) = confirm_sni_binding(resolver, rests_on_sni, sni.as_deref(), dest_ip, dest_port).await {
        return ProxyOutcome::Denied(denial);
    }

    let interception = interception_for(
        policy,
        tls_intercept,
        resolver,
        sni.as_deref(),
        dest_ip,
        dest_port,
        rests_on_sni,
    )
    .await;

    let forwarded = forward_decided(
        client,
        prefix,
        sni.as_deref(),
        action,
        dest.with_port(dest_port).into(),
        tls_intercept,
        opaque_tunnels(policy),
        tls_intercept.request_checks(policy),
        tls_intercept.tunnel_deadlines(),
        interception,
    );
    let Some(flows) = tls_intercept.live_flows.as_ref() else {
        return forwarded.await;
    };
    let mut guard = flows.register(FlowFacts {
        sni: sni.clone(),
        dest_ip,
        dest_port,
        decider_allowed: decider_allowed_name,
    });
    tokio::select! {
        outcome = forwarded => outcome,
        reason = guard.cancelled() => ProxyOutcome::Denied(Denial::new(sni, reason)),
    }
}

/// How an admitted flow is carried given the operator's interception
/// settings. Runs after normal destination admission and the name binding
/// check, so nothing here can admit a flow, only change how it is carried.
pub(crate) async fn interception_for(
    policy: &NetworkPolicy,
    tls_intercept: &TlsInterceptContext,
    resolver: &VettingResolver,
    sni: Option<&str>,
    dest_ip: IpAddr,
    dest_port: u16,
    rests_on_sni: bool,
) -> Interception {
    if exempt_from_interception(tls_intercept, resolver, sni, dest_ip, dest_port).await {
        return Interception::Exempt;
    }
    if names_alone_carry_the_allow(tls_intercept, policy, rests_on_sni, dest_ip, dest_port) {
        return Interception::Required;
    }
    Interception::Default
}

/// Whether the allow rests on the presented name alone, no address rule backs
/// it, and the operator requires such flows to be terminated.
fn names_alone_carry_the_allow(
    tls_intercept: &TlsInterceptContext,
    policy: &NetworkPolicy,
    rests_on_sni: bool,
    dest_ip: IpAddr,
    dest_port: u16,
) -> bool {
    domain_only_allows_requires_termination(tls_intercept.network_options.domain_only_allows)
        && rests_on_sni
        && !address_allowed_exactly(policy, dest_ip, dest_port)
}

/// Whether the operator's exemption list relays this flow untouched. An
/// address exemption covers the destination whether or not the client sent a
/// name. A hostname exemption applies only when the proxy's own lookup of the
/// presented name, made now, binds it to the destination; otherwise the flow
/// is carried as if there were no exemption, so an unbound name cannot buy an
/// uninspected tunnel.
async fn exempt_from_interception(
    tls_intercept: &TlsInterceptContext,
    resolver: &VettingResolver,
    sni: Option<&str>,
    dest_ip: IpAddr,
    dest_port: u16,
) -> bool {
    let exemptions = &tls_intercept.network_options.tls_interception_exemptions;
    if exemptions.is_empty() {
        return false;
    }
    if crate::policy::addresses_contain(exemptions.addresses(), dest_ip) {
        return true;
    }
    let (Some(Ok(name)), Ok(port)) = (sni.map(Host::new), Port::new(dest_port)) else {
        return false;
    };
    matches!(name.kind(), HostKind::Domain(_))
        && tls_exemptions_name_host(exemptions, &name, dest_port)
        && resolver.confirm_binding(&name, port, dest_ip).await.is_ok()
}

/// Refuses a connection whose allow rests on a name the destination does not
/// belong to (domain fronting, or naming an allowed host while redirected to
/// another address). Returns `None` when the connection may proceed.
///
/// The proxy resolves the name itself and the original destination must be
/// among its answers. Answers can differ from the client's own lookup
/// (short TTLs, round-robin or geo-steered DNS, CDNs), so a legitimate
/// connection can be refused; the check never admits an address the proxy
/// does not itself resolve. A failed lookup refuses the connection.
async fn confirm_sni_binding(
    resolver: &VettingResolver,
    rests_on_sni: bool,
    sni: Option<&str>,
    dest_ip: IpAddr,
    dest_port: u16,
) -> Option<Denial> {
    if !rests_on_sni {
        return None;
    }
    let sni = sni?;
    let denied = |reason| Some(Denial::new(Some(sni.to_string()), reason));
    let (Ok(name), Ok(port)) = (Host::new(sni), Port::new(dest_port)) else {
        return denied(EgressViolationReason::SniResolutionFailed);
    };
    match resolver.confirm_binding(&name, port, dest_ip).await {
        Ok(()) => None,
        Err(err) => denied(err.sni_violation()),
    }
}

fn address_denied(host: Option<String>, class: AddressDenialClass) -> ProxyOutcome {
    ProxyOutcome::Denied(Denial::new(
        host,
        EgressViolationReason::DestinationAddressDenied(class),
    ))
}

/// Copies both ways until either side ends, reporting what each side sent.
async fn relay<C, U>(client: &mut C, upstream: &mut U) -> ProxyOutcome
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    match tokio::io::copy_bidirectional(client, upstream).await {
        Ok((bytes_client_to_upstream, bytes_upstream_to_client)) => ProxyOutcome::Completed {
            bytes_client_to_upstream,
            bytes_upstream_to_client,
        },
        Err(err) => ProxyOutcome::UpstreamConnectFailed(err),
    }
}

/// Writes `prefix` to the upstream ahead of the splice. The splice only
/// flushes what it relays, so a TLS record left buffered here would never
/// reach the upstream.
async fn splice_with_prefix<C, U>(mut client: C, mut upstream: U, prefix: &[u8]) -> ProxyOutcome
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let led = async {
        tokio::io::AsyncWriteExt::write_all(&mut upstream, prefix).await?;
        tokio::io::AsyncWriteExt::flush(&mut upstream).await
    };
    if let Err(err) = led.await {
        return ProxyOutcome::UpstreamConnectFailed(err);
    }
    relay(&mut client, &mut upstream).await
}

/// What the operator's interception settings say about an admitted flow.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Interception {
    /// Whatever the rules, secrets and options already decide.
    Default,
    /// Relay without terminating, whatever else would terminate.
    Exempt,
    /// Terminate even where the rules would relay, so the request's `Host`
    /// is checked: the allow rests on the name alone.
    Required,
}

/// Carries out an allowed connection: splices to `upstream`, or
/// terminates TLS for a `Transform`/`Forward` rule. `prefix` is whatever the
/// caller already read off `client` and must still reach the peer. Shared by
/// transparent mode and the explicit proxy ingress, which both decide before
/// calling.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn forward_decided<S>(
    mut client: S,
    prefix: Vec<u8>,
    sni: Option<&str>,
    action: Option<DomainRuleAction>,
    upstream: LegAddresses,
    tls_intercept: &TlsInterceptContext,
    opaque_tunnels: OpaqueTunnelPolicy,
    requests: Option<RequestChecks>,
    deadlines: TunnelDeadlines,
    interception: Interception,
) -> ProxyOutcome
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let dispatch = dispatch_with_secrets(
        dispatch_for_action(sni, action.as_ref()),
        sni,
        upstream.port(),
        tls_intercept.secrets_for(sni, upstream.port()),
    );
    let dispatch = match (dispatch, sni) {
        (ConnectionDispatch::RawSplice, Some(sni_host))
            if tls_intercept.terminates_host(Some(sni_host), upstream.port()) =>
        {
            ConnectionDispatch::TlsIntercept {
                sni_host,
                domain_action: None,
            }
        }
        (dispatch, _) => dispatch,
    };
    let dispatch = match (interception, dispatch, sni) {
        // An exemption relays the bytes: no rule, secret binding or
        // terminate-all switch gets to look inside.
        (Interception::Exempt, _, _) => ConnectionDispatch::RawSplice,
        (Interception::Required, ConnectionDispatch::RawSplice, Some(sni_host)) => ConnectionDispatch::TlsIntercept {
            sni_host,
            domain_action: None,
        },
        (_, dispatch, _) => dispatch,
    };
    match dispatch {
        ConnectionDispatch::RawSplice if opaque_tunnels.refuses() => ProxyOutcome::Denied(opaque_tunnel::refusal(sni)),
        ConnectionDispatch::RawSplice => {
            let upstream = match tls_intercept.dial_routed_any(sni, true, &upstream.endpoints()).await {
                Ok(stream) => stream,
                Err(err) => return ProxyOutcome::UpstreamConnectFailed(err),
            };

            splice_with_prefix(client, upstream, &prefix).await
        }
        ConnectionDispatch::TlsInterceptRequiresSni => ProxyOutcome::TlsInterceptRequiresSni,
        ConnectionDispatch::TlsIntercept {
            sni_host,
            domain_action,
        } => {
            let prefix =
                match opaque_tunnel::first_bytes(&mut client, prefix, deadlines.handshake_left(), opaque_tunnels).await
                {
                    FirstBytes::Terminable(prefix) => prefix,
                    FirstBytes::NotTls => {
                        return ProxyOutcome::Denied(opaque_tunnel::refusal(Some(sni_host)));
                    }
                };
            let client_stream = PrefixedStream::new(client, prefix);

            let credential_injection = match domain_action {
                Some(DomainAction::Transform(transform)) => Some(CredentialInjectionContext {
                    transform,
                    credential_source: tls_intercept.credential_source.as_ref(),
                }),
                _ => None,
            };
            let forward = match domain_action {
                Some(DomainAction::Forward(forward)) => Some(ForwardContext {
                    forward,
                    signer: &tls_intercept.oidc_identity.signer,
                    issuer: &tls_intercept.oidc_identity.issuer,
                    subject: &tls_intercept.oidc_identity.subject,
                    tenant_id: &tls_intercept.oidc_identity.tenant_id,
                    lease_id: &tls_intercept.oidc_identity.lease_id,
                    original_scheme: "https",
                    original_port: upstream.port(),
                    marking: tls_intercept.marking,
                }),
                _ => None,
            };

            match terminate_and_forward_checked(
                client_stream,
                sni_host,
                &tls_intercept.ca,
                upstream,
                tls_intercept.marking,
                Arc::clone(&tls_intercept.upstream_tls_config),
                credential_injection,
                forward,
                tls_intercept.secrets.as_ref(),
                deadlines,
                requests,
                response_strip(&tls_intercept.network_options),
                Some(tls_intercept.shared_queue.clone()),
                Some(&tls_intercept.network_options),
            )
            .await
            {
                Ok((bytes_client_to_upstream, bytes_upstream_to_client)) => ProxyOutcome::Completed {
                    bytes_client_to_upstream,
                    bytes_upstream_to_client,
                },
                Err(err) => ProxyOutcome::TlsInterceptFailed(err),
            }
        }
    }
}

/// Postgres-specific control flow: connect to upstream and relay the
/// `SSLRequest`/`S`/`N`/`E` negotiation live before any policy decision can
/// be made, since a Postgres client will not emit its ClientHello until it
/// has seen the real upstream's response. This is safe to do earlier than
/// the generic path because `dest_ip`/`dest_port` are already the resolved
/// real destination (transparent redirect already happened); connecting
/// early is not a new information leak, only an earlier connection. Any
/// bytes relayed during negotiation are protocol negotiation only, not data
/// access: on Deny, both sockets are closed immediately with nothing further
/// forwarded either direction.
async fn handle_postgres_connection(
    mut client: TcpStream,
    policy: &NetworkPolicy,
    dest: VettedEndpoint,
    ssl_request: Vec<u8>,
    resolver: &VettingResolver,
    tls_intercept: &TlsInterceptContext,
) -> ProxyOutcome {
    let marking = tls_intercept.marking;
    let mut upstream = match dial::connect(marking, dest).await {
        Ok(stream) => stream,
        Err(err) => return ProxyOutcome::PostgresUpstreamConnectFailed(err),
    };

    let (outcome, _relayed) = match postgres::negotiate(&mut client, &mut upstream, &ssl_request).await {
        Ok(result) => result,
        Err(err) => return ProxyOutcome::PostgresNegotiationFailed(err),
    };

    let sni = match outcome {
        PgHandshakeOutcome::TlsUpgraded { sni } => sni,
        PgHandshakeOutcome::PlaintextDeclined => {
            // sslmode=require+ parity: a connection that never upgraded to
            // TLS is denied outright, not evaluated against IP-range rules -
            // otherwise a client could bypass all domain rules simply by
            // declining TLS.
            drop(client);
            drop(upstream);
            return ProxyOutcome::Denied(Denial::new(None, EgressViolationReason::PostgresTlsDeclined));
        }
    };

    let decision = decide(policy, sni.as_deref(), dest.addr().ip(), dest.port());
    if decision == Decision::Deny {
        drop(client);
        drop(upstream);
        return ProxyOutcome::Denied(Denial::new(sni, EgressViolationReason::PolicyDenied));
    }
    if let Some(denial) = confirm_sni_binding(
        resolver,
        allow_rests_on_sni(policy, sni.as_deref(), dest.addr().ip(), dest.port()),
        sni.as_deref(),
        dest.addr().ip(),
        dest.port(),
    )
    .await
    {
        drop(client);
        drop(upstream);
        return ProxyOutcome::Denied(denial);
    }

    // A Postgres session is relayed without terminating, so a name-only
    // allow cannot be carried when the operator requires termination for it.
    if names_alone_carry_the_allow(
        tls_intercept,
        policy,
        allow_rests_on_sni(policy, sni.as_deref(), dest.addr().ip(), dest.port()),
        dest.addr().ip(),
        dest.port(),
    ) {
        drop(client);
        drop(upstream);
        return ProxyOutcome::Denied(Denial::new(sni, EgressViolationReason::OpaqueTunnelRefused));
    }

    relay(&mut client, &mut upstream).await
}

/// Identical to [`sniff_client_hello`], except the caller has already read
/// some bytes off `stream` (e.g. while checking for a Postgres `SSLRequest`
/// that turned out not to match) and wants those bytes treated as already
/// part of the sniff buffer instead of re-reading the socket.
async fn sniff_client_hello_with_seed(stream: &mut TcpStream, seed: Vec<u8>) -> io::Result<(Option<String>, Vec<u8>)> {
    let mut buf = seed;

    match extract_sni(&buf) {
        SniExtraction::Incomplete => {}
        SniExtraction::Found(host) => return Ok((Some(host), buf)),
        SniExtraction::NotPresent | SniExtraction::Unreadable | SniExtraction::NotTls => {
            return Ok((None, buf));
        }
    }

    let deadline = timeout(SNIFF_TIMEOUT, async {
        loop {
            if buf.len() >= MAX_SNIFF_BYTES {
                return Ok::<_, io::Error>(SniExtraction::NotPresent);
            }

            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(extract_sni(&buf));
            }
            buf.extend_from_slice(&chunk[..n]);

            match extract_sni(&buf) {
                SniExtraction::Incomplete => continue,
                other => return Ok(other),
            }
        }
    })
    .await;

    let extraction = match deadline {
        Ok(result) => result?,
        Err(_) => SniExtraction::Incomplete,
    };

    let sni = match extraction {
        SniExtraction::Found(host) => Some(host),
        SniExtraction::NotPresent | SniExtraction::Unreadable | SniExtraction::NotTls | SniExtraction::Incomplete => {
            None
        }
    };

    Ok((sni, buf))
}

#[cfg(test)]
mod tests;

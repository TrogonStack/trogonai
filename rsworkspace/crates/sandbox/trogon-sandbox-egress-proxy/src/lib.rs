//! Provider-agnostic transparent TCP egress proxy core.
//!
//! Given a [`trogon_sandbox_contract::NetworkPolicy`] and an inbound TCP
//! connection from a sandboxed process, this crate decides whether to allow
//! or deny the connection. For a matched `PassThrough` domain rule (or any
//! non-domain-rule allow), it splices the raw bytes bidirectionally to the
//! real destination unmodified. For a matched `Transform` domain rule, it
//! terminates the sandboxed client's TLS with a per-process CA (see
//! [`tls_ca`]/[`tls_intercept`]), re-encrypts to the real upstream, and (see
//! [`http_intercept`]) parses each HTTP/1.1 request on the connection to
//! inject a broker-fetched credential (see [`credential_source`]) into
//! requests matching the rule's `match_rules`. It does not wire itself into
//! any specific provider's network setup (iptables, Docker network modes,
//! etc.); it is a reusable core any worker can drive.

mod address_turns;
mod aws_sigv4;
pub mod binding_cache;
mod body_limit;
#[cfg(test)]
mod body_streaming_tests;
#[cfg(test)]
mod bulk_transfers;
mod chunked_body;
#[cfg(test)]
mod chunked_decode_tests;
mod client_io;
mod connect_target;
mod connection_headers;
mod constants;
pub mod credential_source;
pub mod decider;
pub mod decider_stream;
mod denial;
mod deny_explanation;
pub mod deny_header_style;
pub mod dial;
mod early_answer;
mod fold_mapped;
mod framed_head;
mod framing;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzz_support;
mod head_parse_error;
pub mod header_mutator;
mod header_syntax;
#[cfg(test)]
mod held_writes;
mod host_kind;
mod host_mismatch_error;
pub mod http_intercept;
mod http_intercept_error;
pub mod in_process;
pub mod ingress;
mod ingress_outcome;
mod ingress_rejection_error;
mod invalid_match_rule_error;
#[cfg(test)]
mod jwt_masking_tests;
#[cfg(test)]
mod large_transfer_tests;
pub mod live_flows;
pub mod network_env;
mod network_env_error;
pub mod oidc_signer;
mod opaque_tunnel;
pub mod parent_pool;
pub mod policy;
pub mod postgres;
pub mod proxy;
mod proxy_headers;
mod proxy_outcome;
mod refusal_error;
pub mod request_body;
pub mod request_decider;
pub mod request_decision;
pub mod request_filter;
mod request_line;
mod request_version;
pub mod resolve;
mod response_head;
mod response_relay;
#[cfg(test)]
mod response_scrubbing_tests;
mod response_strip;
#[cfg(test)]
mod response_strip_tests;
#[cfg(test)]
mod revalidate_tests;
pub mod secret_substitution;
pub mod server;
pub mod sigv4_resign;
#[cfg(test)]
mod sigv4_resign_tests;
pub mod sni;
mod status_code;
mod stream_use;
#[cfg(test)]
mod substitution_tests;
#[cfg(test)]
mod terminated_response_tests;
pub mod tls_ca;
pub mod tls_intercept;
mod tls_intercept_error;
mod tls_target;
pub mod trust_bundle;
pub mod tunnel_deadlines;
#[cfg(test)]
mod tunnel_ends_tests;
pub mod tunnel_gate;
#[cfg(test)]
mod tunnel_stays_open_tests;
pub(crate) mod upstream_leg;
mod upstream_stream;
pub mod violation;
pub mod violation_store;

pub use credential_source::{
    BrokerCredentialSource, CredentialSource, CredentialSourceError, SecretLoadError, load_sentinel_registry,
};
pub use decider::{
    Decider, DeciderLimits, DeciderTransport, DeciderTransportError, Judgment, LivePolicyAmender, NatsDeciderTransport,
    NatsPolicyPublisher, PolicyAmender, PolicyPublisher,
};
pub use header_mutator::HeaderMutator;
pub use http_intercept::{
    BodyLimit, CompiledMatchRules, HostMismatchError, HttpInterceptError, InvalidMatchRuleError, RefusalError,
    RelayCounts, RequestGuard, StatusCode, TlsTarget, run_http_intercept,
};
pub use in_process::{
    AskCallback, AskQuestion, AskVerdict, CallbackRequestFilter, InProcessDeciderTransport, RequestCallback,
    RequestInfo, in_process_decider,
};
pub use ingress::{
    ConnectTarget, Ingress, IngressLimits, IngressOutcome, IngressRejectionError, InvalidProxyTokenError, Protocol,
    ProxyToken,
};
pub use network_env_error::NetworkEnvError;
pub use oidc_signer::{MintRequest, OidcSigner, OidcSignerError, validation_for_audience};
pub use policy::{
    Decision, Ruling, allows_by_address, amended, decide, decide_host, decide_with_action, domain_matches,
    refuses_address, rule, rule_host,
};
pub use postgres::PgHandshakeOutcome;
pub use proxy::{MAX_SNIFF_BYTES, ProxyOutcome, SNIFF_TIMEOUT, handle_connection, sniff_client_hello};
pub use request_body::{BodyRead, BodyUnavailableError, MAX_BODY_READ, RequestBody, RequestBodyReader};
pub use request_decider::{CredentialPlacement, CredentialScheme, MAX_REPLY_FRAME, MAX_REQUEST_BODY, RequestDecider};
pub use request_decision::{
    DEFAULT_DENY_REASON, DenyMark, HeaderEdits, MalformedDecisionError, RequestAllow, RequestDenial, ResponseHook,
    fold_header_name,
};
pub use request_filter::{
    DeniedRequestObserver, PolicyRequestFilter, RequestChecks, RequestDecision, RequestFilter, RequestScheme,
    RequestView, TargetForms, all_of,
};
pub use resolve::{
    AllowRanges, InvalidAddressError, LocalInterfaces, NameLookup, ResolveError, SsrfFloor, SystemInterfaces,
    SystemLookup, VettedAddr, VettedEndpoint, VettingResolver, parse_address,
};
pub use secret_substitution::{
    CompressedResponsePolicy, SecretSubstitution, SentinelMisdirectionObserver, SubstitutionScope,
};
pub use server::{
    BoundAddress, PolicyHandle, PolicyRejectedError, ProxyServer, ProxyServerBuilder, SharedPolicy,
    intercept_domains_outside_ca, permitted_domains_for, system_upstream_tls_config, upstream_tls_config_trusting,
};
pub use sigv4_resign::{Sigv4BodyLimit, Sigv4Policies, Sigv4RefusalError, UnsupportedSigv4Policy};
pub use sni::{SniExtraction, extract_sni};
pub use tls_ca::{
    CaPairError, CaThumbprint, GeneratedCa, SandboxCa, SandboxCaError, ValidatedCa, cert_thumbprint, generate_ca,
    install_default_crypto_provider, validate_ca_pair,
};
pub use tls_intercept::{PrefixedStream, TlsInterceptError, terminate_and_forward, terminate_and_forward_with_secrets};
pub use trust_bundle::workload_trust_bundle;
pub use tunnel_deadlines::{
    ByteBudget, DeadlineStream, SHARED_QUEUE_BUDGET, SharedQueueBudget, TunnelDeadlines, TunnelEndError,
};
pub use tunnel_gate::{TunnelGate, TunnelSlot};
pub use violation::{
    NatsViolationSink, ViolationPublishError, ViolationRateLimit, ViolationRateLimiter, ViolationReporter,
    ViolationSink,
};
pub use violation_store::{
    CommandId, DenialReason, DenialReasons, IgnoreViolations, SandboxViolation, StoreViolationSink, Subscription,
    ViolationFilter, ViolationLine, ViolationStore,
};

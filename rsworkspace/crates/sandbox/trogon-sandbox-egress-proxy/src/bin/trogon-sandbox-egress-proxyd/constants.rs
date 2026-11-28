use std::time::Duration;

/// Prefix shared by every variable the daemon reads, checked for valid Unicode at startup.
pub(super) const ENV_VAR_PREFIX: &str = "EGRESS_PROXY_";

/// Loopback: the transparent redirect of a sidecar lands on 127.0.0.1, so
/// nothing needs this port reachable from another host.
pub(super) const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:3128";

/// `true` lets the transparent or explicit listener bind a non-loopback
/// address, where anything that can reach the port uses the proxy. Unset, a
/// non-loopback address is refused at startup. The flag is
/// `--allow-non-loopback-listen`.
pub(super) const ALLOW_NON_LOOPBACK_LISTEN_ENV_VAR: &str = "EGRESS_PROXY_ALLOW_NON_LOOPBACK_LISTEN";

/// `TRANSPARENT_LISTENER_OFF` here runs the explicit proxy alone, for a
/// per-lease instance that must not open a second, unauthenticated port.
pub(super) const LISTEN_ADDR_ENV_VAR: &str = "EGRESS_PROXY_LISTEN_ADDR";

pub(super) const TRANSPARENT_LISTENER_OFF: &str = "off";

pub(super) const POLICY_FILE_ENV_VAR: &str = "EGRESS_PROXY_POLICY_FILE";

/// Optional: if set, the freshly generated per-process CA's PEM is written
/// here so a provider (Docker sidecar mount, k8s ConfigMap) can expose it to
/// the sandboxed workload for trust, the same way `POLICY_FILE_ENV_VAR` is
/// used the other direction (provider writes, this process reads).
pub(super) const CA_OUT_FILE_ENV_VAR: &str = "EGRESS_PROXY_CA_OUT_FILE";

/// Optional: PEM files of a CA to sign with instead of a generated one, as
/// `tlsTerminate.caCertPath` and `caKeyPath` do. Both or neither.
pub(super) const CA_CERT_FILE_ENV_VAR: &str = "EGRESS_PROXY_CA_CERT_FILE";

pub(super) const CA_KEY_FILE_ENV_VAR: &str = "EGRESS_PROXY_CA_KEY_FILE";

/// Optional: NATS URL for the credential broker this process's sandbox talks
/// to. Required only when the loaded policy has at least one `Transform`
/// domain rule; a `Transform` rule matching with no broker connection
/// configured fails the request rather than forwarding it uncredentialed
/// (see `UnconfiguredCredentialSource`).
pub(super) const CREDENTIAL_BROKER_NATS_URL_ENV_VAR: &str = "EGRESS_PROXY_CREDENTIAL_BROKER_NATS_URL";

/// Required alongside `CREDENTIAL_BROKER_NATS_URL_ENV_VAR`: the tenant this
/// sandbox belongs to, used to scope every credential-broker connection.
pub(super) const CREDENTIAL_BROKER_TENANT_ID_ENV_VAR: &str = "EGRESS_PROXY_CREDENTIAL_BROKER_TENANT_ID";

/// Optional, together with `CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR`: the
/// lease this proxy serves. Every credential request then carries the lease
/// and the committed policy revision, and the broker resolves only references
/// that revision names. Unset, requests carry no fence, which a broker that
/// requires the fence refuses.
pub(super) const CREDENTIAL_BROKER_LEASE_ID_ENV_VAR: &str = "EGRESS_PROXY_CREDENTIAL_BROKER_LEASE_ID";

/// The committed policy revision the proxy was started from (an integer).
pub(super) const CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR: &str = "EGRESS_PROXY_CREDENTIAL_BROKER_POLICY_REVISION";

/// Required only when the loaded policy has at least one `Forward` domain
/// rule: the tenant this sandbox belongs to, minted into every OIDC token's
/// `tenant_id` claim. Deliberately a separate env var (and config path) from
/// `CREDENTIAL_BROKER_TENANT_ID_ENV_VAR` even though both are "the tenant":
/// the credential broker and OIDC identity are unrelated systems (one talks
/// to NATS, the other only ever signs a JWT locally) that happen to both
/// need tenant context, and requiring an operator to configure them
/// independently avoids this proxy silently assuming they're always the same
/// system, or always both present, or interchangeable.
pub(super) const OIDC_TENANT_ID_ENV_VAR: &str = "EGRESS_PROXY_OIDC_TENANT_ID";

/// Required alongside `OIDC_TENANT_ID_ENV_VAR`: the lease this sandbox is
/// running under, minted into every OIDC token's `lease_id` claim.
pub(super) const OIDC_LEASE_ID_ENV_VAR: &str = "EGRESS_PROXY_OIDC_LEASE_ID";

/// Required alongside `OIDC_TENANT_ID_ENV_VAR`: the sandbox/workload identity
/// minted into every OIDC token's `sub` claim.
pub(super) const OIDC_SUBJECT_ENV_VAR: &str = "EGRESS_PROXY_OIDC_SUBJECT";

/// Optional: a PKCS#8 PEM-encoded EC private key file used to sign OIDC
/// tokens, so the signing key (and the public key a receiving proxy must
/// trust) survives process restarts. If unset, a fresh ephemeral key pair is
/// generated at startup (see `OidcSigner::generate`), the same trust model
/// `SandboxCa`'s own leaf-signing key already uses in this process.
pub(super) const OIDC_SIGNING_KEY_PEM_FILE_ENV_VAR: &str = "EGRESS_PROXY_OIDC_SIGNING_KEY_PEM_FILE";

/// Optional: NATS URL egress violation events are published to. Requires
/// `VIOLATION_LEASE_ID_ENV_VAR`.
pub(super) const VIOLATION_NATS_URL_ENV_VAR: &str = "EGRESS_PROXY_VIOLATION_NATS_URL";

/// Required alongside `VIOLATION_NATS_URL_ENV_VAR`: the lease every violation
/// event is attributed to.
pub(super) const VIOLATION_LEASE_ID_ENV_VAR: &str = "EGRESS_PROXY_LEASE_ID";

/// Optional: address of the authenticated explicit proxy listener (HTTP
/// CONNECT, absolute-form HTTP and SOCKS5 on one port), served alongside the
/// transparent listener. Requires `MUX_TOKEN_ENV_VAR`.
pub(super) const MUX_LISTEN_ADDR_ENV_VAR: &str = "EGRESS_PROXY_MUX_LISTEN_ADDR";

/// Required alongside `MUX_LISTEN_ADDR_ENV_VAR`: the per-lease token clients
/// present as the proxy password. Never logged.
pub(super) const MUX_TOKEN_ENV_VAR: &str = "EGRESS_PROXY_MUX_TOKEN";

/// Optional: how long, in milliseconds, to wait for the decider's reply
/// before denying. Setting it asks the decider over NATS about destinations
/// the policy neither allows nor denies instead of denying them. Requires
/// `VIOLATION_NATS_URL_ENV_VAR` and `VIOLATION_LEASE_ID_ENV_VAR`, whose NATS
/// server and lease the questions go to.
pub(super) const DECIDER_TIMEOUT_MS_ENV_VAR: &str = "EGRESS_PROXY_DECIDER_TIMEOUT_MS";

/// Optional: how many distinct destinations may be awaiting the decider at
/// once; further ones are denied immediately. Only read alongside
/// `DECIDER_TIMEOUT_MS_ENV_VAR`.
pub(super) const DECIDER_MAX_IN_FLIGHT_ENV_VAR: &str = "EGRESS_PROXY_DECIDER_MAX_IN_FLIGHT";

/// File descriptor number of an inherited pipe carrying the sentinel registry
/// (real credential values included). Read to EOF before anything is served.
/// A descriptor rather than an env var, argv entry or file so the values are
/// never visible in `/proc/<pid>/{environ,cmdline}` or on disk.
pub(super) const SECRETS_FD_ENV_VAR: &str = "EGRESS_PROXY_SECRETS_FD";

/// Uid the transparent redirect exempts with `--uid-owner` so this process's
/// upstream dials are not redirected back into itself. When set, upstream
/// sockets are not marked (no `CAP_NET_ADMIN` needed) and startup fails
/// unless this process runs as exactly this uid. When unset, upstream
/// sockets are marked instead.
pub(super) const EXEMPT_UID_ENV_VAR: &str = "EGRESS_PROXY_EXEMPT_UID";

pub(super) const POLICY_RELOAD_POLL_INTERVAL: Duration = Duration::from_secs(1);

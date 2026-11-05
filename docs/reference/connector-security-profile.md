# Connector Security Profile

This profile fixes conservative deployment and operational defaults for draft
ADRs0092-0096. Values below are platform design decisions, not limits prescribed
by an RFC or a claim of compliance. They remain subject to implementation and
the [conformance gate](./connector-conformance.md).

## Authority and rollout

Ship project-owned Connections first. Schema arms for person/workspace ownership
remain policy-disabled until their offboarding/delegation gates pass. No
transparent proxy, raw token issuance, node-scoped app or cross-project app
borrowing is enabled implicitly. Explicit authenticated HTTPS relay is the
ordinary provider traffic path; trusted MCP uses its own protocol/session grant.

Future Gmail onboarding uses a customer-supplied app first. External secret
sources such as 1Password remain sources behind the existing secrets port, not
an alternate credential execution or agent-token delivery plane. These are
rollout defaults for deferred integrations, not claims of vendor approval.

The broker is a minimal module inside the secrets service and the only code
performing token/registration calls with refresh tokens, client secrets or app
private keys. Parsing ordinary provider responses and fetching discovery data
stay outside that process. Use a sandboxed, keyless discovery worker with fixed
CPU/memory/deadline budgets and host-enforced destination checks.

Credential-bearing relay/MCP workers serve one tenant per workload identity and
process boundary. They share fleet infrastructure, not access-credential caches
or wildcard custody authority. Their network role reaches approved provider
origins and authorized custody operations only. They cannot mint grants,
administer OpenBao or read another tenant's service subjects. A customer-managed
key tenant follows the same boundary plus its key-veto requirements. Shared
brokers still need per-tenant quotas and independently authenticated requests.

## OAuth edge and discovery

Use one configured HTTPS console origin per environment for the callback and
public Client ID Metadata Document. Configure an exact callback path; no user
input, wildcard or arbitrary return URL chooses it. Missing origin configuration
blocks app activation. Development has a distinct app and environment; a
production callback never points to a development tunnel.

Use the reviewed [MCP2026 authorization profile](./connector-mcp-authorization.md)
where applicable, independently of transport/session versions. Use authorization
code with S256 PKCE. Discovery must explicitly support the
required flow; absent evidence does not mean support. V1 grants no automatic
provider exception for missing PKCE. The callback verifies the same initiating
session, opaque initiator binding, browser binding, state, issuer, one-use claim
and deadline. A public CIMD document contains no tenant identity or secret.
[OAuth security guidance](https://www.rfc-editor.org/rfc/rfc9700.html#section-2.1)
requires exact redirect matching and protection against open redirects and CSRF.

| Limit | Default and ceiling |
| --- | --- |
| Attempt, state and custody capsule lifetime | 10 minutes; no extension after exchange starts |
| Callback attempts | 10/session/minute, 60/source-address/minute and 5 failed claims/attempt; exceeding an attempt limit quarantines the attempt rather than bypassing session checks |
| Metadata body | 64 KiB wire content and decoded content, bounded before buffering/parsing |
| Token/registration response | 16 KiB wire content and decoded content, bounded before buffering/parsing |
| Discovery redirects | At most 3, same exact HTTPS origin; validate each resolved address before connection |
| Discovery/token connect and total deadline | 5 seconds and 15 seconds |
| Scope step-up initiation | At most 2/connection/day; a user starts the new consent explicitly |

Count per-attempt callback failures only after authenticating the initiating
session and validating signed state. Anonymous/wrong-initiator traffic is
source-throttled and cannot consume another user's attempt budget.

Discovery, token and registration HTTP clients request Accept-Encoding: identity,
disable automatic decompression and reject every Content-Encoding field, not
just bodies that expand beyond a decoded limit. This makes content-byte limits
apply before buffering with no decompressor-work loophole. Header sections are
limited to 16 KiB and 128 fields. Framing validation and the same limits apply
to every redirect response and error body, not just a final success. A provider
requiring compressed control responses needs a separately reviewed profile.

For every connection/retry/address fallback, reject private, loopback, link-local,
multicast, reserved and cloud-metadata destinations; validate IPv4 and IPv6.
Resolve once, validate all candidate addresses and bind the socket to an approved
address while preserving the approved hostname for SNI and certificate validation.
No second unchecked lookup or redirect can select a different destination. This
follows [OWASP SSRF prevention guidance](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html).

## Custody and lifecycle

Seal token sets, staged material, capsules and registration credentials for every
tenant under the existing ADR0083 credential/sealing scope. The connector drafts
refine the named sensitive-stream classes; they do not silently change the status
or scope of accepted ADRs. Broker replies carrying material remain directly sealed
to the authorized caller and never become persistent RPC/idempotency results.

Offer revoke-all before a customer destroys a key, but never delay or pretend to
undo the customer's destructive action. If a key veto prevents provider revocation,
record that limitation safely and refuse local use. Destroyed/restored custody
never authorizes automatic refresh or relay replay. Only reviewed releases may
declare stable refresh tokens or reuse grace; discovered servers use the strict
rotating-token default.

Project archive suspends use immediately. If not restored within 7 days, start
disconnect/revoke cleanup; the tenant may configure a shorter window. Membership
and tenant offboarding refuse new use immediately through their authoritative
facts. A claim is released only after its Connection is terminal, never because
a hold timer expired. Profile erasure starts immediately on disconnect/offboarding;
debt is visible until every revision/copy in scope is purged. No numeric cleanup
SLA substitutes for completed erasure evidence.

Future person delegations bind exact AgentIds, expire after 24 hours by default
and cannot exceed 7 days. Renewal is a new owner-authorized action. Node subjects
cannot carry personal delegations. Workspace integration-project binding belongs
to workspace policy and governs creation; it does not move existing custody.

## Operator authority and retention

Connection upgrades are owner-initiated, including a non-widening release change.
Ordinary platform release blocking/app revocation requires a recorded typed reason
and approval from distinct platform-operator principals. An emergency operator may
block immediately to contain compromise; the durable action requires independent
review within 24 hours. Missing review escalates and never automatically unblocks
the resource. Operators cannot use this role to read customer credentials.

Retain body-free connection audit facts for 30 days by default. A deployment must
explicitly configure a shorter privacy window or a longer justified retention/hold
policy before admitting affected tenants; there is no silent indefinite retention.
Retention applies to stream replicas, exports and backups through a documented
deletion schedule. A legal or contractual hold is an external configured policy,
not an exception invented by the proxy. Log-management standards describe policy
and operational processes rather than prescribing our chosen day count.
[NIST SP800-92](https://csrc.nist.gov/pubs/sp/800/92/final)
is the process reference; this profile supplies the product default.

Keep request/response bodies, query strings, callback URLs with parameters,
signature headers and raw provider errors out of logs, traces and audit.
Security metrics use bounded reason codes, workload/project references and
counts. Alert on replay attempts, denied custody subjects, scope widening,
stale authorization, restore epoch mismatch, cleanup debt and exhausted budgets.

## Enablement evidence

App registration/approvals, configured DNS/callback certificates, workload
identities, network policy, sandbox support and retention/backup settings must
be verified in the deployment. Their absence is a concrete enablement blocker;
it does not reopen the contract defaults above. No production approval,
certification, runtime isolation or vendor registration is claimed by this page.

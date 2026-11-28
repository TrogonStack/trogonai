# Egress: name to address attribution and TLS interception exemptions

How `trogon-sandbox-egress-proxy` decides which destination address a domain allow
covers, and how an operator relays chosen destinations without terminating TLS.

## The problem

A domain allow (`*.example.com`) is decided on the name the client presents in
the TLS ClientHello. On a shared edge (a CDN or load balancer) one address
serves many unrelated names. Two attacks follow:

1. The client presents an allowed name but is steered to an address that does
   not belong to it (domain fronting, or a forged guest-side DNS answer).
2. The client presents an allowed name in the ClientHello, then asks the same
   edge for a different virtual host inside the encrypted session. A
   pass-through relay never sees the `Host` header, so the two are
   indistinguishable.

## Attribution: only what the proxy itself resolved

`BindingCache` (`rsworkspace/crates/sandbox/trogon-sandbox-egress-proxy/src/binding_cache.rs`) records each (name, address) pair that the
proxy's own `VettingResolver` lookups return, for a short TTL (60 seconds by
default, 16384 entries at most). It is keyed on the pair, so a binding for one
name says nothing about another name on the same address.

`confirm_binding` is the only way a name is attributed to a destination: it
looks the name up, records every answer, and then asks the cache whether the
destination is bound. What the guest resolved is never an input, so a forged
guest answer does not help. The cost is that the proxy's lookup can differ from
the guest's (short TTLs, geo-steered or round-robin DNS), which can refuse a
legitimate flow. The check never admits an address the proxy did not resolve.

## Name-only allows on pass-through flows

`NetworkOptions::domain_only_allows` (`DomainOnlyAllows`):

- `PassThrough` (default): unchanged behavior. The allow is bound to the
  proxy's own resolution of the name, but the relay cannot see `Host`.
- `RequireTermination`: a flow whose allow rests on the presented name alone
  (a domain rule or a decider verdict) is terminated even when its rule says
  `PassThrough`, so the `Host` header, absolute-form authority and TLS server
  name are all checked against the target (refusal `421`). A flow whose
  destination address is exactly allowed (an allow range of one address, or an
  exact rule for the address literal) is exempt from the requirement, since
  the name is not what admits it. Postgres sessions cannot be terminated and are
  refused with `OpaqueTunnelRefused` under this setting.

Why the default is off: turning it on makes every pass-through domain rule
terminate TLS. That needs the sandbox CA to cover the allowed names (the CA is
name constrained) and every client to trust it; clients that pin certificates
or speak non-HTTP protocols over TLS fail. Rollout: list the exact hosts that
must stay opaque in the exemption list (below) or as exact-address allows, set
`RequireTermination` on a canary, watch for `TlsInterceptFailed` outcomes,
then widen.

## Live flows and policy replacement

With `NetworkOptions::resweep_live_flows` (off by default, because without it a
policy replacement never ends a flow already admitted), the transparent
listener registers each admitted flow in `LiveFlows`. `PolicyHandle::replace`
then sweeps: a flow the new policy no longer admits is cancelled
(`PolicyDenied`), and a flow whose admission rested on a name is re-resolved by
the proxy; if the name no longer resolves to the flow's address it is cancelled
(`SniDestinationMismatch`), and if the name cannot be resolved it survives only
while the cache still holds a live binding (`SniResolutionFailed` otherwise).
Another flow on the same shared address whose name is still allowed and bound
is left alone.

## TLS interception exemptions

`NetworkOptions::tls_interception_exemptions` lists hostname patterns and
address ranges relayed without termination, overriding a `Transform`/`Forward`
rule, a secret binding, the terminate-all switch and `RequireTermination`.

1. An exemption grants no egress. It is consulted only after normal destination
   admission (policy or decider, the SSRF floor and, for an allow on a name,
   the binding check). A destination the policy does not admit stays refused.
2. A hostname exemption applies only when the proxy's own lookup of the
   presented name, made at connect time, binds it to the destination. Without
   that binding the exemption does not apply and the flow is carried as if
   there were none (terminated, if it would have been). A name alone cannot buy
   an uninspected tunnel.
3. An address exemption matches the destination address and so also covers
   TLS with no SNI.
4. An exempt flow gets no HTTP inspection and no credential injection: request
   rules, secret substitution and `Transform`/`Forward` actions do not run.
   `opaque_tunnels` still applies: a policy that refuses opaque tunnels refuses
   an exempt flow.

This is distinct from `tls_exclusions`, which only keeps a host out
of terminate-all and secret termination and requires no binding.

## Configuring the daemon

`trogon-sandbox-egress-proxyd` reads these environment variables; each is optional and unset keeps the default behavior. Workers that start the daemon pass them through, and the `trogon-sandbox-subprocess-worker` chart (`devops/helm/charts/trogon-sandbox-subprocess-worker`) sets them from its `egressProxy` values.

| Variable | Effect |
|----------|--------|
| `EGRESS_PROXY_BINDING_TTL_SECS` | Seconds an observed name-to-address binding stays live (default 60). |
| `EGRESS_PROXY_REVALIDATE_INTERVAL_SECS` | Re-resolves the names of open flows on this period, apart from policy changes. `0` or unset is off. Implies live-flow tracking. |
| `EGRESS_PROXY_RESWEEP_LIVE_FLOWS` | `true` sweeps open flows on a policy replacement. |
| `EGRESS_PROXY_REQUIRE_TERMINATION_FOR_NAME_ONLY` | `true` terminates TLS for allows that rest on the name alone. |
| `EGRESS_PROXY_TLS_EXEMPT_HOSTS` | Comma-separated host patterns relayed without termination. |
| `EGRESS_PROXY_TLS_EXEMPT_ADDRESSES` | Comma-separated CIDR ranges relayed without termination. |

A malformed value stops the daemon at startup naming the variable. Set the revalidation interval at or below the binding TTL: a binding that expires between sweeps leaves a flow on a lapsed binding until the next sweep. Each sweep re-resolves every named open flow, so a very short interval costs DNS lookups.

### Listen addresses

| Variable or flag | Effect |
|------------------|--------|
| `EGRESS_PROXY_LISTEN_ADDR` | Transparent listener. Defaults to `127.0.0.1:3128`, where a sidecar's `iptables` redirect of locally generated traffic lands. `off` runs the explicit proxy alone. |
| `EGRESS_PROXY_MUX_LISTEN_ADDR`, `--listen` | Authenticated explicit listener, a TCP address or `unix:<path>`. |
| `EGRESS_PROXY_ALLOW_NON_LOOPBACK_LISTEN=true`, `--allow-non-loopback-listen` | Required to bind a non-loopback TCP address on either listener. Without it the daemon refuses to start, because every host that can reach the port would use the proxy. Set it only where traffic arrives on another interface, for example a proxy serving a VM tap, and give the listener that interface's address. |

## Residual risk

- Within the TTL a name stays attributed to every address the proxy resolved
  for it. A client can use any of them, and a CDN that serves other tenants on
  those addresses still serves them over a pass-through flow unless
  `RequireTermination` is on.
- The proxy and the guest may resolve a name differently; flows then fail
  closed (`SniDestinationMismatch`), not open.
- An exempt flow is uninspected by design. A hostname exemption is only as
  tight as the proxy's DNS view of the name; an address exemption trusts the
  whole address, including anything else served from it.
- `RequireTermination` relies on the client trusting the sandbox CA. A client
  that does not will fail rather than fall back to pass-through.
- Resweep covers transparent flows only, runs on policy replacement and, when
  `EGRESS_PROXY_REVALIDATE_INTERVAL_SECS` is set, on that timer (off by
  default), and a flow that stays within a still-valid binding is not ended by
  the TTL expiring on its own.

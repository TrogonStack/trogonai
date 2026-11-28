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

## Names the proxy never resolves

A DNS query carries the name to whoever answers for its zone, so resolving a
name is itself egress. A workload that cannot reach `attacker.example` could
still send `<data>.attacker.example` to the attacker's name server if the
proxy looked it up before refusing it. The explicit proxy (HTTP CONNECT,
absolute-form plain HTTP and SOCKS5) therefore decides on the name before any
lookup:

1. A name that carries a secret sentinel (the `fake_value_` prefix of a minted
   sentinel or the signature of a fake JWT, in any case) is refused with
   `PolicyDenied` and never resolved, even when a rule would allow it.
   `VettingResolver` enforces the same rule on every lookup it makes,
   including the binding checks of transparent flows, so no caller can send
   such a name to DNS.
2. A denied domain refuses the name unresolved.
3. A domain rule that matches the name allows it; the name is then resolved
   to find the address to dial.
4. A name no rule lists is resolved only when an allow range open on the
   destination port (an unscoped range, or one scoped to that port) exists,
   because only then can its address still admit it. Otherwise it goes to the
   decider, when there is one, or is refused, without a lookup.

The cost of rule 4 is unchanged policy semantics: when an allow range is open
on the port, an unlisted name on that port is still looked up, since its
address is what the range judges. Scope allow ranges to the ports they serve
to keep that window narrow.

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
   `opaque_tunnels` still applies: a policy that refuses opaque tunnels, which
   is every `user_defined` policy that does not set `"opaque_tunnels": "allow"`,
   refuses an exempt flow.

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

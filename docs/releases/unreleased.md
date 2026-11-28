# Unreleased

Changes merged to `main` that no versioned release note covers yet. Each entry
moves into the note of the release that first ships it.

## Sandbox egress refuses opaque tunnels by default

**Breaking, security.** A `user_defined` network policy that does not set
`opaque_tunnels` now refuses opaque tunnels. Before, it allowed them.

An opaque tunnel is a connection the egress proxy would carry without reading
it: one it neither terminates as TLS nor parses as HTTP. That covers raw bytes
written after an explicit-proxy `CONNECT`, an SSH session or a database wire
protocol, every SOCKS5 tunnel, and a TLS connection to a `pass_through` rule
that the proxy does not terminate. Allowing them by default let a sandboxed
process write arbitrary bytes, a secret sentinel included, to any allowed host
and port without the proxy inspecting them. With `refuse`, the proxy closes
such a connection with `403` (or a SOCKS general failure) and records an
`opaque_tunnel_refused` violation, so every byte that leaves is one it
inspected.

What changes:

- `OpaqueTunnelPolicy::default()` is `Refuse`, and a policy that omits
  `opaque_tunnels` reads as `"refuse"`.
- The field is now always serialized. A writer on this version never omits
  it, so an older reader that took an absent field to mean `allow` cannot widen
  a policy written by a newer one.
- `allow_all` and `deny_all` are unchanged.

Migration:

- Only traffic the proxy reads still flows by default: plain HTTP sent to the
  explicit proxy listener, and TLS the proxy terminates (a `transform` or
  `forward` rule, a secret binding for the host, or terminate-all with the
  sandbox CA trusted by the client). HTTPS relayed untouched to a
  `pass_through` host or an allow range, any connection on the transparent
  listener that is not terminated (plain HTTP included), SOCKS5, SSH and other
  non-HTTP protocols need `"opaque_tunnels": "allow"` in the `user_defined`
  policy.
- The E2B, Daytona and Vercel workers cannot enforce a refusal, so they reject
  a policy that refuses opaque tunnels with `InvalidNetworkPolicy`. A
  `user_defined` policy sent to them must now say `"opaque_tunnels": "allow"`.
- A stored policy written before this change without the field reads as
  `refuse` when it is loaded again.
- The broker's operation fingerprint covers the serialized request, so an
  operation id first submitted before the upgrade and retried after it with a
  `user_defined` policy is reported as a different request.
- `allowListPolicy` in `@trogonai/sandbox-ws-client` builds pass-through host
  rules, so allow-listed hosts now carry only TLS and HTTP. A raw
  `user_defined` policy can set `opaque_tunnels: 'allow'`.
- The `trogon-sandbox-policy-exec` settings key `refuseOpaqueTunnels` keeps its
  own default (`false`), which it writes into the policy explicitly.

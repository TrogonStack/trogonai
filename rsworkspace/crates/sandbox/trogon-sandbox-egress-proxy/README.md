# Sandbox egress proxy

`trogon-sandbox-egress-proxy` is the egress proxy that sits between a sandboxed process and the network. It
takes a `trogon_sandbox_contract::NetworkPolicy` and decides, per connection and per request, whether to allow
it, deny it, or rewrite it.

- Ingress accepts HTTP proxy `CONNECT`, plain HTTP, and SOCKS5 connections behind a proxy token, and sniffs the
  protocol when the client does not say.
- A destination matched by a pass-through rule is spliced byte for byte to the vetted upstream address. The
  resolver refuses loopback, private and link-local ranges unless the policy allows them.
- A destination matched by a transform rule has its TLS terminated with a per-process certificate authority and
  re-encrypted upstream. Each HTTP/1.1 request is parsed, checked against the rule's match rules, and has the
  credential for its binding substituted for the sentinel the sandbox holds.
- Credentials come from the credential broker through `BrokerCredentialSource`, which wraps
  `trogon-sandbox-credential-client`. AWS requests are re-signed with SigV4 and OIDC tokens are minted for the
  sandbox identity.
- Refused requests are reported to a `ViolationSink`, and a `Decider` can judge requests the static policy
  leaves open, over NATS or in process.
- `ProxyServer` embeds the proxy in a process. The `trogon-sandbox-egress-proxyd` binary runs it as a daemon
  configured from the environment.

TLS uses `rustls` with the `aws-lc-rs` provider.

The `fuzzing` feature exposes `fuzz_support`, entry points that feed untrusted bytes to the HTTP request
parsers. It is for fuzz targets and is not meant for production builds.

The cargo-fuzz targets live in `fuzz/`, outside the Cargo workspace, and need a nightly toolchain:
`cargo +nightly fuzz run proxied_head` or `cargo +nightly fuzz run chunked_body` from this directory.

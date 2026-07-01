---
number: "0015"
slug: rust-tls-library
status: accepted
date: 2026-07-01
---

# ADR 0015: Rust TLS Library

## Context

First-party Rust code needs one default TLS implementation for both the client
side (outbound HTTP, WebSocket, and other network clients) and the server side
(in-process HTTPS listeners). Without one default, TLS choices drift across
crates: some link [OpenSSL](https://openssl-library.org/) through
[`native-tls`](https://crates.io/crates/native-tls), others link
[rustls](https://github.com/rustls/rustls), and a single process can end up with
two independent TLS stacks, two certificate-trust models, and two sets of
security advisories to track.

The Rust ecosystem offers two mainstream backends:

- **[rustls](https://github.com/rustls/rustls)** is a pure-Rust TLS
  implementation. It is memory-safe by construction, stewarded by the
  [Internet Security Research Group (ISRG)](https://www.abetterinternet.org/)
  (the nonprofit behind [Let's Encrypt](https://letsencrypt.org/)) under its
  [Prossimo memory-safety program](https://www.memorysafety.org/initiative/rustls/),
  and has been through independent third-party security audits. Its default
  cryptographic provider is [aws-lc-rs](https://github.com/aws/aws-lc-rs), which
  offers a [FIPS 140-3](https://csrc.nist.gov/pubs/fips/140-3/final) validated
  mode; [`ring`](https://crates.io/crates/ring) is available as an alternative
  provider.
- **[OpenSSL](https://openssl-library.org/)** (reached through
  [`native-tls`](https://crates.io/crates/native-tls) or the
  [`openssl`](https://crates.io/crates/openssl) crate) is battle tested and
  ubiquitous, but it is C code with a long history of memory-safety CVEs, and it
  introduces a system `libssl` dependency into the build and runtime image.

The workspace has already converged on rustls in practice.
[`reqwest`](https://crates.io/crates/reqwest),
[`tokio-tungstenite`](https://crates.io/crates/tokio-tungstenite),
[`twilight-gateway`](https://crates.io/crates/twilight-gateway), and
[`opentelemetry-otlp`](https://crates.io/crates/opentelemetry-otlp) are all
configured with their rustls feature sets, and
[`rustls-pemfile`](https://crates.io/crates/rustls-pemfile),
[`rustls-pki-types`](https://crates.io/crates/rustls-pki-types), and
[`rustls-webpki`](https://crates.io/crates/rustls-webpki) are pinned as
workspace dependencies. No first-party crate depends on
[`native-tls`](https://crates.io/crates/native-tls) or
[`openssl`](https://crates.io/crates/openssl).

[ADR 0002](./0002-rust-crate-boundaries.md) makes shared cross-cutting concerns
the responsibility of dedicated reusable packages, and
[ADR 0007](./0007-configuration-sources.md) and
[ADR 0008](./0008-opentelemetry-observability.md) establish the pattern of
naming one default technology and treating anything else as a documented
exception.

## Decision

Prefer [rustls](https://github.com/rustls/rustls) as the TLS implementation for
first-party Rust code unless there is a strictly necessary reason to use
something else.

- Use rustls for both client and server TLS. For HTTP clients such as
  [`reqwest`](https://crates.io/crates/reqwest), enable the `rustls-tls` feature
  family rather than any [`native-tls`](https://crates.io/crates/native-tls) or
  [OpenSSL](https://openssl-library.org/) feature.
- Prefer the [aws-lc-rs](https://github.com/aws/aws-lc-rs) cryptographic
  provider for rustls, so that a
  [FIPS 140-3](https://csrc.nist.gov/pubs/fips/140-3/final) validated path stays
  available for future compliance requirements without a library change. Use the
  [`ring`](https://crates.io/crates/ring) provider only when a target platform
  or build constraint cannot support aws-lc-rs.
- For in-process HTTPS on an [`axum`](https://crates.io/crates/axum) service, use
  [`axum-server`](https://github.com/programatik29/axum-server) with its
  `tls-rustls` feature. It consumes the existing `axum::Router` directly and
  provides certificate hot-reload, which keeps rotation off the critical path.
- Load certificates and keys with the already-pinned rustls tooling
  ([`rustls-pemfile`](https://crates.io/crates/rustls-pemfile),
  [`rustls-pki-types`](https://crates.io/crates/rustls-pki-types),
  [`rustls-webpki`](https://crates.io/crates/rustls-webpki)) rather than
  introducing a parallel parsing stack.
- Do not introduce [`native-tls`](https://crates.io/crates/native-tls), the
  [`openssl`](https://crates.io/crates/openssl) crate, or a vendor TLS SDK as
  the primary TLS boundary when rustls can satisfy the requirement.

Prefer terminating TLS at the deployment edge (ingress, load balancer, or
service mesh) when the platform provides it, so that certificate lifecycle and
cipher policy stay centralized. In-process TLS is the deliberate exception, for
when the deployment does not terminate TLS in the path, when end-to-end
encryption with no plaintext hop is required, or when a compliance requirement
demands encryption into the process. When in-process TLS is used, it uses
rustls per this decision.

## Exceptions

Using something other than rustls requires a concrete constraint, not a
preference. Valid exceptions include:

- A required dependency only exposes a
  [`native-tls`](https://crates.io/crates/native-tls) or
  [OpenSSL](https://openssl-library.org/) interface and offers no rustls-backed
  option.
- A compliance or certification requirement mandates a specific validated
  OpenSSL module that the rustls plus
  [aws-lc-rs](https://github.com/aws/aws-lc-rs) path cannot satisfy.
- Interoperability with a peer requires a cipher suite, protocol extension, or
  legacy TLS behavior that rustls does not support.
- A target platform or build environment cannot support any available rustls
  cryptographic provider.

When an exception is used, isolate the non-rustls TLS to the boundary that
requires it, and document the exception at that package, service, or deployment
boundary. Do not let it become the default for unrelated code.

## Consequences

- A single process and build graph carries one TLS implementation and one
  certificate-trust model, not two.
- First-party binaries avoid a system `libssl` dependency, which reduces image
  size, attack surface, and OS-level [OpenSSL](https://openssl-library.org/)
  patching for those binaries.
- Code review should reject new
  [`native-tls`](https://crates.io/crates/native-tls) or
  [`openssl`](https://crates.io/crates/openssl) features on first-party crates
  unless they satisfy the exception rules and are documented.
- The pinned `rustls-*` workspace dependencies remain the shared toolkit for
  certificate handling.
- Choosing [aws-lc-rs](https://github.com/aws/aws-lc-rs) as the default provider
  keeps a [FIPS 140-3](https://csrc.nist.gov/pubs/fips/140-3/final) validated
  path open without a later library migration.
- Existing non-rustls TLS usage, if any is introduced through a dependency, can
  be migrated to rustls when touched for related work.

## References

- [ADR 0002: Rust Crate Boundaries](./0002-rust-crate-boundaries.md)
- [ADR 0007: Configuration Sources](./0007-configuration-sources.md)
- [ADR 0008: OpenTelemetry Observability](./0008-opentelemetry-observability.md)
- [rustls](https://github.com/rustls/rustls)
- [ISRG Prossimo: rustls initiative](https://www.memorysafety.org/initiative/rustls/)
- [Internet Security Research Group (ISRG)](https://www.abetterinternet.org/)
- [Let's Encrypt](https://letsencrypt.org/)
- [aws-lc-rs](https://github.com/aws/aws-lc-rs)
- [ring](https://crates.io/crates/ring)
- [axum](https://crates.io/crates/axum)
- [axum-server](https://github.com/programatik29/axum-server)
- [reqwest](https://crates.io/crates/reqwest)
- [tokio-tungstenite](https://crates.io/crates/tokio-tungstenite)
- [twilight-gateway](https://crates.io/crates/twilight-gateway)
- [opentelemetry-otlp](https://crates.io/crates/opentelemetry-otlp)
- [rustls-pemfile](https://crates.io/crates/rustls-pemfile)
- [rustls-pki-types](https://crates.io/crates/rustls-pki-types)
- [rustls-webpki](https://crates.io/crates/rustls-webpki)
- [native-tls](https://crates.io/crates/native-tls)
- [openssl (crate)](https://crates.io/crates/openssl)
- [OpenSSL](https://openssl-library.org/)
- [NIST FIPS 140-3](https://csrc.nist.gov/pubs/fips/140-3/final)
- [NIST Cryptographic Module Validation Program (CMVP)](https://csrc.nist.gov/projects/cryptographic-module-validation-program)

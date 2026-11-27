# Sandbox OCI client

`trogon-sandbox-oci` is the OCI registry client for sandbox artifacts and container
images. It pushes and pulls artifacts addressed by the contract crate's
`OciArtifactRef`, resolves container images addressed by `OciImageRef`, and moves
artifacts between registries and OCI image layouts on disk.

`OciClient` is built from an `OciClientConfig`. `OciClientConfig` lists the
registries that may be reached over plain HTTP as `RegistryHost` values, the
credentials to use per registry as `RegistryCredentials`, and the
`DockerConfigSource` that decides whether Docker client configuration is read from
the environment, from an explicit directory, or not at all. `RegistryHost` parses a
`host[:port]` string and returns `RegistryHostError` for anything else.

## Artifacts

`push_artifact` takes the `OciArtifactRef` the caller expects. The client computes
the manifest digest from the layers and annotations with `build_push_target` and
refuses to push when it differs, returning `OciError::DigestMismatch`. The result is
a `PushReceipt`. `pull_artifact` returns a `PulledArtifact` whose layers were
verified against their descriptors. `pull_artifact_bounded` additionally enforces
`ArtifactPullLimits` on the layer count, each layer's size, and the total size,
before any layer is downloaded. `head_manifest` returns a `ManifestSummary` without
downloading layers, and `delete_artifact`, `list_tags`, `list_referrers`, and
`attach_referrer` cover the rest of the lifecycle. Referrers use the registry's referrers API and fall back
to the tag schema when the registry does not provide one.

## Container images

`resolve_container_image` (and `resolve_container_image_with_limits`) pins an `OciImageRef` to a platform-specific manifest and
returns a `ResolvedContainerImage` holding the manifest digest, config digest, and
layer descriptors. `ContainerPullLimits` bounds manifest, config, layer count, and
layer sizes. Nested indexes are followed up to a fixed depth. `export_container_layout` writes the resolved image as an OCI layout, and
`resolve_image_readiness` reports an `ImageReadiness`.

## Bounded transport

Registry metadata requests use a transport that enforces the size limits while
streaming, before the end of the response. Manifests are limited to 1 MiB, token
responses to 64 KiB, and tag pages to 4 MiB. Metadata requests do not follow
redirects. Blob downloads follow at most five redirects and drop credentials when a
redirect leaves the original origin. Every request runs under a deadline. Basic and
Bearer challenges are answered with the configured credentials, and challenge
headers are bounded.

## Layouts

The `layout` module exports an artifact and its referrers with
`export_to_directory` or `export_to_tarball`, and `import_from_directory` pushes a
layout into a registry. Blobs are stored under
their verified digest, so exporting and importing preserves every digest.

## Metrics

`OciMetrics` registers `trogon_sandbox_oci_*` Prometheus counters and histograms for
pushed and pulled bytes, push and pull durations, and referrers fallbacks.
`OciMetrics::new` returns an error when a metric cannot be registered.

## Errors

Operations return `OciError`, which distinguishes network, not-found, unauthorized,
digest mismatch, unsupported media type, registry rejection, layout, configuration,
and invalid reference, digest, or registry failures.

## Fit

The crate depends on `trogon-sandbox-contract` for `Digest`, `ArtifactType`,
`OciArtifactRef`, and `OciImageRef`, and re-exports them. Tests run against an
in-process registry and never reach a real registry or the network.

# trogon-sandbox-cosign

Cosign signing and verification for sandbox snapshot artifacts stored in an OCI registry.

## What it does

- Signs a snapshot manifest and publishes the signature next to it: through the `cosign` CLI for KMS keys
  (`CosignSigner::AwsKms`), or in process for a local PEM key (`LocalKey`, `InMemoryKey`).
- Binds annotations, such as the environment digest (`EnvDigest::ANNOTATION_KEY`), into the signed payload.
- Verifies the signature layers of a snapshot against a PEM public key (`CosignVerifier`), and optionally requires
  that the same signature carries the expected annotations, so a registry cannot relabel a signed snapshot.
- Applies a `VerifyMode` (`off`, `warn`, `require`; no default, the caller states one) when a restore verifies a snapshot, through
  `enforce_restore_signature` and `apply_mode`.

## Main items

| Item | Role |
| --- | --- |
| `CosignSigner` | Signing identity: KMS URI, key file, or in-memory key. Secrets are held as `secrecy` values. |
| `CosignVerifier` | PEM public key plus the registries that may be reached over plain HTTP. |
| `attach_cosign_signature`, `attach_cosign_signature_with_annotations` | Sign a subject and attach the signature. |
| `verify_snapshot_signature`, `verify_snapshot_signature_with_annotations`, `verify_if_configured` | Verify a snapshot. |
| `VerifyMode`, `apply_mode`, `enforce_restore_signature` | Restore policy for failed verification. |
| `CosignError`, `SnapshotPullError`, `into_pull_error` | Failures, and their mapping to `LeaseError::SnapshotSignatureInvalid`. |
| `generate_test_key_pair` | Ephemeral ECDSA key pair for tests. |

## How it fits

The crate pushes and attaches manifests through `trogon-sandbox-oci` and uses the artifact, digest and lease error
types of `trogon-sandbox-contract`.

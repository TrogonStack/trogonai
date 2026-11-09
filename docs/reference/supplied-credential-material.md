# Supplied Credential Material

Status: proposed contract requirements. No new protobuf or runtime API is
implemented by this page. The existing prototype's credential write command
accepts the secret as a plain string. The shape below is a candidate
replacement to evaluate when finalizing the secrets-service boundary, not a
description of that prototype API.

For ownership and event-stream boundaries, see
[Provider Credential Custody](../architecture/provider-credential-custody.md)
and [Secret Management](../architecture/secret-management.md).

## Message responsibilities

- **Payload:** exact secret bytes and optional transfer-integrity information.
- **Typed material:** the credential structure and its domain validation, such
  as an API key. Authentication scheme and credential storage source are
  independent choices.
- **Write-only request:** authenticated resource placement, stable operation
  identity, and typed material. It travels only over the authorized live
  secrets ingress, never a durable command inbox.
- **Response and persisted metadata:** opaque reference, operation outcome, and
  safe version metadata. These types have no field for supplied material.
- **Protection policy:** authorized selection of managed encryption or a
  customer-managed key, separate from the credential bytes. Backend-observed
  key metadata is an output, not a caller's claim of successful protection.

## Candidate protobuf shape

This sketch shows the API-key material case. It deliberately omits final
package/import declarations, authenticated placement/reference types,
validation annotations, and responses. It is not a complete wire contract.

```proto
message SecretPayload {
  bytes data = 1;
  optional int64 data_crc32c = 2;
}

message ApiKeyMaterial {
  SecretPayload value = 1;
}

message PutCredentialMaterialRequest {
  string request_id = 1;

  oneof material {
    ApiKeyMaterial api_key = 2;
  }
}
```

A material `oneof` is useful when supported credential structures differ. A
dedicated API-key request also suffices if that is the only supported type.
Subscription presets do not establish a new material type. They populate
shared provider configuration and may select passthrough authentication, which
does not submit an API key through this operation. Do not invent OAuth, cloud
signing, or certificate fields from provider or preset names.

`bytes` preserves the submitted value. Protobuf JSON base64 is a transport
encoding, not encryption. Required presence, accepted encoding, nonempty API-key
material, maximum size, and provider-specific validation must be explicit in
the final contract. Do not silently trim a value or infer a mandatory key prefix
from a UI placeholder. Unknown material cases reject.

CRC32C is optional transfer-corruption detection. If retained, validate its
range and compare it with a checksum over the received bytes, preserving
absence separately from zero. It is not authentication, encryption, an
idempotency key, or a safe public fingerprint of a secret. Omitting it initially
does not change the custody boundary.

Replacing a version needs an explicit target reference and concurrency rule;
it must not silently overwrite an immutable material version. Do not overload
the initial creation operation with unspecified create-or-replace behavior.

## Retry and lifecycle requirements

Operation identity is scoped to the authenticated caller's authorized resource
and operation. Retrying equivalent input returns the same result. Reusing that
identity with conflicting material rejects. The secure comparison mechanism
belongs behind the secrets boundary; ordinary operation records and events
must not acquire raw checksums or unkeyed secret hashes for deduplication.

Persist nonsecret pending intent before the backend write and record activation
after its outcome is known. A backend write and event append are not a
distributed transaction. Recovery uses backend metadata to distinguish a write
that landed from one that needs resubmission. A lost response must not create a
duplicate version, and an ambiguous timeout must not trigger deletion of a
reference already attached to a binding. Reference attachment and cleanup
require authoritative ownership checks.

Replaying an aggregate reconstructs metadata. It never receives the supplied
payload, decrypts material, or retries the external write. No encrypted-value
field is permitted as a workaround for the plaintext exclusion.

Secret-bearing generated types require explicit diagnostic handling. A
write-only request name, a protobuf annotation, or separate response types do
not prevent a generic request logger, derived debug output, tracing interceptor,
or dead-letter handler from exposing input. Payloads are excluded from those
paths and held only for the bounded authorized operation.

## External-source references

External registration uses a separate secrets-service contract. Its approved
backend connection and provider-specific locator remain behind an opaque
reference issued to the provider control plane. The unresolved contract must
define version selection, access checks, rotation ownership, and behavior when
the external source disappears. Draft key-backend routing is not evidence of
an implemented external secret-storage adapter.

## Published API precedents

Google Secret Manager separates secret metadata creation from adding a secret
version. Its payload is bytes with an optional CRC32C checksum. This supports a
generic storage payload, but does not supply our credential-domain validation.
[Google payload protobuf](https://github.com/googleapis/googleapis/blob/master/google/cloud/secretmanager/v1/resources.proto),
[Google service protobuf](https://github.com/googleapis/googleapis/blob/master/google/cloud/secretmanager/v1/service.proto).

AWS `PutSecretValue` accepts either text or binary. Its client request token
deduplicates the same material and rejects a conflicting value for an existing
version identity. This is a useful retry precedent; automatically making a
write current is not a substitute for our separate validation and activation.
[AWS PutSecretValue](https://docs.aws.amazon.com/secretsmanager/latest/apireference/API_PutSecretValue.html).

Azure `Set Secret` accepts a string and optional `contentType`; setting an
existing secret creates another version. A descriptive content type does not
replace typed credential messages or validation.
[Azure Set Secret](https://learn.microsoft.com/en-us/rest/api/keyvault/secrets/set-secret/set-secret?view=rest-keyvault-secrets-2025-07-01).

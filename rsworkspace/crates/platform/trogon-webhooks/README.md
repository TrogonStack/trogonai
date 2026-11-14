# trogon-webhooks

Reusable authenticated HTTP ingress for dynamic webhook endpoints. Mount the
router in a gateway and consume its protobuf delivery events through NATS
JetStream.

- `EndpointLookup` supplies current validated endpoint configuration.
- `NatsEndpointRegistry` reads protobuf records from NATS KV.
- `WebhookVerifier` performs request authentication.
- `SecretWebhookVerifier` implements bearer and Standard Webhooks HMAC-SHA256
  authentication through a `WebhookSecretLookup` implementation.
- `EndpointRegistration::encode` produces registry values from validated
  configuration.

The receiver preserves request body bytes and publishes registry-owned identity
and revision metadata. Only a broker storage acknowledgement produces `202`.
The receiver does not start downstream executions.

See the [gateway webhook reference](../../../../docs/reference/gateway-webhooks.md)
for configuration, sender requirements, and the consumer contract.

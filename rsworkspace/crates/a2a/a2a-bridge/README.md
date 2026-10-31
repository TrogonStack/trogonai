# a2a-bridge

HTTPS ↔ NATS A2A sidecar foundation: typed caller identity value objects and the unified `BridgeError` surface that the inbound (HTTPS) and outbound (NATS / upstream HTTPS) wiring will build on in the next slices.

## Configuration

`AUTH_CALLOUT_XKEY_PUBLIC` is the public curve key of `a2a-auth-callout`. The
bridge seals every caller credential to it before the mint exchange crosses
NATS, so only the callout can read the credential. Without it the bridge
refuses every mint. Take it from the same bootstrap that hands the callout its
`AUTH_CALLOUT_XKEY_SEED`, because a credential sealed to any other key does
not open.

## Related

- [A2A bridge sketch (`docs/a2a/explanation/bridge-sketch.md`)](../../../../docs/a2a/explanation/bridge-sketch.md)
- [Gateway roadmap — bridge identity & policy](../../../../docs/a2a/explanation/gateway-roadmap.md)

# Integrate Connector HTTP Use

Use this guide when wiring a trusted Session supervisor or SDK to an implemented
connection relay. Follow the [binding contract](../reference/connector-binding-integration.md)
and [wire profile](../reference/connection-http-proxy.md); ordinary HTTPS_PROXY
configuration alone does not implement credential injection.

1. Resolve the agent's connection declarations at admission. Supply trusted
   binding selections for mode and explicit destination when needed. Refuse
   missing/ambiguous destinations; never infer an API host from token or
   discovery endpoints. Different origins need distinct declaration bindings
   and grants.
1. Prepare and activate the attempt-bound grant with the immutable binding and
   attested confirmation public key. Keep the private signing capability in
   the trusted supervisor. Agent code receives a host-bound client interface,
   not a provider token or the signing key.
1. Bind that client to the admitted declaration/destination. Reject caller
   Authorization, Cookie, relay-control, framing and transport-owned overrides.
   Apply the finite request limits and canonical path policy before signing.
1. Construct the exact relay target and context, digest the complete bounded
   body and sign every forwarded end-to-end header under RFC9421. The proxy
   independently checks live policy, grant, logical upstream egress, destination
   and credential role; a signed caller request is not policy authority.
1. Preserve provider HTTP status/body within the response policy and distinguish
   the relay-owned provenance/failure headers. Treat unsupported responses,
   timeouts and unknown outcomes as possible provider effects. Do not
   automatically mint a fresh invocation or replay a write after refresh.
1. Run the checked-in conformance artifacts and then independent production
   adapter tests, including cross-replica replay, custody denial, restore,
   parser ambiguity, response leakage and live revocation. Do not claim
   transparent interception or runtime isolation from a schema example.

# Connect or Reconnect a Provider

Use the generated browser client for the
[ConnectionControlService](../reference/connector-control-api.md) in a deployment
that has enabled these draft contracts. This guide describes the intended flow;
it does not identify a live gateway or a command-line tool.

1. Sign into the console and choose the authorized custody project. Select a
   reviewed ConnectorRelease and permitted scopes. The service determines the
   eligible ProviderApp; its project must match custody or it must be platform
   scoped. Initial rollout accepts project owners only.
1. Call StartConnect with the typed parent and a fresh mutation id. Preserve that
   id for recovery of this intent. Do not supply callback/session/state/PKCE,
   issuer, client material or capsule fields; trusted services own them.
1. Open the transient consent redirect returned on the first successful
   response. Keep it out of logs, analytics, clipboard history and durable
   client state. Consent must return to the same initiating browser session.
1. Read GetAuthorizationStatus until a terminal phase, within the configured
   polling budget. Status contains sanitized lifecycle facts, not another copy
   of the redirect or provider code. A lost redirect requires the documented
   cancel/restart flow; replaying the mutation does not persist secret-bearing
   redirect material for retrieval.
1. Resolve an existing-account conflict by reconnecting the existing authorized
   Connection. Never replace it, steal its claim on a timeout or remotely revoke
   the losing attempt's token family. Service cleanup destroys staged material.
1. For reconnect/step-up, read the current Connection status and call Reconnect
   with its typed name, observed revision, expected authorization generation and
   explicitly requested scopes. The ConnectionId and canonical account remain
   fixed; another account fails rather than switching the connection silently.
1. For disconnect, call Disconnect with the current revision and preserve its
   mutation receipt. Local use stops after the durable terminal intent; inspect
   cleanup status separately for provider revoke, material destruction and
   profile erasure. Those remote/local obligations are not instant success.

If a control response is lost, use GetControlRequest with the original intent's
identity and authenticated caller. A new mutation id is a new intent, not a way
to hide an ambiguous result. Permission, concurrency, expired-attempt and
unsupported-provider failures use the typed control error contract.

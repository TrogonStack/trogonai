# Add a Connector Provider

Use this procedure when preparing a new reviewed provider release. It targets
the draft contracts; a deployment must implement the services and pass
[connector conformance](../reference/connector-conformance.md) before activation.

1. Read current official provider authentication, API, identity, revoke and
   quota documentation. Record source date and exact profile versions. Keep app
   registration/approval work separate from local contract validation.
1. Choose the minimum provider scope and actor needed. Pin an exact issuer and
   declared OAuth endpoints, or the supported MCP discovery revision. Require
   S256 PKCE. Missing identity/refresh/revoke capabilities must have explicit
   unsupported behavior; never infer them from a successful HTTP request.
1. Define the fixed identity probe and declared string fields. Exclude email,
   profile URLs and unnecessary names. Send raw identity only through the
   [sealed custody operation](../reference/connector-account-identity.md).
1. Define exact API destinations and compatible credential roles independently
   of OAuth/probe egress hosts. Select a default explicitly or require a
   destination id. Add only enforceable HTTP restrictions; a GraphQL POST path
   is not a read-only policy.
1. Select the [finite transport profile](../reference/connector-http-transport.md)
   and a reviewed response policy. Pin the exact policy bytes by digest and
   require a registered implementation. Start with the
   [Linear candidate](../reference/connector-linear-profile.md) as an example,
   not a universal provider template.
1. Register a separate ProviderApp for the intended environment. The configured
   callback/CIMD origin is fixed; development never reuses the production app.
   Submit material through sealed custody management, never in a release JSON,
   domain event or log. Use the explicit platform/project parent and operator
   authority prescribed by the [security profile](../reference/connector-security-profile.md).
1. Compile and validate the definition, hash/store its exact bytes and publish a
   new immutable release through the authorized catalog management path. Review
   all declared hosts, roles, scopes and filter policy; do not patch an existing
   release or silently widen a customer's grant.
1. Run the provider enablement gate with the actual app and deployment. Preserve
   typed receipts and sanitized failure evidence; leave the release disabled
   until callback, consent, identity, refresh, revoke, response and recovery
   behavior are proved. Never mark an unsupported remote revoke successful.

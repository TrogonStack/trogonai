# Sandbox credential client

`trogon-sandbox-credential-client` is the typed NATS client for the sandbox credential broker. It sends
issue, revoke, and rotate requests for one credential provider and one tenant, and waits a bounded time for
each reply.

- `CredentialClient` is generic over `trogon_nats::RequestClient`, so it runs on a real NATS connection or a
  test double. It builds subjects with `trogon-sandbox-protocol` and speaks the request and reply types of
  `trogon-sandbox-contract`.
- `IssueParams` carries what an issue request asks for. The time to live is a `NonZeroU32`, so a zero value
  cannot be built.
- `CredentialClient::with_policy_fence` stamps every issue request with the lease and committed policy
  revision the client acts for. Without a fence a broker that requires one refuses the request.
- `CredentialClient::with_timeout` sets how long to wait for a reply (five seconds by default).
- `ClientError` separates a broker refusal, a timeout, a transport failure, and request or reply encoding
  failures.
- `RegistryAuth` turns an issued token into registry login credentials. Its `Debug` output hides the password.

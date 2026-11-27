# Sandbox SSH gateway

`trogon-sandbox-ssh-gateway` lets an SSH client reach a sandbox lease. The gateway
terminates SSH with russh and tunnels each session to the worker that holds the lease
over NATS, so the worker needs no inbound port.

## Login

The SSH user is `<lease>@gw` and the password is an API key. The gateway hashes the key,
resolves it in an `ApiKeyStore` (a JetStream key-value bucket in the binary), and
refuses the login when the key is unknown. A refused login does not say whether the key
or the lease was wrong.

Each login gets an ephemeral Ed25519 session key. The gateway asks the worker to attach
the public key for the lease (`SshAttachRequest`), and detaches it (`SshDetachRequest`)
when the last channel closes, the connection drops, or the gateway shuts down. When a
`DispatchSigner` is configured, attach and detach requests are signed. When a
`SessionKeyMint` is available, the gateway also registers the session with the credential
broker as a `CredentialScope::SshSession` token and revokes it when the session ends. A
failed revoke is logged and never keeps the session open.

## Tunnel

`SshTunnelBackend` is the gateway's view of the worker. `NatsSshTunnelBackend` is the
implementation: attach and detach are NATS requests, and each channel's bytes travel as
`SshStreamFrame` values on a per-session pair of subjects. A frame carries data, an
end-of-input marker, or an `SshStreamControl` message (open a shell or an exec channel,
resize, close). `SshSessionId` is parsed once and every subject is built from it.

## Worker side

`SshStreamBridge` answers attach and detach and runs the sessions for a worker. It turns
the frames of a session into `TerminalRequest` values for an `SshTerminalHost`, and turns
the terminal's `TerminalOutput` back into frames. It enforces session, channel, idle,
time-to-live, and buffered-byte limits (`BridgeLimits`). `SshStreamBridge::serve` is the
ready-made NATS responder for a worker with no attach handling of its own.

`TerminalHub` implements the terminal side over a `GuestShell`, the SSH client of the
guest. `start_pty_channel` and `PinnedHostKey` connect to the guest with a pinned host
key and pump a pty channel.

## Binary

`trogon-sandbox-ssh-gateway` serves SSH until Ctrl+C.

| Flag | Environment | Default |
| --- | --- | --- |
| `--listen` | | `0.0.0.0:2222` |
| `--nats-url` | `NATS_URL` | required |
| `--api-key-bucket` | `SANDBOX_API_KEY_BUCKET` | `sandbox-api-keys` |
| `--credential-broker-url` | `SANDBOX_CREDENTIAL_BROKER_URL` | the NATS server above |
| `--ssh-pull-secret-id` | `SANDBOX_SSH_PULL_SECRET_ID` | `ssh-gateway` |
| `--ssh-gateway-tenant` | `SANDBOX_SSH_GATEWAY_TENANT` | `platform` |
| `--host-key-path` | | `data/trogon-sandbox-ssh-gateway/host_key` |
| `--max-connections` | | `100` |

The dispatch signing key comes from `SANDBOX_DISPATCH_SIGNING_KEY` or
`SANDBOX_DISPATCH_SIGNING_KEY_FILE`; without one, attach and detach are sent unsigned
and the gateway logs a warning. The host key is read from `--host-key-path`, or
generated and written there on first start. When the credential broker cannot be
reached, sessions run without registered tokens and the gateway logs a warning.

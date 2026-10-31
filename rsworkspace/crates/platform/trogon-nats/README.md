# trogon-nats

Zero-cost `NatsClient` trait over `async_nats::Client` for testability, with connection management, messaging helpers, and mock clients.

## Environment

These variables accept the same values as the `nats` CLI.

- `NATS_URL`: comma-separated server list. Defaults to `localhost:4222`.
- `NATS_USER` and `NATS_PASSWORD`: user/password authentication.
- `NATS_CREDS`: credentials file, a path or `file://path`.
- `NATS_NKEY`: an NKey seed, a seed file as a path or `file://path`, or `env://NAME` naming the variable that holds the seed. A seed file may be plain or decorated (a `.creds` file works); the first line starting with a seed prefix is used, and the file is read at connect time.
- `NATS_JWT` and `NATS_SEED`: user JWT signed with the seed. `NATS_JWT` alone sends an empty signature.
- `NATS_TOKEN`: token.

Authentication resolves in this order: `NATS_CREDS`, `NATS_NKEY`, `NATS_USER` plus `NATS_PASSWORD`, `NATS_TOKEN` (or `NATS_USER` set without `NATS_PASSWORD`), `NATS_JWT` (signed when `NATS_SEED` is set), then no auth.

`NATS_JWT`, `NATS_SEED`, `NATS_PASSWORD` and `NATS_TOKEN` are literals unless they start with `file://path` (file contents, trailing newlines trimmed) or `env://NAME` (value of another variable). The `op`, `nsc` and `data` schemes, and any scheme on a path setting other than `file`, fail at connect time with `ConnectError::InvalidSetting`.

`NatsSettings` holds the raw values and is the single place the precedence is decided, so environment variables, CLI flags and config files resolve identically.

## Connecting

`connect` returns before any server answers and keeps dialing in the background, then reconnects whenever the connection drops, so a service can start before NATS does. The first dial goes out at once and later attempts back off up to 30 seconds.

`connect_once` makes a single pass over the servers and fails once that pass does, for a connection that serves one request. It takes the authentication to use in place of the configured one, so each request can present its caller's token; give that token as `CredentialValue::Literal` so it is never read as a `file://` or `env://` reference. Both apply the same TLS settings. A connection from `connect_once` that drops redials at most once per configured server and then closes for good, so it cannot go on dialing with the caller's token after its request is over.

Both fail with `ConnectError::NoServers` when `NATS_URL` names no server, such as `,`.

The message of `ConnectError::ConnectionFailed` never names the servers, because callers pass error text on to remote clients. The error keeps them in its `servers` field, and `connect` and `connect_once` log them with the `Failed to connect to NATS` warning.

## TLS

Every connection uses TLS and verifies the server against the system roots. Nothing turns TLS on; configuration can only turn it off.

- `NATS_CA`: a private CA certificate, a path or `file://path`. It replaces the system roots rather than adding to them, so the server has to present a certificate issued by this CA.
- `NATS_CERT` and `NATS_KEY`: a client certificate and its key for mutual TLS, each a path or `file://path`. Setting only one of them fails the connection.
- `NATS_TLS_DISABLED`: disables TLS when set to `true`, `1` or `yes`, in any letter case. This is only for tests and local debugging, and `connect` and `connect_once` log a warning whenever it is used. An empty value counts as unset. Any other value fails the connection, and so does combining it with `NATS_CA`, `NATS_CERT` or `NATS_KEY`, both with `ConnectError::InvalidSetting`.

Only the environment can set `NATS_TLS_DISABLED`. `NatsSettings` has no field for it, so no CLI flag or config file can disable TLS.

trogon-nats builds async-nats on aws-lc-rs, and `connect` installs aws-lc-rs as the process-wide rustls provider before it dials unless the binary installed one first. Both are needed: rustls panics on the first handshake when ring and aws-lc-rs are both compiled in and no provider is installed, and dependencies such as reqwest, hyper-rustls and bollard can still bring ring in.

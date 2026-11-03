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
- `NATS_CERT` and `NATS_KEY`: client certificate and key. Setting only one of them fails the connection.
- `NATS_CA`: root CA certificate.

Authentication resolves in this order: `NATS_CREDS`, `NATS_NKEY`, `NATS_USER` plus `NATS_PASSWORD`, `NATS_TOKEN` (or `NATS_USER` set without `NATS_PASSWORD`), `NATS_JWT` (signed when `NATS_SEED` is set), then no auth.

`NATS_JWT`, `NATS_SEED`, `NATS_PASSWORD` and `NATS_TOKEN` are literals unless they start with `file://path` (file contents, trailing newlines trimmed) or `env://NAME` (value of another variable). The `op`, `nsc` and `data` schemes, and any scheme on a path setting other than `file`, fail at connect time with `ConnectError::InvalidSetting`.

Setting any TLS variable requires TLS on the connection.

`NatsSettings` holds the raw values and is the single place the precedence is decided, so environment variables, CLI flags and config files resolve identically.

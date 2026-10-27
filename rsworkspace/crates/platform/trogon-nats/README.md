# trogon-nats

Zero-cost `NatsClient` trait over `async_nats::Client` for testability, with connection management, messaging helpers, and mock clients.

## Environment

These variables are read the same way as by the `nats` CLI.

- `NATS_URL`: comma-separated server list. Defaults to `localhost:4222`.
- `NATS_USER` and `NATS_PASSWORD`: user/password authentication.
- `NATS_CREDS`: credentials file, a path or `file://path`.
- `NATS_NKEY`: NKey seed file, plain or decorated (a `.creds` file works), as a path or `file://path`. The first line starting with a seed prefix is used, and the file is read at connect time.
- `NATS_JWT` and `NATS_SEED`: user JWT signed with the seed. `NATS_JWT` alone sends an empty signature.
- `NATS_TOKEN`: token.
- `NATS_CERT` and `NATS_KEY`: client certificate and key. Both are required.
- `NATS_CA`: root CA certificate.

Authentication resolves in this order: `NATS_USER` plus `NATS_PASSWORD`, `NATS_CREDS`, `NATS_NKEY`, `NATS_JWT` (signed when `NATS_SEED` is set), then no auth.

A token is applied on top of the chosen method. It is `NATS_USER` when that is set without `NATS_PASSWORD` (overriding `NATS_TOKEN`), otherwise `NATS_TOKEN`.

`NATS_JWT`, `NATS_SEED`, `NATS_PASSWORD` and `NATS_TOKEN` are literals unless they start with `file://path` (file contents, trailing newlines trimmed) or `env://NAME` (value of another variable). The `op`, `nsc` and `data` schemes, and any scheme on a path setting other than `file`, fail at connect time with `ConnectError::InvalidSetting`.

Setting any TLS variable requires TLS on the connection.

`NatsSettings` holds the raw values and is the single place the precedence is decided, so environment variables, CLI flags and config files resolve identically.

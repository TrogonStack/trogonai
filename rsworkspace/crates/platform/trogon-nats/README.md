# trogon-nats

Zero-cost `NatsClient` trait over `async_nats::Client` for testability, with connection management, messaging helpers, and mock clients.

## Environment

- `NATS_URL`: comma-separated server list. Defaults to `localhost:4222`.
- `NATS_CREDS`: credentials file path.
- `NATS_NKEY`: NKey value.
- `NATS_USER` and `NATS_PASSWORD`: user/password authentication. Both values are required.
- `NATS_TOKEN`: token authentication.

Authentication resolves in this order: `NATS_CREDS`, `NATS_NKEY`, `NATS_USER` plus `NATS_PASSWORD`, `NATS_TOKEN`, then no auth.

# Running OpenBao Locally

A single-node [OpenBao](https://openbao.org) for local development, standing in
for the platform-operated OpenBao that
[ADR#0023](../../../../../docs/adr/0023-secret-management-and-key-custody-direction.md)
describes. It stores everything in the compose `postgres` service, so its state
survives `docker compose restart` and disappears with `docker compose down -v`.

> **This is not a deployment model.** The listener serves plaintext HTTP, the
> root key sits under a checked-in static seal key, and a fixed root token is
> handed out on startup. Nothing here should be carried into an environment
> holding real key material.

## Quick start

```bash
cd devops/docker/compose
docker compose up -d openbao-bootstrap
```

Name `openbao-bootstrap`, not `openbao`. The server is uninitialized and sealed
until bootstrap runs against it, and because the dependency points from
bootstrap to the server, starting `openbao` alone never triggers it (the
gateway waits for bootstrap to complete, so `trogon-gateway` does). Bootstrap
pulls in `openbao` and `postgres` as its own dependencies, then runs to
completion: on a fresh database it initializes the server, installs the fixed
dev root token, mounts a transit key, and mounts the KV v2 store the gateway
reads. Every step is skipped only when the
thing it creates is already there, so re-running it reconciles rather than
duplicates, and a run killed partway finishes on the next `up`. A plain
`docker compose up` starts the whole stack and needs no special casing.

The service publishes no host port. OrbStack routes to the container directly,
so reach it by its container DNS name:

```bash
export BAO_ADDR=http://openbao.trogonai.orb.local:8200
export BAO_TOKEN=local-dev-root-token

bao status
bao read transit/keys/local-dev-kek
```

The UI is at <http://openbao.trogonai.orb.local:8200/ui>.

## What bootstrap sets up

| Thing | Default | Override |
|---|---|---|
| Root token | `local-dev-root-token` | `OPENBAO_DEV_ROOT_TOKEN` |
| Transit mount | `transit/` | `OPENBAO_DEV_TRANSIT_MOUNT` |
| Transit key (`aes256-gcm96`) | `local-dev-kek` | `OPENBAO_DEV_TRANSIT_KEY` |
| KV v2 mount | `secret/` | `OPENBAO_DEV_KV_MOUNT` |

`aes256-gcm96` is the key type the key-custody design needs, because it is the
type whose transit `associated_data` carries the platform's authenticated
binding ([ADR#0030](../../../../../docs/adr/0030-customer-controlled-key-backend-routing.md)
Decision 2).

The mount is created with `disable_upsert=true`, which OpenBao leaves off by
default. Registration rejects a transit mount that leaves upsert enabled, so
the local mount is shaped like one a customer deployment must present
([ADR#0030](../../../../../docs/adr/0030-customer-controlled-key-backend-routing.md)
Decision 5). A side effect worth knowing: encrypting under a key name that does
not exist yet now fails instead of quietly creating the key.

Verify the round trip:

```bash
bao write -field=ciphertext transit/encrypt/local-dev-kek \
  plaintext="$(printf 'hello' | base64)"
```

## Storage

OpenBao gets its own database on the shared postgres instance, created when
missing by the `openbao-database` one-shot service running
`services/postgres/ensure-openbao-database.sh`. It runs on every `up`, so a
postgres volume that predates OpenBao needs no manual step. The name defaults to
`openbao` and both the connection and the provisioning read the same
`OPENBAO_POSTGRES_DB`, so overriding it stays consistent.

Both connect over TLS with `sslmode=verify-full` against the dev CA that
`pki-bootstrap` issues into the `dev_secrets` volume, and postgres rejects any
network client that does not use TLS (`services/postgres/pg_hba.conf`).

## Auto-unseal

`seal "static"` keeps the root key under a symmetric key read from
`BAO_STATIC_SEAL_KEY`, so the server unseals itself on every start and no
developer ever handles an unseal share. `bao operator init` still runs once
against a fresh database; with an auto-unseal seal it returns recovery keys
rather than unseal keys, and bootstrap discards them because a local database is
not worth recovering.

To use a different key, set `OPENBAO_STATIC_SEAL_KEY` to the base64 of exactly
32 bytes:

```bash
openssl rand -base64 32
```

Changing the key on an initialized database leaves the server unable to decrypt
its own root key. Reset with `docker compose down -v` when that happens.

# Deploy NATS

**State: no deployed NATS exists.** This guide is the procedure a deployment
follows to run one. [nats-server.conf](nats-server.conf) is the configuration
it runs, with placeholders for the server name, paths and limits.
[`devops/docker/compose/services/nats`](../docker/compose/services/nats/) runs
the same shape for local development and generates all of its material on the
first `up`.

Every procedure here preserves these properties of a deployed server:

- Clients connect over TLS 1.3 only, and each presents a client certificate the
  server verifies against its CA.
- An account exists only once the operator has signed it and it has been
  pushed to the server's full resolver.
- JetStream encrypts its store with ChaCha20-Poly1305 under a key the
  deployment provides.

## Before you start

- `nsc` and the `nats` CLI on the machine allowed to hold the operator signing
  seed. The `natsio/nats-box` image carries both, and `jq`.
- `openssl`, for the JetStream key.
- A CA that issues the server certificate and a client certificate for every
  process that connects, including the one that pushes account JWTs.
- A place in OpenBao for the material listed in
  [Where production keys live](#where-production-keys-live).

## Bootstrap the operator

Run this once per environment. `--require-signing-keys` makes the server
reject any account or user JWT signed by an identity key, so identity keys
stay offline.

```sh
nsc add operator --name trogonai --sys --generate-signing-key
nsc edit operator --require-signing-keys
```

`--sys` also creates `SYS`, the system account, with a signing key and its
`sys` user. `sys` pushes account JWTs and reads server state. No workload ever
receives SYS credentials, and SYS never gets application users.

Create the account that hosts the auth callout and the account platform
workloads connect to. Each gets a signing key, which is what issues its users:

```sh
for account in AUTH PLATFORM; do
  nsc add account --name "$account"
  nsc edit account --name "$account" --sk generate
done
nsc edit account --name PLATFORM \
  --js-mem-storage 1g --js-disk-storage 50g --js-streams -1 --js-consumer -1
```

The JetStream limits are placeholders. Size them for the deployment together
with `max_memory_store` and `max_file_store` in nats-server.conf. AUTH needs
no JetStream.

Issue a user and its credentials for every workload that connects to PLATFORM,
and credentials for `sys`:

```sh
for workload in trogon-gateway decider-host a2a-gateway a2a-bridge acp-nats mcp-nats channel-bridge-telegram; do
  nsc add user --account PLATFORM --name "$workload"
  nsc generate creds --account PLATFORM --name "$workload" --output-file "$workload.creds"
done
nsc generate creds --account SYS --name sys --output-file sys.creds
```

Configure AUTH for the auth callout. The callout's own user bypasses the
callout, and `--curve generate` creates the xkey the server encrypts callout
requests to. nsc keeps the xkey seed in its keystore under the xkey's public
key.

```sh
nsc add user --account AUTH --name auth-callout
nsc generate creds --account AUTH --name auth-callout --output-file auth-callout.creds
nsc edit authcallout --account AUTH \
  --auth-user "$(nsc describe user --account AUTH --name auth-callout --json | jq -r .sub)" \
  --curve generate
```

`a2a-auth-callout` cannot serve this role yet. It connects with a user and
password rather than credentials, pins a server key that nats-server
regenerates on every start, and signs user JWTs whose issuer is not a signing
key of the target account. Until it can, nothing connects with AUTH
credentials except the callout's own user. PLATFORM and tenant users
authenticate with their own credentials and never reach the callout.

Move every `.creds` file and every seed into OpenBao as
[Where production keys live](#where-production-keys-live) describes, then
delete the local copies. `nsc env` shows where nsc keeps them: the store
directory holds the JWTs, and the keystore (`$NKEYS_PATH`) holds the seeds.

## Render the server files

nats-server.conf reads everything environment-specific from
`/etc/nats/secrets`. The deployment renders these files there from OpenBao
before the server starts and mounts the directory read-only:

| File | Contents | Produced by |
| --- | --- | --- |
| `operator.jwt` | The operator JWT | `nsc describe operator --raw` |
| `identity.conf` | The system account's public key and its JWT, preloaded so the server trusts SYS before any push | The snippet below |
| `jetstream-key.conf` | `key: "<64 hex characters>"`, the store encryption key | `openssl rand -hex 32`, generated once per store |
| `server.pem`, `server-key.pem` | The server certificate chain and key, with a SAN for every name clients dial | The deployment's CA |
| `ca.pem` | The CA bundle that verifies client certificates | The deployment's CA |

```sh
sys=$(nsc describe account --name SYS --json | jq -r .sub)
cat > identity.conf <<EOF
SYSTEM_ACCOUNT: "$sys"
SYSTEM_ACCOUNT_PRELOAD: {
  $sys: "$(nsc describe account --name SYS --raw)"
}
EOF
```

Render `identity.conf` again whenever the SYS JWT changes. Check the result
before the server starts:

```sh
nats-server -t -c /etc/nats/nats-server.conf
```

## Onboard an account

An account is signed with nsc and pushed to the running server, which keeps it
in its resolver directory. Onboarding needs no restart and no configuration
change.

```sh
nsc add account --name ACME
nsc edit account --name ACME --sk generate
nsc edit account --name ACME \
  --js-mem-storage 256m --js-disk-storage 10g --js-streams -1 --js-consumer -1
```

When the auth callout will place users in the account, add it to AUTH's
allowed accounts:

```sh
nsc edit authcallout --account AUTH \
  --allowed-account "$(nsc describe account --name ACME --json | jq -r .sub)"
```

Push the account as `sys`, and push AUTH too whenever its callout settings
changed:

```sh
for account in ACME AUTH; do
  nats --server tls://nats.example.internal:4222 \
    --tlsca ca.pem --tlscert pusher.pem --tlskey pusher-key.pem --creds sys.creds \
    request '$SYS.REQ.CLAIMS.UPDATE' "$(nsc describe account --name "$account" --raw)" \
    --raw --timeout 5s
done
```

Each reply carries `"code":200` and `"message":"jwt updated"` under `data`.
Anything else means the server rejected the JWT, and the reply says why.

`nsc push` does the same. It takes a `nats://` URL, and the connection is
still TLS because the server accepts nothing else:

```sh
nsc push --account ACME --account-jwt-server-url nats://nats.example.internal:4222 \
  --system-user sys --ca-cert ca.pem --client-cert pusher.pem --client-key pusher-key.pem
```

Issue the account's users the same way as PLATFORM's, then confirm the server
serves the account by connecting as one of them:

```sh
nsc add user --account ACME --name acme-worker
nsc generate creds --account ACME --name acme-worker --output-file acme-worker.creds
nats --server tls://nats.example.internal:4222 \
  --tlsca ca.pem --tlscert client.pem --tlskey client-key.pem --creds acme-worker.creds \
  account info
```

Every later change to an account, such as its limits, signing keys or user
revocations, goes out the same way: edit it with nsc, then push it.

## Where production keys live

Production NATS material lives in OpenBao. The deployment reads it with its
own deployment-attested identity and renders it into each consumer's
filesystem before that process starts. It never goes through the
`SecretStore` or `KeyManagement` ports: NATS is on the cold-start path to the
secrets service, so its keys are platform boot keys
([ADR#0033](../../docs/adr/0033-key-custody-product-model.md), Decision 5,
draft), provisioned under the
[ADR#0007](../../docs/adr/0007-configuration-sources.md) bootstrap rule. No
workload fetches its own NATS material. The secrets service is the only
OpenBao client
([ADR#0023](../../docs/adr/0023-secret-management-and-key-custody-direction.md),
Decision 3), and even its NATS credentials come from the deployment.

| Material | Used by | Rendered to |
| --- | --- | --- |
| Operator identity seed | Changes to the operator JWT itself, such as adding or revoking an operator signing key | Nowhere. Strict signing-key mode rejects accounts it signs |
| Operator signing seed | nsc, to create and edit accounts | The machine running nsc, for that session only |
| SYS signing seed | nsc, to issue `sys` | The machine running nsc, for that session only |
| `sys.creds` and a client certificate | Pushing account JWTs and reading server state | The pusher |
| Account identity seeds | Nothing once the account has a signing key | Nowhere. Strict signing-key mode rejects users they sign |
| Account signing seeds | nsc, to issue users, and the auth callout once it signs users in operator mode | The machine running nsc, and later the callout. They are platform boot keys for tenant accounts too, never tenant-tier keys |
| Workload `.creds`, client certificate and key | Each workload, through `NATS_CREDS`, `NATS_CERT` and `NATS_KEY` | That workload only |
| Callout xkey seed | The auth callout | The callout |
| Server certificate and key, CA bundle | nats-server | `/etc/nats/secrets` |
| JetStream key | nats-server | `/etc/nats/secrets/jetstream-key.conf` |
| Operator and account JWTs | nats-server and nsc | Not secret. The server keeps pushed account JWTs in its resolver directory |

Keep all of it outside `secret/data/trogonai/{owner_id}/credentials/`.
[`gateway_read`](../openbao/policies/gateway-read.hcl) grants read on that
whole tree, so a NATS seed stored there is readable by every holder of that
policy.

Keep nsc's store directory along with its seeds. A server that loses its
resolver directory knows only SYS until every account JWT is pushed again, and
the store directory is where those JWTs come from.

## Rotate the JetStream key

The key rotates in place. Restart the server with the new key as `key` and the
key it replaces as `prev_key`, and nats-server re-encrypts every stream under
the new key on start. A new `key` without `prev_key` leaves every existing
stream unreadable.

nats-server 2.14.2 drops every durable consumer during that conversion: each
one fails to restore with `cipher: message authentication failed` and the
server removes it, while the messages stay. Pick the path by whether the
consumers must keep their state. Each path is verified on a single server and
untested on a cluster.

### Rotate in place and create the consumers again

1. Stop every client of the server.
2. While the server still runs under the old key, save every durable consumer,
   connected as a user of its account:

   ```sh
   nats --server tls://nats.example.internal:4222 \
     --tlsca ca.pem --tlscert client.pem --tlskey client-key.pem --creds platform-user.creds \
     consumer info <stream> <consumer> --json > <stream>-<consumer>.json
   ```

3. Stop the server. In `jetstream-key.conf`, set `key` to the new key and
   `prev_key` to the old one, then start the server.
4. Check that the log shows `Restored <n> messages for stream` for every
   stream, and `Error adding consumer` for every consumer the server removed.
5. In each saved file, set `config.deliver_policy` to `by_start_sequence` and
   `config.opt_start_seq` to one more than `ack_floor.stream_seq`, so the
   consumer does not replay the stream. Then create it again as the same user:

   ```sh
   nats --server tls://nats.example.internal:4222 \
     --tlsca ca.pem --tlscert client.pem --tlskey client-key.pem --creds platform-user.creds \
     consumer add <stream> --config <stream>-<consumer>.json
   ```

6. Stop the server, remove `prev_key` from `jetstream-key.conf`, and start the
   server. Then start the clients.
7. Destroy the old key and the saved files.

### Rotate with a backup and keep the consumers

1. Stop every client of the server.
2. While the server still runs under the old key, back up each account that
   has JetStream, connected as a user of that account:

   ```sh
   nats --server tls://nats.example.internal:4222 \
     --tlsca ca.pem --tlscert client.pem --tlskey client-key.pem --creds platform-user.creds \
     backup account ./backup/PLATFORM --consumers -f
   ```

3. Stop the server. Move `/data/jetstream` aside, leave `/data/jwt` (the
   resolver directory) in place, and render the new key into
   `jetstream-key.conf`.
4. Start the server and restore each account as the same user:

   ```sh
   nats --server tls://nats.example.internal:4222 \
     --tlsca ca.pem --tlscert client.pem --tlskey client-key.pem --creds platform-user.creds \
     backup restore account ./backup/PLATFORM
   ```

5. Compare `nats stream report` and `nats consumer report <stream>` with the
   state before the backup. Messages, consumers and their ack floors carry
   over. Then start the clients.
6. Destroy the backup and the old store. A backup holds the messages in clear,
   because snapshots do not carry the store encryption, so protect it like the
   key until it is gone.

### When the server starts under the wrong key

If the server ever starts under the wrong key, it logs
`Error decrypting our stream metafile` and leaves those streams unloaded but on
disk. Stop it before any client connects and restore the right key. A client
that creates one of those streams in the meantime, as trogon-gateway does at
startup, makes the server delete the stream's directory, and the right key
cannot bring it back.

A lost key is a lost store. Keep the key for as long as anything written under
it must stay readable.

## Keep TLS on

TLS is never disabled in a deployed environment. nats-server.conf has no
plaintext listener, and a deployment never adds one:

- No `allow_non_tls`, no `http_port`, and no websocket `no_tls`. Monitoring
  stays on `https_port`.
- `https_port` does not ask for a client certificate, so keep it off client
  networks.
- A cluster, gateway or leafnode block gets its own `tls` block with
  `verify: true`, and a websocket or MQTT block gets its own `tls` block. Route
  and gateway certificates carry both the serverAuth and clientAuth extended
  key usages, because every server also dials its peers.
- A load balancer or mesh in front of the server passes TCP through and never
  terminates TLS, because the server has to see the client certificate.
- Clients connect with a `tls://` URL and the trogon-nats variables
  `NATS_CA`, `NATS_CERT`, `NATS_KEY` and `NATS_CREDS`. A deployment never sets
  anything that turns client TLS off.
- When a certificate does not verify, fix the certificate. There is no
  plaintext fallback to reach for.

## Open decisions

- How the deployment renders NATS material from OpenBao, and the mount and
  paths that hold it.
- `a2a-auth-callout` in operator mode. Until it signs users for tenant
  accounts, tenants connect with users issued by nsc.
- Subject permissions per workload. PLATFORM users are issued without publish
  or subscribe restrictions.

## See also

- [ADR#0007: Configuration Sources](../../docs/adr/0007-configuration-sources.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](../../docs/adr/0023-secret-management-and-key-custody-direction.md)
- [ADR#0033: Key Custody Product Model](../../docs/adr/0033-key-custody-product-model.md)
- [Credential platform runbooks](../openbao/runbooks/README.md)

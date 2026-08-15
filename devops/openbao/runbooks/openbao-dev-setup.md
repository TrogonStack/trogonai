# Runbook: OpenBao Dev Setup

Bring up an OpenBao a developer or CI job can write credentials to.

**Customer-visible impact:** none. This is local only.

## Start it

```bash
cd devops/docker/compose
docker compose up -d openbao
```

The compose service runs `openbao/openbao:2.5.5` in `-dev` mode. Dev mode
means in-memory storage, a single unseal share held by the server itself,
and a fixed root token (`OPENBAO_DEV_ROOT_TOKEN`, default `dev-only-token`).
Everything is lost on restart, which is the point: no dev secret survives
to be mistaken for a real one.

The gateway service in the same compose file already points at it through
`OPENBAO_ADDR=http://openbao:8200`. From the host, OrbStack resolves
`http://openbao.trogonai.orb.local:8200`.

## Verify it

```bash
docker compose exec openbao bao status -address=http://127.0.0.1:8200
```

`Sealed false` and `Storage Type inmem` confirm dev mode.

Round-trip a value through the same mount the platform uses:

```bash
docker compose exec openbao \
  bao kv put -mount=secret trogonai/manual/roundtrip value=hello
docker compose exec openbao \
  bao kv get -format=json -mount=secret trogonai/manual/roundtrip
```

## Apply the policies

Dev mode gives you a root token, so nothing enforces the role split by
default. To test against the real policies:

```bash
cd devops/openbao/policies
./verify.sh
```

`verify.sh` is self-contained: it starts its own dev server on port 18201,
applies all five policies, mints one token per role, and runs the full
assertion matrix against real percent-encoded credential paths. It does not
touch the compose stack. Use it after any policy edit; a policy that parses
and uploads cleanly can still deny every real request, which is exactly the
failure it was written to catch.

To apply the policies to the compose instance instead:

```bash
cd devops/openbao/policies
for policy in *.hcl; do
  name="${policy%.hcl}"
  docker compose -f ../../docker/compose/compose.yml exec -T openbao \
    bao policy write "${name//-/_}" - < "$policy"
done
```

## Point the gateway at it

The compose stack does this already. For a gateway running outside compose:

```bash
export OPENBAO_ADDR=http://openbao.trogonai.orb.local:8200
export OPENBAO_TOKEN=dev-only-token
```

## Integration tests

The testcontainer-backed tests start their own OpenBao and need no running
stack:

```bash
mise exec -- cargo test -p trogon-gateway openbao_testcontainer_roundtrips_precise_value
```

The tests that talk to an already-running server are `#[ignore]` by
default:

```bash
OPENBAO_ADDR=http://openbao.trogonai.orb.local:8200 OPENBAO_TOKEN=dev-only-token \
  mise exec -- cargo test -p trogon-gateway openbao_dev_server_roundtrips_precise_value -- --ignored --nocapture
```

## When it will not start

- **Port 8200 already bound.** Another OpenBao is running. Find it with
  `docker ps`, and stop it with `docker stop`, not `kill -9`; the container
  is OrbStack-managed.
- **Gateway healthcheck failing but OpenBao healthy.** The gateway waits on
  the OpenBao healthcheck, so this is the gateway's own problem. Check
  `docker compose logs trogon-gateway`.
- **Reads return 403 with a non-root token.** Almost certainly the policy
  path shape, not the token. See [../policies/README.md](../policies/README.md).

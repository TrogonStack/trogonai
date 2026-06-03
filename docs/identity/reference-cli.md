# CLI reference — `agctl` and `trogon-gateway-ctl`

v0.1 ships two operator CLIs:

- **`agctl`** — admin client. Inspect registered MCP servers, policy
  bundles, stream audit, query traffic.
- **`trogon-gateway-ctl`** — gateway-local control plane. Config dump,
  bundle validation, policy dry-run, audit tail.

Both binaries are shipped as release artifacts (not in the Helm chart).

## `agctl`

```
agctl registry sync <args>     Validate signed agent manifests pre-commit.

agctl traffic query …          Query indexed traffic events in a window.
agctl traffic tail …           Tail recent traffic events for a tenant.

agctl mcp servers list         List registered MCP servers.
agctl mcp servers get <id>     Show one server's record.
agctl mcp policies list        List policy bundles in the KV store.
agctl mcp policies show <id>   Show the policy applied to a server.
agctl mcp audit tail           Subscribe to gateway audit subjects.
agctl mcp health               Report MCP gateway health.
```

Output format is JSON by default; pass `--format=text` on subcommands
that accept it for human-readable output.

## `trogon-gateway-ctl`

```
trogon-gateway-ctl config show [--format=json|yaml]
        Dump the resolved gateway configuration.

trogon-gateway-ctl bundle validate <path> --trusted-keys <path>
        Verify a policy bundle's NKey signatures.

trogon-gateway-ctl policy dry-run --policy <path> --input <path>
        Evaluate a policy against a captured request and print the
        decision without writing audit.

trogon-gateway-ctl audit tail [--max <n>]
        Tail the local audit stream and print one envelope per line.
```

Both CLIs read `NATS_URL` and `NATS_CREDS` from the environment.

## Upstream parity gap

Upstream `agentgateway` ships `config all`, `config backends`, and
`trace <request_id>` subcommands. The Trogon analogues are:

- `config all` → `trogon-gateway-ctl config show`.
- `config backends` → not implemented; queryable today via
  `agctl mcp servers list`.
- `trace <request_id>` → out-of-band via your OTel trace store
  (see `docs/identity/reference-traces.md`); a `trogon-gateway-ctl
  trace` shortcut lands with the v0.2 admin API.

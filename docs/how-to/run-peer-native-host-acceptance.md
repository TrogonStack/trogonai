# Run native peer host acceptance

Run this opt-in fixture locally when you need evidence from the installed host.
It makes model calls and must stay outside ordinary CI. It requires Docker and
an installed `claude` executable with working existing authentication and native
`mcp_tool` hook support. Inspect its current help before running the fixture.
Do not supply a different provider, model, or authentication profile.

If the host reports an expired OAuth session, run `claude auth login` in your
terminal to restore the existing login before trying again. Use a new evidence
directory after authentication succeeds. An authentication failure establishes
no callback or context insertion, even when the plugin connected successfully.

From the repository root, create an ignored evidence parent and run:

```sh
mkdir -p .trogonai/analysis/native-host
cd rsworkspace
PEER_ACCEPTANCE_EVIDENCE="$PWD/../.trogonai/analysis/native-host/attempt-01" \
  mise exec -- cargo test -p trogon-peer-mcp --test native_host_acceptance \
  installed_host_next_prompt_context -- --ignored --exact --nocapture --test-threads=1
```

Choose a new attempt directory for every run. The fixture refuses an existing
directory, preserving earlier evidence. Runtime identifiers, host events and
backend receipts remain in ignored files with the `.internal.trogonai.md` suffix.
Keep this evidence private and never commit it or copy private host settings.

The fixture starts the real connector through the published plugin and reuses
the TLS/nkey-authenticated broker and application test-support ports. Only the
trusted fixture owns privileged service credentials and reconciliation. Hosts
receive caller-scoped credentials without storage access. It disables built-in
tools, excludes user/project/local settings and checks the host's reported MCP
and model tool inventories. It deliberately omits `--strict-mcp-config`, whose
documented restriction to explicit MCP configuration can exclude plugin servers.

The workload is bounded to persistent host processes with at most two prompts
each and `--max-budget-usd 2` per process. It uses the host's default model and
existing authentication. Every turn has a fixed deadline. Failure stops the
flow; repair a demonstrated prerequisite before authorizing another attempt.
An authentication or broker setup failure is not a behavioral RED.

Initial prompts must bind native identities without model-issued hook calls.
The sender discovers the recipient and retries an identical keyed send. The
fixture checks durable acceptance and an empty recipient inbox before trusted
intake. The recipient's next prompt asks for peer context without containing
the fresh marker. Its model response must report that marker, and durable state
must contain the assigned batch. Application acknowledgment is recorded only
when present in backend state; echoing the marker does not manufacture one.
Sender settlement is observed around a separate trusted reconciliation call.

Review the retained host events alongside backend state. A connected MCP server,
successful package validation, accepted send, or prepared batch alone establishes
no host insertion. The fixture uses a static trusted test clock, disposable
content and presentation ports, and an ephemeral broker. It establishes no lease
timing, crash durability, production sealing, full CI or coverage threshold.

See the [connector reference](https://github.com/TrogonStack/trogonai/blob/f79784cdbc412209668c858ce990e117579c78a0/rsworkspace/crates/mcp/trogon-peer-mcp/README.md)
and [native plugin package](https://github.com/TrogonStack/trogonai/blob/f79784cdbc412209668c858ce990e117579c78a0/plugins/claude/peer/README.md).

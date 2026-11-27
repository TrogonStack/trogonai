# Sandbox language server client

`trogon-sandbox-lsp` is a language server client for servers running inside a sandbox. The language server is started
as a detached command with interactive stdin, and JSON-RPC frames travel over stdin and the sandbox evidence stream.
The crate knows nothing about languages: all language specifics are data in `LspPreset` and `LspServerSpec`.

```rust,ignore
let lsp = LspServer::in_sandbox(
    client,
    lease_id,
    LspPresetName::TypeScript,
    ProjectDir::new("/workspace/project")?,
    LspServerOptions::default(),
);
lsp.start().await?;
let main = DocumentPath::new("main.ts")?;
lsp.did_open(&main, source, None).await?;
let symbols = lsp.document_symbols(&main).await?;
let completions = lsp.completions(&main, Position::new(5, 8)).await?;
lsp.stop().await;
```

## Notes

- The language server binary and its dependencies must already be installed in the sandbox, and the sandbox needs
  network access if the installation happens there.
- `start` and `stop` are idempotent. `stop` runs the LSP shutdown sequence, then escalates from closing stdin to TERM
  and finally KILL.
- Output is ordered by per-stream sequence numbers. Duplicates are dropped and a gap in stdout closes the connection.
- Stdin writes honor the sandbox backpressure responses and retry with backoff.
- Custom servers use `LspServerSpec`, and other transports implement `LspLauncher` and `LspTransport`.

## Example

```sh
NATS_URL=nats://localhost:4222 SANDBOX_API_KEY=... SANDBOX_TENANT=... \
  cargo run -p trogon-sandbox-lsp --example typescript
```

It leases a Node sandbox, installs the TypeScript language server, and prints the symbols and completions for a small
file.

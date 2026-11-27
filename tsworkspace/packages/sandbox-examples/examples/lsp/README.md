# Language Server

Run the TypeScript language server inside a sandbox, then ask it for the symbols in a file and the completions at a cursor position.

## Run

```bash
node examples/lsp/index.ts
```

## Notes

- `createLspServer` takes a preset name (`typescript` or `python`) or a spec with `command`, `args` and `languageId`. The language server binary must already be installed in the sandbox; this example installs it with `npm`, which needs network access from the sandbox, so the example sets `networkPolicy: 'allow-all'`. TypeScript is pinned to 5.9.3 because the language server needs a `tsserver.js`, which TypeScript 7 does not ship.
- File paths passed to `didOpen`, `documentSymbols` and `completions` are relative to the project directory.
- Positions are zero-based, as in the Language Server Protocol.
- The server runs as a detached command with interactive stdin, so it stays up until `stop()` or until the sandbox lease ends. Only the `local_docker` provider supports interactive stdin today, so the example pins it with `providerPref`.
- `stop()` sends `shutdown` and `exit`, then kills the process if it does not exit in time.

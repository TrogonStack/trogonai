# Process Execution

Run code and shell commands in an isolated sandbox and check their exit codes.

## Run

```bash
node examples/process-execution/index.ts
```

## Notes

- There is no language-aware `code_run`. Run code by invoking the interpreter, here `python3 -c`, inside an image that ships it.
- `sandbox.runCommand` does not accept a timeout. For `timeoutSeconds`, call the lower-level `client.runCommand`, which returns the raw wire response with `exit_code` and `stdout` bytes.
- For long-running commands with live output, pass `detached: true, streamOutput: true` to `runCommand` and iterate `command.stream()`.

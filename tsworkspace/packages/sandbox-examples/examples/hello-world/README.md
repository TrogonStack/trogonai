# Hello, World

Create a sandbox, run a shell command inside it, print the output, and delete the sandbox.

## Run

```bash
node examples/hello-world/index.ts
```

## Notes

- `runCommand` takes the program and its arguments separately. Wrap the command in `sh -c` to use shell syntax such as quoting or pipes.
- `Sandbox.create` defaults to the `alpine:latest` image. The broker denies a lease with an empty `providerPref`, so the example pins `local_docker`; see [Prerequisites](../../README.md#prerequisites) for which providers can run commands.
- `delete()` removes the sandbox permanently. Use `stop()` to keep it resumable.

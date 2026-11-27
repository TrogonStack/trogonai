# Git Integration

Clone a repository into a sandbox and pull the latest changes.

## Run

```bash
node examples/git/index.ts
```

## Notes

- There is no dedicated Git API. The example runs the `git` binary through `runCommand`, so the image must ship it; `buildpack-deps:bookworm-scm` does. The platform default (`alpine:latest`) does not, so it is not used for coding work.
- The sandbox needs egress to `github.com`. If the lease has a network policy, allow that host.

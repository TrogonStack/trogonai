# File System Operations

List, create, write, search, edit, inspect, move, and delete files in a sandbox.

## Run

```bash
node examples/file-system/index.ts
```

## Notes

- `writeFiles` and `readFile` are native operations. The `sandbox.fs` helpers (`ls`, `mkdir`, `chmod`, `stat`, `mv`, `rm`) run coreutils inside the sandbox.
- There is no workspace-root, content-search, or find-and-replace API. The example uses `pwd`, `grep`, `find`, and `sed` through `runCommand`.
- `fs.chmod` takes a numeric mode, so write it as an octal literal such as `0o755`.

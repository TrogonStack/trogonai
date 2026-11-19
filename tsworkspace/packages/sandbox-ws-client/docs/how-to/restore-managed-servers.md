# Restore managed servers

Keep a server started by `Sandbox.startServer` usable after a snapshot restore or a client reconnect. Use a worker that supports snapshots and port readiness. The image needs `bash` or `nc` for port checks, and `curl` or `wget` if you request an HTTP path check.

The snippets run inside your application's lease-management code. `client` is connected, `source` is a prepared `Sandbox`, `tenant` is a validated `TenantId`, and `replacementEnvironment` holds fresh environment values supplied by your application.

## Restore from a snapshot receipt

Start the server through `startServer`. The server program and its dependencies must already exist in the sandbox. A snapshot receipt captures the registered restart intent alongside the provider's snapshot reference. Pass the entire receipt to `Sandbox.fromSnapshot`:

```ts no-check
await source.startServer({
  cmd: 'node',
  args: ['server.mjs'],
  cwd: '/work',
  port: 3000,
  name: 'web',
  preview: false,
});

const storedReceipt = JSON.stringify(await source.snapshot());
const restored = await Sandbox.fromSnapshot(client, JSON.parse(storedReceipt), {
  tenant,
  providerPref: ['local_docker'],
  env: replacementEnvironment,
});
try {
  const result = await restored.runCommand('node', ['--version']);
  result.throwIfFailed();
} finally {
  await restored.delete();
}
```

`fromSnapshot` starts registered servers and waits for their configured port and HTTP path before it returns. If preview URLs were enabled, it resolves fresh URLs for the restored sandbox. Servers stopped with `server.stop()` or started with `restartOnResume: false` are omitted from the receipt.

Keep the receipt in storage controlled by your application. The SDK does not read executable restart instructions from guest files. Receipts without `clientState` still restore their files without starting servers.

## Handle a failed restore

When a later server fails to initialize, restoration rolls back only the unnamed executions that this attempt launched. It preserves adopted processes, named executions that may have been reused, and the sandbox when its grant was reused or had unknown ownership. Rollback waits for confirmed termination, with a bounded cleanup deadline.

The primary thrown value is preserved. Extensible errors receive `restoreCleanup`, containing `stoppedExecutionIds` and `incompleteExecutionIds`. When cleanup remains incomplete or that context is unavailable, inspect or stop the affected executions before retrying. A stop acknowledgement alone is not termination evidence.

## Supply replacement environment values

Restart plans contain environment variable names and command parameters. They do not contain environment values, abort signals or literal redaction values. Pass replacements for the required names when creating a restored lease; missing values are refused before the lease request. Registered redaction values embedded in command arguments cannot be exported in a restart plan.

A plan is limited to 32 servers and 64 KiB. Unknown fields, unsupported versions and oversized plans are rejected.

## Reconnect a client

Save `source.exportClientState()` in your application's trusted storage. When reconnecting, supply that state explicitly:

```ts no-check
const clientState = source.exportClientState();
const attached = await Sandbox.connect(client, source.sandboxId, {
  clientState,
  env: replacementEnvironment,
});
await attached.resume();
```

`Sandbox.get(client, { name, clientState, env })` accepts the same options. `resume()` and `wake()` restart registered servers only when their execution is gone. A plain name or ID lookup does not recover another client's server intent. Keep using `try/finally` for cleanup on native Node 22; `await using` requires a supporting runtime or compiler.

Known disk-only snapshots, including native Vercel, Modal and Fly snapshots, replay registered servers in a newly created sandbox. Firecracker OCI snapshots and E2B native snapshots preserve execution state; other formats remain unclassified unless their semantics are known. Those formats need authoritative discovery that identifies a surviving process or confirms the original execution exited before a restart. Unavailable, malformed or ambiguous discovery is refused. Absence from an inventory alone does not prove that a preserved process exited.

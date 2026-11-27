# Build and use the default agent image

Coding workloads need Git, a compiler toolchain and everyday shell tools. Choose an image explicitly, because the default Alpine image does not provide that toolchain.

The supplied recipe at `devops/docker/images/sandbox-agent/Dockerfile` builds the agent image. Read it for the exact package set and the user it runs as before you depend on it.

## Build and load the image

From the repository root:

```sh
docker build --tag sandbox-agent:dev \
  --tag registry.example.com/sandbox/agent:node22 \
  devops/docker/images/sandbox-agent
```

Build or load this image on the Docker daemon the worker uses. `sandbox-agent:dev` is the tag the local Docker smoke stack and the Docker worker expect by default. Registry distribution is a separate operator step, so select a verified tag or digest when you deploy the image to another worker. See [Add a registry pull secret](./add-a-pull-secret.md) when your registry requires credentials.

Selecting the image in each lease makes its toolchain and provenance explicit.

## Use the image

The client needs the gateway URLs, an API key and a tenant. Load them from the environment and keep the key out of logs:

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';

const client = await SandboxWsClient.connect({
  gatewayHttpUrl: process.env.SANDBOX_GATEWAY_HTTP_URL!,
  gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL!,
  apiKey: process.env.SANDBOX_API_KEY!,
});
try {
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId(process.env.SANDBOX_TENANT!),
    providerPref: ['local_docker'],
    image: {
      kind: 'registry',
      reference: { registry: 'registry.example.com', repository: 'sandbox/agent', tag: 'node22' },
    },
  });
  try {
    const command = await sandbox.runCommand({ cmd: 'git', args: ['--version'] });
    process.stdout.write(command.stdout());
  } finally {
    await sandbox.delete();
  }
} finally {
  client.close();
}
```

Replace the registry, repository and tag with the image you pushed. The client package is described in [Sandbox foundation](../../architecture/sandbox-foundation.md).

## Use an existing image

A supplied image must contain the tools the workload needs. Any image the provider can pull works, provided it has what your commands call.

## Related

- [Run a headless browser](./run-a-headless-browser.md) for the browser capability.
- [Provider option matrix](../../reference/sandbox/provider-option-matrix.md) for provider image boundaries.

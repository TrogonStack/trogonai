import { MemorySandboxClient, Sandbox, tenantId } from '../../src/index.ts';
import type { ProviderKind } from '../../src/index.ts';

const providers: ProviderKind[] = ['fly', 'proxmox'];
const client = new MemorySandboxClient();

void Sandbox.create(client, {
  tenant: tenantId('consumer'),
  providerPref: providers,
  requiredCapabilities: { needs_file_ops: true, needs_terminal: true },
});

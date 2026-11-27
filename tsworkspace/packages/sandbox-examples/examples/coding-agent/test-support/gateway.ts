import { MemorySandboxClient, Sandbox, SandboxWsClient } from '@trogonai/sandbox-ws-client';
import type { LeaseRequest } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

import { withFakeRepo } from '../memory-repo.ts';

const mode = z.enum(['writable-root', 'no-connect']).parse(process.env.CODING_CLI_FIXTURE);
const send = (event: Record<string, unknown>): void => {
  process.send?.(event);
};
const client = withFakeRepo(new MemorySandboxClient({ commands: { pwd: () => ({ exitCode: 0, stdout: '/\n' }) } }));
const lease = client.lease.bind(client);
client.lease = async (
  request: LeaseRequest,
  ...options: Parameters<typeof lease> extends [unknown, ...infer O] ? O : never
) => {
  send({ type: 'lease', providers: request.provider_pref });
  return lease(request, ...options);
};
SandboxWsClient.connect = async () => {
  send({ type: 'connect' });
  if (mode === 'no-connect') throw new Error('gateway was reached before provider configuration was validated');
  return client as unknown as SandboxWsClient;
};
const getter = Object.getOwnPropertyDescriptor(Sandbox.prototype, 'git')!.get!;
Object.defineProperty(Sandbox.prototype, 'git', {
  configurable: true,
  get(this: Sandbox) {
    const git = getter.call(this) as Sandbox['git'];
    return new Proxy(git, {
      get(target, key) {
        if (key === 'clone')
          return async (options: Parameters<typeof git.clone>[0]) => {
            send({ type: 'clone', path: options.path });
            if (!options.path?.startsWith('/tmp/'))
              throw new Error('EACCES: non-root cannot clone into the image root');
            return target.clone(options);
          };
        const value = Reflect.get(target, key);
        return typeof value === 'function' ? value.bind(target) : value;
      },
    });
  },
});
const remove = Sandbox.prototype.delete;
Sandbox.prototype.delete = async function (this: Sandbox, ...args: Parameters<typeof remove>) {
  send({ type: 'delete' });
  return remove.apply(this, args);
};

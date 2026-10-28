import { CredentialBindingLifecycle, ProviderConnectionLifecycle } from '@trogonai/proto';
import { CircleCheckIcon, ClockIcon, DropletsIcon, PlugZapIcon } from 'lucide-react';

import { Stat, StatStrip, StatStripSkeleton } from '@/components/stat-strip';

import type { ConnectionView } from '../domain/view';

export function ConnectionStats({ connections }: { connections: readonly ConnectionView[] }) {
  const count = (lifecycle: ProviderConnectionLifecycle) =>
    connections.filter((connection) => connection.lifecycle === lifecycle).length;
  const draining = connections
    .flatMap((connection) => connection.bindings)
    .filter((binding) => binding.lifecycle === CredentialBindingLifecycle.DRAINING).length;
  const pinned = connections
    .flatMap((connection) => connection.bindings)
    .reduce((total, binding) => total + binding.pinnedSessions, 0);
  const revoked = count(ProviderConnectionLifecycle.REVOKED);

  return (
    <StatStrip label="Connection summary">
      <Stat
        label="Connections"
        value={connections.length}
        caption={revoked > 0 ? `${revoked} revoked` : 'None revoked'}
        icon={PlugZapIcon}
        led="[--led:var(--primary)]"
      />
      <Stat
        label="Active"
        value={count(ProviderConnectionLifecycle.ACTIVE)}
        caption="Serving requests"
        icon={CircleCheckIcon}
        led="[--led:var(--success)]"
      />
      <Stat
        label="Pending validation"
        value={count(ProviderConnectionLifecycle.PENDING)}
        caption="Awaiting activation"
        icon={ClockIcon}
        led="[--led:var(--warning)]"
        blink
      />
      <Stat
        label="Draining bindings"
        value={draining}
        caption={`${pinned} pinned session${pinned === 1 ? '' : 's'}`}
        icon={DropletsIcon}
        led="[--led:var(--info)]"
      />
    </StatStrip>
  );
}

export const ConnectionStatsSkeleton = StatStripSkeleton;

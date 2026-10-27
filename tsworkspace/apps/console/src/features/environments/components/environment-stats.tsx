import { AlertTriangleIcon, LayersIcon, LockIcon, VaultIcon } from 'lucide-react';

import { Stat, StatStrip, StatStripSkeleton } from '@/components/stat-strip';

import { summarizeEnvironments, type EnvironmentView } from '../domain/view';

export function EnvironmentStats({ environments }: { environments: readonly EnvironmentView[] }) {
  const summary = summarizeEnvironments(environments);
  return (
    <StatStrip label="Environment summary">
      <Stat
        label="Environments"
        value={summary.total}
        caption={`${summary.builtIn} built-in, ${summary.custom} custom`}
        icon={LayersIcon}
        led="[--led:var(--primary)]"
      />
      <Stat
        label="In use"
        value={summary.inUse}
        caption="Holding at least one vault"
        icon={VaultIcon}
        led="[--led:var(--success)]"
      />
      <Stat
        label="Unused"
        value={summary.unused}
        caption="Nothing is placed here"
        icon={LockIcon}
        led="[--led:var(--info)]"
      />
      <Stat
        label="Needs attention"
        value={summary.needsAttention}
        caption={summary.needsAttention === 0 ? 'Nothing to act on' : 'Credentials across environments'}
        icon={AlertTriangleIcon}
        led="[--led:var(--danger)]"
        blink={summary.needsAttention > 0}
      />
    </StatStrip>
  );
}

export const EnvironmentStatsSkeleton = StatStripSkeleton;

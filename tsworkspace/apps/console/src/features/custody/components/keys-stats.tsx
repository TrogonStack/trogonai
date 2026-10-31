import { KeySquareIcon, PauseCircleIcon, ServerIcon, ShieldAlertIcon } from 'lucide-react';

import { Stat, StatStrip, StatStripSkeleton } from '@/components/stat-strip';

import type { CustodyView } from '../domain/view';

export function KeysStats({ stats }: { stats: CustodyView['stats'] }) {
  return (
    <StatStrip label="Encryption keys summary">
      <Stat
        label="Backends"
        value={stats.backends}
        caption="Registered key backends"
        icon={ServerIcon}
        led="[--led:var(--info)]"
      />
      <Stat
        label="Paused"
        value={stats.pausedBackends}
        caption={stats.pausedBackends === 0 ? 'Every backend is admitted' : 'The platform makes no call to these'}
        icon={PauseCircleIcon}
        led="[--led:var(--warning)]"
        blink={stats.pausedBackends > 0}
      />
      <Stat
        label="Bound keys"
        value={stats.keys}
        caption="Dedicated and customer managed"
        icon={KeySquareIcon}
        led="[--led:var(--success)]"
      />
      <Stat
        label="Keys denied"
        value={stats.deniedKeys}
        caption={stats.deniedKeys === 0 ? 'Every key is usable' : 'Paused, disabled or destroyed'}
        icon={ShieldAlertIcon}
        led="[--led:var(--danger)]"
        blink={stats.deniedKeys > 0}
      />
    </StatStrip>
  );
}

export const KeysStatsSkeleton = StatStripSkeleton;

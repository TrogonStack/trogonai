import { ActivityIcon, RadioIcon, ShieldXIcon, TriangleAlertIcon } from 'lucide-react';

import { Stat, StatStrip, StatStripSkeleton } from '@/components/stat-strip';

import { summarizeSources, type SourceView } from '../domain/view';

export function SourcesStats({ sources }: { sources: readonly SourceView[] }) {
  const overview = summarizeSources(sources);
  return (
    <StatStrip label="Sources summary">
      <Stat
        label="Integrations"
        value={overview.total}
        caption="Configured sources"
        icon={RadioIcon}
        led="[--led:var(--info)]"
      />
      <Stat
        label="Receiving"
        value={overview.receiving}
        caption="Verifying deliveries"
        icon={ActivityIcon}
        led="[--led:var(--success)]"
      />
      <Stat
        label="Needs attention"
        value={overview.needsAttention}
        caption={overview.needsAttention === 0 ? 'Nothing to act on' : 'Degraded or misconfigured'}
        icon={TriangleAlertIcon}
        led="[--led:var(--warning)]"
        blink={overview.needsAttention > 0}
      />
      <Stat
        label="Rejected deliveries"
        value={overview.rejected24h}
        caption="Last 24 hours"
        icon={ShieldXIcon}
        led="[--led:var(--danger)]"
      />
    </StatStrip>
  );
}

export const SourcesStatsSkeleton = StatStripSkeleton;

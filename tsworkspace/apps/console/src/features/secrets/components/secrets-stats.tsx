import { ArrowLeftRightIcon, HourglassIcon, KeyRoundIcon, TriangleAlertIcon } from 'lucide-react';

import { Stat, StatStrip, StatStripSkeleton } from '@/components/stat-strip';

import { summarize, type CredentialView } from '../domain/view';

export function SecretsStats({ credentials }: { credentials: readonly CredentialView[] }) {
  const summary = summarize(credentials);
  return (
    <StatStrip label="Secrets summary">
      <Stat
        label="Active credentials"
        value={summary.activeCredentials}
        caption={`${credentials.length} in total`}
        icon={KeyRoundIcon}
        led="[--led:var(--success)]"
      />
      <Stat
        label="Open overlaps"
        value={summary.openOverlaps}
        caption="Webhook rotation windows"
        icon={ArrowLeftRightIcon}
        led="[--led:var(--info)]"
      />
      <Stat
        label="Writes in flight"
        value={summary.writesInFlight}
        caption="Awaiting activation"
        icon={HourglassIcon}
        led="[--led:var(--warning)]"
        blink={summary.writesInFlight > 0}
      />
      <Stat
        label="Needs attention"
        value={summary.needsAttention}
        caption={summary.needsAttention === 0 ? 'Nothing to act on' : 'Open a credential to resolve'}
        icon={TriangleAlertIcon}
        led="[--led:var(--danger)]"
        blink={summary.needsAttention > 0}
      />
    </StatStrip>
  );
}

export const SecretsStatsSkeleton = StatStripSkeleton;

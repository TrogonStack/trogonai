import { CheckIcon } from 'lucide-react';

import { FaultAlert } from '@/components/fault-alert';
import { StatusDot, StatusPill } from '@/components/lifecycle-badge';
import { Button } from '@/components/ui/button';
import { useNotify } from '@/components/ui/toaster';
import { cn } from '@/lib/utils';

import { versionStateLabels, versionStateTones } from '../domain/labels';
import type { CredentialView, VersionRole, VersionView } from '../domain/view';
import { useEndOverlap } from '../queries';
import { RelativeTime } from './relative-time';
import { countdown, formatCompact, formatTimestamp, useNow } from './time';

const roleLabels: Record<VersionRole, string> = {
  CURRENT: 'Current',
  OVERLAP: 'Previous, still accepted',
  CANDIDATE: 'Candidate',
  RETIRED: 'Retired',
  REJECTED: 'Rejected',
  DESTROYED: 'Destroyed',
};

function Moment({ label, date }: { label: string; date: Date | undefined }) {
  if (!date) return null;
  return (
    <span className="text-muted-foreground">
      {label} <RelativeTime date={date} />
    </span>
  );
}

function OverlapRow({ credential, version }: { credential: CredentialView; version: VersionView }) {
  const now = useNow();
  const endOverlap = useEndOverlap();
  const notify = useNotify();
  const expiresAt = version.overlapExpiresAt;
  if (!expiresAt) return null;
  return (
    <div className="border-info/30 bg-info-soft text-info-ink mt-2 flex flex-wrap items-center justify-between gap-2 rounded-md border px-2.5 py-1.5 text-xs">
      <span>
        Both versions verify webhooks until{' '}
        <time dateTime={expiresAt.toISOString()} title={formatTimestamp(expiresAt)} className="font-mono">
          {formatCompact(expiresAt)}
        </time>
        . Remaining{' '}
        <span role="timer" aria-label="Overlap remaining" className="font-mono font-semibold tabular-nums">
          {countdown(expiresAt, now)}
        </span>
      </span>
      <Button
        type="button"
        variant="outline"
        size="xs"
        disabled={!credential.actions.endOverlap.available || endOverlap.isPending}
        onClick={() =>
          endOverlap.mutate(
            { secret: credential.secret, expectedRevision: credential.revision },
            {
              onSuccess: () => notify.success('Overlap ended', `Version ${version.version} no longer verifies`),
              onError: (error) => notify.failure('Could not end the overlap', error.message),
            },
          )
        }
      >
        End overlap now
      </Button>
      {endOverlap.error ? <FaultAlert error={endOverlap.error} /> : null}
    </div>
  );
}

export function VersionLadder({ credential }: { credential: CredentialView }) {
  if (credential.versions.length === 0) {
    return <p className="text-muted-foreground text-[13px]">No version exists yet. The first write is in progress.</p>;
  }
  return (
    <ol aria-label="Version ladder" className="flex flex-col">
      {credential.versions.map((version, index) => {
        const current = version.role === 'CURRENT';
        return (
          <li key={version.version} className="relative flex gap-3 pb-4 last:pb-0">
            {index < credential.versions.length - 1 ? (
              <span aria-hidden="true" className="bg-border absolute top-4 bottom-0 left-[5px] w-px" />
            ) : null}
            <span className="relative mt-1.5 flex size-3 shrink-0 items-center justify-center">
              <StatusDot
                tone={current ? 'success' : versionStateTones[version.state]}
                dim={version.role === 'DESTROYED' || version.role === 'REJECTED'}
                blink={version.role === 'CANDIDATE'}
              />
            </span>
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-sm font-semibold tabular-nums">v{version.version}</span>
                {current ? (
                  <span className="micro border-primary/50 bg-primary/15 text-primary-ink inline-flex h-5 items-center gap-1 rounded-sm border px-1.5 text-[10px]">
                    <CheckIcon className="size-3" aria-hidden="true" />
                    Current
                  </span>
                ) : (
                  <span
                    className={cn(
                      'micro text-muted-foreground text-[10px]',
                      version.role === 'OVERLAP' && 'text-info-ink',
                    )}
                  >
                    {roleLabels[version.role]}
                  </span>
                )}
                <StatusPill
                  tone={versionStateTones[version.state]}
                  struck={version.role === 'DESTROYED'}
                  label={versionStateLabels[version.state]}
                />
              </div>
              <p className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs">
                <Moment label="Observed" date={version.observedAt} />
                <Moment label="Activated" date={version.activatedAt} />
                <Moment label="Retired" date={version.retiredAt} />
                <Moment label="Destroyed" date={version.destroyedAt} />
              </p>
              {version.role === 'OVERLAP' ? <OverlapRow credential={credential} version={version} /> : null}
            </div>
          </li>
        );
      })}
    </ol>
  );
}

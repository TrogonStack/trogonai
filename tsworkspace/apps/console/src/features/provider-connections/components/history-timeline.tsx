import type { ProviderConnectionEvent } from '@trogonai/proto';

import { toneVariable, type Tone } from '@/components/lifecycle-badge';
import { cn } from '@/lib/utils';

import type { StoredEvent } from '../client';
import { replacementModeLabels } from '../domain/labels';

function describe(event: ProviderConnectionEvent): { title: string; detail?: string; tone: Tone } {
  const body = event.event;
  switch (body.case) {
    case 'registered': {
      const registration = body.value.registration;
      return {
        title: 'Registered',
        tone: 'info',
        detail: `Initial binding ${registration?.initialBinding?.id?.value ?? ''} pending validation`,
      };
    }
    case 'activated':
      return {
        title: 'Activated',
        tone: 'success',
        detail: `Binding ${body.value.bindingId?.value ?? ''} validated (${body.value.validation?.validationId ?? ''})`,
      };
    case 'bindingReplaced':
      return {
        title: 'Binding replaced',
        tone: 'info',
        detail: `${body.value.previousBindingId?.value ?? ''} replaced by ${body.value.replacement?.id?.value ?? ''}. ${replacementModeLabels[body.value.mode]}`,
      };
    case 'bindingRevoked':
      return { title: 'Binding revoked', tone: 'danger', detail: body.value.bindingId?.value };
    case 'revoked':
      return { title: 'Connection revoked', tone: 'danger' };
    case 'profileSet':
      return { title: 'Profile updated', tone: 'neutral', detail: body.value.profile?.displayName };
    case 'routingPolicySet':
      return {
        title: 'Routing policy updated',
        tone: 'warning',
        detail: `Preference ${body.value.routingPolicy?.preference ?? 0}`,
      };
    default:
      return { title: 'Unknown event', tone: 'neutral' };
  }
}

function relativeTime(date: Date): string {
  const seconds = Math.max(0, Math.round((Date.now() - date.getTime()) / 1000));
  if (seconds < 60) return 'just now';
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export function HistoryTimeline({ events }: { events: readonly StoredEvent[] }) {
  if (events.length === 0) return <p className="micro text-muted-foreground">No events recorded.</p>;
  return (
    <div className="well border-border overflow-hidden rounded-lg border font-mono text-xs">
      <div className="micro border-border bg-card/60 text-muted-foreground flex items-center justify-between border-b px-3 py-2 text-[10px]">
        <span>event log</span>
        <span className="tabular-nums">{events.length} entries</span>
      </div>
      <ol aria-label="Event history" className="flex flex-col py-1">
        {events.map((stored) => {
          const { title, detail, tone } = describe(stored.event);
          return (
            <li
              key={stored.revision}
              className="hover:bg-foreground/[0.03] grid grid-cols-[3.25rem_minmax(0,1fr)] gap-x-3 px-3 py-1.5 sm:grid-cols-[3.25rem_12rem_minmax(0,1fr)]"
            >
              <span className="border-border text-muted-foreground self-start border-r pr-3 text-right tabular-nums">
                r{stored.revision}
              </span>
              <time
                dateTime={stored.occurredAt.toISOString()}
                title={stored.occurredAt.toLocaleString()}
                className="text-muted-foreground hidden tabular-nums sm:block"
              >
                {stored.occurredAt.toLocaleTimeString([], { hour12: false })}
                <span className="text-muted-foreground/60"> {relativeTime(stored.occurredAt)}</span>
              </time>
              <div className="flex min-w-0 flex-col gap-0.5">
                <div className="flex flex-wrap items-center gap-2">
                  <span
                    className={cn(
                      'micro inline-flex h-5 items-center gap-1.5 rounded-sm border border-(--led)/35 px-1.5 text-[10px] text-foreground',
                      toneVariable(tone),
                    )}
                  >
                    <span aria-hidden="true" className="led size-1.5 rounded-full" />
                    {title}
                  </span>
                  <span className="text-muted-foreground">by {stored.actor}</span>
                </div>
                {detail ? <p className="text-foreground/80 break-words">{detail}</p> : null}
              </div>
            </li>
          );
        })}
      </ol>
      <div
        aria-hidden="true"
        className="border-border text-muted-foreground flex items-center gap-2 border-t px-3 py-2"
      >
        <span className="text-primary-ink">$</span>
        <span className="caret bg-primary inline-block h-3.5 w-2" />
      </div>
    </div>
  );
}

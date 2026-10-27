import type { ProjectEvent } from '@trogonai/proto';

import { toneVariable, type Tone } from '@/components/lifecycle-badge';
import { relativeTime } from '@/features/secrets/components/time';
import { cn } from '@/lib/utils';

import type { StoredProjectEvent } from '../client';
import { accessPresetLabels } from '../domain/labels';

interface Description {
  title: string;
  detail?: string;
  tone: Tone;
}

export function describeProjectEvent(event: ProjectEvent): Description {
  const fact = event.event;
  switch (fact.case) {
    case 'projectCreated':
      return {
        title: 'Created',
        tone: 'info',
        detail: `${fact.value.displayName}, ${accessPresetLabels[fact.value.initialAccessPreset].toLowerCase()} access`,
      };
    case 'projectRenamed':
      return { title: 'Renamed', tone: 'neutral', detail: fact.value.displayName };
    case 'projectGoalRestated':
      return { title: 'Goal restated', tone: 'neutral', detail: fact.value.goal };
    case 'projectArchived':
      return { title: 'Archived', tone: 'warning' };
    case 'projectUnarchived':
      return { title: 'Unarchived', tone: 'success' };
    default:
      return { title: 'Event', tone: 'neutral' };
  }
}

export function ProjectTimeline({ events }: { events: readonly StoredProjectEvent[] }) {
  if (events.length === 0) return <p className="micro text-muted-foreground">No events recorded.</p>;
  const ordered = [...events].sort((left, right) => right.revision - left.revision);
  return (
    <div className="well border-border overflow-hidden rounded-lg border font-mono text-xs">
      <div className="micro border-border bg-card/60 text-muted-foreground flex items-center justify-between border-b px-3 py-2 text-[10px]">
        <span>event log</span>
        <span className="tabular-nums">{events.length} entries</span>
      </div>
      <ol aria-label="Project activity" className="flex flex-col py-1">
        {ordered.map((stored) => {
          const { title, detail, tone } = describeProjectEvent(stored.event);
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
                      'micro text-foreground inline-flex h-5 items-center gap-1.5 rounded-sm border border-(--led)/35 px-1.5 text-[10px]',
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
    </div>
  );
}

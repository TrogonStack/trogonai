import { toneVariable, type Tone } from '@/components/lifecycle-badge';
import { relativeTime } from '@/features/secrets/components/time';
import { cn } from '@/lib/utils';

import type { StoredHierarchyEvent } from '../client';
import { factTouches, type HierarchyFact } from '../domain/events';
import type { NodeId } from '../domain/node-id';

interface Description {
  title: string;
  detail?: string;
  tone: Tone;
}

export function describeHierarchyFact(
  fact: HierarchyFact,
  subject: NodeId,
  nameOf: (nodeId: NodeId) => string,
): Description {
  switch (fact.case) {
    case 'nodeAdded':
      return fact.nodeId === subject
        ? {
            title: 'Added',
            tone: 'info',
            detail: fact.parent ? `Below ${nameOf(fact.parent)}` : 'The tree started here',
          }
        : { title: 'Child added', tone: 'success', detail: nameOf(fact.nodeId) };
    case 'nodeMoved':
      if (fact.nodeId === subject) {
        return {
          title: 'Moved',
          tone: 'warning',
          detail: `From ${nameOf(fact.from)} to ${nameOf(fact.to)}`,
        };
      }
      return fact.to === subject
        ? { title: 'Child moved in', tone: 'info', detail: `${nameOf(fact.nodeId)} from ${nameOf(fact.from)}` }
        : { title: 'Child moved out', tone: 'neutral', detail: `${nameOf(fact.nodeId)} to ${nameOf(fact.to)}` };
    case 'nodeRemoved':
      return fact.nodeId === subject
        ? { title: 'Removed', tone: 'danger', detail: `From ${nameOf(fact.parent)}` }
        : { title: 'Child removed', tone: 'neutral', detail: nameOf(fact.nodeId) };
  }
}

export function HierarchyTimeline({
  events,
  nodeId,
  nameOf,
}: {
  events: readonly StoredHierarchyEvent[];
  nodeId: NodeId;
  nameOf: (nodeId: NodeId) => string;
}) {
  const touching = events.filter((stored) => factTouches(stored.event.fact, nodeId));
  if (touching.length === 0) return <p className="micro text-muted-foreground">No tree operations recorded.</p>;
  const ordered = [...touching].sort((left, right) => right.revision - left.revision);
  return (
    <div className="well border-border overflow-hidden rounded-lg border font-mono text-xs">
      <div className="micro border-border bg-card/60 text-muted-foreground flex items-center justify-between border-b px-3 py-2 text-[10px]">
        <span>event log</span>
        <span className="tabular-nums">{touching.length} entries</span>
      </div>
      <ol aria-label="Node activity" className="flex flex-col py-1">
        {ordered.map((stored) => {
          const { title, detail, tone } = describeHierarchyFact(stored.event.fact, nodeId, nameOf);
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

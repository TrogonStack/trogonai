import { useNavigate } from '@tanstack/react-router';
import { PauseIcon, PlayIcon, Trash2Icon } from 'lucide-react';
import { useState, type ReactNode } from 'react';

import { BackLink } from '@/components/back-link';
import { ConfirmDialog } from '@/components/confirm-dialog';
import { CopyButton } from '@/components/copy-button';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { PageHeader } from '@/components/page-header';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { strategyLabels } from '@/features/schedules/draft';
import {
  base64Of,
  definitionSummary,
  durationLabel,
  instantLabel,
  scheduleKind,
  statusLabel,
} from '@/features/schedules/model';
import { useScheduleAction, useScheduleQuery } from '@/features/schedules/queries';

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid gap-1 text-[13px] sm:grid-cols-[10rem_minmax(0,1fr)]">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 font-mono text-xs break-words">{children}</dd>
    </div>
  );
}
function Panel({ title, description, children }: { title: string; description: string; children: ReactNode }) {
  return (
    <Card>
      <CardHeader className="flex-col items-start gap-1">
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>
        <dl className="flex flex-col gap-4">{children}</dl>
      </CardContent>
    </Card>
  );
}
export function ScheduleDetailScreen({ scheduleId }: { scheduleId: string }) {
  const query = useScheduleQuery(scheduleId);
  const mutation = useScheduleAction();
  const navigate = useNavigate();
  const [removing, setRemoving] = useState(false);
  const [encoding, setEncoding] = useState<'utf8' | 'base64'>('utf8');
  const back = <BackLink to="/schedules">All schedules</BackLink>;
  if (query.isPending)
    return (
      <div role="status" aria-label="Loading schedule">
        <Skeleton className="h-72" />
      </div>
    );
  if (query.error) return <FaultAlert error={query.error} title="Could not load schedule" />;
  const row = query.data;
  if (!row)
    return (
      <NotFoundState back={back} title="Schedule not found">
        The schedule may have been removed or belongs to another project.
      </NotFoundState>
    );
  const definition = row.schedule!.kind;
  const delivery = row.delivery!.kind.value!;
  const content = row.message!.content!;
  const paused = row.status?.kind.case === 'paused';
  const timezone = definition.case === 'cron' || definition.case === 'rrule' ? definition.value.timezone : undefined;
  const act = (action: 'pause' | 'resume') => mutation.mutate({ action, id: scheduleId });
  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <PageHeader
        eyebrow="Automation / Schedule"
        title={delivery.subject}
        description={scheduleId}
        badge={
          <>
            <Badge variant="outline">{statusLabel(row)}</Badge>
            {row.completed ? <Badge variant="outline">Completed</Badge> : null}
          </>
        }
        actions={
          <>
            {!row.completed ? (
              <Button variant="outline" disabled={mutation.isPending} onClick={() => act(paused ? 'resume' : 'pause')}>
                {paused ? <PlayIcon /> : <PauseIcon />}
                {paused ? 'Resume schedule' : 'Pause schedule'}
              </Button>
            ) : null}
            <Button variant="outline" disabled={mutation.isPending} onClick={() => setRemoving(true)}>
              <Trash2Icon />
              Remove schedule
            </Button>
          </>
        }
      />
      {mutation.error ? <FaultAlert error={mutation.error} title="Could not update schedule" /> : null}
      <div className="border-border bg-surface-deep/50 text-muted-foreground rounded-lg border px-4 py-3 text-xs">
        In-memory demo. Timing is illustrative; create, pause, resume and remove update local state only.{' '}
        {row.completed
          ? 'This recurrence is exhausted and will not fire again.'
          : 'The mock does not arm or deliver messages.'}
      </div>
      <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <div className="flex min-w-0 flex-col gap-4">
          <Panel title="Timing" description="The immutable definition recorded at creation.">
            <Fact label="Strategy">{strategyLabels[scheduleKind(row)]}</Fact>
            <Fact label="Definition">{definitionSummary(row)}</Fact>
            {timezone ? (
              <>
                <Fact label="Timezone">{timezone.id || 'Default (UTC for recurrence)'}</Fact>
                <Fact label="tzdb version">{timezone.version || 'Not specified'}</Fact>
              </>
            ) : null}
            {definition.case === 'rrule' ? (
              <>
                <Fact label="Recurrence start">{instantLabel(definition.value.dtstart)}</Fact>
                <Fact label="Included dates">
                  {definition.value.rdate.length
                    ? definition.value.rdate.map((date, index) => <div key={index}>{instantLabel(date)}</div>)
                    : 'None'}
                </Fact>
                <Fact label="Excluded dates">
                  {definition.value.exdate.length
                    ? definition.value.exdate.map((date, index) => <div key={index}>{instantLabel(date)}</div>)
                    : 'None'}
                </Fact>
              </>
            ) : null}
          </Panel>
          <Panel
            title="Message"
            description={
              delivery.source
                ? 'Static content is retained. Latest-message sampling supplies the payload at fire time.'
                : 'Opaque static content published with the configured media type.'
            }
          >
            <Fact label="Content type">{content.contentType}</Fact>
            <Fact label="Payload size">{content.data.length} bytes</Fact>
            <div className="flex items-center justify-between gap-2">
              <span className="text-muted-foreground text-xs">Payload</span>
              <Button variant="outline" size="sm" onClick={() => setEncoding(encoding === 'utf8' ? 'base64' : 'utf8')}>
                Show {encoding === 'utf8' ? 'base64' : 'UTF-8'}
              </Button>
            </div>
            <pre className="border-border bg-surface-deep/50 max-h-80 overflow-auto rounded-md border p-3 font-mono text-xs break-all whitespace-pre-wrap">
              {encoding === 'utf8' ? new TextDecoder().decode(content.data) : base64Of(content.data)}
            </pre>
            <Fact label="Headers">
              {row.message!.headers.length ? (
                <ul className="flex flex-col gap-2">
                  {row.message!.headers.map((header, index) => (
                    <li key={index}>
                      {header.name}: {header.value}
                    </li>
                  ))}
                </ul>
              ) : (
                'None'
              )}
            </Fact>
          </Panel>
        </div>
        <div className="flex min-w-0 flex-col gap-4">
          <Panel title="Delivery" description="NATS message destination and payload selection.">
            <Fact label="Subject">{delivery.subject}</Fact>
            <Fact label="Message TTL">{durationLabel(delivery.ttl)}</Fact>
            <Fact label="Payload source">{delivery.source?.kind.value?.subject ?? 'Static message'}</Fact>
          </Panel>
          <Panel title="Occurrences" description="Read-model fixture timestamps, shown in UTC.">
            <Fact label="Lifecycle">{statusLabel(row)}</Fact>
            <Fact label="Recurrence complete">{row.completed ? 'Yes' : 'No'}</Fact>
            <Fact label="Next occurrence">
              {row.completed
                ? 'Exhausted'
                : paused
                  ? 'Paused'
                  : row.nextOccurrenceAt
                    ? instantLabel(row.nextOccurrenceAt)
                    : 'Not armed in mock'}
            </Fact>
            <Fact label="Last occurrence">{instantLabel(row.lastOccurrenceAt)}</Fact>
          </Panel>
          <div className="flex items-center gap-2 text-xs">
            <span className="text-muted-foreground">Schedule id</span>
            <CopyButton value={row.scheduleId} label="schedule id" />
          </div>
        </div>
      </div>
      <ConfirmDialog
        open={removing}
        onOpenChange={setRemoving}
        icon={<Trash2Icon />}
        title="Remove schedule?"
        description={`Remove the schedule for ${delivery.subject}. Its definition will disappear from this demo.`}
        confirmLabel="Remove schedule"
        onConfirm={() => {
          if (mutation.isPending) return;
          mutation.mutate(
            { action: 'remove', id: scheduleId },
            {
              onSuccess: () => {
                void navigate({ to: '/schedules' });
              },
            },
          );
        }}
      />
    </div>
  );
}

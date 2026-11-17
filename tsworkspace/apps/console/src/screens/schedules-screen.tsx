import { Link, useNavigate } from '@tanstack/react-router';
import type { ColumnDef } from '@tanstack/react-table';
import type { ScheduleProjection } from '@trogonai/proto';
import { CalendarClockIcon, CheckCircle2Icon, PauseIcon, PlusIcon, RepeatIcon } from 'lucide-react';
import { useState } from 'react';

import { DataTable, TableEmptyState, TableFooter, TableSkeleton } from '@/components/data-table';
import { FaultAlert } from '@/components/fault-alert';
import { NativeSelect } from '@/components/field';
import { PageHeader } from '@/components/page-header';
import { Stat, StatStrip } from '@/components/stat-strip';
import { Badge } from '@/components/ui/badge';
import { buttonVariants } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { strategyLabels, strategyOptions } from '@/features/schedules/draft';
import {
  definitionSummary,
  deliverySubject,
  instantLabel,
  scheduleKind,
  statusLabel,
} from '@/features/schedules/model';
import { useSchedulesQuery } from '@/features/schedules/queries';

const columns: ColumnDef<ScheduleProjection>[] = [
  {
    accessorKey: 'scheduleId',
    header: 'Schedule',
    cell: ({ row }) => (
      <div className="flex flex-col gap-1">
        <Link
          to="/schedules/$scheduleId"
          params={{ scheduleId: row.original.scheduleId }}
          className="hover:text-primary-ink font-mono text-xs underline-offset-4 hover:underline"
        >
          {row.original.scheduleId}
        </Link>
        <span className="text-muted-foreground font-mono text-xs">{deliverySubject(row.original)}</span>
      </div>
    ),
  },
  {
    id: 'timing',
    header: 'Timing',
    accessorFn: (row) => definitionSummary(row),
    cell: ({ row }) => (
      <div className="flex max-w-72 flex-col gap-1">
        <span className="text-[13px] font-medium">{strategyLabels[scheduleKind(row.original)]}</span>
        <span className="text-muted-foreground truncate font-mono text-xs" title={definitionSummary(row.original)}>
          {definitionSummary(row.original)}
        </span>
      </div>
    ),
  },
  {
    id: 'status',
    header: 'Status',
    accessorFn: statusLabel,
    cell: ({ row }) => (
      <div className="flex flex-wrap gap-1">
        <Badge variant="outline">{statusLabel(row.original)}</Badge>
        {row.original.completed ? <Badge variant="outline">Completed</Badge> : null}
      </div>
    ),
  },
  {
    id: 'next',
    header: 'Next occurrence (UTC)',
    accessorFn: (row) => (row.nextOccurrenceAt ? instantLabel(row.nextOccurrenceAt) : ''),
    cell: ({ row }) => (
      <span className="text-muted-foreground font-mono text-xs">
        {row.original.completed
          ? 'Exhausted'
          : row.original.status?.kind.case === 'paused'
            ? 'Paused'
            : row.original.nextOccurrenceAt
              ? instantLabel(row.original.nextOccurrenceAt)
              : 'Not armed in mock'}
      </span>
    ),
  },
];
export function SchedulesScreen() {
  const query = useSchedulesQuery();
  const navigate = useNavigate();
  const [search, setSearch] = useState('');
  const [status, setStatus] = useState('all');
  const [kind, setKind] = useState('all');
  const rows = query.data ?? [];
  const filtered = rows.filter(
    (row) =>
      (!search ||
        `${row.scheduleId} ${deliverySubject(row)} ${definitionSummary(row)}`
          .toLowerCase()
          .includes(search.toLowerCase())) &&
      (kind === 'all' || scheduleKind(row) === kind) &&
      (status === 'all' ||
        (status === 'completed' ? row.completed : !row.completed && row.status?.kind.case === status)),
  );
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Automation"
        title="Schedules"
        badge={<Badge variant="outline">Preview</Badge>}
        description="Decide when messages arrive. One-shot, interval, cron and recurrence schedules in one place."
        actions={
          <Link to="/schedules/new" className={buttonVariants()}>
            <PlusIcon />
            Create schedule
          </Link>
        }
      />
      <div
        role="note"
        className="border-border bg-surface-deep/50 text-muted-foreground rounded-lg border px-4 py-3 text-xs"
      >
        In-memory scheduling demo. Occurrence timestamps are illustrative fixtures. No messages are published and no
        recurrence rules are executed.
      </div>
      {query.error ? (
        <FaultAlert error={query.error} title="Could not load schedules" />
      ) : query.isPending ? (
        <TableSkeleton label="Loading schedules" />
      ) : (
        <>
          <StatStrip label="Schedules summary">
            <Stat
              label="Scheduled"
              value={rows.filter((row) => !row.completed && row.status?.kind.case === 'scheduled').length}
              caption="Participating in future fires"
              icon={CalendarClockIcon}
              led="[--led:var(--primary)]"
            />
            <Stat
              label="Paused"
              value={rows.filter((row) => !row.completed && row.status?.kind.case === 'paused').length}
              caption="Definitions retained"
              icon={PauseIcon}
              led="[--led:var(--warning)]"
            />
            <Stat
              label="Recurring"
              value={rows.filter((row) => scheduleKind(row) !== 'at' && !row.completed).length}
              caption="Interval, cron and rules"
              icon={RepeatIcon}
              led="[--led:var(--primary)]"
            />
            <Stat
              label="Completed"
              value={rows.filter((row) => row.completed).length}
              caption="Recurrence exhausted"
              icon={CheckCircle2Icon}
              led="[--led:var(--muted-foreground)]"
            />
          </StatStrip>
          <DataTable
            data={filtered}
            columns={columns}
            onOpen={(row) => void navigate({ to: '/schedules/$scheduleId', params: { scheduleId: row.scheduleId } })}
            toolbar={
              <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_10rem_11rem]">
                <Input
                  aria-label="Search schedules"
                  placeholder="Search id, subject or timing…"
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                />
                <NativeSelect
                  label="Filter schedule status"
                  value={status}
                  onChange={setStatus}
                  options={[
                    { value: 'all', label: 'All statuses' },
                    { value: 'scheduled', label: 'Scheduled' },
                    { value: 'paused', label: 'Paused' },
                    { value: 'completed', label: 'Completed' },
                  ]}
                />
                <NativeSelect
                  label="Filter schedule strategy"
                  value={kind}
                  onChange={setKind}
                  options={[
                    { value: 'all', label: 'All strategies' },
                    ...strategyOptions.map(({ value, label }) => ({ value, label })),
                  ]}
                />
              </div>
            }
            empty={
              <TableEmptyState
                title={rows.length ? 'No matching schedules' : 'No schedules yet'}
                message={
                  rows.length
                    ? 'Try another search or clear the filters.'
                    : 'Create a schedule to define when a message should be delivered.'
                }
              />
            }
            footer={<TableFooter shown={filtered.length} total={rows.length} noun="schedule" />}
          />
        </>
      )}
    </div>
  );
}

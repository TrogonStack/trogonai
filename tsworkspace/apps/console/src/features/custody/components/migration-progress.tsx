import { conflictsRetried } from '../domain/migration';
import type { MigrationView } from '../domain/view';

export function MigrationProgressBar({ view, compact }: { view: MigrationView; compact?: boolean }) {
  const { migration, percent } = view;
  return (
    <div className="flex min-w-32 flex-col gap-1">
      <div
        role="progressbar"
        aria-label={`Migration ${migration.id} progress`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={`${migration.rewrapped} of ${migration.total} payloads rewrapped`}
        className="bg-muted h-2 w-full overflow-hidden rounded-full"
      >
        <div
          className={
            migration.status === 'completed'
              ? 'bg-success h-full transition-[width]'
              : 'bg-primary h-full transition-[width]'
          }
          style={{ width: `${percent}%` }}
        />
      </div>
      {compact ? (
        <span className="text-muted-foreground font-mono text-[11px] tabular-nums">
          {migration.rewrapped} / {migration.total}
        </span>
      ) : null}
    </div>
  );
}

export function MigrationCounters({ view }: { view: MigrationView }) {
  const items = [
    { label: 'Rewrapped', value: view.migration.rewrapped },
    { label: 'Remaining', value: view.remaining },
    { label: 'Conflicts retried', value: conflictsRetried(view.migration) },
    { label: 'Total', value: view.migration.total },
  ];
  return (
    <dl className="grid grid-cols-2 gap-4 sm:grid-cols-4">
      {items.map((item) => (
        <div key={item.label} className="flex flex-col gap-1">
          <dt className="micro text-muted-foreground text-[10px]">{item.label}</dt>
          <dd className="font-mono text-2xl tabular-nums">{item.value}</dd>
        </div>
      ))}
    </dl>
  );
}

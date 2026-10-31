import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { CustodyMigrationNote } from '@/features/custody/components/custody-migration-note';
import { MigrationStatusBadge } from '@/features/custody/components/key-badges';
import { MigrationActions } from '@/features/custody/components/migration-actions';
import { MigrationEndpoint } from '@/features/custody/components/migration-endpoint';
import { MigrationCounters, MigrationProgressBar } from '@/features/custody/components/migration-progress';
import { migrationDirectionLabels } from '@/features/custody/domain/migration';
import { useMigrationQuery } from '@/features/custody/queries';
import { RelativeTime } from '@/features/secrets/components/relative-time';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading migration</span>
      <Skeleton className="h-16 w-96" />
      <Skeleton className="h-48" />
    </div>
  );
}

export function MigrationDetailScreen({ migrationId }: { migrationId: string }) {
  const query = useMigrationQuery(migrationId);
  const back = <BackLink to="/keys/migrations">All migrations</BackLink>;

  if (query.isPending) return <DetailSkeleton />;
  if (query.isError) return <FaultAlert error={query.error} title="Could not load the migration" />;

  const view = query.data;
  if (!view) {
    return (
      <NotFoundState back={back} title="Migration not found">
        No migration has the id <span className="font-mono">{migrationId}</span>.
      </NotFoundState>
    );
  }

  const { migration } = view;

  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <DetailHeader
        title={`Rewrap ${view.purposeLabel.toLowerCase()}`}
        status={<MigrationStatusBadge status={migration.status} />}
        identifier={<span className="font-mono">{migration.id}</span>}
        copyValue={migration.id}
        copyLabel="migration id"
        facts={[`${migration.total} payloads`, migrationDirectionLabels[view.direction]]}
        actions={<MigrationActions view={view} />}
      />

      <CustodyMigrationNote />

      {view.blocked ? (
        <p
          role="alert"
          className="border-danger/30 bg-danger-soft text-danger-ink rounded-lg border px-4 py-3 text-[13px]"
        >
          {view.blocked}
        </p>
      ) : null}

      <Card>
        <CardHeader>
          <div className="flex flex-col gap-1">
            <CardTitle>Progress</CardTitle>
            <CardDescription>
              Conflicts are payloads that changed while being rewrapped. They are retried, never skipped.
            </CardDescription>
          </div>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          <MigrationProgressBar view={view} />
          <MigrationCounters view={view} />
          <p className="text-muted-foreground text-xs">
            Started <RelativeTime date={migration.startedAt} />
            {migration.completedAt ? (
              <>
                , completed <RelativeTime date={migration.completedAt} />.
              </>
            ) : (
              '.'
            )}
          </p>
        </CardContent>
      </Card>

      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>From</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2">
            <MigrationEndpoint end={view.from} />
            <p className="text-muted-foreground text-xs">
              {migration.status === 'completed'
                ? 'Nothing is wrapped under this key through this migration any more.'
                : 'Still needed to unwrap payloads that have not moved. It and its backend must stay available.'}
            </p>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>To</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2">
            <MigrationEndpoint end={view.to} />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

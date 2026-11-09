import { ArrowRightLeftIcon } from 'lucide-react';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { Button } from '@/components/ui/button';
import { ChangeCustodyDialog } from '@/features/custody/components/change-custody-dialog';
import { CustodyMigrationNote } from '@/features/custody/components/custody-migration-note';
import { KeysSubnav } from '@/features/custody/components/keys-subnav';
import { MigrationsTable, MigrationsTableSkeleton } from '@/features/custody/components/migrations-table';
import { useCustodyQuery } from '@/features/custody/queries';

export function MigrationsScreen() {
  const query = useCustodyQuery();
  const [starting, setStarting] = useState(false);
  const noPurposes = query.data?.purposes.length === 0;
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Security"
        title="Migrations"
        description="Rewrap payloads from one key to another, into or out of customer managed custody. Only the data keys are rewrapped, and the encrypted data is untouched."
        actions={
          <Button type="button" size="lg" disabled={!query.data || noPurposes} onClick={() => setStarting(true)}>
            <ArrowRightLeftIcon data-icon="inline-start" />
            Start a migration
          </Button>
        }
      />
      <KeysSubnav />
      <CustodyMigrationNote />
      {noPurposes ? (
        <p className="text-muted-foreground text-[13px]">No key purposes are available to migrate.</p>
      ) : null}
      {query.error ? <FaultAlert error={query.error} title="Could not load migrations" /> : null}
      {query.data ? (
        <>
          <MigrationsTable migrations={query.data.migrations} />
          <ChangeCustodyDialog data={query.data} open={starting} onOpenChange={setStarting} />
        </>
      ) : query.error ? null : (
        <MigrationsTableSkeleton />
      )}
    </div>
  );
}

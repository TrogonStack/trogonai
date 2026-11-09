import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { CreateEnvironmentDialog } from '@/features/environments/components/create-environment-dialog';
import { EnvironmentStats, EnvironmentStatsSkeleton } from '@/features/environments/components/environment-stats';
import { EnvironmentsTable, EnvironmentsTableSkeleton } from '@/features/environments/components/environments-table';

import { useEnvironmentWorkspace } from './use-environment-workspace';

export function EnvironmentsScreen() {
  const { environments, error } = useEnvironmentWorkspace();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Project"
        title="Environments"
        description="The places a vault can live, such as production or staging. Vaults pick one when they are created and keep it for good."
        actions={<CreateEnvironmentDialog />}
      />
      {error ? <FaultAlert error={error} title="Could not load environments" /> : null}
      {environments ? (
        <>
          <EnvironmentStats environments={environments} />
          <EnvironmentsTable environments={environments} />
        </>
      ) : error ? null : (
        <>
          <EnvironmentStatsSkeleton />
          <EnvironmentsTableSkeleton />
        </>
      )}
    </div>
  );
}

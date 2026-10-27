import { Skeleton } from '@/components/ui/skeleton';

import type { ProjectId } from '../domain/project-id';
import { useProjectResourcesQuery } from '../queries';

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? '' : 's'}`;
}

export function ResourceCounts({ projectId }: { projectId: ProjectId }) {
  const { data } = useProjectResourcesQuery(projectId);
  if (!data) return <Skeleton className="h-4 w-40" />;
  return (
    <span className="text-muted-foreground font-mono text-[11px] tabular-nums">
      {plural(data.vaults, 'vault')} / {plural(data.credentials, 'credential')} /{' '}
      {plural(data.environments, 'environment')}
    </span>
  );
}

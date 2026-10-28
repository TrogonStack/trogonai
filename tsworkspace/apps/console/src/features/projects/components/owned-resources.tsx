import { Link } from '@tanstack/react-router';
import { KeyRoundIcon, LayersIcon, VaultIcon } from 'lucide-react';

import { Stat, StatStrip, StatStripSkeleton } from '@/components/stat-strip';
import { buttonVariants } from '@/components/ui/button';

import type { ProjectId } from '../domain/project-id';
import { useProjectResourcesQuery } from '../queries';

export function OwnedResources({ projectId }: { projectId: ProjectId }) {
  const { data } = useProjectResourcesQuery(projectId);
  if (!data) return <StatStripSkeleton />;
  return (
    <section aria-label="Owned resources" className="flex flex-col gap-3">
      <h2 className="micro text-muted-foreground flex items-center gap-2 text-[10px]">
        <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
        Owned resources
      </h2>
      <StatStrip label="Owned resources summary">
        <Stat
          label="Vaults"
          value={data.vaults}
          caption="Not yet destroyed"
          icon={VaultIcon}
          led="[--led:var(--primary)]"
        />
        <Stat
          label="Credentials"
          value={data.credentials}
          caption="Held in this project's vaults"
          icon={KeyRoundIcon}
          led="[--led:var(--success)]"
        />
        <Stat
          label="Environments"
          value={data.environments}
          caption="Built-in and custom"
          icon={LayersIcon}
          led="[--led:var(--info)]"
        />
      </StatStrip>
      <div className="flex flex-wrap items-center gap-2">
        <Link
          to="/secrets"
          search={{ project: projectId }}
          className={buttonVariants({ variant: 'outline', size: 'sm' })}
        >
          <KeyRoundIcon data-icon="inline-start" />
          Open secrets
        </Link>
        <Link
          to="/environments"
          search={{ project: projectId }}
          className={buttonVariants({ variant: 'outline', size: 'sm' })}
        >
          <LayersIcon data-icon="inline-start" />
          Open environments
        </Link>
      </div>
    </section>
  );
}

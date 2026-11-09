import { LockIcon } from 'lucide-react';

import { accessPresetDescriptions } from '../domain/labels';
import type { ProjectView } from '../domain/view';
import { AccessPresetChip } from './access-preset-chip';
import { useNodeLabel } from './node-label';

export function GenesisFacts({ project }: { project: ProjectView }) {
  const nodeLabel = useNodeLabel();
  return (
    <section aria-label="Genesis facts" className="border-border bg-card flex flex-col gap-3 rounded-lg border p-4">
      <h2 className="micro text-muted-foreground flex items-center gap-2 text-[10px]">
        <LockIcon className="size-3" aria-hidden="true" />
        Genesis facts
        <span className="border-border rounded-sm border px-1.5 py-px">Immutable</span>
      </h2>
      <p className="text-muted-foreground text-xs">
        These were fixed when the project was created. No command changes them later.
      </p>
      <dl className="grid gap-x-6 gap-y-3 text-[13px] sm:grid-cols-2">
        <div className="flex flex-col gap-1">
          <dt className="text-muted-foreground text-xs">Project id</dt>
          <dd className="font-mono text-xs">{project.projectId}</dd>
        </div>
        <div className="flex flex-col gap-1">
          <dt className="text-muted-foreground text-xs">Parent node</dt>
          <dd>
            {nodeLabel(project.parent)}{' '}
            <span className="text-muted-foreground font-mono text-xs">{project.parent}</span>
          </dd>
        </div>
        <div className="flex flex-col gap-1 sm:col-span-2">
          <dt className="text-muted-foreground text-xs">Initial access preset</dt>
          <dd className="flex flex-col items-start gap-1">
            <AccessPresetChip preset={project.initialAccessPreset} />
            <span className="text-muted-foreground text-xs">
              {accessPresetDescriptions[project.initialAccessPreset]}
            </span>
          </dd>
        </div>
      </dl>
    </section>
  );
}

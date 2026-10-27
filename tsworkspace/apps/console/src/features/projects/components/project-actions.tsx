import { ArchiveIcon, ArchiveRestoreIcon, PencilIcon, TargetIcon } from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { FaultAlert } from '@/components/fault-alert';
import { Button } from '@/components/ui/button';
import { useNotify } from '@/components/ui/toaster';

import type { ProjectView } from '../domain/view';
import { useArchiveProject, useUnarchiveProject } from '../queries';
import { RenameProjectDialog } from './rename-project-dialog';
import { RestateGoalDialog } from './restate-goal-dialog';

export function ProjectActions({ project }: { project: ProjectView }) {
  const archive = useArchiveProject();
  const unarchive = useUnarchiveProject();
  const notify = useNotify();
  const [renaming, setRenaming] = useState(false);
  const [restating, setRestating] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const [unarchiving, setUnarchiving] = useState(false);

  const { actions } = project;
  const reason = actions.rename.reason ?? actions.restateGoal.reason ?? actions.unarchive.reason;
  const target = { projectId: project.projectId, expectedRevision: project.revision };

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="lg"
          disabled={!actions.rename.available}
          title={actions.rename.reason}
          onClick={() => setRenaming(true)}
        >
          <PencilIcon data-icon="inline-start" />
          Rename
        </Button>
        <Button
          type="button"
          variant="outline"
          size="lg"
          disabled={!actions.restateGoal.available}
          title={actions.restateGoal.reason}
          onClick={() => setRestating(true)}
        >
          <TargetIcon data-icon="inline-start" />
          Restate goal
        </Button>
        {project.archived ? (
          <Button
            type="button"
            variant="outline"
            size="lg"
            disabled={!actions.unarchive.available || unarchive.isPending}
            title={actions.unarchive.reason}
            onClick={() => setUnarchiving(true)}
          >
            <ArchiveRestoreIcon data-icon="inline-start" />
            Unarchive
          </Button>
        ) : (
          <Button
            type="button"
            variant="destructive"
            size="lg"
            disabled={!actions.archive.available || archive.isPending}
            title={actions.archive.reason}
            onClick={() => setArchiving(true)}
          >
            <ArchiveIcon data-icon="inline-start" />
            Archive
          </Button>
        )}
      </div>
      {project.archived && reason ? (
        <p className="text-muted-foreground max-w-xs text-right text-xs">{reason}</p>
      ) : null}
      <FaultAlert error={archive.error ?? unarchive.error} />
      <RenameProjectDialog project={project} open={renaming} onOpenChange={setRenaming} />
      <RestateGoalDialog project={project} open={restating} onOpenChange={setRestating} />
      <ConfirmDialog
        open={archiving}
        onOpenChange={setArchiving}
        icon={<ArchiveIcon />}
        title={`Archive ${project.displayName}?`}
        description="An archived project accepts nothing but unarchive until you bring it back. Its vaults and environments stay where they are."
        confirmLabel="Archive project"
        onConfirm={() => {
          setArchiving(false);
          archive.mutate(target, {
            onSuccess: () => notify.success('Project archived', project.displayName),
            onError: (error) => notify.failure('Archive failed', error.message),
          });
        }}
      />
      <ConfirmDialog
        open={unarchiving}
        onOpenChange={setUnarchiving}
        icon={<ArchiveRestoreIcon />}
        title={`Unarchive ${project.displayName}?`}
        description="The project becomes active again and accepts renames, goal changes and archiving."
        confirmLabel="Unarchive project"
        onConfirm={() => {
          setUnarchiving(false);
          unarchive.mutate(target, {
            onSuccess: () => notify.success('Project unarchived', project.displayName),
            onError: (error) => notify.failure('Unarchive failed', error.message),
          });
        }}
      />
    </div>
  );
}

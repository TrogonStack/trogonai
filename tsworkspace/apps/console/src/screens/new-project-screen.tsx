import { useNavigate } from '@tanstack/react-router';
import { ProjectAccessPreset } from '@trogonai/proto';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { FieldShell, RadioCards, SelectField, TextField, describedBy } from '@/components/field';
import { PageHeader } from '@/components/page-header';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Textarea } from '@/components/ui/textarea';
import { useNotify } from '@/components/ui/toaster';
import {
  MAX_GOAL_LENGTH,
  MAX_DISPLAY_NAME_LENGTH,
  displayNameProblem,
  goalProblem,
} from '@/features/projects/domain/decider';
import {
  accessPresetDescriptions,
  accessPresetLabels,
  selectableAccessPresets,
} from '@/features/projects/domain/labels';
import { parseNodeId } from '@/features/projects/domain/node-id';
import {
  MAX_PROJECT_ID_LENGTH,
  parseProjectId,
  projectIdProblem,
  suggestProjectId,
} from '@/features/projects/domain/project-id';
import { useCreateProject, useParentCandidatesQuery } from '@/features/projects/queries';

const presetOptions = selectableAccessPresets.map((preset) => ({
  value: preset,
  label: accessPresetLabels[preset],
  description: accessPresetDescriptions[preset],
}));

export function NewProjectScreen() {
  const navigate = useNavigate();
  const parents = useParentCandidatesQuery();
  const create = useCreateProject();
  const notify = useNotify();
  const [name, setName] = useState('');
  const [typedId, setTypedId] = useState<string | undefined>();
  const [goal, setGoal] = useState('');
  const [parent, setParent] = useState<string | undefined>();
  const [preset, setPreset] = useState<ProjectAccessPreset>(ProjectAccessPreset.UNSPECIFIED);
  const [attempted, setAttempted] = useState(false);

  const id = typedId ?? suggestProjectId(name);
  const parentValue = parent ?? parents.data?.[0]?.nodeId ?? '';
  const goalHint = `Up to ${MAX_GOAL_LENGTH} characters.`;

  const nameError = attempted ? displayNameProblem(name) : undefined;
  const idError = attempted ? projectIdProblem(id) : undefined;
  const goalError = attempted ? goalProblem(goal) : undefined;
  const presetError =
    attempted && preset === ProjectAccessPreset.UNSPECIFIED
      ? 'Choose an access preset. It is fixed once the project exists.'
      : undefined;

  const submit = async () => {
    setAttempted(true);
    const projectId = parseProjectId(id);
    const parentId = parseNodeId(parentValue);
    if (
      displayNameProblem(name) ||
      goalProblem(goal) ||
      !projectId ||
      !parentId ||
      preset === ProjectAccessPreset.UNSPECIFIED
    ) {
      return;
    }
    create.reset();
    try {
      await create.mutateAsync({
        projectId,
        displayName: name,
        goal,
        parent: parentId,
        initialAccessPreset: preset,
      });
      notify.success('Project created', name.trim());
      await navigate({ to: '/projects/$projectId', params: { projectId } });
    } catch {
      return;
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Workspace"
        title="New project"
        description="The parent, the id and the access preset are fixed at creation. Only the name and the goal can change later."
      />
      {parents.isError ? <FaultAlert error={parents.error} title="Could not load parent nodes" /> : null}
      {parents.data ? (
        <form
          noValidate
          aria-label="Create project"
          className="flex max-w-xl flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={create.error} title="The project was rejected" />
          <TextField
            id="project-name"
            label="Project name"
            value={name}
            onChange={setName}
            error={nameError}
            placeholder="Customer portal"
            hint={`Up to ${MAX_DISPLAY_NAME_LENGTH} characters.`}
          />
          <TextField
            id="project-id"
            label="Project id"
            value={id}
            onChange={setTypedId}
            error={idError}
            placeholder="customer-portal"
            hint={`Lowercase letters, digits and hyphens, starting with a letter, up to ${MAX_PROJECT_ID_LENGTH} characters. It cannot change later.`}
            mono
          />
          <FieldShell id="project-goal" label="Goal" error={goalError} hint={goalHint}>
            <Textarea
              id="project-goal"
              name="project-goal"
              value={goal}
              rows={3}
              placeholder="What is this project for?"
              aria-invalid={goalError ? true : undefined}
              aria-describedby={describedBy('project-goal', goalError, goalHint)}
              onChange={(event) => setGoal(event.target.value)}
            />
          </FieldShell>
          <SelectField
            id="project-parent"
            label="Parent node"
            value={parentValue}
            options={parents.data.map((node) => ({
              value: node.nodeId,
              label: node.level === 'workspace' ? `${node.displayName} (workspace)` : node.displayName,
            }))}
            onChange={setParent}
            hint="The project lives under a workspace or a folder in it, never at the tenant root."
          />
          <RadioCards
            name="project-access-preset"
            label="Access preset"
            value={preset}
            options={presetOptions}
            onChange={setPreset}
            error={presetError}
            hint="This decides who can work in the project from the start. It is a fact about how the project began and is never changed by a command."
          />
          <div className="flex items-center gap-2">
            <Button type="submit" size="lg" disabled={create.isPending}>
              {create.isPending ? 'Creating...' : 'Create project'}
            </Button>
            <Button type="button" variant="outline" size="lg" onClick={() => void navigate({ to: '/projects' })}>
              Cancel
            </Button>
          </div>
        </form>
      ) : parents.isError ? null : (
        <Skeleton role="status" aria-label="Loading form" className="h-96 w-full max-w-xl" />
      )}
    </div>
  );
}

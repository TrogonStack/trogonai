import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { TextField } from '@/components/field';
import { PageHeader } from '@/components/page-header';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { useNotify } from '@/components/ui/toaster';
import {
  MAX_NODE_ID_LENGTH,
  newNodeIdProblem,
  parseNewNodeId,
  suggestNodeId,
} from '@/features/hierarchy/domain/node-id';
import { MAX_NODE_NAME_LENGTH, nodeNameProblem, parseNodeName } from '@/features/hierarchy/domain/node-name';
import { organizationNameDefault, revealsOrganization } from '@/features/hierarchy/domain/view';
import { useAddNode, useHierarchyView } from '@/features/hierarchy/queries';

export function NewWorkspaceScreen() {
  const navigate = useNavigate();
  const { view, error } = useHierarchyView();
  const add = useAddNode();
  const notify = useNotify();
  const [name, setName] = useState('');
  const [typedId, setTypedId] = useState<string | undefined>();
  const [typedOrganization, setTypedOrganization] = useState<string | undefined>();
  const [attempted, setAttempted] = useState(false);

  const id = typedId ?? suggestNodeId(name);
  const asksOrganization = view ? revealsOrganization(view, view.rootId) : false;
  const organization = typedOrganization ?? (view ? organizationNameDefault(view) : '');

  const nameError = attempted ? nodeNameProblem(name) : undefined;
  const idError = attempted ? newNodeIdProblem(id) : undefined;
  const organizationError = attempted && asksOrganization ? nodeNameProblem(organization) : undefined;

  const submit = async () => {
    if (!view?.rootId) return;
    setAttempted(true);
    const label = parseNodeName(name);
    const nodeId = parseNewNodeId(id);
    const organizationName = asksOrganization ? parseNodeName(organization) : undefined;
    if (!label || !nodeId || (asksOrganization && !organizationName)) return;
    add.reset();
    try {
      await add.mutateAsync({
        nodeId,
        parent: view.rootId,
        name: label,
        rootId: view.rootId,
        expectedRevision: view.revision,
        ...(organizationName ? { organizationName } : {}),
      });
      notify.success('Workspace created', label);
      await navigate({ to: '/workspaces/$nodeId', params: { nodeId } });
    } catch {
      return;
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Workspace"
        title="New workspace"
        description="A workspace is a direct child of the organization. Its id is fixed at creation, while its name is only a label."
      />
      {error ? <FaultAlert error={error} title="Could not load the tree" /> : null}
      {view ? (
        <form
          noValidate
          aria-label="Create workspace"
          className="flex max-w-xl flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={add.error} title="The workspace was rejected" />
          <TextField
            id="workspace-name"
            label="Workspace name"
            value={name}
            onChange={setName}
            error={nameError}
            placeholder="Payments workspace"
            hint={`Up to ${MAX_NODE_NAME_LENGTH} characters.`}
          />
          <TextField
            id="workspace-id"
            label="Workspace id"
            value={id}
            onChange={setTypedId}
            error={idError}
            placeholder="payments-workspace"
            hint={`Lowercase letters, digits and hyphens, starting with a letter, up to ${MAX_NODE_ID_LENGTH} characters. It cannot change later.`}
            mono
          />
          {asksOrganization ? (
            <TextField
              id="organization-name"
              label="Organization name"
              value={organization}
              onChange={setTypedOrganization}
              error={organizationError}
              hint="With a second workspace the organization becomes visible. It starts out named after your first workspace."
            />
          ) : null}
          <div className="flex items-center gap-2">
            <Button type="submit" size="lg" disabled={add.isPending || view.rootId === undefined}>
              {add.isPending ? 'Creating...' : 'Create workspace'}
            </Button>
            <Button type="button" variant="outline" size="lg" onClick={() => void navigate({ to: '/workspaces' })}>
              Cancel
            </Button>
          </div>
        </form>
      ) : error ? null : (
        <Skeleton role="status" aria-label="Loading form" className="h-72 w-full max-w-xl" />
      )}
    </div>
  );
}

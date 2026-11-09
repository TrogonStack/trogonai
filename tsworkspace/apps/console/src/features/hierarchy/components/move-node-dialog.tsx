import { TriangleAlertIcon } from 'lucide-react';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { SelectField, TextField } from '@/components/field';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { useNotify } from '@/components/ui/toaster';

import { parseNodeId, type NodeId } from '../domain/node-id';
import { nodeNameProblem, parseNodeName } from '../domain/node-name';
import { nodeWordLabels } from '../domain/tree';
import {
  nameOf,
  organizationNameDefault,
  revealsOrganization,
  type HierarchyView,
  type NodeView,
} from '../domain/view';
import { useMoveNode } from '../queries';

function targetLabel(view: HierarchyView, nodeId: NodeId): string {
  const target = view.byId.get(nodeId);
  if (!target || target.word === 'organization') return 'Organization root';
  return `${target.name} (${nodeWordLabels[target.word].toLowerCase()})`;
}

interface MoveNodeDialogProps {
  node: NodeView;
  view: HierarchyView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function MoveNodeDialog({ node, view, open, onOpenChange }: MoveNodeDialogProps) {
  const move = useMoveNode();
  const notify = useNotify();
  const [target, setTarget] = useState<string | undefined>();
  const [typedOrganization, setTypedOrganization] = useState<string | undefined>();
  const [attempted, setAttempted] = useState(false);

  const targetValue = target ?? node.moveTargets[0]?.nodeId ?? '';
  const chosen = node.moveTargets.find((candidate) => candidate.nodeId === targetValue);
  const targetId: NodeId | undefined = parseNodeId(targetValue);
  const wordChanges = chosen !== undefined && chosen.becomes !== node.word;
  const asksOrganization = targetId !== undefined && revealsOrganization(view, targetId);
  const organization = typedOrganization ?? organizationNameDefault(view);
  const organizationError = attempted && asksOrganization ? nodeNameProblem(organization) : undefined;

  const close = (next: boolean) => {
    if (!next) {
      move.reset();
      setAttempted(false);
      setTarget(undefined);
      setTypedOrganization(undefined);
    }
    onOpenChange(next);
  };

  const submit = async () => {
    setAttempted(true);
    if (!view.rootId || !targetId || !chosen) return;
    const organizationName = asksOrganization ? parseNodeName(organization) : undefined;
    if (asksOrganization && !organizationName) return;
    try {
      await move.mutateAsync({
        nodeId: node.nodeId,
        parent: targetId,
        rootId: view.rootId,
        expectedRevision: view.revision,
        ...(organizationName ? { organizationName } : {}),
      });
      notify.success('Moved', `${node.name} is now below ${nameOf(view, targetId)}`);
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Move {node.name}</DialogTitle>
          <DialogDescription>
            Only nodes that keep the tree acyclic and within the depth limit are offered. Projects stay attached to{' '}
            {node.name}.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Move node"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={move.error} title="The move was rejected" />
          <SelectField
            id="move-target"
            label="New parent"
            value={targetValue}
            options={node.moveTargets.map((candidate) => ({
              value: candidate.nodeId,
              label: targetLabel(view, candidate.nodeId),
            }))}
            onChange={setTarget}
          />
          {wordChanges ? (
            <p
              role="status"
              className="border-warning/35 bg-warning-soft text-warning-ink flex gap-2 rounded-lg border p-3 text-[13px]"
            >
              <TriangleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              <span>
                {node.name} changes from {nodeWordLabels[node.word].toLowerCase()} to{' '}
                {nodeWordLabels[chosen.becomes].toLowerCase()}. Nothing is stored for the word, it follows the depth.
              </span>
            </p>
          ) : null}
          {asksOrganization ? (
            <TextField
              id="move-organization-name"
              label="Organization name"
              value={organization}
              onChange={setTypedOrganization}
              error={organizationError}
              hint="This makes a second workspace, so the organization becomes visible."
            />
          ) : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={move.isPending || !chosen}>
              {move.isPending ? 'Moving...' : 'Move'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

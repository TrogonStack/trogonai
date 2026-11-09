import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { TextField } from '@/components/field';
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
import { useOnOpen } from '@/components/use-on-open';

import { MAX_NODE_NAME_LENGTH, nodeNameProblem, parseNodeName } from '../domain/node-name';
import { nodeWordLabels } from '../domain/tree';
import type { NodeView } from '../domain/view';
import { useRenameNode } from '../queries';

interface RenameNodeDialogProps {
  node: NodeView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RenameNodeDialog({ node, open, onOpenChange }: RenameNodeDialogProps) {
  const rename = useRenameNode();
  const notify = useNotify();
  const [name, setName] = useState(node.name);
  const [attempted, setAttempted] = useState(false);

  useOnOpen(open, () => setName(node.name));

  const word = nodeWordLabels[node.word].toLowerCase();
  const nameError = attempted ? nodeNameProblem(name) : undefined;

  const close = (next: boolean) => {
    if (!next) {
      rename.reset();
      setAttempted(false);
    }
    onOpenChange(next);
  };

  const submit = async () => {
    setAttempted(true);
    const label = parseNodeName(name);
    if (!label) return;
    try {
      await rename.mutateAsync({ nodeId: node.nodeId, name: label });
      notify.success('Label changed', label);
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Rename {word}</DialogTitle>
          <DialogDescription>
            The name is only a label kept next to the tree. The id and the position stay the same.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label={`Rename ${word}`}
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={rename.error} title="The rename was rejected" />
          <TextField
            id="rename-node-name"
            label="Name"
            value={name}
            onChange={setName}
            error={nameError}
            hint={`Up to ${MAX_NODE_NAME_LENGTH} characters.`}
          />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={rename.isPending}>
              {rename.isPending ? 'Saving...' : 'Save name'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

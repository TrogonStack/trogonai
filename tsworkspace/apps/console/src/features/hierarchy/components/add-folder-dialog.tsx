import { PlusIcon } from 'lucide-react';
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
  DialogTrigger,
} from '@/components/ui/dialog';
import { useNotify } from '@/components/ui/toaster';

import { MAX_NODE_ID_LENGTH, newNodeIdProblem, parseNewNodeId, suggestNodeId } from '../domain/node-id';
import { MAX_NODE_NAME_LENGTH, nodeNameProblem, parseNodeName } from '../domain/node-name';
import type { HierarchyView, NodeView } from '../domain/view';
import { useAddNode } from '../queries';

export function AddFolderDialog({ parent, view }: { parent: NodeView; view: HierarchyView }) {
  const add = useAddNode();
  const notify = useNotify();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [typedId, setTypedId] = useState<string | undefined>();
  const [attempted, setAttempted] = useState(false);

  const id = typedId ?? suggestNodeId(name);
  const nameError = attempted ? nodeNameProblem(name) : undefined;
  const idError = attempted ? newNodeIdProblem(id) : undefined;

  const close = (next: boolean) => {
    if (!next) {
      add.reset();
      setAttempted(false);
      setName('');
      setTypedId(undefined);
    }
    setOpen(next);
  };

  const submit = async () => {
    if (!view.rootId) return;
    setAttempted(true);
    const label = parseNodeName(name);
    const nodeId = parseNewNodeId(id);
    if (!label || !nodeId) return;
    try {
      await add.mutateAsync({
        nodeId,
        parent: parent.nodeId,
        name: label,
        rootId: view.rootId,
        expectedRevision: view.revision,
      });
      notify.success('Folder added', label);
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogTrigger render={<Button type="button" size="lg" variant="outline" />}>
        <PlusIcon data-icon="inline-start" />
        Add folder
      </DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add folder</DialogTitle>
          <DialogDescription>The folder is created below {parent.name}. Its id cannot change later.</DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Add folder"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={add.error} title="The folder was rejected" />
          <TextField
            id="folder-name"
            label="Folder name"
            value={name}
            onChange={setName}
            error={nameError}
            placeholder="Billing"
            hint={`Up to ${MAX_NODE_NAME_LENGTH} characters.`}
          />
          <TextField
            id="folder-id"
            label="Folder id"
            value={id}
            onChange={setTypedId}
            error={idError}
            placeholder="billing"
            hint={`Lowercase letters, digits and hyphens, starting with a letter, up to ${MAX_NODE_ID_LENGTH} characters.`}
            mono
          />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={add.isPending}>
              {add.isPending ? 'Adding...' : 'Add folder'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

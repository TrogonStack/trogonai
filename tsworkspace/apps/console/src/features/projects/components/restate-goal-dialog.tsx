import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { FieldShell, describedBy } from '@/components/field';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Textarea } from '@/components/ui/textarea';
import { useNotify } from '@/components/ui/toaster';

import { MAX_GOAL_LENGTH, goalProblem } from '../domain/decider';
import type { ProjectView } from '../domain/view';
import { useRestateProjectGoal } from '../queries';

interface RestateGoalDialogProps {
  project: ProjectView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RestateGoalDialog({ project, open, onOpenChange }: RestateGoalDialogProps) {
  const restate = useRestateProjectGoal();
  const notify = useNotify();
  const [goal, setGoal] = useState(project.goal);
  const [attempted, setAttempted] = useState(false);

  const goalError = attempted ? goalProblem(goal) : undefined;
  const hint = `Up to ${MAX_GOAL_LENGTH} characters.`;

  const close = (next: boolean) => {
    if (next) {
      setGoal(project.goal);
    } else {
      restate.reset();
      setAttempted(false);
    }
    onOpenChange(next);
  };

  const submit = async () => {
    setAttempted(true);
    if (goalProblem(goal) !== undefined) return;
    try {
      await restate.mutateAsync({ projectId: project.projectId, expectedRevision: project.revision, goal });
      notify.success('Goal restated', project.displayName);
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Restate goal</DialogTitle>
          <DialogDescription>
            Say what this project is for now. The earlier goal stays in the activity log.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Restate goal"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={restate.error} title="The goal was rejected" />
          <FieldShell id="restate-goal" label="Goal" error={goalError} hint={hint}>
            <Textarea
              id="restate-goal"
              name="restate-goal"
              value={goal}
              rows={3}
              aria-invalid={goalError ? true : undefined}
              aria-describedby={describedBy('restate-goal', goalError, hint)}
              onChange={(event) => setGoal(event.target.value)}
            />
          </FieldShell>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={restate.isPending}>
              {restate.isPending ? 'Saving...' : 'Save goal'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

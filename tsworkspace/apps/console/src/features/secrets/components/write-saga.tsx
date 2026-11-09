import { MaterialVerificationOutcome } from '@trogonai/proto';
import { CheckIcon, MinusIcon, XIcon } from 'lucide-react';
import { useState, type ReactNode } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { StatusDot } from '@/components/lifecycle-badge';
import { Button } from '@/components/ui/button';
import { Segmented } from '@/components/ui/segmented';
import { useNotify } from '@/components/ui/toaster';
import { cn } from '@/lib/utils';

import { failureReasonMessage } from '../domain/faults';
import { originLabels, verificationOutcomeLabels, writeKindLabels } from '../domain/labels';
import type { CredentialView, SagaStepView } from '../domain/view';
import { useCompleteVerification } from '../queries';
import { RelativeTime } from './relative-time';

function StepMarker({ step }: { step: SagaStepView }) {
  switch (step.status) {
    case 'done':
      return (
        <span className="bg-success-soft text-success-ink flex size-5 items-center justify-center rounded-full">
          <CheckIcon className="size-3" aria-hidden="true" />
        </span>
      );
    case 'failed':
      return (
        <span className="bg-danger-soft text-danger-ink flex size-5 items-center justify-center rounded-full">
          <XIcon className="size-3" aria-hidden="true" />
        </span>
      );
    case 'current':
      return (
        <span className="border-warning/40 flex size-5 items-center justify-center rounded-full border">
          <StatusDot tone="warning" blink />
        </span>
      );
    case 'skipped':
      return (
        <span className="border-border text-muted-foreground flex size-5 items-center justify-center rounded-full border border-dashed">
          <MinusIcon className="size-3" aria-hidden="true" />
        </span>
      );
    default:
      return <span className="border-border size-5 rounded-full border border-dashed" />;
  }
}

const statusWords: Record<SagaStepView['status'], string> = {
  done: 'Done',
  current: 'In progress',
  pending: 'Pending',
  failed: 'Failed',
  skipped: 'Skipped',
};

const verdicts = [MaterialVerificationOutcome.PASSED, MaterialVerificationOutcome.AUTHENTICATION_REJECTED] as const;

function CompleteVerification({ credential }: { credential: CredentialView }) {
  const complete = useCompleteVerification();
  const notify = useNotify();
  const [verdict, setVerdict] = useState<string>(String(MaterialVerificationOutcome.PASSED));
  return (
    <div className="border-border bg-surface-deep/40 flex min-w-0 flex-col gap-2 rounded-md border border-dashed p-3">
      <p className="micro text-muted-foreground text-[10px]">Simulation</p>
      <p className="text-[13px]">
        The candidate is waiting for the provider to verify it. No real provider is called, so choose the verdict.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Segmented
          label="Verification verdict"
          value={verdict}
          options={verdicts.map((outcome) => ({ value: String(outcome), label: verificationOutcomeLabels[outcome] }))}
          onChange={setVerdict}
          size="sm"
        />
        <Button
          type="button"
          size="sm"
          disabled={!credential.actions.completeVerification.available || complete.isPending}
          onClick={() =>
            complete.mutate(
              {
                secret: credential.secret,
                expectedRevision: credential.revision,
                outcome: Number(verdict) as MaterialVerificationOutcome,
              },
              {
                onSuccess: () => notify.success('Verification completed', credential.displayName),
                onError: (error) => notify.failure('Could not complete verification', error.message),
              },
            )
          }
        >
          {complete.isPending ? 'Completing...' : 'Complete verification'}
        </Button>
      </div>
      <FaultAlert error={complete.error} />
    </div>
  );
}

const sagaStatusLabels = { IN_FLIGHT: 'In flight', ACTIVATED: 'Activated', REJECTED: 'Rejected' } as const;

function SagaChip({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'micro border-border text-muted-foreground inline-flex h-5 items-center rounded-sm border px-1.5 text-[10px] whitespace-nowrap',
        className,
      )}
    >
      {children}
    </span>
  );
}

export function WriteSaga({ credential }: { credential: CredentialView }) {
  const saga = credential.saga;
  if (!saga) {
    return <p className="text-muted-foreground text-[13px]">No write has been recorded for this credential.</p>;
  }
  return (
    <div className="flex min-w-0 flex-col gap-4">
      <div className="flex flex-wrap items-center gap-1.5">
        <SagaChip className="text-foreground">{writeKindLabels[saga.kind]}</SagaChip>
        <SagaChip>{originLabels[saga.origin]}</SagaChip>
        <SagaChip className="font-mono normal-case">candidate v{saga.candidateVersion}</SagaChip>
        <SagaChip className={saga.status === 'REJECTED' ? 'text-danger-ink' : undefined}>
          {sagaStatusLabels[saga.status]}
        </SagaChip>
      </div>
      <ol aria-label="Write progress" className="flex flex-col">
        {saga.steps.map((step, index) => (
          <li
            key={step.key}
            aria-current={step.status === 'current' ? 'step' : undefined}
            className={cn(
              'relative flex gap-3 pb-4 last:pb-0',
              (step.status === 'pending' || step.status === 'skipped') && 'text-muted-foreground',
            )}
          >
            {index < saga.steps.length - 1 ? (
              <span aria-hidden="true" className="bg-border absolute top-6 bottom-0 left-2.5 w-px" />
            ) : null}
            <span className="relative flex size-5 shrink-0 items-center justify-center">
              <StepMarker step={step} />
            </span>
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              <div className="flex flex-wrap items-baseline justify-between gap-x-3">
                <span className="text-[13px] leading-5 font-medium">{step.label}</span>
                {step.at ? (
                  <RelativeTime date={step.at} className="text-muted-foreground font-mono text-[11px]" />
                ) : null}
              </div>
              <span className={cn('micro text-[10px]', step.status === 'failed' && 'text-danger-ink')}>
                {statusWords[step.status]}
              </span>
              {step.detail ? <span className="text-muted-foreground text-xs">{step.detail}</span> : null}
              {step.failure !== undefined ? (
                <span className="text-danger-ink text-xs">{failureReasonMessage(step.failure)}</span>
              ) : null}
            </div>
          </li>
        ))}
      </ol>
      {saga.awaitingVerification ? <CompleteVerification credential={credential} /> : null}
    </div>
  );
}

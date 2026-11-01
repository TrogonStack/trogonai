import { EyeIcon } from 'lucide-react';

import { StatusPill } from '@/components/lifecycle-badge';

import type { SlotState } from '../domain/integration';
import {
  registrationLabels,
  registrationTones,
  slotStateLabels,
  slotStateTones,
  statusLabels,
  statusTones,
} from '../domain/labels';
import type { IntegrationStatus, RegistrationStatus } from '../domain/view';

export function SourceStatusBadge({ status }: { status: IntegrationStatus }) {
  return <StatusPill tone={statusTones[status]} blink={status === 'degraded'} label={statusLabels[status]} />;
}

export function SlotStateBadge({ state }: { state: SlotState }) {
  return (
    <StatusPill
      tone={slotStateTones[state]}
      blink={state === 'pending_write' || state === 'pending_rotation'}
      struck={state === 'revoked' || state === 'destroyed'}
      label={slotStateLabels[state]}
    />
  );
}

export function RegistrationBadge({ status }: { status: RegistrationStatus }) {
  return (
    <StatusPill
      tone={registrationTones[status]}
      blink={status === 'registration_pending'}
      label={registrationLabels[status]}
    />
  );
}

export function PreviewBadge() {
  return (
    <span
      title="Everything on this page is generated fake data. Nothing is read from or written to a gateway."
      className="micro border-border text-muted-foreground inline-flex h-5 w-fit items-center gap-1.5 rounded-sm border border-dashed px-1.5 text-[10px]"
    >
      <EyeIcon className="size-3" aria-hidden="true" />
      Preview
    </span>
  );
}

import { KeyBackendAdmission, KeyBackendLifecycle, type KeyBackendFailureReason } from '@trogonai/proto';

import { StatusPill, type Tone } from '@/components/lifecycle-badge';

import { admissionLabels, failureReasonLabels, lifecycleLabels, type ConnectionStatus } from '../domain/backend';

const lifecycleTones: Record<KeyBackendLifecycle, Tone> = {
  [KeyBackendLifecycle.UNSPECIFIED]: 'neutral',
  [KeyBackendLifecycle.PENDING_VALIDATION]: 'warning',
  [KeyBackendLifecycle.ACTIVE]: 'success',
  [KeyBackendLifecycle.RETIRED]: 'neutral',
};

export function LifecycleBadge({ lifecycle }: { lifecycle: KeyBackendLifecycle }) {
  return (
    <StatusPill
      tone={lifecycleTones[lifecycle]}
      blink={lifecycle === KeyBackendLifecycle.PENDING_VALIDATION}
      struck={lifecycle === KeyBackendLifecycle.RETIRED}
      label={lifecycleLabels[lifecycle]}
    />
  );
}

export function AdmissionBadge({ admission }: { admission: KeyBackendAdmission }) {
  const paused = admission === KeyBackendAdmission.PAUSED;
  return <StatusPill tone={paused ? 'warning' : 'neutral'} blink={paused} label={admissionLabels[admission]} />;
}

const connectionLabels: Record<ConnectionStatus['kind'], string> = {
  unknown: 'Not observed',
  connected: 'Connected',
  failed: 'Failed',
};

const connectionTones: Record<ConnectionStatus['kind'], Tone> = {
  unknown: 'neutral',
  connected: 'success',
  failed: 'danger',
};

export function ConnectionBadge({ connection }: { connection: ConnectionStatus }) {
  return <StatusPill tone={connectionTones[connection.kind]} label={connectionLabels[connection.kind]} />;
}

export function FailureReasonBadge({ reason }: { reason: KeyBackendFailureReason }) {
  return <StatusPill tone="danger" label={failureReasonLabels[reason]} className="normal-case" />;
}

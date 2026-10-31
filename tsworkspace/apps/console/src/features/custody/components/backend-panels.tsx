import { timestampDate, type Timestamp } from '@bufbuild/protobuf/wkt';
import { KeyBackendAdmission, type KeyBackendAccess } from '@trogonai/proto';
import { PauseIcon, PlayIcon, RefreshCwIcon } from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { FaultAlert } from '@/components/fault-alert';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { useNotify } from '@/components/ui/toaster';
import { RelativeTime } from '@/features/secrets/components/relative-time';
import { formatTimestamp } from '@/features/secrets/components/time';

import { pauseCopy } from '../domain/admission';
import {
  EXPIRY_WARNING_DAYS,
  expirySignalLabels,
  failureReasonHints,
  failureReasonLabels,
  type ExpirySignal,
} from '../domain/backend';
import type { BackendView } from '../domain/view';
import { usePauseBackend, useResumeBackend, useValidateBackend } from '../queries';
import { AdmissionBadge, ConnectionBadge } from './key-badges';

export function ConnectionPanel({ backend }: { backend: BackendView }) {
  const validate = useValidateBackend();
  const notify = useNotify();
  const { connection } = backend;
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Connection status</CardTitle>
          <CardDescription>
            What the platform last observed when it reached your provider. It is advisory and never decides whether the
            registration is admitted.
          </CardDescription>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!backend.validate.available || validate.isPending}
          title={backend.validate.reason}
          onClick={() =>
            validate.mutate(backend.id, {
              onSuccess: () => notify.success('Connection checked', backend.displayName),
              onError: (error) => notify.failure('Could not check the connection', error.message),
            })
          }
        >
          <RefreshCwIcon data-icon="inline-start" />
          Check connection
        </Button>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <ConnectionBadge connection={connection} />
          {connection.kind === 'unknown' ? (
            <span className="text-muted-foreground text-[13px]">The platform has not observed this backend yet.</span>
          ) : (
            <RelativeTime date={connection.observedAt} className="text-muted-foreground text-xs" />
          )}
        </div>
        {connection.kind === 'failed' ? (
          <div
            role="alert"
            className="border-danger/30 bg-danger-soft text-danger-ink flex flex-col gap-1 rounded-lg border px-4 py-3 text-[13px]"
          >
            <span className="font-medium">{failureReasonLabels[connection.reason]}</span>
            <span>{failureReasonHints[connection.reason]}</span>
          </div>
        ) : null}
        <FaultAlert error={validate.error} title="The connection could not be checked" />
        {!backend.validate.available && backend.validate.reason ? (
          <p className="text-muted-foreground text-xs">{backend.validate.reason}</p>
        ) : null}
      </CardContent>
    </Card>
  );
}

const expiryFields: readonly (readonly [
  ExpirySignal,
  (
    | 'serverCertificateNotAfter'
    | 'clientCertificateNotAfter'
    | 'trustRootNotAfter'
    | 'credentialExpiresAt'
    | 'credentialIssuedAt'
  ),
])[] = [
  ['server_certificate', 'serverCertificateNotAfter'],
  ['client_certificate', 'clientCertificateNotAfter'],
  ['trust_root', 'trustRootNotAfter'],
  ['credential_expiry', 'credentialExpiresAt'],
  ['credential_issued', 'credentialIssuedAt'],
];

function dateOf(value: Timestamp | undefined): Date | undefined {
  return value ? timestampDate(value) : undefined;
}

export function ExpiryPanel({ backend }: { backend: BackendView }) {
  const expiry = backend.registration.health?.trustExpiry;
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Expiry and credential age</CardTitle>
          <CardDescription>
            A signal the backend has not reported is shown as not reported. Expiry never blocks admission, the
            connection simply fails when a certificate does expire.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {backend.expiry.length > 0 ? (
          <ul aria-label="Expiry warnings" className="flex flex-col gap-2">
            {backend.expiry.map((warning) => (
              <li
                key={warning.signal}
                role="alert"
                className={
                  warning.severity === 'expired'
                    ? 'border-danger/30 bg-danger-soft text-danger-ink rounded-lg border px-4 py-2.5 text-[13px]'
                    : 'border-warning/35 bg-warning-soft text-warning-ink rounded-lg border px-4 py-2.5 text-[13px]'
                }
              >
                {warningText(warning.signal, warning.severity, warning.daysRemaining)}
              </li>
            ))}
          </ul>
        ) : null}
        <dl className="grid gap-3 md:grid-cols-2">
          {expiryFields.map(([signal, field]) => {
            const at = dateOf(expiry?.[field]);
            return (
              <div key={signal} className="flex flex-col gap-1">
                <dt className="micro text-muted-foreground text-[10px]">{expirySignalLabels[signal]}</dt>
                <dd className="font-mono text-xs">{at ? formatTimestamp(at) : 'Not reported'}</dd>
              </div>
            );
          })}
        </dl>
      </CardContent>
    </Card>
  );
}

function warningText(signal: ExpirySignal, severity: 'expired' | 'soon' | 'later', daysRemaining: number): string {
  const label = expirySignalLabels[signal];
  if (signal === 'credential_issued') return `${label}: issued ${-daysRemaining} days ago. Consider rotating it.`;
  if (severity === 'expired') return `${label} expired ${-daysRemaining} days ago.`;
  return `${label} expires in ${daysRemaining} days, within the ${EXPIRY_WARNING_DAYS} day warning window.`;
}

export function AdmissionPanel({ backend }: { backend: BackendView }) {
  const pause = usePauseBackend();
  const resume = useResumeBackend();
  const notify = useNotify();
  const [confirming, setConfirming] = useState(false);
  const paused = backend.admission === KeyBackendAdmission.PAUSED;

  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Admission</CardTitle>
          <CardDescription>Only you, the owner of this registration, can pause or resume it.</CardDescription>
        </div>
        <AdmissionBadge admission={backend.admission} />
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <p className="text-muted-foreground text-[13px]">{pauseCopy}</p>
        {paused ? (
          <p
            role="status"
            className="border-warning/35 bg-warning-soft text-warning-ink rounded-lg border px-4 py-3 text-[13px]"
          >
            While paused, binding a key, setting a default and migrating from or to this backend are all denied with
            backend paused. Resuming checks the registration again before the platform calls your provider.
          </p>
        ) : null}
        <FaultAlert
          error={pause.error ?? resume.error}
          title={paused ? 'The registration was not resumed' : 'The registration was not paused'}
        />
        <div>
          {paused ? (
            <Button
              type="button"
              disabled={!backend.resume.available || resume.isPending}
              title={backend.resume.reason}
              onClick={() => {
                pause.reset();
                resume.mutate(backend.id, {
                  onSuccess: () => notify.success('Registration resumed', backend.displayName),
                });
              }}
            >
              <PlayIcon data-icon="inline-start" />
              {resume.isPending ? 'Validating...' : 'Resume and validate'}
            </Button>
          ) : (
            <Button
              type="button"
              variant="outline"
              disabled={!backend.pause.available || pause.isPending}
              title={backend.pause.reason}
              onClick={() => {
                resume.reset();
                setConfirming(true);
              }}
            >
              <PauseIcon data-icon="inline-start" />
              Pause registration
            </Button>
          )}
        </div>
      </CardContent>
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        icon={<PauseIcon />}
        title="Pause this registration?"
        description={pauseCopy}
        confirmLabel="Pause registration"
        onConfirm={() => {
          setConfirming(false);
          pause.mutate(backend.id, {
            onSuccess: () => notify.success('Registration paused', backend.displayName),
          });
        }}
      />
    </Card>
  );
}

function accessFacts(access: KeyBackendAccess | undefined): readonly (readonly [string, string])[] {
  switch (access?.provider.case) {
    case 'awsKms': {
      const aws = access.provider.value;
      return [
        ['Mechanism', 'STS AssumeRole'],
        ['Region', aws.region?.value ?? ''],
        ['Account', aws.account?.value ?? ''],
        ['Role ARN', aws.authentication?.role?.value ?? ''],
        ['External ID', aws.authentication?.externalId?.value ?? ''],
      ];
    }
    case 'googleCloudKms': {
      const gcp = access.provider.value;
      const impersonated = gcp.authentication?.impersonatedServiceAccount?.value;
      return [
        ['Mechanism', 'Workload Identity Federation'],
        ['Project', gcp.project?.value ?? ''],
        ['Location', gcp.location?.value ?? ''],
        ['Workload identity provider', gcp.authentication?.workloadIdentityProvider?.value ?? ''],
        ...(impersonated ? ([['Service account', impersonated]] as const) : []),
      ];
    }
    case 'openBaoTransit': {
      const bao = access.provider.value;
      const method = bao.authentication?.method;
      const role = method?.case === undefined ? undefined : method.value.role?.value;
      return [
        ['Mechanism', method?.case === 'certificate' ? 'Certificate authentication' : 'JWT authentication'],
        ['Endpoint', bao.endpoint?.baseUrl ?? ''],
        ['Namespace', bao.namespace?.value ?? 'Root namespace'],
        ['Transit mount', bao.transitMount?.value ?? ''],
        ['Auth role', role ?? ''],
        ['Trust roots', String(bao.trustRoots.length)],
      ];
    }
    default:
      return [];
  }
}

export function AccessPanel({ backend }: { backend: BackendView }) {
  const facts = accessFacts(backend.registration.configuration?.access);
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Access profile</CardTitle>
          <CardDescription>{backend.profileSummary}. No long-lived secret is stored.</CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        <dl className="grid gap-4">
          {facts.map(([label, value]) => (
            <div key={label} className="flex flex-col gap-1">
              <dt className="micro text-muted-foreground text-[10px]">{label}</dt>
              <dd className="font-mono text-xs break-all">{value}</dd>
            </div>
          ))}
        </dl>
      </CardContent>
    </Card>
  );
}

export function RevisionHistoryPanel({ backend }: { backend: BackendView }) {
  const revisions = [...backend.revisions].sort((a, b) => b.revision - a.revision);
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Revision history</CardTitle>
          <CardDescription>
            A new revision takes effect only after it validates. Until then the current revision stays in force.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        <ol className="flex flex-col gap-3" aria-label="Revisions">
          {revisions.map((revision) => (
            <li key={revision.revision} className="flex items-start gap-3">
              <span className="micro bg-muted text-muted-foreground rounded-sm px-1.5 py-0.5 font-mono text-[10px]">
                r{revision.revision}
              </span>
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="text-[13px]">{revision.summary}</span>
                <RelativeTime date={revision.at} className="text-muted-foreground text-xs" />
              </span>
            </li>
          ))}
        </ol>
      </CardContent>
    </Card>
  );
}

export function BoundKeysPanel({ backend }: { backend: BackendView }) {
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Keys bound</CardTitle>
          <CardDescription>Keys the platform may use inside this backend.</CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        {backend.keys.length === 0 ? (
          <p className="text-muted-foreground text-[13px]">No key is bound to this backend yet.</p>
        ) : (
          <ul className="divide-border divide-y" aria-label="Bound keys">
            {backend.keys.map((key) => (
              <li key={key.keyRef} className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2.5 first:pt-0 last:pb-0">
                <span className="font-mono text-[13px]">{key.keyRef}</span>
                <span className="text-muted-foreground text-xs">{key.purposeLabel}</span>
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

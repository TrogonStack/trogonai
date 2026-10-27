import { timestampDate } from '@bufbuild/protobuf/wkt';
import type { CredentialEvent } from '@trogonai/proto';
import { MaterialVerificationOutcome } from '@trogonai/proto';

import { toneVariable, type Tone } from '@/components/lifecycle-badge';
import { cn } from '@/lib/utils';

import type { StoredCredentialEvent } from '../client';
import { failureReasonMessage } from '../domain/faults';
import {
  credentialLifecycleLabels,
  credentialLifecycleTones,
  originLabels,
  policyReasonLabels,
  purposeLabels,
  useActionLabels,
  useResultLabels,
  verificationOutcomeLabels,
  authorizationEndLabels,
  leaseEndLabels,
  writeKindLabels,
} from '../domain/labels';
import { formatCompact, relativeTime } from './time';

interface Description {
  title: string;
  detail?: string;
  tone: Tone;
}

export function describeCredentialEvent(event: CredentialEvent): Description {
  const fact = event.fact;
  switch (fact.case) {
    case 'reserved':
      return {
        title: 'Reserved',
        tone: 'info',
        detail: `${purposeLabels[fact.value.policy?.purpose ?? 0]} in vault ${fact.value.vaultId}`,
      };
    case 'candidateWriteReserved':
      return {
        title: 'Write reserved',
        tone: 'info',
        detail: `${writeKindLabels[fact.value.kind]}, ${originLabels[fact.value.origin].toLowerCase()} material`,
      };
    case 'candidateObserved':
      return {
        title: 'Candidate observed',
        tone: 'info',
        detail: `Version ${String(fact.value.candidate?.reference?.version ?? '')} is stored`,
      };
    case 'materialVerificationRecorded': {
      const outcome = fact.value.verification?.outcome ?? MaterialVerificationOutcome.UNSPECIFIED;
      return {
        title: 'Verification recorded',
        tone: outcome === MaterialVerificationOutcome.PASSED ? 'success' : 'danger',
        detail: verificationOutcomeLabels[outcome],
      };
    }
    case 'versionActivated':
      return {
        title: 'Version activated',
        tone: 'success',
        detail: fact.value.previousWebhookVersion
          ? `Version ${String(fact.value.version?.version ?? '')}. The previous version stays accepted until ${formatCompact(timestampDate(fact.value.previousWebhookVersion.expiresAt!))}`
          : `Version ${String(fact.value.version?.version ?? '')}`,
      };
    case 'writeRejected':
      return { title: 'Write rejected', tone: 'danger', detail: failureReasonMessage(fact.value.reason) };
    case 'lifecycleChanged':
      return {
        title: credentialLifecycleLabels[fact.value.lifecycle],
        tone: credentialLifecycleTones[fact.value.lifecycle],
        detail: policyReasonLabels[fact.value.reason],
      };
    case 'destructionScheduled':
      return {
        title: 'Destruction scheduled',
        tone: 'danger',
        detail: `Executes after ${formatCompact(timestampDate(fact.value.executeAfter!))}. ${policyReasonLabels[fact.value.reason]}`,
      };
    case 'versionDestructionObserved':
      return {
        title: 'Version destroyed',
        tone: 'neutral',
        detail: `Version ${String(fact.value.version?.version ?? '')}`,
      };
    case 'metadataPurgeObserved':
      return { title: 'Metadata purged', tone: 'neutral' };
    case 'destroyed':
      return { title: 'Destroyed', tone: 'neutral' };
    case 'webhookRotationOverlapEnded':
      return {
        title: 'Overlap ended',
        tone: 'info',
        detail: `Version ${String(fact.value.previousVersion?.version ?? '')} no longer verifies`,
      };
    case 'metadataUpdated':
      return { title: 'Renamed', tone: 'neutral', detail: fact.value.displayName };
    case 'audit': {
      const audit = fact.value.fact;
      switch (audit.case) {
        case 'accepted': {
          const authorization = audit.value.authorization;
          return {
            title: 'Use authorized',
            tone: 'info',
            detail: authorization
              ? `${useActionLabels[authorization.action]} for ${authorization.workloadPrincipalId}`
              : undefined,
          };
        }
        case 'completed':
          return {
            title: 'Use completed',
            tone: audit.value.failureReason === undefined ? 'success' : 'danger',
            detail: useResultLabels[audit.value.result],
          };
        case 'denied':
          return {
            title: 'Use denied',
            tone: 'danger',
            detail: `${audit.value.workloadPrincipalId}: ${failureReasonMessage(audit.value.reason)}`,
          };
        case 'authorizationEnded':
          return { title: 'Authorization ended', tone: 'neutral', detail: authorizationEndLabels[audit.value.reason] };
        default:
          return { title: 'Audit', tone: 'neutral' };
      }
    }
    case 'lease': {
      const lease = fact.value.fact;
      switch (lease.case) {
        case 'reserved':
          return { title: 'Lease reserved', tone: 'info', detail: `For ${lease.value.workloadPrincipalId}` };
        case 'issued':
          return {
            title: 'Lease issued',
            tone: 'success',
            detail: `Expires ${formatCompact(timestampDate(lease.value.expiresAt!))}`,
          };
        case 'useEnded':
          return { title: 'Lease ended', tone: 'neutral', detail: leaseEndLabels[lease.value.reason] };
        case 'cleanupConfirmed':
          return { title: 'Lease cleanup confirmed', tone: 'neutral' };
        default:
          return { title: 'Lease', tone: 'neutral' };
      }
    }
    default:
      return { title: 'Event', tone: 'neutral' };
  }
}

export function ActivityTimeline({ events }: { events: readonly StoredCredentialEvent[] }) {
  if (events.length === 0) return <p className="micro text-muted-foreground">No events recorded.</p>;
  const ordered = [...events].sort((left, right) => right.revision - left.revision);
  return (
    <div className="well border-border overflow-hidden rounded-lg border font-mono text-xs">
      <div className="micro border-border bg-card/60 text-muted-foreground flex items-center justify-between border-b px-3 py-2 text-[10px]">
        <span>event log</span>
        <span className="tabular-nums">{events.length} entries</span>
      </div>
      <ol aria-label="Credential activity" className="flex flex-col py-1">
        {ordered.map((stored) => {
          const { title, detail, tone } = describeCredentialEvent(stored.event);
          return (
            <li
              key={stored.revision}
              className="hover:bg-foreground/[0.03] grid grid-cols-[3.25rem_minmax(0,1fr)] gap-x-3 px-3 py-1.5 sm:grid-cols-[3.25rem_12rem_minmax(0,1fr)]"
            >
              <span className="border-border text-muted-foreground self-start border-r pr-3 text-right tabular-nums">
                r{stored.revision}
              </span>
              <time
                dateTime={stored.occurredAt.toISOString()}
                title={stored.occurredAt.toLocaleString()}
                className="text-muted-foreground hidden tabular-nums sm:block"
              >
                {stored.occurredAt.toLocaleTimeString([], { hour12: false })}
                <span className="text-muted-foreground/60"> {relativeTime(stored.occurredAt)}</span>
              </time>
              <div className="flex min-w-0 flex-col gap-0.5">
                <div className="flex flex-wrap items-center gap-2">
                  <span
                    className={cn(
                      'micro text-foreground inline-flex h-5 items-center gap-1.5 rounded-sm border border-(--led)/35 px-1.5 text-[10px]',
                      toneVariable(tone),
                    )}
                  >
                    <span aria-hidden="true" className="led size-1.5 rounded-full" />
                    {title}
                  </span>
                  <span className="text-muted-foreground">by {stored.actor}</span>
                </div>
                {detail ? <p className="text-foreground/80 break-words">{detail}</p> : null}
              </div>
            </li>
          );
        })}
      </ol>
    </div>
  );
}

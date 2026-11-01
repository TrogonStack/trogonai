import { TriangleAlertIcon } from 'lucide-react';

import { KeyFault } from '@/features/custody/domain/faults';
import { EnvironmentFault } from '@/features/environments/domain/faults';
import { SourceFault } from '@/features/gateway-sources/domain/faults';
import { HierarchyFault } from '@/features/hierarchy/domain/faults';
import { ProjectFault } from '@/features/projects/domain/faults';
import { ConcurrencyFault, ProviderConnectionFault } from '@/features/provider-connections/domain/faults';
import { SecretFault } from '@/features/secrets/domain/faults';

export function FaultAlert({ error, title = 'The command was rejected' }: { error: unknown; title?: string }) {
  if (!error) return null;
  const message = error instanceof Error ? error.message : 'Unexpected failure.';
  const reason =
    error instanceof ProviderConnectionFault ||
    error instanceof KeyFault ||
    error instanceof ConcurrencyFault ||
    error instanceof SecretFault ||
    error instanceof SourceFault ||
    error instanceof EnvironmentFault ||
    error instanceof ProjectFault ||
    error instanceof HierarchyFault
      ? error.reason
      : undefined;
  const violations = error instanceof ProviderConnectionFault ? error.violations : [];
  return (
    <div
      role="alert"
      className="border-danger/30 bg-danger-soft text-danger-ink flex gap-2.5 rounded-lg border p-3 text-[13px]"
    >
      <TriangleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <div className="flex min-w-0 flex-col gap-1">
        <p className="flex flex-wrap items-center gap-2 font-medium">
          {title}
          {reason ? (
            <span className="bg-danger/10 rounded px-1.5 py-px font-mono text-[11px] font-normal">{reason}</span>
          ) : null}
        </p>
        <p>{message}</p>
        {violations.length > 0 ? (
          <ul className="flex flex-col gap-0.5 font-mono text-xs">
            {violations.map((violation) => (
              <li key={`${violation.path}:${violation.constraint}`}>
                {violation.path}: {violation.message}
              </li>
            ))}
          </ul>
        ) : null}
      </div>
    </div>
  );
}

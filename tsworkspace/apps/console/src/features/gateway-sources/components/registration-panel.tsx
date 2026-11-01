import { TriangleAlertIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { RelativeTime } from '@/features/secrets/components/relative-time';

import { formatVersion } from '../domain/secret-version';
import { slotLabels } from '../domain/source-kind';
import type { RegistrationView } from '../domain/view';
import { RegistrationBadge } from './status-badges';

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="micro text-muted-foreground text-[10px]">{label}</dt>
      <dd className="text-[13px]">{children}</dd>
    </div>
  );
}

export function RegistrationPanel({ registration }: { registration: RegistrationView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Webhook registration</CardTitle>
        <CardDescription>
          The provider signs deliveries with the secret it was given at registration. It has to match the active
          version.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex items-center gap-2">
          <RegistrationBadge status={registration.status} />
        </div>
        {registration.status === 'not_applicable' ? (
          <p className="text-muted-foreground text-[13px]">
            This source does not register a webhook with the provider, so there is nothing to keep in sync.
          </p>
        ) : (
          <dl className="grid grid-cols-2 gap-4">
            {registration.slot ? <Fact label="Secret slot">{slotLabels[registration.slot]}</Fact> : null}
            <Fact label="Registered version">
              {registration.registeredVersion !== undefined ? (
                <span className="font-mono tabular-nums">{formatVersion(registration.registeredVersion)}</span>
              ) : (
                'None'
              )}
            </Fact>
            <Fact label="Active version">
              {registration.activeVersion !== undefined ? (
                <span className="font-mono tabular-nums">{formatVersion(registration.activeVersion)}</span>
              ) : (
                'None'
              )}
            </Fact>
            {registration.registeredAt ? (
              <Fact label="Registered">
                <RelativeTime date={registration.registeredAt} />
              </Fact>
            ) : null}
          </dl>
        )}
        {registration.mismatch ? (
          <div
            role="status"
            className="border-warning/35 bg-warning-soft text-warning-ink flex items-start gap-2.5 rounded-lg border px-3 py-2.5 text-[13px]"
          >
            <TriangleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            Registered with a superseded secret. The provider still signs with an older version, so deliveries fail
            verification until the webhook is registered again.
          </div>
        ) : null}
        {registration.failure ? (
          <div
            role="status"
            className="border-danger/30 bg-danger-soft text-danger-ink flex items-start gap-2.5 rounded-lg border px-3 py-2.5 text-[13px]"
          >
            <TriangleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            {registration.failure}
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}
